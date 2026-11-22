//! The hint system: `CBasePlayer::HintMessage`/`HintMessageEx`, the per-player queue
//! `CHintMessageQueue` and the display history `m_flDisplayHistory` (`DHF_*`) that keeps each
//! hint from being shown twice. Ported from ReGameDLL_CS `hintmessage.h`/`hintmessage.cpp`,
//! `player.cpp` (`HintMessageEx`, `HintMessage`, `PreThink`, `SetPrefsFromUserinfo`) and
//! `util.cpp` (`UTIL_ShowMessageArgs`), read 2026-11-22.
//!
//! A hint is queued by [`hint_message`] and shown by [`update_queue`] (`PreThink`), one at a
//! time, each for its duration (6 seconds by default). The queue sends `HudTextArgs` with the
//! text, `isHint` and no arguments: `CHintMessage::Send` always passes its (empty) argument
//! vector, so the `HudTextPro` branch of `UTIL_ShowMessageArgs` is never taken from here. A
//! client that sets the userinfo `_ah` to 0 (`cl_autohelp`) gets none of the hints that are not
//! forced (`bOverride`).
//!
//! The hints whose cause is not ported (bomb, hostages, the VIP, the zones of the objectives)
//! are not sent; each file that ports the cause names its hint.

use std::collections::VecDeque;

use cs_engine::{EngineApi, MsgDest};

use crate::GameState;
use crate::messages::send_raw;
use crate::player::DEAD_NO;

/// `DHF_*` (`hintmessage.h`): the hints a player has been shown. (`BIT(n)` is `1 << n`.) Only the
/// flags of code that exists are here; the hostage and zone flags come with 2E.
pub(crate) mod dhf {
    pub const ROUND_STARTED: u32 = 1 << 1;
    pub const HOSTAGE_KILLED: u32 = 1 << 6;
    pub const FRIEND_SEEN: u32 = 1 << 7;
    pub const ENEMY_SEEN: u32 = 1 << 8;
    pub const FRIEND_INJURED: u32 = 1 << 9;
    pub const FRIEND_KILLED: u32 = 1 << 10;
    pub const ENEMY_KILLED: u32 = 1 << 11;
    pub const BOMB_RETRIEVED: u32 = 1 << 12;
    pub const AMMO_EXHAUSTED: u32 = 1 << 15;
    pub const IN_TARGET_ZONE: u32 = 1 << 16;
    pub const IN_RESCUE_ZONE: u32 = 1 << 17;
    pub const IN_ESCAPE_ZONE: u32 = 1 << 18;
    pub const IN_VIPSAFETY_ZONE: u32 = 1 << 19;
    pub const NIGHTVISION: u32 = 1 << 20;
    pub const SPEC_DUCK: u32 = 1 << 22;

    /// `DHM_ROUND_CLEAR`: what a spawn forgets.
    pub const ROUND_CLEAR: u32 = ROUND_STARTED | HOSTAGE_KILLED | FRIEND_KILLED | BOMB_RETRIEVED;
}

/// The seconds a hint stays up (`HintMessage` passes `6.0f` to `HintMessageEx`).
const HINT_DURATION: f32 = 6.0;

/// `CHintMessage` without arguments: the text, whether it is a hint (`isHint`) and for how
/// long it holds the queue.
#[derive(Debug, Clone)]
struct QueuedHint {
    text: String,
    is_hint: bool,
    duration: f32,
}

/// `CHintMessageQueue`: `m_tmMessageEnd` and the messages still to show.
#[derive(Debug, Clone, Default)]
pub(crate) struct HintQueue {
    /// `m_tmMessageEnd`: server time the hint on screen is done.
    message_end: f32,
    messages: VecDeque<QueuedHint>,
}

impl HintQueue {
    /// `CHintMessageQueue::Reset`.
    pub(crate) fn reset(&mut self) {
        self.message_end = 0.0;
        self.messages.clear();
    }

    /// `CHintMessageQueue::AddMessage` (no arguments): always succeeds.
    fn add_message(&mut self, text: &str, duration: f32, is_hint: bool) -> bool {
        self.messages.push_back(QueuedHint {
            text: text.to_string(),
            is_hint,
            duration,
        });
        true
    }

    /// `IsEmpty`.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// `CHintMessageQueue::Update`: the next hint, when the one on screen is done (`now` is
    /// `gpGlobals->time`). Returns it, the queue having started its time.
    fn next_due(&mut self, now: f32) -> Option<QueuedHint> {
        if now <= self.message_end {
            return None;
        }
        let msg = self.messages.pop_front()?;
        self.message_end = now + msg.duration;
        Some(msg)
    }
}

