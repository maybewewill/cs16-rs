//! Browser input mapping (`KeyboardEvent.code`, Pointer Lock mouse deltas, Web Gamepad API,
//! and touch controls) into CS 1.6 actions and [`GoldSrcUserCmd`] packets.

use std::collections::HashSet;

use cs_client::camera::Camera;
use cs_client::input::{
    CmdClock, CmdParams, Held, MoveSpeeds, accumulate_wheel_notches, build_usercmd,
};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_proto48::delta::GoldSrcUserCmd;
use glam::Vec3;

const GAMEPAD_DEADZONE: f32 = 0.15;

/// Touch/gamepad unified button bitmask (matches `web/input.js`).
pub mod pad_bit {
    pub const FIRE: u32 = 1 << 0;
    pub const ALTFIRE: u32 = 1 << 1;
    pub const DUCK: u32 = 1 << 2;
    pub const RELOAD: u32 = 1 << 3;
    pub const USE: u32 = 1 << 4;
    pub const JUMP: u32 = 1 << 5;
}

/// Action state collected from DOM keyboard/mouse/gamepad/touch events between frames.
#[derive(Debug, Clone)]
pub struct WebInputState {
    pub actions: HashSet<String>,
    /// Same-frame `+action` impulse latch (`kbutton_t` down-impulse bit) so wheel-scroll
    /// `+jump`/`-jump` or `+duck`/`-duck` within one frame sets the button bit on the next cmd.
    pub impulse_actions: HashSet<String>,
    pub mouse1: bool,
    pub mouse2: bool,
    pub pending_impulse: u8,
    pub wheel_line_accum: f32,
    pub wheel_pixel_accum: f32,
    pub gamepad_axes: [f32; 4],
    pub gamepad_mask: u32,
    pub touch_axes: [f32; 2],
    pub touch_mask: u32,
    pub pending_commands: Vec<String>,
    pub clock: CmdClock,
}

impl Default for WebInputState {
    fn default() -> Self {
        Self::new()
    }
}

impl WebInputState {
    pub fn new() -> Self {
        Self {
            actions: HashSet::new(),
            impulse_actions: HashSet::new(),
            mouse1: false,
            mouse2: false,
            pending_impulse: 0,
            wheel_line_accum: 0.0,
            wheel_pixel_accum: 0.0,
            gamepad_axes: [0.0; 4],
            gamepad_mask: 0,
            touch_axes: [0.0; 2],
            touch_mask: 0,
            pending_commands: Vec::new(),
            clock: CmdClock::default(),
        }
    }

    /// Maps a DOM `KeyboardEvent.code` transition to GoldSrc held actions or one-shot commands.
    pub fn on_key(&mut self, code: &str, pressed: bool) {
        let held_action = match code {
            "KeyW" | "ArrowUp" => Some("forward"),
            "KeyS" | "ArrowDown" => Some("back"),
            "KeyA" => Some("moveleft"),
            "KeyD" => Some("moveright"),
            "Space" => Some("jump"),
            "ControlLeft" | "ControlRight" => Some("duck"),
            "ShiftLeft" | "ShiftRight" => Some("speed"),
            "KeyR" => Some("reload"),
            "KeyE" => Some("use"),
            "Tab" => Some("showscores"),
            _ => None,
        };
        if let Some(action) = held_action {
            if pressed {
                self.impulse_actions.insert(action.to_string());
                self.actions.insert(action.to_string());
            } else {
                self.actions.remove(action);
            }
            return;
        }

        if !pressed {
            return;
        }

        let one_shot = match code {
            "KeyG" => Some("drop"),
            "KeyB" => Some("buy"),
            "KeyQ" => Some("lastinv"),
            "Digit1" => Some("slot1"),
            "Digit2" => Some("slot2"),
            "Digit3" => Some("slot3"),
            "Digit4" => Some("slot4"),
            "Digit5" => Some("slot5"),
            "Digit6" => Some("slot6"),
            "Digit7" => Some("slot7"),
            "Digit8" => Some("slot8"),
            "Digit9" => Some("slot9"),
            "Digit0" => Some("slot10"),
            _ => None,
        };
        if let Some(cmd) = one_shot {
            self.pending_commands.push(cmd.to_string());
        }
    }

