//! Touches and impacts: the dispatch of [`GameDll::touch`].
//!
//! Ported from ReHLDS `rehlds/engine/world.cpp` (`SV_TouchLinks`, the brush-trigger hull test;
//! read 2026-11-21) and `rehlds/engine/sv_phys.cpp` (`SV_Impact`, read 2026-11-21; see
//! [`Server::impact`] for what is left out). The box test and the queueing happen in `EngineApi::link_entity`
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
//! The queue is drained after every game call (`drain_outbox`), after each entity's step of the
//! physics pass (`physics.rs`, whose moves link without a game call) and at the end of the
//! frame's physics (`finish_frame_touches`); the settle passes of map activation are physics
//! passes too, so their touches are delivered before the baselines are cut. Touches a touch causes (it relinks or hits
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
use crate::world::consts::{FL_KILLME, SOLID_NOT, SOLID_TRIGGER};
use crate::world::{PendingTouch, TouchKind};

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
    /// `SV_Impact` (ReHLDS `sv_phys.cpp`, read 2026-11-21): nothing happens when either entity
    /// has `FL_KILLME`; `touch(e1, e2)` is queued when `e1` is not `SOLID_NOT`, then
    /// `touch(e2, e1)` when `e2` is not (each side on its own). The second half is skipped when
    /// either entity got `FL_KILLME` during the first (the `REHLDS_FIXES` branch): the dispatch
    /// re-checks `FL_KILLME` before every touch. Either both halves are queued or, when fewer
    /// than two places are left in the frame's budget, neither. A stale entity queues nothing.
    ///
    /// Differences from the source: the `groupinfo` filter is not modelled; the trace globals
    /// (`SV_SetGlobalTrace`) are not set, the game gets no trace; the touches are delivered
    /// after the current call, not inside it.
    // Called by the physics and the player movement of later tasks.
    #[allow(dead_code)]
    pub(crate) fn impact(&mut self, e1: EdictId, e2: EdictId) {
        let vars = |id| self.edicts.get(id).map(|e| (e.vars.solid, e.vars.flags));
        let (Some((s1, f1)), Some((s2, f2))) = (vars(e1), vars(e2)) else {
            return;
        };
        if (f1 | f2) & FL_KILLME != 0 {
            return;
        }
        let half = |toucher, other| PendingTouch {
            toucher,
            other,
            kind: TouchKind::Impact,
        };
        let halves = [half(e1, e2), half(e2, e1)];
        let queued = match (s1 != SOLID_NOT, s2 != SOLID_NOT) {
            (true, true) => &halves[..],
            (true, false) => &halves[..1],
            (false, true) => &halves[1..],
            (false, false) => return,
        };
        self.world.queue_touches(queued);
    }

    /// Calls [`GameDll::touch`] for every queued touch, oldest first, until the queue is empty.
    /// Before each one the world is checked again: an entity removed since (its serial no longer
    /// resolves) or marked `FL_KILLME` drops the touch, and so does a trigger that is no longer
    /// `SOLID_TRIGGER`. Each call is a game call like any other, so what the game queued is
    /// drained afterwards; the touches it caused join the queue and are delivered by this loop.
    /// A call made while the loop is already running (a kick released a slot, whose disconnect
    /// is a game call too) leaves the queue to the running loop.
    pub(crate) fn dispatch_touches(&mut self) {
        if self.world.dispatching {
            return;
        }
        self.world.dispatching = true;
        while let Some(PendingTouch {
            toucher,
            other,
            kind,
        }) = self.world.pop_touch()
        {
            let vars = |id| self.edicts.get(id).map(|e| (e.vars.solid, e.vars.flags));
            let (Some((solid, flags)), Some((_, other_flags))) = (vars(toucher), vars(other))
            else {
                continue;
            };
            if (flags | other_flags) & FL_KILLME != 0
                || (kind == TouchKind::Trigger && solid != SOLID_TRIGGER)
            {
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
            self.net_stats.touches_dropped += dropped as u64;
            self.net_stats.touch_cap_frames += 1;
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
    use crate::world::consts::SOLID_BBOX;

    /// Records the touches it receives.
    #[derive(Default)]
    struct Recorder {
        touches: Vec<(u16, u16)>,
        /// The first touch marks its `other` with `FL_KILLME`.
        kill_other_on_touch: bool,
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
        fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
            self.touches.push((toucher.index, other.index));
            if self.kill_other_on_touch {
                eng.ent_mut(other).unwrap().flags |= FL_KILLME;
            }
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
    fn impact_ignores_a_stale_entity() {
        let mut s = server();
        let a = entity(&mut s, SOLID_BBOX);
        let gone = entity(&mut s, SOLID_BBOX);
        s.call_game(|_, eng| eng.remove_entity(gone));
        s.impact(a, gone);
        s.impact(gone, a);
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

    #[test]
    fn impact_does_nothing_when_either_side_is_killed() {
        let mut s = server();
        let a = entity(&mut s, SOLID_BBOX);
        let b = entity(&mut s, SOLID_BBOX);
        s.call_game(|_, eng| eng.ent_mut(b).unwrap().flags |= FL_KILLME);
        s.impact(a, b);
        s.impact(b, a);
        s.drain_outbox();
        assert!(s.game.touches.is_empty());
    }

    #[test]
    fn impact_sides_are_independent() {
        let mut s = server();
        let solid = entity(&mut s, SOLID_BBOX);
        let ghost = entity(&mut s, SOLID_NOT);
        // A non-solid side gets no touch, the other side still does.
        s.impact(solid, ghost);
        s.impact(ghost, solid);
        s.drain_outbox();
        assert_eq!(
            s.game.touches,
            [(solid.index, ghost.index), (solid.index, ghost.index)]
        );
    }

    #[test]
    fn second_half_is_skipped_when_the_first_killed_an_entity() {
        let mut s = server();
        s.game.kill_other_on_touch = true;
        let a = entity(&mut s, SOLID_BBOX);
        let b = entity(&mut s, SOLID_BBOX);
        s.impact(a, b);
        s.drain_outbox();
        assert_eq!(s.game.touches, [(a.index, b.index)]);
    }

    #[test]
    fn impact_queues_both_halves_or_neither() {
        let mut s = server();
        let a = entity(&mut s, SOLID_BBOX);
        let b = entity(&mut s, SOLID_BBOX);
        // Leave room for one touch only.
        for _ in 0..MAX_TOUCHES_PER_FRAME - 1 {
            s.world.queue_touches(&[PendingTouch {
                toucher: a,
                other: b,
                kind: TouchKind::Impact,
            }]);
        }
        s.impact(a, b);
        assert_eq!(s.world.reset_touch_budget(), 2);
        s.dispatch_touches();
        assert_eq!(s.game.touches.len(), MAX_TOUCHES_PER_FRAME - 1);
    }

    #[test]
    fn trigger_touches_are_rechecked_at_dispatch() {
        let mut s = server();
        let trigger = entity(&mut s, SOLID_TRIGGER);
        let other = entity(&mut s, SOLID_BBOX);
        s.world.queue_touch(trigger, other);
        s.call_game(|_, eng| eng.ent_mut(trigger).unwrap().solid = SOLID_NOT);
        s.dispatch_touches();
        assert!(s.game.touches.is_empty());
        // An impact side is not held to being a trigger.
        s.world.queue_touches(&[PendingTouch {
            toucher: trigger,
            other,
            kind: TouchKind::Impact,
        }]);
        s.dispatch_touches();
        assert_eq!(s.game.touches.len(), 1);
    }
}
