//! Authentic GoldSrc first-person view & HUD effects:
//! - View bob (`cl_bob`, `cl_bobcycle`, `cl_bobup` -> `V_CalcBob`)
//! - Strafe view roll (`cl_rollangle`, `cl_rollspeed` -> `V_CalcRoll`)
//! - Weapon viewmodel lag/sway (`cl_viewmodel_lag` -> `V_CalcViewModelLag`)
//! - `ScreenShake` procedural pitch/yaw/roll jitter
//! - `ScreenFade` (`FFADE_IN`, `FFADE_OUT`, `FFADE_MODULATE`, `FFADE_STAYOUT`, `FFADE_PURGE`)
//! - Underwater color tint and FOV sinusoidal warp (`CONTENTS_WATER` / `SLIME` / `LAVA`)

use cs_clientcore::hud::{FFADE_MODULATE, FFADE_OUT, FFADE_PURGE, FFADE_STAYOUT, ScreenFade, ScreenShake};
use cs_pm::collision::{CONTENTS_LAVA, CONTENTS_SLIME, CONTENTS_WATER};
use glam::Vec3;

/// GoldSrc fixed-point `1 << 12` (4096) scale for `ScreenFade` & `ScreenShake`.
const FIXED_12: f32 = 4096.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BobParams {
    pub cl_bob: f32,
    pub cl_bobcycle: f32,
    pub cl_bobup: f32,
}

impl Default for BobParams {
    fn default() -> Self {
        Self {
            cl_bob: 0.01,
            cl_bobcycle: 0.8,
            cl_bobup: 0.5,
        }
    }
}

/// Port of GoldSrc `V_CalcBob`: returns vertical/forward view bob scalar in HL units.
pub fn calc_bob(time_sec: f32, velocity: Vec3, onground: bool, params: &BobParams) -> f32 {
    if params.cl_bob <= 0.0 || params.cl_bobcycle <= 0.0 || !onground {
        return 0.0;
    }
    let cycle_time = params.cl_bobcycle.max(0.05);
    let bob_up = params.cl_bobup.clamp(0.05, 0.95);
    let cycle = time_sec / cycle_time;
    let frac = (cycle - cycle.floor()) * cycle_time;
    let mut cycle_phase = if frac < bob_up * cycle_time {
        std::f32::consts::PI * frac / (bob_up * cycle_time)
    } else {
        std::f32::consts::PI
            + std::f32::consts::PI * (frac - bob_up * cycle_time) / (cycle_time * (1.0 - bob_up))
    };
    if !cycle_phase.is_finite() {
        cycle_phase = 0.0;
    }
    let speed = Vec3::new(velocity.x, velocity.y, 0.0).length();
    (speed * params.cl_bob * (0.3 + 0.7 * cycle_phase.sin())).clamp(-7.0, 4.0)
}

/// Port of GoldSrc `V_CalcRoll`: strafe roll angle in degrees based on sideways speed.
pub fn calc_roll(yaw_deg: f32, velocity: Vec3, rollangle: f32, rollspeed: f32) -> f32 {
    if rollangle.abs() <= 1e-4 || rollspeed <= 1.0 {
        return 0.0;
    }
    let yaw = yaw_deg.to_radians();
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let side = velocity.dot(right);
    let sign = side.signum();
    let abs_side = side.abs();
    let mag = if abs_side < rollspeed {
        abs_side * rollangle / rollspeed
    } else {
        rollangle
    };
    mag * sign
}

/// Stateful weapon viewmodel inertia / lag (`V_CalcViewModelLag`, `cl_viewmodel_lag`).
#[derive(Debug, Clone, Default)]
pub struct ViewModelLagState {
    last_forward: Option<Vec3>,
    lag_offset: Vec3,
}

impl ViewModelLagState {
    pub fn reset(&mut self) {
        self.last_forward = None;
        self.lag_offset = Vec3::ZERO;
    }

