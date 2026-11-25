//! Authentic CS 1.6 Kreedz JumpStats (`LJ`, `HJ`, `CJ`, `DCJ`, `BJ`, `SBJ`, `DropBJ`).
//!
//! Tracks takeoff -> airborne strafes/sync -> landing across `pm_shared` steps using the
//! standard `Hull::Standing` width offset (`+32.0 u`), floor-normalized height checks, and
//! ladder / water / trigger_push / teleport / noclip invalidation.

use glam::Vec3;

use crate::pm::consts::{PM_VEC_DUCK_HULL_MIN, PM_VEC_HULL_MIN};

/// GoldSrc standing hull width (`16 - (-16) = 32.0 u`) added to center-to-center takeoff->landing distance.
pub const STANDING_HULL_WIDTH: f32 = 32.0;

/// Supported Kreedz jump classifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpKind {
    Lj,
    Hj,
    Cj,
    Dcj,
    Bj,
    Sbj,
    DropBj,
}

impl JumpKind {
    /// Short classic Kreedz tag (`"LJ"`, `"HJ"`, `"CJ"`, `"DCJ"`, `"BJ"`, `"SBJ"`, `"DropBJ"`).
    pub fn tag(self) -> &'static str {
        match self {
            Self::Lj => "LJ",
            Self::Hj => "HJ",
            Self::Cj => "CJ",
            Self::Dcj => "DCJ",
            Self::Bj => "BJ",
            Self::Sbj => "SBJ",
            Self::DropBj => "DropBJ",
        }
    }
}

/// Completed valid jump metrics produced on landing.
#[derive(Debug, Clone, PartialEq)]
pub struct JumpStats {
    pub kind: JumpKind,
    pub distance: f32,
    pub prestrafe: f32,
    pub max_speed: f32,
    pub strafes: u32,
    pub sync_pct: f32,
    pub height_diff: f32,
}

impl JumpStats {
    /// Classic single-line KZ jump summary (`[LJ] Distance: 252.4 u | Pre: ...`).
    pub fn format_summary(&self) -> String {
        format!(
            "[{}] Distance: {:.1} u | Pre: {:.1} | Max: {:.1} | Strafes: {} | Sync: {:.0}%",
            self.kind.tag(),
            self.distance,
            self.prestrafe,
            self.max_speed,
            self.strafes,
            self.sync_pct,
        )
    }
}

/// Per-frame movement snapshot fed to [`JumpTracker::observe`].
#[derive(Debug, Clone, Copy)]
pub struct JumpFrameInput {
    pub dt: f32,
    pub origin: Vec3,
    pub velocity: Vec3,
    pub on_ground: bool,
    pub ducking: bool,
    pub waterlevel: i32,
    pub on_ladder: bool,
    pub noclip: bool,
    pub jump_pressed: bool,
    pub sidemove: f32,
    pub forwardmove: f32,
    pub gravity: f32,
    pub airaccelerate: f32,
    pub maxspeed: f32,
    pub is_highjump_edge: bool,
    pub min_distance: f32,
}

#[derive(Debug, Clone)]
struct ActiveJump {
    kind: JumpKind,
    takeoff_origin: Vec3,
    takeoff_floor_z: f32,
    prestrafe: f32,
    max_speed: f32,
    prev_air_speed: f32,
    strafes: u32,
    last_strafe_dir: i8,
    sync_good: u32,
    sync_total: u32,
    invalid: bool,
}

/// Stateful per-player Kreedz JumpStats tracker shared by server (`sv_ljstats`) and client (`cl_ljstats`).
#[derive(Debug, Clone)]
pub struct JumpTracker {
    initialized: bool,
    last_on_ground: bool,
    last_origin: Vec3,
    last_floor_z: f32,
    last_speed_2d: f32,
    prev_ducking: bool,
    ground_frames: u32,
    recent_landing_dz: f32,
    recent_landing_ducked: bool,
    duck_tap_count: u8,
    duck_tap_timer: f32,
    active: Option<ActiveJump>,
}

