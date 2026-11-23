//! VIP assassination mode (`as_*` maps): the VIP queue, choosing/rotating the VIP, promoting and
//! resetting VIP status, and VIP disconnect cleanup, ported from ReGameDLL_CS (`player.cpp`
//! `CBasePlayer::MakeVIP`, `multiplay_gamerules.cpp` `StackVIPQueue`/`IsVIPQueueEmpty`/
//! `AddToVIPQueue`/`ResetCurrentVIP`/`PickNextVIP`/`ClientDisconnected`, `client.cpp` `become_vip`,
//! vanilla `!REGAMEDLL_FIXES` / `!REGAMEDLL_ADD`).

use cs_engine::EngineApi;

use crate::GameState;
use crate::chat::client_print;
use crate::menus::HUD_PRINTCENTER;
use crate::player::Team;

/// `MAX_VIP_QUEUES` (`gamerules.h`): number of slots in the VIP waiting queue.
pub const MAX_VIP_QUEUES: usize = 5;
/// VIP movement speed limit in units/s (`CBasePlayer::ResetMaxSpeed`, `player.cpp:8123`).
pub const VIP_MAX_SPEED: f32 = 227.0;
/// VIP spawn vest+helmet armor (`CBasePlayer::Spawn`, `player.cpp:6098`).
pub const VIP_ARMOR: f32 = 200.0;

/// `CHalfLifeMultiplay::StackVIPQueue` (`multiplay_gamerules.cpp` 2190-2204, read 2026-11-23),
/// ported as written. Quirk kept: the loop runs `i = MAX_VIP_QUEUES - 2 .. 1` and shifts at most
/// one step per call, so it does not fully compact a sparse queue.
pub(crate) fn stack_vip_queue(g: &mut GameState) {
    let q = &mut g.rules.vip_queue;
    for i in (1..=MAX_VIP_QUEUES - 2).rev() {
        if q[i - 1].is_some() {
            if q[i].is_none() {
                q[i] = q[i + 1];
                q[i + 1] = None;
            }
        } else {
            q[i - 1] = q[i];
            q[i] = q[i + 1];
            q[i + 1] = None;
        }
    }
}

/// The first loop of `IsVIPQueueEmpty`/`AddToVIPQueue`: a queued player who is no longer a CT is
/// dropped (a freed slot is dropped too: deviation, the original keeps a dangling pointer).
fn prune_vip_queue(g: &mut GameState) {
    for entry in &mut g.rules.vip_queue {
        let valid_ct = entry.is_none_or(|slot| {
            g.players
                .get(usize::from(slot))
                .and_then(Option::as_ref)
                .is_some_and(|p| p.team == Team::Ct)
        });
        if !valid_ct {
            *entry = None;
        }
    }
}

/// `CHalfLifeMultiplay::IsVIPQueueEmpty` (`multiplay_gamerules.cpp` 2206-2218).
pub(crate) fn is_vip_queue_empty(g: &mut GameState) -> bool {
    prune_vip_queue(g);
    stack_vip_queue(g);
    g.rules.vip_queue.iter().all(Option::is_none)
}

/// `CHalfLifeMultiplay::AddToVIPQueue` (`multiplay_gamerules.cpp` 2220-2258): queues `slot` (when
/// a CT) for the next VIP pick, informing them of their 1-based queue position or full queue.
pub(crate) fn add_to_vip_queue(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    prune_vip_queue(g);
    stack_vip_queue(g);
    let is_ct = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.team == Team::Ct);
    if !is_ct {
        return false;
    }

    if let Some(pos) = g.rules.vip_queue.iter().position(|&entry| entry == Some(slot)) {
        client_print(
            eng,
            slot,
            HUD_PRINTCENTER,
            "#Game_in_position",
            &[&(pos + 1).to_string()],
        );
        return false;
    }

    if let Some(free) = g.rules.vip_queue.iter().position(Option::is_none) {
        g.rules.vip_queue[free] = Some(slot);
        stack_vip_queue(g);
        client_print(
            eng,
            slot,
            HUD_PRINTCENTER,
            "#Game_added_position",
            &[&(free + 1).to_string()],
        );
        return true;
    }

    client_print(eng, slot, HUD_PRINTCENTER, "#All_VIP_Slots_Full", &[]);
    false
}

/// `CHalfLifeMultiplay::ResetCurrentVIP` (`multiplay_gamerules.cpp` 2260-2302, CS 1.6 vanilla,
/// read 2026-11-23): restores a random regular CT skin to the current VIP and strips VIP status
/// (`m_bIsVIP = false`, `m_bNotKilled = false`). Quirk kept: `m_pVIP` itself is not cleared (only
/// `MakeVIP` and a disconnect change it).
pub(crate) fn reset_current_vip(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let Some(slot) = g.rules.vip_slot else {
        return;
    };
    let roll = g.random_range(0, 4);
    let class = match roll {
        1 => 2, // gsg9
        2 => 3, // sas
        3 => 4, // gign
        _ => 1, // urban
    };
    if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        p.is_vip = false;
        p.not_killed = false;
        p.class = class;
    }
    eng.set_client_key(
        slot,
        "model",
        crate::player::model_name(Team::Ct, class),
    );
}

/// `CBasePlayer::MakeVIP` (`player.cpp` 3794-3811): makes `slot` the Counter-Terrorist VIP.
pub(crate) fn make_vip(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        p.is_vip = true;
        p.team = Team::Ct;
        p.not_killed = false;
    }
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.body = 0;
    }
    eng.set_client_key(slot, "model", "vip");
    tracing::info!(slot, "Became_VIP");
    g.rules.vip_slot = Some(slot);
    g.rules.consecutive_vip = 1;
}

