//! The scoreboard and the death notices: what a death does to the frags and deaths of the players
//! and what everyone is told about it. Ported from ReGameDLL_CS `multiplay_gamerules.cpp`
//! (`CHalfLifeMultiplay::PlayerKilled`, `DeathNotice`, `SendDeathMessage`, `IPointsForKill`,
//! `UpdateTeamScores`, `ClientDisconnected`) and `player.cpp` (`CBasePlayer::GetKillerWeaponName`,
//! `SetScoreAttrib`, `SetScoreboardAttributes`, `Killed`), read 2026-11-22, vanilla behaviour
//! (not `REGAMEDLL_FIXES`/`REGAMEDLL_ADD`).
//!
//! The messages and where they are sent:
//! - `DeathMsg` (killer, victim, headshot byte, weapon string) to everyone, reliable
//!   (`MSG_ALL`), from [`player_killed`].
//! - `ScoreInfo` (index, frags, deaths, class 0, team) of the victim, then of a killer that is a
//!   player, from [`player_killed`]. The victim's line is `MSG_BROADCAST` in vanilla (the
//!   unreliable queue of every active client; `REGAMEDLL_FIXES` makes it `MSG_ALL`), the killer's
//!   is `MSG_ALL` (reliable) always. On the wire the reliable messages of a frame (the
//!   `DeathMsg`, the killer's line, the `ScoreAttrib`) therefore precede the victim's line. The
//!   other `ScoreInfo` senders (spawn, join, team change, leaving) are in [`crate::messages`],
//!   [`crate::teams`] and [`crate::rules`].
//! - `ScoreAttrib` (index, flags) of a player to every player in the game
//!   (`SetScoreboardAttributes`, [`crate::messages::set_scoreboard_attributes`]): sent when a
//!   player dies (`Killed`, flag `SCORE_STATUS_DEAD`, after the observer setup) and when it leaves
//!   (`ClientDisconnected`, the leaving player counts as dead).
//! - `TeamScore` (team name, rounds won) is `UpdateTeamScores` of the round rules, `CT` then
//!   `TERRORIST`, to everyone ([`crate::rules`]); a joining player gets them in `InitHUD`
//!   (`TERRORIST` first) and `UpdateClientData` (`CT` first) ([`crate::messages`]).
//!
//! The frag rules of `PlayerKilled`: a player that killed itself (the victim is the killer, the
//! `kill` command, or its own grenade) loses a frag; a player killed by a teammate costs the killer
//! `IPointsForKill` (1) frags; by an enemy gives the killer 1; killed by the world (a killer that
//! is no player) changes nobody's frags. Money and the team kills of the same death are
//! [`money::killed_by`], called from here between the frags and the `ScoreInfo`, where the
//! original puts them.
//!
//! Deviations and what waits for later tasks:
//! - The weapon name of a killer that is its own inflictor is the active item's name
//!   (`m_pActiveItem->pszName()` without `weapon_`); there are no weapons until Phase 2D, so it is
//!   `world` as for a player without an active item. `g_pevLastInflictor`
//!   (`GetLastInflictor()`, what `CBasePlayer::Killed` passes as the inflictor) is 2D's, with the
//!   grenades and the weapons that inflict.
//! - The `m_iNumKilledByUnanswered` revenge statistics, the assists and the kill rarity of
//!   `DeathNotice`/`SendDeathMessage` exist only in newer builds (`REGAMEDLL_ADD` writes the extra
//!   death message fields) and are not ported; the byte vanilla writes for the headshot is
//!   `KILLRARITY_HEADSHOT` (1), a boolean.
//! - `DeathNotice` calls `CheckWinConditions()` once more, inside `PlayerKilled`, which
//!   `CBasePlayer::Killed` runs before it sets `deadflag = DEAD_DYING` (`player.cpp`). It is not
//!   ported as a second call: the one at the end of `Killed` ([`crate::player`]) is the one
//!   ported (the original's extra call is marked as probably unnecessary by its own TODO).
//! - The `SVC_DIRECTOR` death event to `MSG_SPEC` (HLTV), the log lines (`UTIL_LogPrintf`),
//!   `FireTargets("game_playerdie"/"game_playerkill")`, the vehicle driver as killer, the hints
//!   (`#Hint_careful_around_teammates`, `#Hint_win_round_by_killing_enemy`: no hint queue yet),
//!   the VIP kill reward and the decal timer are not ported.
//! - `SCORE_STATUS_BOMB` and `SCORE_STATUS_VIP` come with the bomb and the VIP (2E); the
//!   defuser kit flag is `BUILD_LATEST` only.