impl Default for JumpTracker {
    fn default() -> Self {
        Self {
            initialized: false,
            last_on_ground: false,
            last_origin: Vec3::ZERO,
            last_floor_z: 0.0,
            last_speed_2d: 0.0,
            prev_ducking: false,
            ground_frames: 100,
            recent_landing_dz: 0.0,
            recent_landing_ducked: false,
            duck_tap_count: 0,
            duck_tap_timer: 0.0,
            active: None,
        }
    }
}

impl JumpTracker {
    /// Invalidates any in-flight jump (teleports, KZ checkpoints, `trigger_push`, spawn/death).
    pub fn invalidate(&mut self) {
        self.active = None;
        self.ground_frames = 100;
        self.recent_landing_dz = 0.0;
        self.recent_landing_ducked = false;
        self.duck_tap_count = 0;
        self.duck_tap_timer = 0.0;
        self.initialized = false;
    }

    /// Invalidates the current jump when `trigger_push` touches the player.
    pub fn mark_pushed(&mut self) {
        if let Some(j) = self.active.as_mut() {
            j.invalid = true;
        }
        self.ground_frames = 100;
    }

    fn floor_z(origin: Vec3, ducking: bool) -> f32 {
        let min_z = if ducking {
            PM_VEC_DUCK_HULL_MIN
        } else {
            PM_VEC_HULL_MIN
        };
        origin.z + min_z
    }