/// `CHalfLifeMultiplay::PickNextVIP` (`multiplay_gamerules.cpp` 2305-2395, read 2026-11-23),
/// statement by statement: (1) a non-empty queue promotes its first entry; (2) else, with
/// `m_iConsecutiveVIP >= 3`, the `m_iLastPick`-th CT in entity order takes over (quirks kept: the
/// pick is stale when `m_iNumCT` is, a pick equal to the current VIP is swapped for the previous
/// non-spectator player, who may be a T); (3) else, with no VIP, the first CT. `RestartRound`
/// increments `m_iConsecutiveVIP` afterwards. Players are walked in slot order (the engine's
/// `UTIL_FindEntityByClassname("player")` order); a free slot stands for a dormant edict.
pub(crate) fn pick_next_vip(g: &mut GameState, eng: &mut EngineApi<'_>) {
    if !is_vip_queue_empty(g) {
        // Remove the current VIP from his VIP status and make him a regular CT.
        if g.rules.vip_slot.is_some() {
            reset_current_vip(g, eng);
        }
        for i in 0..MAX_VIP_QUEUES {
            if let Some(slot) = g.rules.vip_queue[i] {
                make_vip(g, eng, slot);
                g.rules.vip_queue[i] = None;
                stack_vip_queue(g);
                g.rules.consecutive_vip = 0;
                return;
            }
        }
    } else if g.rules.consecutive_vip >= 3 {
        g.rules.last_vip_pick += 1;
        if g.rules.last_vip_pick > g.rules.num_ct as i32 {
            g.rules.last_vip_pick = 1;
        }
        let mut count = 1;
        let mut last_player: Option<u8> = None;
        for idx in 0..g.players.len() {
            let Some(team) = g.players[idx].as_ref().map(|p| p.team) else {
                continue;
            };
            let slot = idx as u8;
            if team == Team::Ct && count == g.rules.last_vip_pick {
                let mut pick = slot;
                if g.rules.vip_slot == Some(slot)
                    && let Some(prev) = last_player
                {
                    pick = prev;
                }
                // Remove the current VIP from his VIP status and make him a regular CT.
                if g.rules.vip_slot.is_some() {
                    reset_current_vip(g, eng);
                }
                make_vip(g, eng, pick);
                g.rules.consecutive_vip = 0;
                break;
            }
            if team == Team::Ct {
                count += 1;
            }
            if team != Team::Spectator {
                last_player = Some(slot);
            }
        }
    } else if g.rules.vip_slot.is_none() {
        // There is no VIP and no one waiting: just pick the first CT player we can find.
        let first = g
            .players
            .iter()
            .position(|p| p.as_ref().is_some_and(|p| p.team == Team::Ct));
        if let Some(idx) = first {
            make_vip(g, eng, idx as u8);
            g.rules.consecutive_vip = 0;
        }
    }
}

/// `become_vip` console command (`client.cpp` 3273-3286, vanilla `!REGAMEDLL_FIXES`).
pub(crate) fn become_vip_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let eligible = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.joining == crate::teams::JoinState::Joined && p.team == Team::Ct);
    if !eligible {
        client_print(eng, slot, HUD_PRINTCENTER, "#Command_Not_Available", &[]);
        return;
    }
    add_to_vip_queue(g, eng, slot);
}

/// `CHalfLifeMultiplay::ClientDisconnected` (`multiplay_gamerules.cpp` 3604-3607, read
/// 2026-11-23): only `m_pVIP = nullptr` when the leaver is the VIP; `m_iConsecutiveVIP` and
/// `m_iLastPick` are kept. Deviation: queue entries are slots here, not pointers, so a leaver's
/// entry is purged (the original leaves a dangling pointer that `IsVIPQueueEmpty` would test).
pub(crate) fn on_disconnect(g: &mut GameState, slot: u8) {
    for entry in &mut g.rules.vip_queue {
        if *entry == Some(slot) {
            *entry = None;
        }
    }
    if g.rules.vip_slot == Some(slot) {
        g.rules.vip_slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn stack_vip_queue_is_the_one_pass_loop_of_the_source() {
        // `i = 3`: `[2]` empty -> `[2] = [3]`, `[3] = [4]`; `i = 1`: `[0]` empty -> `[0] = [1]`,
        // `[1] = [2]`, `[2] = [3]` ... one pass, not a full compaction (the quirk is kept).
        let mut g = GameState::new();
        g.rules.vip_queue = [None, Some(3), None, Some(1), Some(4)];
        stack_vip_queue(&mut g);
        assert_eq!(g.rules.vip_queue, [Some(3), Some(1), None, Some(4), None]);
        // A queue that is already packed stays as it is.
        g.rules.vip_queue = [Some(3), Some(1), Some(4), None, None];
        stack_vip_queue(&mut g);
        assert_eq!(g.rules.vip_queue, [Some(3), Some(1), Some(4), None, None]);
    }

    #[test]
    fn is_vip_queue_empty_prunes_non_ct_and_disconnects() {
        let mut g = GameState::new();
        let mut t = Player::new(800);
        t.team = Team::Terrorist;
        let mut ct = Player::new(800);
        ct.team = Team::Ct;
        g.players[1] = Some(t);
        g.players[2] = Some(ct);
        g.rules.vip_queue = [Some(0), Some(1), Some(2), None, None];
        assert!(!is_vip_queue_empty(&mut g));
        assert_eq!(g.rules.vip_queue, [Some(2), None, None, None, None]);

        on_disconnect(&mut g, 2);
        assert!(is_vip_queue_empty(&mut g));
    }
}