use cs_engine::consts::FL_CLIENT;
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;

use crate::GameState;
use crate::messages;
use crate::money;
use crate::player::{HITGROUP_HEAD, player_slot};

/// `CHalfLifeMultiplay::IPointsForKill`: what a kill is worth, whoever the players are.
pub(crate) const fn points_for_kill() -> i32 {
    1
}

/// The weapon name of a death message (`CBasePlayer::GetKillerWeaponName`, `player.cpp`, read
/// 2026-11-22): the inflictor's classname without its `weapon_`, `monster_` or `func_` prefix. A
/// killer that is no client with no inflictor is the world (`VARS(eoNullEntity)`): `worldspawn`
/// (the original dereferences the null inflictor there; `REGAMEDLL_FIXES` says `world`). A client
/// killer that is its own inflictor names its active weapon, `world` while there are no weapons.
pub(crate) fn killer_weapon_name(
    eng: &EngineApi<'_>,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
) -> String {
    let killer_is_client = attacker
        .and_then(|a| eng.ent(a))
        .is_some_and(|v| v.flags & FL_CLIENT != 0);
    let world_class = eng
        .ent(eng.world())
        .map_or("worldspawn", |v| v.classname.as_str());
    let name = match inflictor {
        Some(i) if !(killer_is_client && Some(i) == attacker) => {
            eng.ent(i).map_or("world", |v| v.classname.as_str())
        }
        None if !killer_is_client => world_class,
        _ => "world",
    };
    ["weapon_", "monster_", "func_"]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
        .to_string()
}

/// `ScoreInfo` to `dest`: the scoreboard line of the player of `slot`, as `PlayerKilled` sends it.
fn score_info(g: &GameState, eng: &mut EngineApi<'_>, dest: MsgDest, slot: usize) {
    let Some(p) = g.players.get(slot).and_then(Option::as_ref) else {
        return;
    };
    eng.send_usermsg(
        dest,
        &CsUserMsg::ScoreInfo {
            player_id: (slot + 1) as u8,
            frags: p.frags.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            deaths: p.deaths.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            class_id: 0,
            team_id: i16::from(p.team.id()),
        },
    );
}

/// `CHalfLifeMultiplay::PlayerKilled` with `DeathNotice` and `SendDeathMessage` for the player of
/// `slot` killed by `attacker` with `inflictor` (edicts; `None` for the world), in the original's
/// order: the death notice, the victim's death, the frags, the money and the team kills, the
/// scoreboard lines of the victim and of a killer that is a player.
pub(crate) fn player_killed(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    attacker: Option<EdictId>,
    inflictor: Option<EdictId>,
) {
    let index = usize::from(slot);
    let victim_edict = eng.client_edict(slot);
    // The killer is the attacker when that is a player; an attacker that is anything else is the
    // world as far as the scores are concerned.
    let killer = attacker.and_then(|a| player_slot(g, eng, a));
    let weapon = killer_weapon_name(eng, inflictor, attacker);
    let Some(victim) = g.players[index].as_ref() else {
        return;
    };
    let headshot = victim.last_hit_group == HITGROUP_HEAD;
    let victim_team = victim.team;

    // DeathNotice -> SendDeathMessage: `(pKiller && pKiller->IsPlayer()) ? entindex : 0`.
    let killer_index = killer.map_or(0, |k| k as u16 + 1);
    messages::death_msg(eng, killer_index, victim_edict.index, headshot, &weapon);

    // PlayerKilled.
    if let Some(victim) = g.players[index].as_mut() {
        victim.deaths += 1;
        victim.not_killed = false;
    }
    if let Some(k) = killer {
        if k == index {
            // Players lose a frag for killing themselves.
            if let Some(kp) = g.players[k].as_mut() {
                kp.frags -= 1;
            }
        } else {
            let teammate = g.players[k].as_ref().map(|p| p.team) == Some(victim_team);
            if let Some(kp) = g.players[k].as_mut() {
                kp.frags += if teammate {
                    -points_for_kill()
                } else {
                    points_for_kill()
                };
            }
        }
    }
    // The money and the team kills (`AddAccount`, `m_iTeamKills`, `mp_autokick`).
    money::killed_by(g, eng, slot, attacker);

    // The scoreboard: the victim's line (`MSG_BROADCAST`, unreliable, in vanilla; `MSG_ALL` with
    // `REGAMEDLL_FIXES`), then the killer's if it is a player (`MSG_ALL`, reliable, always).
    score_info(g, eng, MsgDest::Broadcast, index);
    if let Some(k) = killer {
        score_info(g, eng, MsgDest::All, k);
    }
}