    /// Processes one movement step and returns [`JumpStats`] when a valid jump lands `>= min_distance`.
    pub fn observe(&mut self, input: JumpFrameInput) -> Option<JumpStats> {
        let floor_z = Self::floor_z(input.origin, input.ducking);
        let speed_2d = (input.velocity.x * input.velocity.x + input.velocity.y * input.velocity.y).sqrt();

        let env_invalid = !input.origin.is_finite()
            || !input.velocity.is_finite()
            || input.waterlevel > 0
            || input.on_ladder
            || input.noclip
            || (input.gravity - 800.0).abs() > 0.5
            || (input.airaccelerate - 10.0).abs() > 0.5
            || input.maxspeed > 260.0;

        if env_invalid {
            self.active = None;
            self.ground_frames = 100;
            self.recent_landing_dz = 0.0;
            self.recent_landing_ducked = false;
            self.duck_tap_count = 0;
            self.duck_tap_timer = 0.0;
            self.record_prev(input, floor_z, speed_2d);
            return None;
        }

        if !self.initialized {
            self.initialized = true;
            self.record_prev(input, floor_z, speed_2d);
            return None;
        }

        let mut completed = None;

        if input.on_ground {
            if let Some(j) = self.active.take() {
                let dx = input.origin.x - j.takeoff_origin.x;
                let dy = input.origin.y - j.takeoff_origin.y;
                let distance = (dx * dx + dy * dy).sqrt() + STANDING_HULL_WIDTH;
                let height_diff = floor_z - j.takeoff_floor_z;

                self.recent_landing_dz = height_diff;
                self.recent_landing_ducked = input.ducking;
                self.ground_frames = 0;

                let flat_ok = height_diff.abs() <= 0.5;
                let min_dist = if input.min_distance.is_finite() && input.min_distance > 0.0 {
                    input.min_distance
                } else {
                    215.0
                };
                if !j.invalid && flat_ok && distance >= min_dist && distance <= 400.0 {
                    let sync_pct = if j.sync_total > 0 {
                        (j.sync_good as f32 / j.sync_total as f32) * 100.0
                    } else {
                        0.0
                    };
                    completed = Some(JumpStats {
                        kind: j.kind,
                        distance,
                        prestrafe: j.prestrafe,
                        max_speed: j.max_speed,
                        strafes: j.strafes,
                        sync_pct,
                        height_diff,
                    });
                }
            } else {
                self.ground_frames = self.ground_frames.saturating_add(1);
                if self.ground_frames > 6 {
                    self.recent_landing_dz = 0.0;
                    self.recent_landing_ducked = false;
                }
            }

            // Track ground duck-taps for CJ / DCJ.
            if self.duck_tap_timer > 0.0 {
                self.duck_tap_timer = (self.duck_tap_timer - input.dt.max(0.0)).max(0.0);
                if self.duck_tap_timer == 0.0 {
                    self.duck_tap_count = 0;
                }
            }
            if !input.ducking && self.prev_ducking {
                self.duck_tap_count = self.duck_tap_count.saturating_add(1);
                self.duck_tap_timer = 0.45;
            }
        } else if self.last_on_ground && !input.on_ground {
            // Takeoff frame: only track real jumps (upward vertical velocity and +jump).
            if input.velocity.z > 100.0 && input.jump_pressed {
                let kind = if self.ground_frames <= 6 {
                    if self.recent_landing_dz < -4.0 {
                        JumpKind::DropBj
                    } else if self.recent_landing_ducked && !input.ducking {
                        JumpKind::Sbj
                    } else {
                        JumpKind::Bj
                    }
                } else if self.duck_tap_count >= 2 {
                    JumpKind::Dcj
                } else if self.duck_tap_count == 1 {
                    JumpKind::Cj
                } else if input.is_highjump_edge {
                    JumpKind::Hj
                } else {
                    JumpKind::Lj
                };
                self.duck_tap_count = 0;
                self.duck_tap_timer = 0.0;
                self.active = Some(ActiveJump {
                    kind,
                    takeoff_origin: self.last_origin,
                    takeoff_floor_z: self.last_floor_z,
                    prestrafe: self.last_speed_2d,
                    max_speed: speed_2d.max(self.last_speed_2d),
                    prev_air_speed: speed_2d,
                    strafes: 0,
                    last_strafe_dir: 0,
                    sync_good: 0,
                    sync_total: 0,
                    invalid: false,
                });
            } else {
                self.active = None;
            }
        } else if let Some(j) = self.active.as_mut() {
            // Airborne frame.
            j.max_speed = j.max_speed.max(speed_2d);
            let dir = if input.sidemove < -1.0 {
                -1
            } else if input.sidemove > 1.0 {
                1
            } else {
                0
            };
            if dir != 0 && dir != j.last_strafe_dir {
                j.strafes = j.strafes.saturating_add(1);
                j.last_strafe_dir = dir;
            }
            if input.sidemove.abs() > 1.0 || input.forwardmove.abs() > 1.0 {
                j.sync_total = j.sync_total.saturating_add(1);
                if speed_2d > j.prev_air_speed + 0.0001 {
                    j.sync_good = j.sync_good.saturating_add(1);
                }
            }
            j.prev_air_speed = speed_2d;
        }

        self.record_prev(input, floor_z, speed_2d);
        completed
    }

