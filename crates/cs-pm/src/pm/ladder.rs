//! Ladders: `PM_Ladder`, `PM_LadderMove` (ReGameDLL_CS `pm_shared.cpp`, read 2026-11-22).

use glam::Vec3;

use super::consts::{
    CONTENTS_SOLID, FL_DUCKING, IN_BACK, IN_FORWARD, IN_JUMP, IN_MOVELEFT, IN_MOVERIGHT, MAX_CLIMB_SPEED,
    MOVETYPE_FLY, MOVETYPE_NOCLIP, MOVETYPE_WALK, PLAYER_DUCKING_MULTIPLIER,
};
use super::math::{angle_vectors, cross, dot, vector_ma, vector_normalize, vector_scale, vector_subtract};
use super::types::PmCtx;

/// `PM_Ladder` (pm_shared.cpp 2344-2375, read 2026-11-22): the index of the first moveent that is
/// a ladder (brush model with `skin == CONTENTS_LADDER`) whose hull contains the player origin.
/// The model, model-type, skin and `PM_HullForBsp` + `PM_HullPointContents` tests are
/// `PmWorld::ladder_contains`.
pub(crate) fn pm_ladder(c: &mut PmCtx<'_>) -> Option<usize> {
    let n = c.w.num_moveents();
    let origin = c.pm.origin;
    let hull = c.pm.usehull;
    (0..n).find(|&i| c.w.ladder_contains(i, origin, hull))
}

