//! Touches and impacts: the dispatch of [`GameDll::touch`].
//!
//! Ported from ReHLDS `rehlds/engine/world.cpp` (`SV_TouchLinks`, the brush-trigger hull test;
//! read 2026-11-21). `SV_Impact` (`sv_phys.cpp`) is written from memory of its behaviour
//! (assumption, not checked against the source): it calls the touch of each side when neither
//! entity is `SOLID_NOT`. The box test and the queueing happen in `EngineApi::link_entity`
//! (`world.rs`); this file holds the hull test for brush triggers, [`Server::impact`] and the
//! dispatch loop.
//!
//! Difference from HLDS: there, `pfnTouch` is called synchronously inside `SV_LinkEdict` and
//! `SV_Impact`, in the middle of the game's own call. Here the touch is queued and dispatched
//! after the game call (or the physics step) that found it has returned, so the game is never
//! entered twice at once. The order of the calls can differ from the original's (the code that
//! linked the entity runs to the end of its call before the touch is delivered); assumption:
//! harmless for game logic, which cannot rely on the order of a touch against the rest of the
//! call that caused it.
//!
//! The queue is drained after every game call. Touches a touch causes (it relinks or hits
//! something) are appended behind the ones waiting and dispatched in the same loop. At most
//! [`MAX_TOUCHES_PER_FRAME`] touches are queued per frame; the rest are dropped with one
//! `tracing::warn!` at the end of the frame. HLDS has no such bound (it prevents recursion
//! with a semaphore instead).

use glam::Vec3;

use crate::collision::CONTENTS_SOLID;
use crate::edict::{EdictId, EntVars};
use crate::game::{EngineApi, GameDll};
use crate::server::Server;
use crate::trace_ents::{brush_model, bsp_hull_for, bsp_offset};
use crate::world::consts::SOLID_NOT;

/// Touches queued, and so dispatched, in one server frame; further ones are dropped.
pub const MAX_TOUCHES_PER_FRAME: usize = 4096;

impl EngineApi<'_> {
    /// The precise test of `SV_TouchLinks` for a brush trigger: the hull `SV_HullForBsp` picks
    /// for a box of `mins..maxs` must hold `origin` (shifted by the trigger's origin) as solid.
    /// A trigger that is not a brush model of this map is accepted: its box already matched.
    pub(crate) fn brush_trigger_holds(
        &self,
        trigger: &EntVars,
        origin: Vec3,
        mins: Vec3,
        maxs: Vec3,
    ) -> bool {
        let Some(collision) = self.map.collision() else {
            return true;
        };
        let Some(model) = brush_model(trigger).filter(|&m| m < collision.model_count()) else {
            return true;
        };
        let hull = bsp_hull_for(mins, maxs);
        let offset = bsp_offset(hull, mins, trigger.origin);
        collision.model_point_contents(model, offset, origin, hull) == CONTENTS_SOLID
    }
}

impl<G: GameDll> Server<G> {
    /// `SV_Impact`: queues `touch(e1, e2)` then `touch(e2, e1)`, each only if neither entity is
    /// `SOLID_NOT`. Both must be live. The pairs are dispatched at the next dispatch point
    /// (after the current game call, or by [`dispatch_touches`](Self::dispatch_touches)).
    // Called by the physics and the player movement of later tasks.
    #[allow(dead_code)]
    pub(crate) fn impact(&mut self, e1: EdictId, e2: EdictId) {
        let solid = |id| self.edicts.get(id).map(|e| e.vars.solid);
        let (Some(s1), Some(s2)) = (solid(e1), solid(e2)) else {
            return;
        };
        if s1 == SOLID_NOT || s2 == SOLID_NOT {
            return;
        }
        self.world.queue_touch(e1, e2);
        self.world.queue_touch(e2, e1);
    }

