//! Jumping: `PM_Jump`, `PM_PreventMegaBunnyJumping`.
//! Ported from ReGameDLL_CS `regamedll/pm_shared/pm_shared.cpp` (vanilla branch), read 2026-11-22.

use super::consts::{
    ATTN_NORM, BUNNYJUMP_MAX_SPEED_FACTOR, CHAN_BODY, CONTENTS_SLIME, CONTENTS_WATER, FL_DUCKING, IN_JUMP, PITCH_NORM,
    VOL_NORM,
};
use super::math::length;
use super::sound::{pm_catagorize_texture_type, pm_map_texture_type_step_type, pm_play_step_sound};
use super::types::PmCtx;
use super::walk::pm_fixup_gravity_velocity;

/// `PM_PreventMegaBunnyJumping` (pm_shared.cpp 2587-2612, read 2026-11-22): crops the speed to
/// 0.8 of the limit when a jump starts above `1.2 * maxspeed`.
///
/// Quirk kept: `0.8` is a `double` literal, so the fraction is computed in `f64` and stored as `f32`.
/// Vanilla calls this on every jump (the `sv_enablebunnyhopping` gate is `REGAMEDLL_ADD`).
pub(crate) fn pm_prevent_mega_bunny_jumping(c: &mut PmCtx<'_>) {
    // Speed at which bunny jumping is limited
    let maxscaledspeed: f32 = BUNNYJUMP_MAX_SPEED_FACTOR * c.pm.maxspeed;

    // Don't divide by zero
    if maxscaledspeed <= 0.0 {
        return;
    }

    // Current player speed
    let spd = length(c.pm.velocity);

    if spd <= maxscaledspeed {
        return;
    }

    // Returns the modifier for the velocity
    let fraction = (f64::from(maxscaledspeed / spd) * 0.8) as f32;

    // Crop it down!.
    c.pm.velocity *= fraction;
}

/// `PM_JumpHeight(false)` (pm_shared.cpp 2614-2627, vanilla): `sqrt(2 * 800 * 45)` = 268.328.
/// QUIRK: the 800 is hard-coded, not `sv_gravity`.
fn pm_jump_height() -> f32 {
    (2.0f64 * 800.0 * 45.0).sqrt() as f32
}

