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

/// `CHalfLifeMultiplay::StackVIPQueue` (`multiplay_gamerules.cpp` 2191-2204): compacts occupied
/// entries toward the front of `m_pVIPQueue` while preserving FIFO order.
pub(crate) fn stack_vip_queue(g: &mut GameState) {
    let mut write = 0usize;
    for read in 0..MAX_VIP_QUEUES {
        if let Some(slot) = g.rules.vip_queue[read].take() {
            g.rules.vip_queue[write] = Some(slot);
            write += 1;
        }
    }
}

/// Drops any queue entry whose player left or is no longer on Counter-Terrorists, then compacts.
fn prune_vip_queue(g: &mut GameState) {
    for entry in &mut g.rules.vip_queue {
        let valid_ct = entry.is_some_and(|slot| {
            g.players
                .get(usize::from(slot))
                .and_then(Option::as_ref)
                .is_some_and(|p| p.team == Team::Ct)
        });
        if !valid_ct {
            *entry = None;
        }
    }
    stack_vip_queue(g);
}

/// `CHalfLifeMultiplay::IsVIPQueueEmpty` (`multiplay_gamerules.cpp` 2206-2218).
pub(crate) fn is_vip_queue_empty(g: &mut GameState) -> bool {
    prune_vip_queue(g);
    g.rules.vip_queue[0].is_none()
}

/// `CHalfLifeMultiplay::AddToVIPQueue` (`multiplay_gamerules.cpp` 2220-2258): queues `slot` (when
/// a CT) for the next VIP pick, informing them of their 1-based queue position or full queue.
pub(crate) fn add_to_vip_queue(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    prune_vip_queue(g);
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

/// `CHalfLifeMultiplay::ResetCurrentVIP` (`multiplay_gamerules.cpp` 2260-2302, CS 1.6 vanilla):
/// restores a random regular CT skin to the current VIP and strips VIP status (`m_bIsVIP = false`,
/// `m_bNotKilled = false` so they respawn with regular gear).
pub(crate) fn reset_current_vip(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let Some(slot) = g.rules.vip_slot.take() else {
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

/// `CHalfLifeMultiplay::PickNextVIP` (`multiplay_gamerules.cpp` 2305-2395): promotes from the VIP
/// queue if non-empty; else rotates after 3 consecutive rounds; else picks the first available CT
/// when no VIP is active. `RestartRound` increments `consecutive_vip` afterward.
pub(crate) fn pick_next_vip(g: &mut GameState, eng: &mut EngineApi<'_>) {
    if !is_vip_queue_empty(g) {
        if g.rules.vip_slot.is_some() {
            reset_current_vip(g, eng);
        }
        let next = g.rules.vip_queue[0].take();
        stack_vip_queue(g);
        if let Some(slot) = next
            && g.players
                .get(usize::from(slot))
                .and_then(Option::as_ref)
                .is_some_and(|p| p.team == Team::Ct)
        {
            make_vip(g, eng, slot);
            g.rules.consecutive_vip = 0;
        }
        return;
    }

    let exclude = if g.rules.vip_slot.is_some() {
        if g.rules.consecutive_vip < 3 {
            return;
        }
        let prev = g.rules.vip_slot;
        reset_current_vip(g, eng);
        prev
    } else {
        None
    };

    let max = g.players.len();
    let start = if exclude.is_some() {
        g.rules
            .last_vip_pick
            .map_or(0, |s| (usize::from(s) + 1) % max)
    } else {
        0
    };
    let mut picked = None;
    let mut fallback = None;
    for offset in 0..max {
        let idx = (start + offset) % max;
        if g.players[idx].as_ref().is_some_and(|p| p.team == Team::Ct) {
            let slot = idx as u8;
            if Some(slot) == exclude {
                fallback = Some(slot);
            } else {
                picked = Some(slot);
                break;
            }
        }
    }
    if let Some(slot) = picked.or(fallback) {
        g.rules.last_vip_pick = Some(slot);
        make_vip(g, eng, slot);
        g.rules.consecutive_vip = 0;
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

/// Releases any VIP slot, queue slot, or rotation cursor held by a disconnecting player
/// (`ClientDisconnected`, `multiplay_gamerules.cpp:3604-3607`).
pub(crate) fn on_disconnect(g: &mut GameState, slot: u8) {
    for entry in &mut g.rules.vip_queue {
        if *entry == Some(slot) {
            *entry = None;
        }
    }
    stack_vip_queue(g);
    if g.rules.vip_slot == Some(slot) {
        g.rules.vip_slot = None;
        g.rules.consecutive_vip = 0;
    }
    if g.rules.last_vip_pick == Some(slot) {
        g.rules.last_vip_pick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;

    #[test]
    fn stack_vip_queue_compacts_preserving_order() {
        let mut g = GameState::new();
        g.rules.vip_queue = [None, Some(3), None, Some(1), Some(4)];
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