/// `PM_LadderMove(pLadder)` (pm_shared.cpp 2218-2342, read 2026-11-22).
///
/// Quirks kept: the ladder centre comes from the model bounds as the engine returns them (no
/// entity origin added); the movetype becomes FLY and the gravity 0 even when the trace to the
/// centre misses (then the velocity is left alone); a jump leaves the ladder as WALK with the
/// velocity `normal * 270`; the "on ground moving away from the ladder" push adds
/// `MAX_CLIMB_SPEED * normal`; `PLAYER_DUCKING_MULTIPLIER` is a `double` literal (0.333), so the
/// ducking speed is a `double` product stored into the `float`.
pub(crate) fn pm_ladder_move(c: &mut PmCtx<'_>, ladder: usize) {
    if c.pm.movetype == MOVETYPE_NOCLIP {
        return;
    }

    let (modelmins, modelmaxs) = c.w.moveent_bounds(ladder);

    let ladder_center = vector_scale(modelmins + modelmaxs, 0.5);

    c.pm.movetype = MOVETYPE_FLY;

    // On ladder, convert movement to be relative to the ladder
    let mut floor = c.pm.origin;
    floor[2] += c.pm.player_mins[c.pm.hull_index()][2] - 1.0;

    let on_floor = c.w.point_contents(floor).0 == CONTENTS_SOLID;

    c.pm.gravity = 0.0;
    let (fraction, normal_vec) = c.w.trace_model(ladder, c.pm.origin, ladder_center);

    if fraction != 1.0 {
        let mut forward = 0.0f32;
        let mut right = 0.0f32;
        let mut fl_speed = MAX_CLIMB_SPEED;

        // they shouldn't be able to move faster than their maxspeed
        if fl_speed > c.pm.maxspeed {
            fl_speed = c.pm.maxspeed;
        }

        let (vpn, v_right, _) = angle_vectors(c.pm.angles);

        if (c.pm.flags & FL_DUCKING) != 0 {
            // `flSpeed *= PLAYER_DUCKING_MULTIPLIER;` — float times the double literal, in double.
            fl_speed = (f64::from(fl_speed) * PLAYER_DUCKING_MULTIPLIER) as f32;
        }

        let buttons = i32::from(c.pm.cmd.buttons);
        if (buttons & IN_BACK) != 0 {
            forward -= fl_speed;
        }
        if (buttons & IN_FORWARD) != 0 {
            forward += fl_speed;
        }
        if (buttons & IN_MOVELEFT) != 0 {
            right -= fl_speed;
        }
        if (buttons & IN_MOVERIGHT) != 0 {
            right += fl_speed;
        }

        if (buttons & IN_JUMP) != 0 {
            c.pm.movetype = MOVETYPE_WALK;
            c.pm.velocity = vector_scale(normal_vec, 270.0);
        } else if forward != 0.0 || right != 0.0 {
            let mut velocity = vector_scale(vpn, forward);
            velocity = vector_ma(velocity, right, v_right);

            let mut tmp = Vec3::new(0.0, 0.0, 1.0);

            let mut perp = cross(tmp, normal_vec);
            vector_normalize(&mut perp);

            // decompose velocity into ladder plane
            let normal = dot(velocity, normal_vec);
            // This is the velocity into the face of the ladder
            let cross_v = vector_scale(normal_vec, normal);

            // This is the player's additional velocity
            let lateral = vector_subtract(velocity, cross_v);

            // This turns the velocity into the face of the ladder into velocity that
            // is roughly vertically perpendicular to the face of the ladder.
            // NOTE: It IS possible to face up and move down or face down and move up
            // because the velocity is a sum of the directional velocity and the converted
            // velocity through the face of the ladder - by design.
            tmp = cross(normal_vec, perp);
            c.pm.velocity = vector_ma(lateral, -normal, tmp);

            // On ground moving away from the ladder
            if on_floor && normal > 0.0 {
                c.pm.velocity = vector_ma(c.pm.velocity, MAX_CLIMB_SPEED, normal_vec);
            }
        } else {
            c.pm.velocity = Vec3::ZERO;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pm::{PlayerMove, PmShared, PmTrace, PmWorld, pm_init};

    /// A world with one ladder: the trace to its centre hits with `normal` at `fraction`,
    /// `floor_solid` is the answer of the point-contents probe under the feet.
    struct LadderWorld {
        normal: Vec3,
        fraction: f32,
        floor_solid: bool,
    }

    impl PmWorld for LadderWorld {
        fn player_trace(&mut self, _s: Vec3, e: Vec3, _h: i32, _f: i32, _i: i32) -> PmTrace {
            PmTrace::miss(e)
        }
        fn test_player_position(&mut self, _o: Vec3, p: Vec3, _h: i32) -> (i32, PmTrace) {
            (-1, PmTrace::miss(p))
        }
        fn point_contents(&mut self, _p: Vec3) -> (i32, i32) {
            let c = if self.floor_solid { CONTENTS_SOLID } else { -1 };
            (c, c)
        }
        fn num_moveents(&self) -> usize {
            1
        }
        fn ladder_contains(&mut self, _m: usize, _o: Vec3, _h: i32) -> bool {
            true
        }
        fn moveent_bounds(&self, _m: usize) -> (Vec3, Vec3) {
            (Vec3::new(20.0, -16.0, 0.0), Vec3::new(24.0, 16.0, 100.0))
        }
        fn physent_is_player(&self, _ent: i32) -> bool {
            false
        }
        fn physent_has_model(&self, _ent: i32) -> bool {
            false
        }
        fn trace_model(&mut self, _m: usize, _s: Vec3, _e: Vec3) -> (f32, Vec3) {
            (self.fraction, self.normal)
        }
        fn trace_texture(&mut self, _g: i32, _s: Vec3, _e: Vec3) -> Option<String> {
            None
        }
        fn play_sound(&mut self, _c: i32, _s: &str, _v: f32, _a: f32, _f: i32, _p: i32) {}
        fn random_long(&mut self, low: i32, _high: i32) -> i32 {
            low
        }
        fn sys_float_time(&mut self) -> f64 {
            0.0
        }
        fn physent_by_info(&self, _i: i32) -> Option<(Vec3, Vec3)> {
            None
        }
    }

    fn run(pm: &mut PlayerMove, w: &mut LadderWorld) {
        let mut sh: PmShared = pm_init(None);
        let mut c = PmCtx { pm, sh: &mut sh, w };
        pm_ladder_move(&mut c, 0);
    }

    fn world(floor_solid: bool) -> LadderWorld {
        LadderWorld { normal: Vec3::new(-1.0, 0.0, 0.0), fraction: 0.5, floor_solid }
    }

    #[test]
    fn ducking_scales_the_climb_speed_by_the_double_literal() {
        // flSpeed = 200 * 0.333 (double product stored to float); facing the ladder (yaw 0,
        // normal -x): velocity = (flSpeed, 0, 0) -> (0, 0, flSpeed).
        let mut pm = PlayerMove { maxspeed: 250.0, flags: FL_DUCKING, ..PlayerMove::default() };
        pm.cmd.buttons = IN_FORWARD as u16;
        run(&mut pm, &mut world(false));
        let expected = (200.0f64 * 0.333) as f32;
        assert_eq!(pm.velocity, Vec3::new(0.0, 0.0, expected));
    }

    #[test]
    fn ducking_multiplier_is_the_c_double_not_the_widened_f32() {
        // 69 * 0.333 (double) rounds to a different f32 than 69 * f64::from(0.333f32).
        let mut pm = PlayerMove { maxspeed: 69.0, flags: FL_DUCKING, ..PlayerMove::default() };
        pm.cmd.buttons = IN_FORWARD as u16;
        run(&mut pm, &mut world(false));
        assert_eq!(pm.velocity.z, (69.0f64 * 0.333) as f32);
    }

    #[test]
    fn maxspeed_caps_the_climb_speed() {
        let mut pm = PlayerMove { maxspeed: 150.0, ..PlayerMove::default() };
        pm.cmd.buttons = IN_FORWARD as u16;
        run(&mut pm, &mut world(false));
        assert_eq!(pm.velocity, Vec3::new(0.0, 0.0, 150.0));
    }

    #[test]
    fn on_the_floor_moving_away_from_the_ladder_adds_the_climb_push() {
        // Facing away (yaw 180): vpn = (-1, ~0, 0). forward = 200 -> velocity (-200, ~0, 0);
        // normal.velocity = +200 > 0; result = lateral - 200 * (0,0,1) = (0, 0, -200), and on the
        // floor MAX_CLIMB_SPEED * normal (-200, 0, 0) is added: (-200, 0, -200).
        let mut pm = PlayerMove { maxspeed: 250.0, angles: Vec3::new(0.0, 180.0, 0.0), ..PlayerMove::default() };
        pm.cmd.buttons = IN_FORWARD as u16;
        run(&mut pm, &mut world(true));
        assert!((pm.velocity.x - -200.0).abs() < 1e-3, "{:?}", pm.velocity);
        assert!(pm.velocity.y.abs() < 1e-3, "{:?}", pm.velocity);
        assert!((pm.velocity.z - -200.0).abs() < 1e-3, "{:?}", pm.velocity);

        // Off the floor there is no push: the player just slides down at 200.
        let mut pm = PlayerMove { maxspeed: 250.0, angles: Vec3::new(0.0, 180.0, 0.0), ..PlayerMove::default() };
        pm.cmd.buttons = IN_FORWARD as u16;
        run(&mut pm, &mut world(false));
        assert!(pm.velocity.x.abs() < 1e-3, "{:?}", pm.velocity);
        assert!((pm.velocity.z - -200.0).abs() < 1e-3, "{:?}", pm.velocity);
    }

    #[test]
    fn a_missed_centre_trace_keeps_the_velocity_but_still_flies() {
        let mut pm = PlayerMove { velocity: Vec3::new(1.0, 2.0, 3.0), ..PlayerMove::default() };
        pm.cmd.buttons = IN_FORWARD as u16;
        let mut w = LadderWorld { normal: Vec3::ZERO, fraction: 1.0, floor_solid: false };
        run(&mut pm, &mut w);
        assert_eq!(pm.velocity, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(pm.movetype, MOVETYPE_FLY);
        assert_eq!(pm.gravity, 0.0);
    }

    #[test]
    fn noclip_does_nothing() {
        let mut pm = PlayerMove { movetype: MOVETYPE_NOCLIP, gravity: 1.0, ..PlayerMove::default() };
        run(&mut pm, &mut world(false));
        assert_eq!(pm.movetype, MOVETYPE_NOCLIP);
        assert_eq!(pm.gravity, 1.0);
    }
}