    /// Calls [`GameDll::touch`] for every queued pair, oldest first, until the queue is empty.
    /// Before a pair its two entities are checked: one removed since (its serial no longer
    /// resolves) drops the pair. Each call is a game call like any other, so what the game
    /// queued is drained afterwards; the touches it caused join the queue and are delivered by
    /// this loop. A call made while the loop is already running (a kick released a slot, whose
    /// disconnect is a game call too) leaves the queue to the running loop.
    pub(crate) fn dispatch_touches(&mut self) {
        if self.world.dispatching {
            return;
        }
        self.world.dispatching = true;
        while let Some((toucher, other)) = self.world.pop_touch() {
            if self.edicts.get(toucher).is_none() || self.edicts.get(other).is_none() {
                continue;
            }
            self.call_game(|game, eng| game.touch(eng, toucher, other));
            self.drain_messages();
        }
        self.world.dispatching = false;
    }

    /// End of a frame's game work: delivers what is still queued, warns once about touches
    /// dropped over [`MAX_TOUCHES_PER_FRAME`] and gives the next frame a fresh budget.
    pub(crate) fn finish_frame_touches(&mut self) {
        self.dispatch_touches();
        let dropped = self.world.reset_touch_budget();
        if dropped > 0 {
            tracing::warn!(
                dropped,
                cap = MAX_TOUCHES_PER_FRAME,
                "touches over the per-frame cap were dropped"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use cs_cvar::CVarRegistry;
    use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
    use cs_proto48::usermsg::UserMsgRegistry;

    use super::*;
    use crate::map::ServerMap;
    use crate::server::ServerConfig;
    use crate::world::consts::{SOLID_BBOX, SOLID_TRIGGER};

    /// Records the touches it receives.
    #[derive(Default)]
    struct Recorder {
        touches: Vec<(u16, u16)>,
    }

    impl GameDll for Recorder {
        fn usermsgs(&self) -> UserMsgRegistry {
            UserMsgRegistry::cs16_default()
        }
        fn server_activate(&mut self, _eng: &mut EngineApi<'_>) {}
        fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {}
        fn spawn_entity(
            &mut self,
            _eng: &mut EngineApi<'_>,
            _id: EdictId,
            _classname: &str,
            _keys: &[(String, String)],
        ) -> bool {
            false
        }
        fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
            Ok(())
        }
        fn client_put_in_server(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
        fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
        fn client_command(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _argv: &[String]) {}
        fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
        fn run_usercmd(
            &mut self,
            _eng: &mut EngineApi<'_>,
            _slot: u8,
            _cmd: &GoldSrcUserCmd,
            _random_seed: u32,
        ) {
        }
        fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {}
        fn update_client_data(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> GoldSrcClientData {
            GoldSrcClientData::default()
        }
        fn touch(&mut self, _eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
            self.touches.push((toucher.index, other.index));
        }
    }

    fn server() -> Server<Recorder> {
        Server::new(
            ServerConfig::default(),
            ServerMap::stub("impact"),
            Recorder::default(),
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        )
    }

    fn entity(server: &mut Server<Recorder>, solid: u8) -> EdictId {
        server.call_game(|_, eng| {
            let id = eng.create_entity("e").expect("free edict");
            eng.ent_mut(id).unwrap().solid = solid;
            id
        })
    }

    #[test]
    fn impact_touches_both_sides_in_order() {
        let mut s = server();
        let a = entity(&mut s, SOLID_BBOX);
        let b = entity(&mut s, SOLID_TRIGGER);
        s.impact(a, b);
        // Queued, not yet delivered.
        assert!(s.game.touches.is_empty());
        s.drain_outbox();
        assert_eq!(s.game.touches, [(a.index, b.index), (b.index, a.index)]);
    }

    #[test]
    fn impact_ignores_a_non_solid_side_and_a_stale_one() {
        let mut s = server();
        let a = entity(&mut s, SOLID_BBOX);
        let ghost = entity(&mut s, SOLID_NOT);
        let gone = entity(&mut s, SOLID_BBOX);
        s.call_game(|_, eng| eng.remove_entity(gone));
        s.impact(a, ghost);
        s.impact(ghost, a);
        s.impact(a, gone);
        s.drain_outbox();
        assert!(s.game.touches.is_empty());
    }

    #[test]
    fn a_pair_whose_entity_was_removed_before_dispatch_is_dropped() {
        let mut s = server();
        let a = entity(&mut s, SOLID_BBOX);
        let b = entity(&mut s, SOLID_BBOX);
        s.impact(a, b);
        s.call_game(|_, eng| eng.remove_entity(b));
        s.dispatch_touches();
        assert!(s.game.touches.is_empty());
    }
}