    /// Maps a DOM `MouseEvent.button` (`0`=Left, `1`=Middle, `2`=Right, `3`=Back, `4`=Forward)
    /// to the GoldSrc bind key name (`mouse1..mouse5`).
    pub fn mouse_button_to_goldsrc(button: u32) -> Option<&'static str> {
        match button {
            0 => Some("mouse1"),
            1 => Some("mouse3"),
            2 => Some("mouse2"),
            3 => Some("mouse4"),
            4 => Some("mouse5"),
            _ => None,
        }
    }

    pub fn on_mouse_button(&mut self, button: u32, pressed: bool) {
        match button {
            0 => {
                if pressed {
                    self.impulse_actions.insert("attack".to_string());
                }
                self.mouse1 = pressed;
            }
            2 => {
                if pressed {
                    self.impulse_actions.insert("attack2".to_string());
                }
                self.mouse2 = pressed;
            }
            _ => {}
        }
    }

    /// Routes DOM `MouseEvent.button` (`mouse1..mouse5`) through [`CVarRegistry`] key bindings.
    pub fn on_mouse_button_bound(
        &mut self,
        button: u32,
        pressed: bool,
        cvars: &mut CVarRegistry,
    ) {
        if let Some(key_name) = Self::mouse_button_to_goldsrc(button) {
            let actions = cvars.handle_key_bind(key_name, pressed);
            self.apply_engine_actions(actions);
        }
    }

    /// Routes DOM `WheelEvent` (`delta_y`, `delta_mode`: `0` = pixels at 120 px/notch, `1` = lines)
    /// through `mwheelup`/`mwheeldown` bindings with GoldSrc press+release impulse semantics.
    pub fn on_mouse_wheel(&mut self, delta_y: f32, delta_mode: u32, cvars: &mut CVarRegistry) {
        // DOM WheelEvent deltaY < 0 means scrolling up (`mwheelup`), > 0 means scrolling down.
        let up_delta = -delta_y;
        let notches = match delta_mode {
            1 => accumulate_wheel_notches(&mut self.wheel_line_accum, up_delta, true),
            _ => accumulate_wheel_notches(&mut self.wheel_pixel_accum, up_delta, false),
        };
        let count = notches.unsigned_abs().min(16);
        let key_name = if notches >= 0 { "mwheelup" } else { "mwheeldown" };
        for _ in 0..count {
            let press = cvars.handle_key_bind(key_name, true);
            self.apply_engine_actions(press);
            let release = cvars.handle_key_bind(key_name, false);
            self.apply_engine_actions(release);
        }
    }

    pub fn apply_engine_actions(&mut self, actions: Vec<EngineAction>) {
        for action in actions {
            match action {
                EngineAction::ActionInput { name, pressed } => {
                    if pressed {
                        self.impulse_actions.insert(name.clone());
                        self.actions.insert(name);
                    } else {
                        self.actions.remove(&name);
                    }
                }
                EngineAction::InvNext => self.pending_commands.push("invnext".to_string()),
                EngineAction::InvPrev => self.pending_commands.push("invprev".to_string()),
                EngineAction::LastInv => self.pending_commands.push("lastinv".to_string()),
                EngineAction::DropWeapon => self.pending_commands.push("drop".to_string()),
                EngineAction::SelectSlot(s) => self.pending_commands.push(format!("slot{s}")),
                EngineAction::Impulse(n) => self.pending_impulse = n,
                EngineAction::OpenBuyMenu => self.pending_commands.push("buy".to_string()),
                EngineAction::ChooseTeam => self.pending_commands.push("chooseteam".to_string()),
                EngineAction::ToggleNightVision => {
                    self.pending_commands.push("nightvision".to_string())
                }
                EngineAction::OpenRadio(g) => self.pending_commands.push(format!("radio{g}")),
                EngineAction::Forward(tokens) if !tokens.is_empty() => {
                    self.pending_commands.push(tokens.join(" "));
                }
                _ => {}
            }
        }
    }

    pub fn on_mouse_move(&self, camera: &mut Camera, dx: f32, dy: f32, sensitivity: f32) {
        camera.apply_mouse_delta(dx, dy, sensitivity, 0.022, 0.022);
    }

    pub fn on_gamepad(&mut self, lx: f32, ly: f32, rx: f32, ry: f32, mask: u32) {
        self.gamepad_axes = [
            apply_deadzone(lx),
            apply_deadzone(ly),
            apply_deadzone(rx),
            apply_deadzone(ry),
        ];
        self.gamepad_mask = mask;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_touch(
        &mut self,
        camera: &mut Camera,
        lx: f32,
        ly: f32,
        dx: f32,
        dy: f32,
        mask: u32,
        sensitivity: f32,
    ) {
        self.touch_axes = [lx.clamp(-1.0, 1.0), ly.clamp(-1.0, 1.0)];
        self.touch_mask = mask;
        if dx != 0.0 || dy != 0.0 {
            camera.apply_mouse_delta(dx, dy, sensitivity, 0.022, 0.022);
        }
    }

    /// Applies queued analog gamepad right-stick look and generates the frame's [`GoldSrcUserCmd`].
    pub fn build_usercmd(
        &mut self,
        camera: &mut Camera,
        dt_secs: f64,
        interp: f32,
        speeds: &MoveSpeeds,
        yaw_speed_deg: f32,
    ) -> GoldSrcUserCmd {
        let dt_f32 = (dt_secs as f32).clamp(0.0, 0.1);
        if self.gamepad_axes[2] != 0.0 || self.gamepad_axes[3] != 0.0 {
            camera.yaw_deg =
                (camera.yaw_deg - self.gamepad_axes[2] * yaw_speed_deg * dt_f32).rem_euclid(360.0);
            camera.pitch_deg =
                (camera.pitch_deg + self.gamepad_axes[3] * yaw_speed_deg * 0.75 * dt_f32)
                    .clamp(-89.0, 89.0);
        }

        let mask = self.gamepad_mask | self.touch_mask;
        let mut combined_actions = self.actions.clone();
        if mask & pad_bit::DUCK != 0 {
            combined_actions.insert("duck".to_string());
        }
        if mask & pad_bit::RELOAD != 0 {
            combined_actions.insert("reload".to_string());
        }
        if mask & pad_bit::USE != 0 {
            combined_actions.insert("use".to_string());
        }
        if mask & pad_bit::JUMP != 0 {
            combined_actions.insert("jump".to_string());
        }

        let held = Held {
            actions: Some(&combined_actions),
            impulse_actions: Some(&self.impulse_actions),
            mouse1: self.mouse1 || (mask & pad_bit::FIRE != 0),
            mouse2: self.mouse2 || (mask & pad_bit::ALTFIRE != 0),
        };
        let params = CmdParams {
            msec: self.clock.msec(dt_secs),
            lerp_msec: self.clock.lerp_msec(interp, dt_f32),
            impulse: std::mem::take(&mut self.pending_impulse),
        };
        let angles = Vec3::new(camera.pitch_deg, camera.yaw_deg, 0.0);
        let mut cmd = build_usercmd(&held, angles, params, speeds);
        self.impulse_actions.clear();

        // Apply analog stick movement when digital keys did not already saturate an axis.
        let analog_side = (self.gamepad_axes[0] + self.touch_axes[0]).clamp(-1.0, 1.0);
        let analog_fwd = (-self.gamepad_axes[1] - self.touch_axes[1]).clamp(-1.0, 1.0);
        if cmd.forwardmove == 0.0 && analog_fwd != 0.0 {
            let max_fwd = if analog_fwd >= 0.0 {
                speeds.forward
            } else {
                speeds.back
            };
            cmd.forwardmove = analog_fwd * max_fwd;
        }
        if cmd.sidemove == 0.0 && analog_side != 0.0 {
            cmd.sidemove = analog_side * speeds.side;
        }
        cmd
    }
}

