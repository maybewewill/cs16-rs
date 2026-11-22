//! Unsticking: `PM_CreateStuckTable`, `PM_GetRandomStuckOffsets`, `PM_ResetStuckOffsets`,
//! `PM_CheckStuck`. Ported from ReGameDLL_CS `regamedll/pm_shared/pm_shared.cpp`, read 2026-11-22.

use glam::Vec3;

use super::consts::{IN_ATTACK, IN_DUCK, IN_JUMP, MAX_CLIENTS, PM_CHECKSTUCK_MINTIME};
use super::types::{PmCtx, PmShared};
use super::walk::pm_add_to_touched;

/// Number of slots of `rgv3tStuckTable` (`ARRAYSIZE`).
const STUCK_TABLE_LEN: usize = 54;

/// The per-player slot of the stuck arrays. The C code indexes `rgStuckLast[player_index]` without
/// a check; an out-of-range index (a client with a bogus slot) wraps here instead of reading out of
/// bounds. Deviation from the source for safety, not reachable with valid indices.
fn slot(player_index: i32) -> usize {
    player_index.rem_euclid(MAX_CLIENTS as i32) as usize
}

/// `PM_CreateStuckTable` (pm_shared.cpp 3406-3535, read 2026-11-22): 53 offsets in the source's
/// order, slot 53 stays zero (`Q_memset`). All values are exact in `float`.
pub(crate) fn pm_create_stuck_table() -> [Vec3; STUCK_TABLE_LEN] {
    let mut table = [Vec3::ZERO; STUCK_TABLE_LEN];
    let mut idx = 0;
    let mut push = |x: f32, y: f32, z: f32| {
        table[idx] = Vec3::new(x, y, z);
        idx += 1;
    };

    // Little moves: Z, then Y, then X.
    for z in [-0.125, 0.0, 0.125] {
        push(0.0, 0.0, z);
    }
    for y in [-0.125, 0.0, 0.125] {
        push(0.0, y, 0.0);
    }
    for x in [-0.125, 0.0, 0.125] {
        push(x, 0.0, 0.0);
    }
    // Remaining multi axis nudges: x outer, y, z inner.
    for x in [-0.125, 0.125] {
        for y in [-0.125, 0.125] {
            for z in [-0.125, 0.125] {
                push(x, y, z);
            }
        }
    }

    // Big moves.
    let zi = [0.0f32, 1.0, 6.0];
    for z in zi {
        push(0.0, 0.0, z);
    }
    for y in [-2.0, 0.0, 2.0] {
        push(0.0, y, 0.0);
    }
    for x in [-2.0, 0.0, 2.0] {
        push(x, 0.0, 0.0);
    }
    for z in zi {
        for x in [-2.0, 0.0, 2.0] {
            for y in [-2.0, 0.0, 2.0] {
                push(x, y, z);
            }
        }
    }
    debug_assert_eq!(idx, 53);
    table
}

/// `PM_GetRandomStuckOffsets` (1695): returns the table index used and the offset.
pub(crate) fn pm_get_random_stuck_offsets(sh: &mut PmShared, player_index: i32, server: bool) -> (usize, Vec3) {
    let last = &mut sh.stuck_last[slot(player_index)][usize::from(server)];
    let idx = *last;
    *last = last.wrapping_add(1);
    let i = idx.rem_euclid(STUCK_TABLE_LEN as i32) as usize;
    (i, sh.stuck_table[i])
}

/// `PM_ResetStuckOffsets` (1703).
pub(crate) fn pm_reset_stuck_offsets(sh: &mut PmShared, player_index: i32, server: bool) {
    sh.stuck_last[slot(player_index)][usize::from(server)] = 0;
}

