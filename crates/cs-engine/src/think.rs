//! Entity think scheduling: calls [`GameDll::think`] for every entity whose `nextthink` has
//! come, once per server frame.

use crate::edict::EntVars;
use crate::game::GameDll;
use crate::server::Server;

/// Entities [`GameDll::think`] is called for in one server frame; the rest keep their
/// `nextthink` and are called in the next frame.
pub const MAX_THINKS_PER_FRAME: usize = 4096;

/// Whether `vars` has a think scheduled at or before `time`: `0 < nextthink <= time`. A
/// `nextthink` that is not finite counts as not scheduled; the field itself is left alone.
fn is_due(vars: &EntVars, time: f64) -> bool {
    let at = vars.nextthink;
    at.is_finite() && at > 0.0 && at <= time
}

impl<G: GameDll> Server<G> {
    /// Calls `think` for the live entities that are due at the server time of this frame, in
    /// ascending index order and at most [`MAX_THINKS_PER_FRAME`] of them.
    ///
    /// The due entities are collected first (one pass over the pool, stopping at the cap). Just
    /// before an entity's turn it is checked again: an entity a previous think removed (its
    /// serial no longer resolves) or rescheduled into the future is skipped. Its `nextthink` is
    /// then set to 0 and `think` is called, so a think that reschedules itself to the current
    /// time waits for the next frame, and an entity scheduled during the pass was not collected.
    /// Each call is a game call like any other: what the game queued is drained afterwards.
    ///
    /// With more than [`MAX_THINKS_PER_FRAME`] entities due in every frame, the highest indices
    /// can starve: the order is ascending and always starts at the same index. Acceptable for
    /// now; HLDS has no cap.
    ///
    /// Known differences from HLDS (assumptions from memory, not verified against the source):
    /// HLDS runs `StartFrame` and the thinks inside `SV_Physics`, after the frame's packets were
    /// read, while here they run before the packets; and HLDS sets the global time to the
    /// entity's own think time for the call, while `EngineApi::time` is the frame's server time
    /// here. Revisit both when physics arrives.
    pub(crate) fn run_thinks(&mut self) {
        let time = self.time;
        let due: Vec<_> = self
            .edicts
            .iter_live()
            .filter(|(_, edict)| is_due(&edict.vars, time))
            .map(|(id, _)| id)
            .take(MAX_THINKS_PER_FRAME)
            .collect();
        for id in due {
            let Some(edict) = self.edicts.get_mut(id) else {
                continue;
            };
            if !is_due(&edict.vars, time) {
                continue;
            }
            edict.vars.nextthink = 0.0;
            self.with_game(|game, eng| game.think(eng, id));
        }
    }
}