fn apply_deadzone(v: f32) -> f32 {
    if !v.is_finite() || v.abs() < GAMEPAD_DEADZONE {
        return 0.0;
    }
    let sign = v.signum();
    let norm = ((v.abs() - GAMEPAD_DEADZONE) / (1.0 - GAMEPAD_DEADZONE)).clamp(0.0, 1.0);
    sign * norm
}

#[cfg(test)]
mod tests {
    use cs_client::input::button;

    use super::*;

    #[test]
    fn keyboard_and_mouse_produce_usercmd() {
        let mut input = WebInputState::new();
        let mut cam = Camera::default();
        input.on_key("KeyW", true);
        input.on_key("Space", true);
        input.on_mouse_button(0, true);
        input.on_mouse_move(&mut cam, 100.0, 0.0, 2.5);
        assert!(cam.yaw_deg > 300.0);

        let cmd = input.build_usercmd(&mut cam, 0.01, 0.1, &MoveSpeeds::default(), 180.0);
        assert_eq!(cmd.msec, 10);
        assert_eq!(cmd.forwardmove, 400.0);
        assert_ne!(cmd.buttons & button::FORWARD, 0);
        assert_ne!(cmd.buttons & button::JUMP, 0);
        assert_ne!(cmd.buttons & button::ATTACK, 0);
    }