/// `CBasePlayer::HintMessageEx(pMessage, duration, bDisplayIfPlayerDead, bOverride)`. A player
/// that is not alive gets no hint unless `display_if_dead`; one that turned the hints off
/// (`_ah 0`) gets only the forced ones (`override_hints`), and `true` is still answered for
/// the ones it does not get. `false` when nothing is queued because the player is dead.
pub(crate) fn hint_message_ex(
    g: &mut GameState,
    eng: &EngineApi<'_>,
    slot: u8,
    text: &str,
    duration: f32,
    display_if_dead: bool,
    override_hints: bool,
) -> bool {
    // `IsAlive()`: `pev->deadflag == DEAD_NO`.
    let alive = eng
        .ent(eng.client_edict(slot))
        .is_some_and(|v| v.deadflag == DEAD_NO);
    if !display_if_dead && !alive {
        return false;
    }
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    if override_hints || p.show_hints {
        return p.hint_queue.add_message(text, duration, true);
    }
    true
}

/// `CBasePlayer::HintMessage(pMessage, bDisplayIfPlayerDead, bOverride)`: a hint of the
/// default duration.
pub(crate) fn hint_message(
    g: &mut GameState,
    eng: &EngineApi<'_>,
    slot: u8,
    text: &str,
    display_if_dead: bool,
    override_hints: bool,
) -> bool {
    hint_message_ex(g, eng, slot, text, HINT_DURATION, display_if_dead, override_hints)
}

/// `CBasePlayer::HintMessage(text)` with the default arguments (`FALSE`, `FALSE`).
pub(crate) fn hint(g: &mut GameState, eng: &EngineApi<'_>, slot: u8, text: &str) -> bool {
    hint_message(g, eng, slot, text, false, false)
}

/// The `m_flDisplayHistory` test the hint call sites share: the player has not been shown the
/// hint `flag` yet. Sets the flag and returns `true` when it is the first time.
pub(crate) fn first_time(g: &mut GameState, slot: u8, flag: u32) -> bool {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    if p.display_history & flag != 0 {
        return false;
    }
    p.display_history |= flag;
    true
}

/// `m_hintMessageQueue.Update(this)` (`PreThink`, "Hint messages should be updated even if the
/// game is over"): shows the next hint if the last one has had its time.
pub(crate) fn update_queue(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time() as f32;
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    if let Some(msg) = p.hint_queue.next_due(now) {
        show_message_args(eng, slot, &msg.text, msg.is_hint);
    }
}

/// `UTIL_ShowMessageArgs(text, player, &args, isHint)` with no arguments: `HudTextArgs` (the
/// text, `isHint`, the argument count 0) to the player.
fn show_message_args(eng: &mut EngineApi<'_>, slot: u8, text: &str, is_hint: bool) {
    let mut payload = Vec::with_capacity(text.len() + 3);
    // A hint text is a short message key; the message is at most 255 bytes.
    let bytes = text.as_bytes();
    payload.extend_from_slice(&bytes[..bytes.len().min(250)]);
    payload.push(0);
    payload.push(u8::from(is_hint));
    payload.push(0);
    send_raw(eng, MsgDest::One(slot), "HudTextArgs", &payload);
}

/// `SetPrefsFromUserinfo`'s `_ah` (`cl_autohelp`): on when the key is missing or empty.
pub(crate) fn prefers_hints(eng: &EngineApi<'_>, slot: u8) -> bool {
    match eng.client_key(slot, "_ah") {
        Some(v) if !v.is_empty() => crate::menus::atoi(v) != 0,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_shows_one_hint_at_a_time_for_its_duration() {
        let mut q = HintQueue::default();
        assert!(q.next_due(1.0).is_none());
        q.add_message("#a", 6.0, true);
        q.add_message("#b", 6.0, true);
        let first = q.next_due(1.0).expect("a is due");
        assert_eq!(first.text, "#a");
        // The one on screen holds the queue until `now > end` (the test is `<=`).
        assert!(q.next_due(7.0).is_none());
        assert_eq!(q.next_due(7.01).expect("b is due").text, "#b");
        assert!(q.is_empty());
    }

    #[test]
    fn reset_forgets_the_queue_and_its_clock() {
        let mut q = HintQueue::default();
        q.add_message("#a", 6.0, true);
        q.next_due(1.0);
        q.add_message("#b", 6.0, true);
        q.reset();
        assert!(q.is_empty());
        q.add_message("#c", 6.0, true);
        assert_eq!(q.next_due(0.5).expect("clock is 0 again").text, "#c");
    }

    #[test]
    fn the_masks_are_those_of_hintmessage_h() {
        assert_eq!(
            dhf::ROUND_CLEAR,
            dhf::ROUND_STARTED | dhf::HOSTAGE_KILLED | dhf::FRIEND_KILLED | dhf::BOMB_RETRIEVED
        );
        assert_eq!(dhf::SPEC_DUCK, 1 << 22);
        assert_eq!(dhf::AMMO_EXHAUSTED, 1 << 15);
    }
}
