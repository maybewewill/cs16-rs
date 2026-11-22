//! Footstep sounds: `PM_UpdateStepSound`, `PM_PlayStepSound`, `PM_CatagorizeTextureType`,
//! `PM_MapTextureTypeStepType`. Ported from ReGameDLL_CS `regamedll/pm_shared/pm_shared.cpp`
//! (vanilla), read 2026-11-22.

use super::consts::{
    ATTN_NORM, CHAN_BODY, CHAR_TEX_CONCRETE, CHAR_TEX_DIRT, CHAR_TEX_GRATE, CHAR_TEX_METAL, CHAR_TEX_SLOSH,
    CHAR_TEX_SNOW, CHAR_TEX_TILE, CHAR_TEX_VENT, CONTENTS_WATER, FL_DUCKING, FL_FROZEN, MOVETYPE_FLY, PITCH_NORM,
    STEP_CONCRETE, STEP_DIRT, STEP_GRATE, STEP_LADDER, STEP_METAL, STEP_SLOSH, STEP_SNOW, STEP_TILE, STEP_VENT,
    STEP_WADE,
};
use super::math::length;
use super::types::PmCtx;

/// `MAX_TEXTURENAME_LENGHT` (sic): `sztexturename` keeps at most 16 characters.
const MAX_TEXTURENAME_LENGHT: usize = 17;

/// `PM_UpdateStepSound` (pm_shared.cpp 375-495, read 2026-11-22): the footstep timer. Plays a step
/// when `flTimeStepSound` has run out and the player is on the ground or a ladder.
///
/// Quirks kept: the texture is categorized before the water probes (so the world sees the
/// `PM_TraceTexture` call first); `flDuckTime < 950` (a 0..1000 ms timer) decides the ducking
/// volume; the ladder branch skips the knee/feet probes; a slow walker only re-arms the timer with
/// 400. `0.3 * height` and the `fvol` constants are `double` expressions stored into floats.
pub(crate) fn pm_update_step_sound(c: &mut PmCtx<'_>) {
    if c.pm.fl_time_step_sound > 0 {
        return;
    }

    if (c.pm.flags & FL_FROZEN) != 0 {
        return;
    }

    let speed = length(c.pm.velocity);

    if speed <= 150.0 {
        c.pm.fl_time_step_sound = 400;
        return;
    }

    // determine if we are on a ladder
    let f_ladder = c.pm.movetype == MOVETYPE_FLY;

    // determine if we are not in air
    let onground = c.pm.onground != -1;

    // If we're on a ladder or on the ground play step sound.
    if f_ladder || onground {
        pm_catagorize_texture_type(c);

        let origin = c.pm.origin;
        let mut knee = origin;
        let mut feet = origin;

        let hull = c.pm.hull_index();
        let height = c.pm.player_maxs[hull][2] - c.pm.player_mins[hull][2];

        knee[2] = (f64::from(origin[2]) - 0.3 * f64::from(height)) as f32;
        feet[2] = (f64::from(origin[2]) - 0.5 * f64::from(height)) as f32;

        let step;
        let mut fvol: f32;

        // find out what we're stepping in or on...
        if f_ladder {
            step = STEP_LADDER;
            fvol = 0.35_f64 as f32;
            c.pm.fl_time_step_sound = 350;
        } else if c.w.point_contents(knee).0 == CONTENTS_WATER {
            step = STEP_WADE;
            fvol = 0.65_f64 as f32;
            c.pm.fl_time_step_sound = 600;
        } else if c.w.point_contents(feet).0 == CONTENTS_WATER {
            step = STEP_SLOSH;
            fvol = 0.5;
            c.pm.fl_time_step_sound = 300;
        } else {
            // find texture under player, if different from current texture,
            // get material type
            step = pm_map_texture_type_step_type(c.pm.chtexturetype);

            // The default arm and CHAR_TEX_CONCRETE/METAL/GRATE/TILE/SLOSH are all 0.5.
            fvol = match c.pm.chtexturetype {
                CHAR_TEX_DIRT => 0.55_f64 as f32,
                CHAR_TEX_VENT => 0.7_f64 as f32,
                _ => 0.5,
            };
            c.pm.fl_time_step_sound = 300;
        }

        if (c.pm.flags & FL_DUCKING) != 0 || f_ladder {
            // slower step time if ducking
            c.pm.fl_time_step_sound += 100;

            // play the sound
            // 35% volume if ducking
            if (c.pm.flags & FL_DUCKING) != 0 && c.pm.fl_duck_time < 950.0 {
                fvol = (f64::from(fvol) * 0.35) as f32;
            }
        }

        pm_play_step_sound(c, step, fvol);
    }
}

