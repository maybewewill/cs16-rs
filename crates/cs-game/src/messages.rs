//! The user messages the player code sends. Every send of `player.rs` goes through here, so the
//! HUD task can take the messages over (and add the rest of the `gmsg*` ones) in one place.

use cs_engine::{EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;

use crate::player::Player;

/// `gmsgScoreInfo` to everyone: the scoreboard line of the player of edict index `index`
/// (ReGameDLL `CBasePlayer::Spawn`, `CHalfLifeMultiplay::PlayerKilled`, read 2026-11-22).
pub(crate) fn score_info(eng: &mut EngineApi<'_>, index: u16, p: &Player) {
    let Ok(player_id) = u8::try_from(index) else {
        return;
    };
    eng.send_usermsg(
        MsgDest::All,
        &CsUserMsg::ScoreInfo {
            player_id,
            frags: p.frags.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            deaths: p.deaths.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            class_id: 0,
            team_id: i16::from(p.team.id()),
        },
    );
}

/// `gmsgDeathMsg` to everyone (`CHalfLifeMultiplay::DeathNotice`): `killer` and `victim` are
/// edict indices, `killer` 0 for the world.
pub(crate) fn death_msg(
    eng: &mut EngineApi<'_>,
    killer: u16,
    victim: u16,
    headshot: bool,
    weapon: &str,
) {
    let (Ok(killer_id), Ok(victim_id)) = (u8::try_from(killer), u8::try_from(victim)) else {
        return;
    };
    eng.send_usermsg(
        MsgDest::All,
        &CsUserMsg::DeathMsg {
            killer_id,
            victim_id,
            headshot,
            weapon: weapon.to_string(),
        },
    );
}

/// `gmsgHealth` to `slot` (`CBasePlayer::UpdateClientData`, `Killed`).
pub(crate) fn health(eng: &mut EngineApi<'_>, slot: u8, hp: u8) {
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::Health(hp));
}