    pub fn update(&mut self, forward: Vec3, dt: f32, lag_scale: f32) -> Vec3 {
        let dir = forward.normalize_or(Vec3::X);
        if lag_scale <= 0.001 || dt <= 0.0 {
            self.last_forward = Some(dir);
            self.lag_offset = Vec3::ZERO;
            return Vec3::ZERO;
        }
        let prev = self.last_forward.unwrap_or(dir);
        let delta = dir - prev;
        self.last_forward = Some(dir);
        let damping = (dt * 12.0).clamp(0.0, 1.0);
        self.lag_offset = self.lag_offset.lerp(-delta * 6.0 * lag_scale, damping);
        if self.lag_offset.length() > 1.5 {
            self.lag_offset = self.lag_offset.normalize() * 1.5;
        }
        self.lag_offset
    }
}

/// Evaluate active `ScreenShake` `(pitch_deg, yaw_deg, roll_deg, z_offset)` jitter.
pub fn calc_screen_shake(shake: Option<&ScreenShake>, now: f64) -> (Vec3, f32) {
    let Some(s) = shake else {
        return (Vec3::ZERO, 0.0);
    };
    let dur = f64::from(s.duration) / f64::from(FIXED_12);
    if dur <= 0.0 || now < s.started_at || now > s.started_at + dur {
        return (Vec3::ZERO, 0.0);
    }
    let elapsed = now - s.started_at;
    let frac = (1.0 - elapsed / dur).clamp(0.0, 1.0) as f32;
    let amp = (f32::from(s.amplitude) / FIXED_12) * frac;
    let freq = (f32::from(s.frequency) / FIXED_12 * 100.0).max(4.0);
    let t = now as f32 * freq;
    let angles = Vec3::new(
        t.sin() * amp * 0.5,
        (t * 1.3 + 0.7).cos() * amp * 0.5,
        (t * 1.7 + 1.4).sin() * amp * 0.35,
    );
    let z = (t * 2.1).cos() * amp * 0.6;
    (angles, z)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvaluatedScreenFade {
    pub rgba: [f32; 4],
    pub modulate: bool,
}

/// Evaluate active `ScreenFade` with `FFADE_IN`, `FFADE_OUT`, `FFADE_MODULATE`, `FFADE_STAYOUT`,
/// and `FFADE_PURGE` semantics (`color` normalized 0..1).
pub fn evaluate_screen_fade(fade: Option<&ScreenFade>, now: f64) -> Option<EvaluatedScreenFade> {
    let f = fade?;
    if f.flags & FFADE_PURGE != 0 {
        return None;
    }
    let stayout = f.flags & FFADE_STAYOUT != 0;
    let fade_out = f.flags & FFADE_OUT != 0;
    let modulate = f.flags & FFADE_MODULATE != 0;
    let dur = f64::from(f.duration) / f64::from(FIXED_12);
    let hold = f64::from(f.hold_time) / f64::from(FIXED_12);
    let elapsed = (now - f.started_at).max(0.0);

    let alpha_frac = if fade_out {
        if dur <= 0.0 {
            if stayout || elapsed <= hold { 1.0 } else { 0.0 }
        } else if elapsed < dur {
            (elapsed / dur).clamp(0.0, 1.0)
        } else if stayout || elapsed <= dur + hold {
            1.0
        } else {
            0.0
        }
    } else if elapsed < hold {
        1.0
    } else if dur > 0.0 && elapsed < hold + dur {
        (1.0 - (elapsed - hold) / dur).clamp(0.0, 1.0)
    } else {
        0.0
    };

    if alpha_frac <= 0.001 {
        return None;
    }
    let a = (f32::from(f.color[3]) / 255.0) * alpha_frac as f32;
    Some(EvaluatedScreenFade {
        rgba: [
            f32::from(f.color[0]) / 255.0,
            f32::from(f.color[1]) / 255.0,
            f32::from(f.color[2]) / 255.0,
            a.clamp(0.0, 1.0),
        ],
        modulate,
    })
}

/// Compute underwater screen tint (`rgba`) and FOV warp multiplier based on BSP eye `watertype`
/// (`CONTENTS_WATER` / `CONTENTS_SLIME` / `CONTENTS_LAVA`).
pub fn underwater_tint_and_fov(watertype: i32, waterlevel: u8, time_sec: f32) -> (Option<[f32; 4]>, f32) {
    if waterlevel < 3 {
        return (None, 0.0);
    }
    let fov_delta = (time_sec * 3.0).sin() * 2.5;
    let tint = match watertype {
        CONTENTS_LAVA => Some([0.95, 0.28, 0.05, 0.42]),
        CONTENTS_SLIME => Some([0.16, 0.65, 0.14, 0.36]),
        CONTENTS_WATER => Some([0.12, 0.42, 0.68, 0.34]),
        _ => None,
    };
    (tint, if tint.is_some() { fov_delta } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bob_and_roll_follow_goldsrc_bounds() {
        let params = BobParams::default();
        assert_eq!(calc_bob(1.0, Vec3::new(250.0, 0.0, 0.0), false, &params), 0.0);
        let b = calc_bob(0.2, Vec3::new(250.0, 0.0, 0.0), true, &params);
        assert!((-7.0..=4.0).contains(&b) && b > 0.0);

        let r = calc_roll(0.0, Vec3::new(0.0, -300.0, 0.0), 2.0, 200.0);
        assert!((r - 2.0).abs() < 1e-4, "expected clamped 2 deg roll, got {r}");
        let half = calc_roll(0.0, Vec3::new(0.0, -100.0, 0.0), 2.0, 200.0);
        assert!((half - 1.0).abs() < 1e-4);
    }

    #[test]
    fn viewmodel_lag_and_shake_behave_smoothly() {
        let mut lag = ViewModelLagState::default();
        assert_eq!(lag.update(Vec3::X, 0.016, 1.0), Vec3::ZERO);
        let off = lag.update(Vec3::new(0.8, 0.6, 0.0), 0.016, 1.0);
        assert!(off.length() > 0.0 && off.length() <= 1.5);

        let shake = ScreenShake {
            amplitude: 4096 * 4,
            duration: 4096,
            frequency: 4096 * 2,
            started_at: 10.0,
        };
        let (ang, z) = calc_screen_shake(Some(&shake), 10.25);
        assert!(ang.length() > 0.0 || z.abs() > 0.0);
        let (ang_end, _) = calc_screen_shake(Some(&shake), 12.0);
        assert_eq!(ang_end, Vec3::ZERO);
    }

    #[test]
    fn screen_fade_and_underwater_work() {
        let fade_in = ScreenFade {
            duration: 4096 * 2,
            hold_time: 4096,
            flags: FFADE_MODULATE,
            color: [255, 255, 255, 200],
            started_at: 5.0,
        };
        let ev = evaluate_screen_fade(Some(&fade_in), 5.5).expect("hold");
        assert!(ev.modulate);
        assert!((ev.rgba[3] - 200.0 / 255.0).abs() < 1e-3);
        assert!(evaluate_screen_fade(Some(&fade_in), 9.0).is_none());

        let fade_out = ScreenFade {
            duration: 4096,
            hold_time: 0,
            flags: FFADE_OUT | FFADE_STAYOUT,
            color: [0, 0, 0, 255],
            started_at: 1.0,
        };
        let stayout = evaluate_screen_fade(Some(&fade_out), 100.0).expect("stayout");
        assert!((stayout.rgba[3] - 1.0).abs() < 1e-3);

        let (tint, dfov) = underwater_tint_and_fov(CONTENTS_WATER, 3, 0.5);
        assert!(tint.is_some() && dfov.abs() > 0.0);
        assert_eq!(underwater_tint_and_fov(CONTENTS_WATER, 1, 0.5), (None, 0.0));
    }
}