/// The file family of a step type: `player/pl_<family><n>.wav` (the `switch (step)` of
/// `PM_PlayStepSound`; everything else, and `STEP_CONCRETE`, is `step`).
fn step_family(step: i32) -> &'static str {
    match step {
        STEP_METAL => "metal",
        STEP_DIRT => "dirt",
        STEP_VENT => "duct",
        STEP_GRATE => "grate",
        STEP_TILE => "tile",
        STEP_SLOSH => "slosh",
        STEP_WADE => "wade",
        STEP_LADDER => "ladder",
        STEP_SNOW => "snow",
        _ => "step",
    }
}

/// Every sample name the movement code can play: the step families (samples 1 to 4, tile 1 to 5)
/// and the wade sounds of a water jump and of entering or leaving water (`pl_wade1-4`, already
/// in the wade family). For the game's precache list.
pub fn all_sample_names() -> Vec<String> {
    let mut names = Vec::new();
    for step in [
        STEP_CONCRETE,
        STEP_METAL,
        STEP_DIRT,
        STEP_VENT,
        STEP_GRATE,
        STEP_TILE,
        STEP_SLOSH,
        STEP_WADE,
        STEP_LADDER,
        STEP_SNOW,
    ] {
        let count = if step == STEP_TILE { 5 } else { 4 };
        for n in 1..=count {
            names.push(format!("player/pl_{}{n}.wav", step_family(step)));
        }
    }
    names
}

/// `PM_PlayStepSound(step, fvol)` (pm_shared.cpp 171-322, read 2026-11-22).
///
/// Quirks kept: `iStepLeft` toggles before the `runfuncs` check (a replayed command still swaps
/// feet); `RandomLong(0, 1)` is drawn before the `mp_footsteps` check; tile steps draw a second
/// `RandomLong(0, 4)` and a 0 selects `pl_tile5`; the static `iSkipStep` (in `PmShared`) makes the
/// wade step skip the call at counter 0 and play the next three (so 3 of every 4 calls play, 1 is
/// skipped), and it only advances for calls that get past the `runfuncs` and `mp_footsteps`
/// checks; the sample numbers are not in foot order (right foot 1, 3; left foot 2, 4), wade is
/// 1..4 in order.
pub(crate) fn pm_play_step_sound(c: &mut PmCtx<'_>, step: i32, fvol: f32) {
    c.pm.i_step_left = i32::from(c.pm.i_step_left == 0);

    if !c.pm.runfuncs {
        return;
    }

    // Wrapping like the C `int` arithmetic: a hostile RNG value must not panic in debug.
    let mut irand = c.w.random_long(0, 1).wrapping_add(c.pm.i_step_left.wrapping_mul(2));

    // FIXME mp_footsteps needs to be a movevar
    if c.pm.multiplayer && !c.pm.movevars.footsteps {
        return;
    }

    // irand - 0,1 for right foot, 2,3 for left foot
    // used to alternate left and right foot
    let family = step_family(step);

    // The sample number for irand 0..3: right foot 1, 3; left foot 2, 4 (wade: 1, 2, 3, 4).
    let mut number = match (step == STEP_WADE, irand) {
        (false, 0) => 1,
        (false, 1) => 3,
        (false, 2) => 2,
        (false, 3) => 4,
        (true, 0) => 1,
        (true, 1) => 2,
        (true, 2) => 3,
        (true, 3) => 4,
        _ => 0,
    };

    match step {
        STEP_TILE => {
            if c.w.random_long(0, 4) == 0 {
                irand = 4;
            }
            if irand == 4 {
                number = 5;
            }
        }
        STEP_WADE => {
            if c.sh.i_skip_step == 0 {
                c.sh.i_skip_step += 1;
                return;
            }

            let before = c.sh.i_skip_step;
            c.sh.i_skip_step += 1;
            if before == 3 {
                c.sh.i_skip_step = 0;
            }
        }
        _ => {}
    }

    // No `case` matched (irand outside 0..=3, only a misbehaving RNG): no sound.
    if number == 0 {
        return;
    }

    let sample = format!("player/pl_{family}{number}.wav");
    c.w.play_sound(CHAN_BODY, &sample, fvol, ATTN_NORM, 0, PITCH_NORM);
}