    fn record_prev(&mut self, input: JumpFrameInput, floor_z: f32, speed_2d: f32) {
        self.last_on_ground = input.on_ground;
        self.last_origin = input.origin;
        self.last_floor_z = floor_z;
        self.last_speed_2d = speed_2d;
        self.prev_ducking = input.ducking;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_frame() -> JumpFrameInput {
        JumpFrameInput {
            dt: 0.01,
            origin: Vec3::new(0.0, 0.0, 36.0),
            velocity: Vec3::new(250.0, 0.0, 0.0),
            on_ground: true,
            ducking: false,
            waterlevel: 0,
            on_ladder: false,
            noclip: false,
            jump_pressed: false,
            sidemove: 0.0,
            forwardmove: 0.0,
            gravity: 800.0,
            airaccelerate: 10.0,
            maxspeed: 250.0,
            is_highjump_edge: false,
            min_distance: 215.0,
        }
    }

    #[test]
    fn flat_lj_adds_32_units_and_counts_strafes_and_sync() {
        let mut tr = JumpTracker::default();
        assert!(tr.observe(base_frame()).is_none());

        // Takeoff at x=0, prestrafe=275.
        let mut f = base_frame();
        f.origin = Vec3::new(0.0, 0.0, 36.0);
        f.velocity = Vec3::new(275.0, 0.0, 0.0);
        assert!(tr.observe(f).is_none());

        f.on_ground = false;
        f.jump_pressed = true;
        f.origin = Vec3::new(2.75, 0.0, 38.5);
        f.velocity = Vec3::new(275.0, 0.0, 268.0);
        assert!(tr.observe(f).is_none());

        // Strafe 1 (left) with accelerating speed -> 2 synced frames.
        for (i, spd) in [280.0_f32, 286.0].into_iter().enumerate() {
            f.origin = Vec3::new(50.0 + (i as f32) * 3.0, 0.0, 60.0);
            f.velocity = Vec3::new(spd, 0.0, 120.0);
            f.sidemove = -400.0;
            assert!(tr.observe(f).is_none());
        }
        // Strafe 2 (right): 1 gain + 1 deceleration frame -> 3/4 sync = 75%.
        for (spd, x) in [(294.0_f32, 120.0_f32), (290.0_f32, 180.0_f32)] {
            f.origin = Vec3::new(x, 0.0, 50.0);
            f.velocity = Vec3::new(spd, 0.0, -40.0);
            f.sidemove = 400.0;
            assert!(tr.observe(f).is_none());
        }

        // Land ducked at x=210 (floor z still 0 -> origin.z=18 with ducking=true) => 210 + 32 = 242.0u!
        f.on_ground = true;
        f.ducking = true;
        f.origin = Vec3::new(210.0, 0.0, 18.0);
        f.velocity = Vec3::new(290.0, 0.0, 0.0);
        f.sidemove = 0.0;
        let stats = tr.observe(f).expect("valid flat LJ");
        assert_eq!(stats.kind, JumpKind::Lj);
        assert!((stats.distance - 242.0).abs() < 0.01, "{}", stats.distance);
        assert!((stats.prestrafe - 275.0).abs() < 0.01);
        assert!((stats.max_speed - 294.0).abs() < 0.01);
        assert_eq!(stats.strafes, 2);
        assert!((stats.sync_pct - 75.0).abs() < 0.01, "{}", stats.sync_pct);
    }

    #[test]
    fn drop_jump_is_rejected_and_ladder_water_tp_invalidate() {
        let mut tr = JumpTracker::default();
        assert!(tr.observe(base_frame()).is_none());

        // Jump from z=36 (floor=0) and land lower at z=-28 (floor=-64).
        let mut f = base_frame();
        f.on_ground = false;
        f.jump_pressed = true;
        f.origin = Vec3::new(3.0, 0.0, 40.0);
        f.velocity = Vec3::new(250.0, 0.0, 268.0);
        assert!(tr.observe(f).is_none());

        f.on_ground = true;
        f.origin = Vec3::new(220.0, 0.0, -28.0);
        f.velocity = Vec3::new(250.0, 0.0, 0.0);
        assert!(tr.observe(f).is_none(), "drop jump must be rejected");

        // Water/ladder/teleport invalidates mid-air jump.
        f.origin = Vec3::new(0.0, 0.0, 36.0);
        assert!(tr.observe(f).is_none());
        f.on_ground = false;
        f.jump_pressed = true;
        f.velocity = Vec3::new(250.0, 0.0, 268.0);
        assert!(tr.observe(f).is_none());
        f.waterlevel = 1;
        assert!(tr.observe(f).is_none());
        f.waterlevel = 0;
        f.on_ground = true;
        f.origin = Vec3::new(210.0, 0.0, 36.0);
        assert!(tr.observe(f).is_none(), "water touch must invalidate jump");
    }
}
