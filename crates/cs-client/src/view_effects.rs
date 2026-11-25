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

/// Port of GoldSrc `V_CalcBob` (`client.dll` `FUN_1006ab20`): returns vertical/forward view bob scalar in HL units.
pub fn calc_bob(time_sec: f32, velocity: Vec3, onground: bool, params: &BobParams) -> f32 {
    if params.cl_bob <= 0.0 || params.cl_bobcycle <= 0.0 || !onground {
        return 0.0;
    }
    let cycle_time = params.cl_bobcycle.max(0.1);
    let bob_up = params.cl_bobup;
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

/// Stateful GoldSrc `V_CalcBob` (`client.dll` `FUN_1006ab20`): freezes and returns the last
/// grounded bob scalar while airborne (`!onground`).
#[derive(Debug, Clone, Default)]
pub struct BobState {
    last_bob: f32,
}

impl BobState {
    pub fn reset(&mut self) {
        self.last_bob = 0.0;
    }

    pub fn calc(&mut self, time_sec: f32, velocity: Vec3, onground: bool, params: &BobParams) -> f32 {
        if !onground {
            return self.last_bob;
        }
        let bob = calc_bob(time_sec, velocity, true, params);
        self.last_bob = bob;
        bob
    }
}

/// GoldSrc `V_AddIdle` (`client.dll` `0x101065b4..1010662c`) angular drift offsets in degrees.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ViewIdleAngles {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

/// Compute `V_AddIdle` offsets (`v_iyaw_cycle=2.0, v_iyaw_level=0.3`,
/// `v_iroll_cycle=0.5, v_iroll_level=0.1`, `v_ipitch_cycle=1.0, v_ipitch_level=0.3`).
pub fn calc_view_idle(time_sec: f32, idle_scale: f32) -> ViewIdleAngles {
    if !idle_scale.is_finite() || idle_scale == 0.0 {
        return ViewIdleAngles::default();
    }
    ViewIdleAngles {
        pitch: idle_scale * (time_sec * 1.0).sin() * 0.3,
        yaw: idle_scale * (time_sec * 2.0).sin() * 0.3,
        roll: idle_scale * (time_sec * 0.5).sin() * 0.1,
    }
}

/// Compute GoldSrc viewmodel `(yaw_deg, pitch_deg, roll_deg)` from camera angles (`client.dll`
/// `0x1006b52f..1006b7a0`, `0x101065b4..1010662c`) with `bob` and `V_AddIdle` counter-offsets.
pub fn calc_viewmodel_angles(
    view_yaw: f32,
    view_pitch: f32,
    view_roll: f32,
    bob: f32,
    idle: ViewIdleAngles,
) -> (f32, f32, f32) {
    let vm_yaw = view_yaw - (idle.yaw + bob * 0.5);
    let vm_pitch = view_pitch - bob * 0.3 - idle.pitch * 0.5;
    let vm_roll = view_roll - (idle.roll + bob * 1.0);
    (vm_yaw, vm_pitch, vm_roll)
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

/// Stateful GoldSrc stair smoothing (`V_CalcFirstPersonView` `FUN_1006ab20`).
#[derive(Debug, Clone, Default)]
pub struct StairSmoothState {
    prev_z: Option<f32>,
}

impl StairSmoothState {
    pub fn reset(&mut self) {
        self.prev_z = None;
    }

    /// Returns a Z offset (`<= 0.0`) added to both eye and viewmodel origin when stepping up stairs.
    pub fn offset(&mut self, view_z: f32, on_ground: bool, dt: f32) -> f32 {
        if !view_z.is_finite() {
            return 0.0;
        }
        if let Some(pz) = self.prev_z
            && on_ground
            && (view_z - pz) > 0.0
        {
            let mut next_z = pz + dt.max(0.0) * 150.0;
            if next_z > view_z {
                next_z = view_z;
            }
            if (view_z - next_z) > 18.0 {
                next_z = view_z - 18.0;
            }
            self.prev_z = Some(next_z);
            return next_z - view_z;
        }
        self.prev_z = Some(view_z);
        0.0
    }
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

    #[test]
    fn stair_smooth_steps_up_and_caps_at_18() {
        let mut s = StairSmoothState::default();
        assert_eq!(s.offset(0.0, true, 0.016), 0.0);
        // Step up 16 units at 10ms -> advances 1.5 units -> offset -14.5
        let off = s.offset(16.0, true, 0.01);
        assert!((off - (-14.5)).abs() < 1e-4, "got {off}");
        // Jump by +50 units -> clamped to within 18 units -> offset -18.0
        let big = s.offset(66.0, true, 0.01);
        assert!((big - (-18.0)).abs() < 1e-4, "got {big}");
        // Not on ground resets baseline immediately
        assert_eq!(s.offset(100.0, false, 0.01), 0.0);
    }

    #[test]
    fn bob_state_freezes_airborne_and_allows_unclamped_bobup() {
        let params = BobParams {
            cl_bob: 0.01,
            cl_bobcycle: 0.8,
            cl_bobup: 0.02,
        };
        let mut bob_state = BobState::default();
        let grounded = bob_state.calc(0.008, Vec3::new(250.0, 0.0, 0.0), true, &params);
        assert!(grounded > 0.0);
        // Airborne preserves last grounded bob instead of snapping to 0.0.
        assert_eq!(bob_state.calc(0.4, Vec3::new(250.0, 0.0, 0.0), false, &params), grounded);
        // Unclamped cl_bobup = 0.02 differs from 0.05 clamped behavior.
        let b_low = calc_bob(0.03, Vec3::new(250.0, 0.0, 0.0), true, &params);
        let b_mid = calc_bob(
            0.03,
            Vec3::new(250.0, 0.0, 0.0),
            true,
            &BobParams { cl_bobup: 0.05, ..params },
        );
        assert!((b_low - b_mid).abs() > 0.01);
        // Edge values 0.0 / 1.0 remain finite.
        assert!(calc_bob(0.1, Vec3::new(250.0, 0.0, 0.0), true, &BobParams { cl_bobup: 0.0, ..params }).is_finite());
        assert!(calc_bob(0.1, Vec3::new(250.0, 0.0, 0.0), true, &BobParams { cl_bobup: 1.0, ..params }).is_finite());
    }

    #[test]
    fn view_idle_and_viewmodel_angles_match_goldsrc_cycles() {
        let t = 1.0f32;
        let idle = calc_view_idle(t, 2.0);
        assert!((idle.roll - 2.0 * (0.5f32).sin() * 0.1).abs() < 1e-5);
        assert!((idle.pitch - 2.0 * (1.0f32).sin() * 0.3).abs() < 1e-5);
        assert!((idle.yaw - 2.0 * (2.0f32).sin() * 0.3).abs() < 1e-5);

        let (vm_yaw, vm_pitch, vm_roll) = calc_viewmodel_angles(90.0, 10.0, 1.0, 2.0, idle);
        assert!((vm_yaw - (90.0 - (idle.yaw + 1.0))).abs() < 1e-5);
        assert!((vm_pitch - (10.0 - 0.6 - idle.pitch * 0.5)).abs() < 1e-5);
        assert!((vm_roll - (1.0 - (idle.roll + 2.0))).abs() < 1e-5);
    }
}