/// `PM_CatagorizeTextureType` (sic; pm_shared.cpp 340-371, read 2026-11-22): the texture under the
/// player, a trace 64 units straight down.
///
/// Quirks kept: the defaults (empty name, concrete) are stored before the lookup, so a miss of
/// `PM_TraceTexture` leaves them; a leading `-` or `+` drops two characters (the `-0` / `+0`
/// animation prefix; the source does not check the string is long enough, here the skip is clamped
/// to the string), then one of `{`, `!`, `~`, space drops one more; the name is cut to 16
/// characters.
pub(crate) fn pm_catagorize_texture_type(c: &mut PmCtx<'_>) {
    let start = c.pm.origin;
    let mut end = c.pm.origin;

    // Straight down
    end[2] -= 64.0_f32;

    // Fill in default values, just in case.
    c.pm.sztexturename.clear();
    c.pm.chtexturetype = CHAR_TEX_CONCRETE;

    let ground = c.pm.onground;
    let Some(name) = c.w.trace_texture(ground, start, end) else {
        return;
    };

    // A C string ends at its first NUL.
    let mut bytes = name.as_bytes().split(|&b| b == 0).next().unwrap_or(&[]);

    // strip leading '-0' or '+0~' or '{' or '!'
    if matches!(bytes.first(), Some(b'-' | b'+')) {
        bytes = &bytes[bytes.len().min(2)..];
    }

    if matches!(bytes.first(), Some(b'{' | b'!' | b'~' | b' ')) {
        bytes = &bytes[1..];
    }

    let keep = bytes.len().min(MAX_TEXTURENAME_LENGHT - 1);
    c.pm.sztexturename = String::from_utf8_lossy(&bytes[..keep]).into_owned();

    // get texture type
    c.pm.chtexturetype = c.sh.textures.find(&c.pm.sztexturename);
}