/// `PM_CheckStuck` (pm_shared.cpp 1711-1827, read 2026-11-22): if `origin` is in a solid position,
/// try nudging it by the stuck table. Returns `true` when the player is (still) stuck and cannot
/// move this command.
///
/// Quirks kept: the offsets found on the server below table index 27 do not move the origin (only
/// `i >= ARRAYSIZE/2` copies `test`); `PM_StuckTouch` is the engine's `PM_AddToTouched` on the
/// server and nothing on the client (no world callback); `fTime` is a `float` and the comparison
/// with `PM_CHECKSTUCK_MINTIME` (a `double`) happens in `f64`.
pub(crate) fn pm_check_stuck(c: &mut PmCtx<'_>) -> bool {
    let player = c.pm.player_index;
    let server = c.pm.server;

    // If position is okay, exit
    let (hitent, mut traceresult) = c.test_position(c.pm.origin);
    if hitent == -1 {
        pm_reset_stuck_offsets(c.sh, player, server);
        return false;
    }

    let base = c.pm.origin;

    // Deal with precision error in network.
    if !server {
        // World or BSP model
        if hitent == 0 || c.w.physent_has_model(hitent) {
            let mut n_reps = 0;
            pm_reset_stuck_offsets(c.sh, player, server);
            loop {
                let (_, offset) = pm_get_random_stuck_offsets(c.sh, player, server);

                let test = base + offset;
                // `traceresult` is rewritten by every call, as the engine does; `hitent` is not.
                let (found, trace) = c.test_position(test);
                traceresult = trace;
                if found == -1 {
                    pm_reset_stuck_offsets(c.sh, player, server);
                    c.pm.origin = test;
                    return false;
                }

                n_reps += 1;
                if n_reps >= STUCK_TABLE_LEN {
                    break;
                }
            }
        }
    }

    // Only an issue on the client.
    let idx = if server { 0 } else { 1 };

    let f_time = c.w.sys_float_time() as f32;

    // Too soon?
    let check_time = &mut c.sh.stuck_check_time[slot(player)][idx];
    if f64::from(*check_time) >= (f64::from(f_time) - PM_CHECKSTUCK_MINTIME) {
        return true;
    }

    *check_time = f_time;

    // PM_StuckTouch: on the server the engine's callback is PM_AddToTouched.
    if server {
        let velocity = c.pm.velocity;
        pm_add_to_touched(c, traceresult, velocity);
    }

    let (i, offset) = pm_get_random_stuck_offsets(c.sh, player, server);

    let test = base + offset;
    let (hitent, _) = c.test_position(test);
    if hitent == -1 {
        pm_reset_stuck_offsets(c.sh, player, server);

        if i >= STUCK_TABLE_LEN / 2 {
            c.pm.origin = test;
        }

        return false;
    }

    // If player is flailing while stuck in another player (should never happen), then see
    //  if we can't "unstick" them forceably.
    if (i32::from(c.pm.cmd.buttons) & (IN_JUMP | IN_DUCK | IN_ATTACK)) != 0 && c.w.physent_is_player(hitent) {
        let xystep = 8.0f32;
        let zstep = 18.0f32;
        let xyminmax = xystep;
        let zminmax = 4.0 * zstep;

        let mut z = 0.0f32;
        while z <= zminmax {
            let mut x = -xyminmax;
            while x <= xyminmax {
                let mut y = -xyminmax;
                while y <= xyminmax {
                    let test = Vec3::new(base[0] + x, base[1] + y, base[2] + z);

                    if c.test_position(test).0 == -1 {
                        c.pm.origin = test;
                        return false;
                    }
                    y += xystep;
                }
                x += xystep;
            }
            z += zstep;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pm::types::{PlayerMove, PmTrace, PmWorld};
    use crate::pm::pm_init;

    #[test]
    fn stuck_table_has_the_source_order() {
        let t = pm_create_stuck_table();
        assert_eq!(t[0], Vec3::new(0.0, 0.0, -0.125));
        assert_eq!(t[2], Vec3::new(0.0, 0.0, 0.125));
        assert_eq!(t[3], Vec3::new(0.0, -0.125, 0.0));
        assert_eq!(t[6], Vec3::new(-0.125, 0.0, 0.0));
        assert_eq!(t[9], Vec3::new(-0.125, -0.125, -0.125));
        assert_eq!(t[10], Vec3::new(-0.125, -0.125, 0.125));
        assert_eq!(t[16], Vec3::new(0.125, 0.125, 0.125));
        assert_eq!(t[17], Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(t[18], Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(t[19], Vec3::new(0.0, 0.0, 6.0));
        assert_eq!(t[20], Vec3::new(0.0, -2.0, 0.0));
        assert_eq!(t[23], Vec3::new(-2.0, 0.0, 0.0));
        assert_eq!(t[26], Vec3::new(-2.0, -2.0, 0.0));
        assert_eq!(t[27], Vec3::new(-2.0, 0.0, 0.0));
        assert_eq!(t[34], Vec3::new(2.0, 2.0, 0.0));
        assert_eq!(t[35], Vec3::new(-2.0, -2.0, 1.0));
        assert_eq!(t[44], Vec3::new(-2.0, -2.0, 6.0));
        assert_eq!(t[52], Vec3::new(2.0, 2.0, 6.0));
        assert_eq!(t[53], Vec3::ZERO);
    }

    /// A world where only the positions `base + table[free]` are free.
    struct StuckWorld {
        free: Vec<Vec3>,
        time: f64,
        tests: usize,
        /// The entity every blocked position reports.
        hit: i32,
    }

    impl PmWorld for StuckWorld {
        fn player_trace(&mut self, _s: Vec3, end: Vec3, _h: i32, _f: i32, _i: i32) -> PmTrace {
            PmTrace::miss(end)
        }
        fn test_player_position(&mut self, _o: Vec3, pos: Vec3, _h: i32) -> (i32, PmTrace) {
            self.tests += 1;
            let hit = if self.free.contains(&pos) { -1 } else { self.hit };
            let mut t = PmTrace::miss(pos);
            t.ent = hit;
            (hit, t)
        }
        fn point_contents(&mut self, _p: Vec3) -> (i32, i32) {
            (-1, -1)
        }
        fn num_moveents(&self) -> usize {
            0
        }
        fn ladder_contains(&mut self, _m: usize, _o: Vec3, _h: i32) -> bool {
            false
        }
        fn moveent_bounds(&self, _m: usize) -> (Vec3, Vec3) {
            (Vec3::ZERO, Vec3::ZERO)
        }
        fn moveent_origin(&self, _m: usize) -> Vec3 {
            Vec3::ZERO
        }
        fn trace_model(&mut self, _m: usize, _s: Vec3, _e: Vec3) -> (f32, Vec3) {
            (1.0, Vec3::ZERO)
        }
        fn trace_texture(&mut self, _g: i32, _s: Vec3, _e: Vec3) -> Option<String> {
            None
        }
        fn play_sound(&mut self, _c: i32, _s: &str, _v: f32, _a: f32, _f: i32, _p: i32) {}
        fn random_long(&mut self, low: i32, _high: i32) -> i32 {
            low
        }
        fn sys_float_time(&mut self) -> f64 {
            self.time
        }
        fn physent_by_info(&self, _i: i32) -> Option<(Vec3, Vec3)> {
            None
        }
    }

    fn setup(server: bool, free: Vec<Vec3>) -> (PlayerMove, PmShared, StuckWorld) {
        let pm = PlayerMove {
            origin: Vec3::new(10.0, 20.0, 30.0),
            server,
            ..PlayerMove::default()
        };
        (pm, pm_init(None), StuckWorld { free, time: 10.0, tests: 0, hit: 3 })
    }

    #[test]
    fn server_walks_the_table_once_per_interval_and_copies_only_the_big_offsets() {
        let base = Vec3::new(10.0, 20.0, 30.0);
        let t = pm_create_stuck_table();
        // Only table[28] is free (index >= 27: the origin is copied); attempt n tests table[n - 1].
        let (mut pm, mut sh, mut w) = setup(true, vec![base + t[28]]);
        for expected_next in 1..=29 {
            let mut c = PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w };
            let stuck = pm_check_stuck(&mut c);
            if expected_next < 29 {
                assert!(stuck, "attempt {expected_next}");
                assert_eq!(sh.stuck_last[0][1], expected_next);
                assert_eq!(pm.origin, base);
                // A second call in the same instant is "too soon": nothing advances.
                let mut c = PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w };
                assert!(pm_check_stuck(&mut c));
                assert_eq!(sh.stuck_last[0][1], expected_next);
                w.time += 0.1;
            } else {
                assert!(!stuck);
            }
        }
        assert_eq!(pm.origin, base + t[28]);
        assert_eq!(sh.stuck_last[0][1], 0, "a free offset resets the walk");
        // The first stuck check recorded the entity it was stuck in.
        assert_eq!(pm.touchindex.len(), 1);
        assert_eq!(pm.touchindex[0].ent, 3);
    }

    #[test]
    fn server_small_offsets_leave_the_origin_alone() {
        let base = Vec3::new(10.0, 20.0, 30.0);
        let t = pm_create_stuck_table();
        let (mut pm, mut sh, mut w) = setup(true, vec![base + t[0]]);
        let mut c = PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w };
        assert!(!pm_check_stuck(&mut c));
        assert_eq!(pm.origin, base, "index 0 < 27: the origin is not moved");
    }

    #[test]
    fn client_takes_the_first_free_offset_at_once() {
        let base = Vec3::new(10.0, 20.0, 30.0);
        let t = pm_create_stuck_table();
        // Stuck in the world (hitent 0): the client loop finds table[19] after 20 tries.
        let (mut pm, mut sh, mut w) = setup(false, vec![base + t[19]]);
        w.hit = 0;
        let mut c = PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w };
        assert!(!pm_check_stuck(&mut c));
        assert_eq!(pm.origin, base + t[19]);
        assert_eq!(w.tests, 1 + 20, "one test of the origin, then table[0..=19]");

        // Stuck in a model-less non-world entity: the client loop does not run, only the timed path.
        let (mut pm, mut sh, mut w) = setup(false, vec![base + t[19]]);
        let mut c = PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w };
        assert!(pm_check_stuck(&mut c));
        assert_eq!(pm.origin, base);
    }
}