/// `PM_Jump` (pm_shared.cpp 2635-2828, vanilla branch, read 2026-11-22): the jump key.
///
/// In the water the player swims up (100 water, 80 slime, 50 lava) and plays a wade sound once per
/// second of `flSwimTime`. On the ground the jump gives `velocity[2] = 268.328`, scaled by the
/// stamina ratio when `fuser2 > 0`, then sets `fuser2` to 1315.789429.
///
/// Quirks kept: no auto-bunnyhop (a held jump key is ignored); a jump while the duck transition is
/// running (`bInDuck` and `FL_DUCKING`) does nothing, without setting `oldbuttons`; the jump height
/// ignores `sv_gravity`; the stamina ratio is a `double` expression stored into a `float`; vanilla has
/// no long jump; `PM_PreventMegaBunnyJumping` runs on every jump; the step sound is played before the
/// velocity changes.
pub(crate) fn pm_jump(c: &mut PmCtx<'_>) {
    if c.pm.dead {
        // don't jump again until released
        c.pm.oldbuttons |= IN_JUMP;
        return;
    }

    // See if we are waterjumping.  If so, decrement count and return.
    if c.pm.waterjumptime != 0.0 {
        c.pm.waterjumptime -= f32::from(c.pm.cmd.msec);

        if c.pm.waterjumptime < 0.0 {
            c.pm.waterjumptime = 0.0;
        }

        return;
    }

    // If we are in the water most of the way...
    if c.pm.waterlevel >= 2 {
        // swimming, not jumping
        c.pm.onground = -1;

        // We move up a certain amount
        if c.pm.watertype == CONTENTS_WATER {
            c.pm.velocity[2] = 100.0;
        } else if c.pm.watertype == CONTENTS_SLIME {
            c.pm.velocity[2] = 80.0;
        } else {
            // LAVA
            c.pm.velocity[2] = 50.0;
        }

        // play swiming sound
        if c.pm.fl_swim_time <= 0.0 {
            // Don't play sound again for 1 second
            c.pm.fl_swim_time = 1000.0;

            // The source's switch has no default: a value outside 0..=3 plays nothing.
            let n = c.w.random_long(0, 3);
            if (0..=3).contains(&n) {
                c.w.play_sound(CHAN_BODY, &format!("player/pl_wade{}.wav", n + 1), VOL_NORM, ATTN_NORM, 0, PITCH_NORM);
            }
        }

        return;
    }

    // No more effect
    // in air, so no effect
    if c.pm.onground == -1 {
        // Flag that we jumped.
        // don't jump again until released
        c.pm.oldbuttons |= IN_JUMP;
        return;
    }

    // don't pogo stick
    if c.pm.oldbuttons & IN_JUMP != 0 {
        return;
    }

    if c.pm.b_in_duck && c.pm.flags & FL_DUCKING != 0 {
        return;
    }

    pm_catagorize_texture_type(c);

    // In the air now.
    c.pm.onground = -1;

    pm_prevent_mega_bunny_jumping(c);

    let fvel = length(c.pm.velocity);
    let fvol = 1.0f32;

    if fvel >= 150.0 {
        let step = pm_map_texture_type_step_type(c.pm.chtexturetype);
        pm_play_step_sound(c, step, fvol);
    }

    // NOTE: don't do it in .f (float)
    c.pm.velocity[2] = pm_jump_height();

    if c.pm.fuser2 > 0.0 {
        // NOTE: don't do it in .f (float)
        let fl_ratio = ((100.0 - f64::from(c.pm.fuser2) * 0.001 * 19.0) * 0.01) as f32;

        c.pm.velocity[2] *= fl_ratio;
    }

    // 1315.789429 in the source; the same f32 as 1315.7894.
    c.pm.fuser2 = 1_315.789_4;

    // Decay it for simulation
    pm_fixup_gravity_velocity(c);

    // Flag that we jumped.
    // don't jump again until released
    c.pm.oldbuttons |= IN_JUMP;
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::pm::consts::CONTENTS_LAVA;
    use crate::pm::{PlayerMove, PmShared, PmTrace, PmWorld, pm_init};

    /// A world that only logs sounds and hands out queued random numbers.
    #[derive(Default)]
    struct SoundWorld {
        sounds: Vec<String>,
        random: Vec<i32>,
        random_calls: Vec<(i32, i32)>,
    }

    impl PmWorld for SoundWorld {
        fn player_trace(&mut self, _s: Vec3, e: Vec3, _h: i32, _f: i32, _i: i32) -> PmTrace {
            PmTrace::miss(e)
        }
        fn test_player_position(&mut self, o: Vec3, _p: Vec3, _h: i32) -> (i32, PmTrace) {
            (-1, PmTrace::miss(o))
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
        fn physent_is_player(&self, _ent: i32) -> bool {
            false
        }
        fn physent_has_model(&self, _ent: i32) -> bool {
            false
        }
        fn trace_model(&mut self, _m: usize, _s: Vec3, _e: Vec3) -> (f32, Vec3) {
            (1.0, Vec3::ZERO)
        }
        fn trace_texture(&mut self, _g: i32, _s: Vec3, _e: Vec3) -> Option<String> {
            None
        }
        fn play_sound(&mut self, channel: i32, sample: &str, volume: f32, attenuation: f32, flags: i32, pitch: i32) {
            assert_eq!((channel, volume, attenuation, flags, pitch), (CHAN_BODY, 1.0, 0.8, 0, 100));
            self.sounds.push(sample.to_string());
        }
        fn random_long(&mut self, low: i32, high: i32) -> i32 {
            self.random_calls.push((low, high));
            if self.random.is_empty() { low } else { self.random.remove(0) }
        }
        fn sys_float_time(&mut self) -> f64 {
            0.0
        }
        fn physent_by_info(&self, _i: i32) -> Option<(Vec3, Vec3)> {
            None
        }
    }

    fn run_jump(pm: &mut PlayerMove, w: &mut SoundWorld) {
        let mut sh: PmShared = pm_init(None);
        let mut c = PmCtx { pm, sh: &mut sh, w };
        pm_jump(&mut c);
    }

    #[test]
    fn jump_height_is_sqrt_of_72000() {
        // sqrt(2 * 800 * 45) = 268.32815...
        assert!((pm_jump_height() - 268.328_16).abs() < 1e-4);
    }

    #[test]
    fn water_jump_velocity_by_liquid_and_wade_once_per_second() {
        for (watertype, vz) in [(CONTENTS_WATER, 100.0), (CONTENTS_SLIME, 80.0), (CONTENTS_LAVA, 50.0)] {
            let mut pm = PlayerMove { waterlevel: 2, watertype, onground: 0, ..PlayerMove::default() };
            let mut w = SoundWorld { random: vec![2], ..SoundWorld::default() };
            run_jump(&mut pm, &mut w);
            assert_eq!(pm.velocity[2], vz);
            assert_eq!(pm.onground, -1);
            assert_eq!(pm.fl_swim_time, 1000.0);
            assert_eq!(w.random_calls, [(0, 3)]);
            assert_eq!(w.sounds, ["player/pl_wade3.wav"]);

            // The timer is running: no second sound, the velocity is still set.
            pm.velocity[2] = 0.0;
            pm.fl_swim_time = 1.0;
            run_jump(&mut pm, &mut w);
            assert_eq!(pm.velocity[2], vz);
            assert_eq!(w.sounds.len(), 1);
            // oldbuttons is not touched in the water (the jump may be held).
            assert_eq!(pm.oldbuttons & IN_JUMP, 0);
        }
    }

    #[test]
    fn water_jump_timer_counts_down_and_blocks_the_jump() {
        let mut pm = PlayerMove { waterjumptime: 15.0, onground: 0, ..PlayerMove::default() };
        pm.cmd.msec = 10;
        let mut w = SoundWorld::default();
        run_jump(&mut pm, &mut w);
        assert_eq!(pm.waterjumptime, 5.0);
        run_jump(&mut pm, &mut w);
        assert_eq!(pm.waterjumptime, 0.0); // clamped, not -5
        assert_eq!(pm.velocity[2], 0.0);
        assert_eq!(pm.onground, 0);
    }

    #[test]
    fn dead_players_only_mark_the_button() {
        let mut pm = PlayerMove { dead: true, onground: 0, ..PlayerMove::default() };
        run_jump(&mut pm, &mut SoundWorld::default());
        assert_eq!(pm.oldbuttons, IN_JUMP);
        assert_eq!(pm.velocity[2], 0.0);
        assert_eq!(pm.onground, 0);
    }

    #[test]
    fn ground_jump_sets_speed_stamina_and_the_gravity_half_step() {
        // vz = sqrt(72000) - gravity * frametime * 0.5 = 268.3282 - 800 * 0.01 * 0.5 = 264.3282.
        let mut pm = PlayerMove { onground: 0, frametime: 0.01, ..PlayerMove::default() };
        run_jump(&mut pm, &mut SoundWorld::default());
        assert!((pm.velocity[2] - 264.328_16).abs() < 1e-3, "{}", pm.velocity[2]);
        assert_eq!(pm.onground, -1);
        assert_eq!(pm.fuser2, 1_315.789_4);
        assert_eq!(pm.oldbuttons & IN_JUMP, IN_JUMP);

        // Held key: nothing happens.
        pm.onground = 0;
        pm.velocity[2] = 0.0;
        run_jump(&mut pm, &mut SoundWorld::default());
        assert_eq!(pm.velocity[2], 0.0);
        assert_eq!(pm.onground, 0);

        // Stamina: ratio = (100 - fuser2 * 0.001 * 19) * 0.01; at 1000 it is (100 - 19) * 0.01 = 0.81.
        let mut pm = PlayerMove { onground: 0, frametime: 0.01, fuser2: 1000.0, ..PlayerMove::default() };
        run_jump(&mut pm, &mut SoundWorld::default());
        let expect = 268.328_16_f64 * 0.81 - 4.0;
        assert!((f64::from(pm.velocity[2]) - expect).abs() < 1e-2, "{}", pm.velocity[2]);

        // A duck transition in progress blocks the jump and leaves oldbuttons alone.
        let mut pm = PlayerMove { onground: 0, b_in_duck: true, flags: FL_DUCKING, ..PlayerMove::default() };
        run_jump(&mut pm, &mut SoundWorld::default());
        assert_eq!((pm.onground, pm.oldbuttons, pm.velocity[2]), (0, 0, 0.0));
    }

    #[test]
    fn mega_bunny_jumping_crops_to_point_eight_of_the_limit() {
        // limit = 1.2 * 250 = 300; at 400 the fraction is (300 / 400) * 0.8 = 0.6.
        let mut pm = PlayerMove { maxspeed: 250.0, velocity: Vec3::new(400.0, 0.0, 0.0), ..PlayerMove::default() };
        let mut sh = pm_init(None);
        let mut w = SoundWorld::default();
        pm_prevent_mega_bunny_jumping(&mut PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w });
        assert!((pm.velocity[0] - 240.0).abs() < 1e-3, "{}", pm.velocity[0]);

        // At or below the limit: untouched. Zero maxspeed: untouched (no division by zero).
        pm.velocity = Vec3::new(300.0, 0.0, 0.0);
        pm_prevent_mega_bunny_jumping(&mut PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w });
        assert_eq!(pm.velocity[0], 300.0);
        pm.maxspeed = 0.0;
        pm.velocity = Vec3::new(500.0, 0.0, 0.0);
        pm_prevent_mega_bunny_jumping(&mut PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w });
        assert_eq!(pm.velocity[0], 500.0);
    }
}