/// `PM_MapTextureTypeStepType(chTextureType)` (pm_shared.cpp 324-338, read 2026-11-22). Wood,
/// computer, grass, glass, flesh and unknown letters map to concrete.
pub(crate) fn pm_map_texture_type_step_type(ch: u8) -> i32 {
    match ch {
        CHAR_TEX_METAL => STEP_METAL,
        CHAR_TEX_DIRT => STEP_DIRT,
        CHAR_TEX_VENT => STEP_VENT,
        CHAR_TEX_GRATE => STEP_GRATE,
        CHAR_TEX_TILE => STEP_TILE,
        CHAR_TEX_SLOSH => STEP_SLOSH,
        CHAR_TEX_SNOW => STEP_SNOW,
        // CHAR_TEX_CONCRETE and `default:`
        _ => STEP_CONCRETE,
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::pm::consts::CONTENTS_EMPTY;
    use crate::pm::{PlayerMove, PmTrace, PmWorld, pm_init};

    /// A world that only answers what the sound code asks: water below a height, the texture
    /// name, a queue of random values and a sound log.
    struct SoundWorld {
        /// Points with `z < water_below` are water.
        water_below: f32,
        texture: Option<String>,
        sounds: Vec<(String, f32)>,
        contents_calls: usize,
    }

    impl SoundWorld {
        fn new() -> Self {
            Self { water_below: f32::NEG_INFINITY, texture: None, sounds: Vec::new(), contents_calls: 0 }
        }
    }

    impl PmWorld for SoundWorld {
        fn player_trace(&mut self, _s: Vec3, e: Vec3, _h: i32, _f: i32, _i: i32) -> PmTrace {
            PmTrace::miss(e)
        }
        fn test_player_position(&mut self, _o: Vec3, _p: Vec3, _h: i32) -> (i32, PmTrace) {
            (-1, PmTrace::default())
        }
        fn point_contents(&mut self, p: Vec3) -> (i32, i32) {
            self.contents_calls += 1;
            if p[2] < self.water_below { (CONTENTS_WATER, CONTENTS_WATER) } else { (CONTENTS_EMPTY, CONTENTS_EMPTY) }
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
            self.texture.clone()
        }
        fn play_sound(&mut self, channel: i32, sample: &str, volume: f32, attn: f32, flags: i32, pitch: i32) {
            assert_eq!((channel, attn, flags, pitch), (CHAN_BODY, ATTN_NORM, 0, PITCH_NORM));
            self.sounds.push((sample.to_string(), volume));
        }
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

    /// A fast walker on the ground (origin z 100).
    fn runner() -> PlayerMove {
        PlayerMove {
            origin: Vec3::new(0.0, 0.0, 100.0),
            velocity: Vec3::new(200.0, 0.0, 0.0),
            onground: 0,
            ..PlayerMove::default()
        }
    }

    fn update(pm: &mut PlayerMove, w: &mut SoundWorld) {
        let mut sh = pm_init(None);
        let mut c = PmCtx { pm, sh: &mut sh, w };
        pm_update_step_sound(&mut c);
    }

    #[test]
    fn ladder_step_uses_ladder_timing_and_skips_water_probes() {
        let mut pm = runner();
        pm.movetype = MOVETYPE_FLY;
        pm.onground = -1;
        let mut w = SoundWorld::new();
        update(&mut pm, &mut w);
        // fvol 0.35, 350 + 100 ms; i_step_left toggles 0 -> 1, RandomLong(0,1)=0 -> irand 2 -> "2".
        assert_eq!(w.sounds, [("player/pl_ladder2.wav".to_string(), 0.35_f64 as f32)]);
        assert_eq!(pm.fl_time_step_sound, 450);
        assert_eq!(w.contents_calls, 0, "the ladder branch never probes the knee or the feet");
    }

    #[test]
    fn ducking_scales_volume_below_950_ms_of_duck_time() {
        let reduced = (0.5_f64 * 0.35) as f32;
        for (duck_time, want) in [(0.0, reduced), (949.5, reduced), (950.0, 0.5)] {
            let mut pm = runner();
            pm.flags |= FL_DUCKING;
            pm.fl_duck_time = duck_time;
            let mut w = SoundWorld::new();
            update(&mut pm, &mut w);
            assert_eq!(w.sounds.len(), 1);
            assert_eq!(w.sounds[0].1, want, "duck time {duck_time}");
            assert_eq!(pm.fl_time_step_sound, 400, "300 + 100 while ducking");
        }

        // Ducking on a ladder: the +100 is applied once, the volume is 0.35 * 0.35.
        let mut pm = runner();
        pm.flags |= FL_DUCKING;
        pm.movetype = MOVETYPE_FLY;
        let mut w = SoundWorld::new();
        update(&mut pm, &mut w);
        assert_eq!(w.sounds[0].1, (f64::from(0.35_f64 as f32) * 0.35) as f32);
        assert_eq!(pm.fl_time_step_sound, 450);
    }

    #[test]
    fn knee_water_wades_and_feet_water_sloshes() {
        // Standing hull: height 72, knee = 100 - 21.6 = 78.4, feet = 100 - 36 = 64.
        let mut pm = runner();
        let mut w = SoundWorld::new();
        w.water_below = 80.0;
        update(&mut pm, &mut w);
        // Wade: the first call only advances iSkipStep, nothing is played, the timer is 600.
        assert!(w.sounds.is_empty());
        assert_eq!(pm.fl_time_step_sound, 600);
        assert_eq!(w.contents_calls, 1, "the knee probe decided it");

        let mut pm = runner();
        let mut w = SoundWorld::new();
        w.water_below = 70.0; // knee (78.4) dry, feet (64) wet
        update(&mut pm, &mut w);
        assert_eq!(w.sounds, [("player/pl_slosh2.wav".to_string(), 0.5)]);
        assert_eq!(pm.fl_time_step_sound, 300);
        assert_eq!(w.contents_calls, 2);

        // The ducked hull is 36 high: knee = 100 - 10.8 = 89.2, feet = 82.
        let mut pm = runner();
        pm.usehull = 1;
        let mut w = SoundWorld::new();
        w.water_below = 85.0; // knee dry, feet wet
        update(&mut pm, &mut w);
        assert_eq!(w.sounds, [("player/pl_slosh2.wav".to_string(), 0.5)]);
    }

    #[test]
    fn early_outs_of_the_step_timer() {
        // Slow: only the 400 ms re-arm, no texture trace, no sound.
        let mut pm = runner();
        pm.velocity = Vec3::new(150.0, 0.0, 0.0);
        let mut w = SoundWorld::new();
        update(&mut pm, &mut w);
        assert_eq!((pm.fl_time_step_sound, w.sounds.len(), w.contents_calls), (400, 0, 0));

        // Frozen: returns before the speed test, the timer stays.
        let mut pm = runner();
        pm.flags |= FL_FROZEN;
        let mut w = SoundWorld::new();
        update(&mut pm, &mut w);
        assert_eq!((pm.fl_time_step_sound, w.sounds.len()), (0, 0));

        // Timer still running: untouched.
        let mut pm = runner();
        pm.fl_time_step_sound = 5;
        let mut w = SoundWorld::new();
        update(&mut pm, &mut w);
        assert_eq!((pm.fl_time_step_sound, w.sounds.len()), (5, 0));

        // In the air (not a ladder): nothing, and the timer is not re-armed.
        let mut pm = runner();
        pm.onground = -1;
        let mut w = SoundWorld::new();
        update(&mut pm, &mut w);
        assert_eq!((pm.fl_time_step_sound, w.sounds.len()), (0, 0));
    }

    #[test]
    fn texture_type_maps_to_step_type() {
        for (ch, step) in [
            (b'C', STEP_CONCRETE),
            (b'M', STEP_METAL),
            (b'D', STEP_DIRT),
            (b'V', STEP_VENT),
            (b'G', STEP_GRATE),
            (b'T', STEP_TILE),
            (b'S', STEP_SLOSH),
            (b'N', STEP_SNOW),
            // Wood, computer, grass, glass, flesh and anything unknown are concrete.
            (b'W', STEP_CONCRETE),
            (b'P', STEP_CONCRETE),
            (b'X', STEP_CONCRETE),
            (b'Y', STEP_CONCRETE),
            (b'F', STEP_CONCRETE),
            (b'?', STEP_CONCRETE),
            (0, STEP_CONCRETE),
        ] {
            assert_eq!(pm_map_texture_type_step_type(ch), step, "letter {}", ch as char);
        }
    }

    #[test]
    fn snow_and_unknown_step_types_have_their_samples() {
        let mut pm = PlayerMove::default();
        let mut sh = pm_init(None);
        let mut w = SoundWorld::new();
        {
            let mut c = PmCtx { pm: &mut pm, sh: &mut sh, w: &mut w };
            pm_play_step_sound(&mut c, STEP_SNOW, 0.5);
            // `default:` of the switch is concrete.
            pm_play_step_sound(&mut c, 42, 0.5);
        }
        // First call: left foot 1, RandomLong 0 -> irand 2 -> "2"; second: left 0 -> irand 0 -> "1".
        assert_eq!(
            w.sounds,
            [("player/pl_snow2.wav".to_string(), 0.5), ("player/pl_step1.wav".to_string(), 0.5)]
        );
    }
}