    #[test]
    fn gamepad_deadzone_and_buttons_map_to_usercmd() {
        let mut input = WebInputState::new();
        let mut cam = Camera::default();
        input.on_gamepad(0.08, -0.8, 0.0, 0.0, pad_bit::FIRE | pad_bit::JUMP);
        let cmd = input.build_usercmd(&mut cam, 0.02, 0.1, &MoveSpeeds::default(), 180.0);
        assert_eq!(cmd.sidemove, 0.0, "axis inside deadzone stays 0");
        assert!(cmd.forwardmove > 250.0, "pushed forward stick drives forwardmove");
        assert_ne!(cmd.buttons & button::ATTACK, 0);
        assert_ne!(cmd.buttons & button::JUMP, 0);
    }

    #[test]
    fn touch_controls_drive_movement_and_buttons() {
        let mut input = WebInputState::new();
        let mut cam = Camera::default();
        input.on_touch(&mut cam, 0.75, -0.5, 10.0, -4.0, pad_bit::RELOAD, 2.0);
        let cmd = input.build_usercmd(&mut cam, 0.016, 0.1, &MoveSpeeds::default(), 180.0);
        assert_eq!(cmd.sidemove, 300.0);
        assert_eq!(cmd.forwardmove, 200.0);
        assert_ne!(cmd.buttons & button::RELOAD, 0);
    }

    #[test]
    fn wheel_bhop_impulse_latches_for_one_frame_and_defaults_cycle_inv() {
        let mut cvars = CVarRegistry::new_with_defaults();
        let mut input = WebInputState::new();
        let mut cam = Camera::default();

        // Default mwheelup -> invprev, mwheeldown -> invnext (DOM deltaY < 0 is wheel up).
        input.on_mouse_wheel(-120.0, 0, &mut cvars);
        input.on_mouse_wheel(120.0, 0, &mut cvars);
        assert_eq!(input.pending_commands, vec!["invprev", "invnext"]);
        input.pending_commands.clear();

        // Bind mwheelup -> +jump and fire one wheel-up notch (line mode deltaY = -1.0).
        cvars.bind("mwheelup", "+jump");
        input.on_mouse_wheel(-1.0, 1, &mut cvars);
        assert!(!input.actions.contains("jump"), "key-up already ran");

        let frame_n = input.build_usercmd(&mut cam, 0.01, 0.1, &MoveSpeeds::default(), 180.0);
        assert_ne!(frame_n.buttons & button::JUMP, 0, "frame N must latch JUMP");

        let frame_n1 = input.build_usercmd(&mut cam, 0.01, 0.1, &MoveSpeeds::default(), 180.0);
        assert_eq!(frame_n1.buttons & button::JUMP, 0, "frame N+1 must clear JUMP");

        // Second notch produces distinct 1 -> 0 -> 1 edge.
        input.on_mouse_wheel(-60.0, 0, &mut cvars);
        let mid = input.build_usercmd(&mut cam, 0.01, 0.1, &MoveSpeeds::default(), 180.0);
        assert_eq!(mid.buttons & button::JUMP, 0);
        input.on_mouse_wheel(-60.0, 0, &mut cvars);
        let frame_n2 = input.build_usercmd(&mut cam, 0.01, 0.1, &MoveSpeeds::default(), 180.0);
        assert_ne!(frame_n2.buttons & button::JUMP, 0);
    }

    #[test]
    fn mouse4_bound_to_voicerecord_toggles_active_action() {
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.bind("mouse4", "+voicerecord");
        let mut input = WebInputState::new();

        // DOM button 3 is Browser Back (mouse4).
        input.on_mouse_button_bound(3, true, &mut cvars);
        assert!(input.actions.contains("voicerecord"));
        input.on_mouse_button_bound(3, false, &mut cvars);
        assert!(!input.actions.contains("voicerecord"));
    }
}
