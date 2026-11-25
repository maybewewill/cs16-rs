//! Input to `clc_move`: the held actions and the view angles become one `usercmd`, and the
//! console commands that only the server understands become `clc_stringcmd`.

use std::collections::{BTreeMap, HashSet};

use cs_clientcore::ConsoleCommand;
use cs_clientcore::hud::WeaponInfo;
use cs_proto48::delta::GoldSrcUserCmd;
use glam::Vec3;

/// `IN_*` button bits (`in_buttons.h`).
pub mod button {
    pub const ATTACK: u16 = 1 << 0;
    pub const JUMP: u16 = 1 << 1;
    pub const DUCK: u16 = 1 << 2;
    pub const FORWARD: u16 = 1 << 3;
    pub const BACK: u16 = 1 << 4;
    pub const USE: u16 = 1 << 5;
    pub const MOVELEFT: u16 = 1 << 9;
    pub const MOVERIGHT: u16 = 1 << 10;
    pub const ATTACK2: u16 = 1 << 11;
    pub const RUN: u16 = 1 << 12;
    pub const RELOAD: u16 = 1 << 13;
    pub const SCORE: u16 = 1 << 15;
}

/// What the player holds down this frame.
#[derive(Debug, Clone, Default)]
pub struct Held<'a> {
    pub actions: Option<&'a HashSet<String>>,
    /// Same-frame `+action` impulse latch (`kbutton_t` down-impulse bit): contributes its
    /// `button::*` bit to the next `build_usercmd` even if `-action` already arrived in the
    /// same frame (e.g. scroll-wheel `+jump` / `-jump`).
    pub impulse_actions: Option<&'a HashSet<String>>,
    pub mouse1: bool,
    pub mouse2: bool,
}

impl Held<'_> {
    fn on(&self, name: &str) -> bool {
        self.actions.is_some_and(|a| a.contains(name))
            || self.impulse_actions.is_some_and(|a| a.contains(name))
    }
}

/// Windows `WHEEL_DELTA` equivalent in `MouseScrollDelta::PixelDelta` pixels per wheel notch.
pub const WHEEL_PIXELS_PER_NOTCH: f32 = 120.0;

/// Accumulates a wheel delta (`LineDelta` when `is_line_delta = true`, or `PixelDelta` at
/// `120.0` px/notch when `false`) and returns the signed notch count
/// (`> 0` = `"mwheelup"`, `< 0` = `"mwheeldown"`).
pub fn accumulate_wheel_notches(accum: &mut f32, delta_y: f32, is_line_delta: bool) -> i32 {
    if !delta_y.is_finite() || delta_y == 0.0 {
        return 0;
    }
    let step = if is_line_delta {
        1.0
    } else {
        WHEEL_PIXELS_PER_NOTCH
    };
    *accum += delta_y;
    let mut notches = 0i32;
    while *accum >= step {
        *accum -= step;
        notches += 1;
    }
    while *accum <= -step {
        *accum += step;
        notches -= 1;
    }
    notches
}

/// Quake/GoldSrc `kbutton_t` held + same-frame down-impulse state (`state & 1` and `state & 2`)
/// and wheel accumulators (`IN_MWheelEvent` / `Key_Event` in Xash3D FWGS
/// `engine/client/input/input.c` & `in_keys.c`, read 2026-11-25).
#[derive(Debug, Clone, Default)]
pub struct InputActionState {
    pub active_actions: HashSet<String>,
    pub impulse_actions: HashSet<String>,
    pub wheel_line_accum: f32,
    pub wheel_pixel_accum: f32,
}

impl InputActionState {
    /// Records a `+action` press (setting both held and down-impulse) or `-action` release
    /// (clearing held while leaving any same-frame down-impulse latched until consumed).
    pub fn apply_action_input(&mut self, name: impl Into<String>, pressed: bool) {
        let name = name.into();
        if pressed {
            self.impulse_actions.insert(name.clone());
            self.active_actions.insert(name);
        } else {
            self.active_actions.remove(&name);
        }
    }

    pub fn held(&self, capture: bool) -> Held<'_> {
        Held {
            actions: capture.then_some(&self.active_actions),
            impulse_actions: capture.then_some(&self.impulse_actions),
            mouse1: false,
            mouse2: false,
        }
    }

    pub fn consume_impulses(&mut self) {
        self.impulse_actions.clear();
    }

    pub fn clear(&mut self) {
        self.active_actions.clear();
        self.impulse_actions.clear();
        self.wheel_line_accum = 0.0;
        self.wheel_pixel_accum = 0.0;
    }

    /// Accumulates a wheel delta (`LineDelta` when `is_line_delta`, else `PixelDelta` at
    /// `120.0` px/notch) and returns the signed notch count (`> 0` up, `< 0` down).
    pub fn accumulate_wheel(&mut self, delta_y: f32, is_line_delta: bool) -> i32 {
        let accum = if is_line_delta {
            &mut self.wheel_line_accum
        } else {
            &mut self.wheel_pixel_accum
        };
        accumulate_wheel_notches(accum, delta_y, is_line_delta)
    }
}

/// The move speeds of the client cvars (`cl_forwardspeed`, `cl_backspeed`, `cl_sidespeed`,
/// `cl_upspeed`) and the factor `+speed` (walk) applies (`cl_movespeedkey`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveSpeeds {
    pub forward: f32,
    pub back: f32,
    pub side: f32,
    pub up: f32,
    pub walk_key: f32,
}

impl Default for MoveSpeeds {
    fn default() -> Self {
        Self {
            forward: 400.0,
            back: 400.0,
            side: 400.0,
            up: 320.0,
            walk_key: 0.52,
        }
    }
}

/// What a frame's usercmd carries besides the held keys.
#[derive(Debug, Clone, Copy, Default)]
pub struct CmdParams {
    pub msec: u8,
    pub lerp_msec: i16,
    /// Sent once (`impulse 201`, the spray; `impulse 100`, the flashlight).
    pub impulse: u8,
}

/// The frame time of the usercmds in whole milliseconds, without drift: the part of a frame
/// that is not a whole millisecond is carried into the next one. Also the interpolation amount
/// the commands announce (`lerp_msec`).
#[derive(Debug, Clone, Default)]
pub struct CmdClock {
    carry: f64,
    interp_amount: f32,
}

impl CmdClock {
    /// The milliseconds of a frame of `dt` seconds, 0 to 255 (`hw.dll:FUN_101a3a40`,
    /// `0x101a3f65`: clamps `cmd.msec` at `255` instead of `100`, allows 0 ms on
    /// sub-millisecond frames while banking the remainder, and drops excess after hitches).
    pub fn msec(&mut self, dt: f64) -> u8 {
        if !dt.is_finite() || dt <= 0.0 {
            return 0;
        }
        let total = self.carry + dt * 1000.0;
        let ms = total.floor().clamp(0.0, 255.0);
        self.carry = if total > 255.0 {
            0.0
        } else {
            (total - ms).clamp(0.0, 1.0)
        };
        ms as u8
    }

    /// `cmd->lerp_msec` of `CL_ComputeClientInterpolationAmount` (Xash3D FWGS
    /// `engine/client/cl_main.c`, read 2026-11-24): `interp` is `ex_interp` already bounded by
    /// the update rate and `MAX_EX_INTERP` (`InterpSettings::interp`); the amount drifts to it
    /// by at most `frametime * 0.05` a frame (`CL_DriftInterpolationAmount`), rounded to
    /// milliseconds and kept in 0..=100.
    pub fn lerp_msec(&mut self, interp: f32, frametime: f32) -> i16 {
        let safe_interp = if interp.is_finite() { interp } else { 0.1 };
        let goal = (safe_interp * 1000.0).round() / 1000.0;
        if goal != self.interp_amount {
            let safe_dt = if frametime.is_finite() {
                frametime.max(0.0)
            } else {
                0.0
            };
            let max_move = safe_dt * 0.05;
            let diff = (goal - self.interp_amount).clamp(-max_move, max_move);
            self.interp_amount += diff;
        }
        ((self.interp_amount * 1000.0).round() as i32).clamp(0, 100) as i16
    }
}

fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// One usercmd.
pub fn build_usercmd(
    held: &Held<'_>,
    view_angles: Vec3,
    p: CmdParams,
    speeds: &MoveSpeeds,
) -> GoldSrcUserCmd {
    let mut buttons = 0u16;
    let mut set = |on: bool, bit: u16| {
        if on {
            buttons |= bit;
        }
    };
    set(held.mouse1 || held.on("attack"), button::ATTACK);
    set(held.mouse2 || held.on("attack2"), button::ATTACK2);
    set(held.on("jump"), button::JUMP);
    set(held.on("duck"), button::DUCK);
    set(held.on("forward"), button::FORWARD);
    set(held.on("back"), button::BACK);
    set(held.on("moveleft"), button::MOVELEFT);
    set(held.on("moveright"), button::MOVERIGHT);
    set(held.on("use"), button::USE);
    set(held.on("reload"), button::RELOAD);
    set(held.on("speed"), button::RUN);
    set(held.on("showscores"), button::SCORE);

    let forward_speed = finite_or_zero(speeds.forward);
    let back_speed = finite_or_zero(speeds.back);
    let side_speed = finite_or_zero(speeds.side);
    let up_speed = finite_or_zero(speeds.up);
    let walk_key = finite_or_zero(speeds.walk_key);
    let walk = if held.on("speed") { walk_key } else { 1.0 };
    // `CL_KeyMove`: each key adds its speed (the forward and back keys cancel).
    let key = |on: &str| f32::from(held.on(on));
    let safe_angles = Vec3::new(
        finite_or_zero(view_angles.x),
        finite_or_zero(view_angles.y),
        finite_or_zero(view_angles.z),
    );
    GoldSrcUserCmd {
        lerp_msec: p.lerp_msec,
        msec: p.msec,
        viewangles: safe_angles,
        forwardmove: (key("forward") * forward_speed - key("back") * back_speed) * walk,
        sidemove: (key("moveright") - key("moveleft")) * side_speed * walk,
        // +moveup / +movedown: swimming and ladders (jump and duck are buttons).
        upmove: (key("moveup") - key("movedown")) * up_speed,
        lightlevel: 128,
        buttons,
        impulse: p.impulse,
        weaponselect: 0,
        impact_index: 0,
        impact_position: Vec3::ZERO,
    }
}

/// Where a console line the shell does not know goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleRoute {
    /// The connection's own console runs it (`cmd <text>` sends the text, `setinfo` changes
    /// the userinfo and tells the server).
    Local(String),
    /// A `clc_stringcmd` for the game, re-quoted as `Cmd_ForwardToServer` does.
    Server(String),
    /// Stays on this machine (passwords, config).
    Denied,
}

/// Routes the tokens of an unknown command. The deny list is checked again here, so no route
/// to the server bypasses it.
pub fn route_unknown(tokens: &[String]) -> ConsoleRoute {
    let Some(name) = tokens.first() else {
        return ConsoleRoute::Denied;
    };
    let lower = name.to_ascii_lowercase();
    if cs_cvar::is_never_forwarded(&lower) {
        return ConsoleRoute::Denied;
    }
    let cmd = ConsoleCommand {
        name: name.clone(),
        args: tokens[1..].to_vec(),
    };
    match lower.as_str() {
        "cmd" => ConsoleRoute::Local(format!("cmd {}", cmd.forward_text())),
        "setinfo" | "userinfo" => ConsoleRoute::Local(cmd.forward_text()),
        _ => ConsoleRoute::Server(cmd.forward_text()),
    }
}

/// The weapon `slot` (1 to 5) selects: the next one after the weapon in hand within the slot,
/// or the first of the slot (`CHud::SelectSlot` of the original: the slot cycles by position).
/// `owned` are the weapons the player has (`weapon_id`s with a `WeaponList` entry and ammo
/// bookkeeping); only those count.
pub fn weapon_for_slot<'a>(
    table: &'a BTreeMap<u8, WeaponInfo>,
    owned: &HashSet<u8>,
    current: Option<u8>,
    slot: u8,
) -> Option<&'a str> {
    let mut in_slot: Vec<&WeaponInfo> = table
        .values()
        .filter(|w| w.slot + 1 == slot && owned.contains(&w.weapon_id))
        .collect();
    in_slot.sort_by_key(|w| w.position);
    let current_pos = current
        .and_then(|id| in_slot.iter().find(|w| w.weapon_id == id))
        .map(|w| w.position);
    let pick = match current_pos {
        Some(pos) => in_slot
            .iter()
            .find(|w| w.position > pos)
            .or_else(|| in_slot.first()),
        None => in_slot.first(),
    };
    pick.map(|w| w.name.as_str())
}

/// Returns all owned weapons sorted in GoldSrc `WeaponInfo` bucket/position order
/// `(slot, position, weapon_id)`.
pub fn sorted_owned_weapons<'a>(
    table: &'a BTreeMap<u8, WeaponInfo>,
    owned: &HashSet<u8>,
) -> Vec<&'a WeaponInfo> {
    let mut list: Vec<&WeaponInfo> = table
        .values()
        .filter(|w| owned.contains(&w.weapon_id))
        .collect();
    list.sort_by_key(|w| (w.slot, w.position, w.weapon_id));
    list
}

/// Selects the next (`forward = true`, `invnext`) or previous (`forward = false`, `invprev`)
/// owned weapon relative to `current` in `(slot, position, weapon_id)` order, wrapping around.
pub fn cycle_inventory_weapon<'a>(
    table: &'a BTreeMap<u8, WeaponInfo>,
    owned: &HashSet<u8>,
    current: Option<u8>,
    forward: bool,
) -> Option<&'a WeaponInfo> {
    let list = sorted_owned_weapons(table, owned);
    if list.is_empty() {
        return None;
    }
    let idx = current.and_then(|id| list.iter().position(|w| w.weapon_id == id));
    let pick = match idx {
        Some(i) => {
            if forward {
                (i + 1) % list.len()
            } else {
                (i + list.len() - 1) % list.len()
            }
        }
        None => {
            if forward {
                0
            } else {
                list.len() - 1
            }
        }
    };
    Some(list[pick])
}

/// Advances `buckets` for `invnext` (`forward = true`) or `invprev` (`forward = false`)
/// across owned weapons in `WeaponInfo` `(slot, position)` order, respecting `hud_fastswitch`.
///
/// With `fastswitch = true`, returns `Some(weapon_name)` immediately and closes the bucket
/// selector. With `fastswitch = false`, opens/advances the bucket highlight to the target
/// weapon's slot and returns `None` until `+attack` (`mouse1`) confirms it.
pub fn step_inventory_cycle(
    buckets: &mut crate::ui::hud_extras::WeaponBucketState,
    hud: &cs_clientcore::hud::HudState,
    owned: &HashSet<u8>,
    forward: bool,
    fastswitch: bool,
) -> Option<String> {
    let reference = if !fastswitch && buckets.active_slot.is_some() {
        buckets
            .highlighted_weapon_id
            .or_else(|| hud.current_weapon.map(|c| c.weapon_id))
    } else {
        hud.current_weapon.map(|c| c.weapon_id)
    };
    let next_w = cycle_inventory_weapon(&hud.weapons, owned, reference, forward)?;
    if fastswitch {
        buckets.active_slot = None;
        buckets.highlighted_weapon_id = Some(next_w.weapon_id);
        buckets.timer_sec = 0.0;
        Some(next_w.name.clone())
    } else {
        buckets.active_slot = Some(next_w.slot + 1);
        buckets.highlighted_weapon_id = Some(next_w.weapon_id);
        buckets.timer_sec = 2.5;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn params(msec: u8) -> CmdParams {
        CmdParams {
            msec,
            ..CmdParams::default()
        }
    }

    #[test]
    fn held_actions_become_buttons_and_moves() {
        let a = actions(&["forward", "moveleft", "jump", "attack"]);
        let held = Held {
            actions: Some(&a),
            ..Held::default()
        };
        let cmd = build_usercmd(&held, Vec3::new(10.0, 90.0, 0.0), params(10), &MoveSpeeds::default());
        assert_eq!(
            cmd.buttons,
            button::FORWARD | button::MOVELEFT | button::JUMP | button::ATTACK
        );
        assert_eq!(cmd.forwardmove, 400.0);
        assert_eq!(cmd.sidemove, -400.0);
        assert_eq!(cmd.viewangles, Vec3::new(10.0, 90.0, 0.0));
        assert_eq!(cmd.msec, 10);
    }

    #[test]
    fn the_speeds_come_from_the_cvars_and_walking_scales_them() {
        let a = actions(&["forward", "moveright", "speed"]);
        let held = Held {
            actions: Some(&a),
            mouse1: true,
            mouse2: false,
            ..Held::default()
        };
        let speeds = MoveSpeeds {
            forward: 250.0,
            side: 200.0,
            ..MoveSpeeds::default()
        };
        let cmd = build_usercmd(
            &held,
            Vec3::ZERO,
            CmdParams {
                msec: 10,
                lerp_msec: 100,
                impulse: 100,
            },
            &speeds,
        );
        assert!((cmd.forwardmove - 250.0 * 0.52).abs() < 1e-3);
        assert!((cmd.sidemove - 200.0 * 0.52).abs() < 1e-3);
        assert!(cmd.buttons & button::ATTACK != 0 && cmd.buttons & button::RUN != 0);
        assert_eq!((cmd.impulse, cmd.lerp_msec), (100, 100));
    }

    #[test]
    fn back_uses_its_own_speed_and_swimming_has_up_and_down() {
        let a = actions(&["back", "moveup"]);
        let held = Held {
            actions: Some(&a),
            ..Held::default()
        };
        let speeds = MoveSpeeds {
            back: 300.0,
            ..MoveSpeeds::default()
        };
        let cmd = build_usercmd(&held, Vec3::ZERO, params(10), &speeds);
        assert_eq!(cmd.forwardmove, -300.0);
        assert_eq!(cmd.upmove, 320.0);
        let b = actions(&["forward", "back"]);
        let both = Held {
            actions: Some(&b),
            ..Held::default()
        };
        assert_eq!(build_usercmd(&both, Vec3::ZERO, params(10), &speeds).forwardmove, 100.0);
    }

    fn toks(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unknown_commands_are_requoted_for_the_server() {
        assert_eq!(
            route_unknown(&toks(&["menuselect", "2"])),
            ConsoleRoute::Server("menuselect 2".into())
        );
        assert_eq!(
            route_unknown(&toks(&["say_team", "hello there"])),
            ConsoleRoute::Server("say_team \"hello there\"".into())
        );
    }

    #[test]
    fn cmd_and_setinfo_run_in_the_connection_and_secrets_stay_home() {
        assert_eq!(
            route_unknown(&toks(&["cmd", "jointeam 2"])),
            ConsoleRoute::Local("cmd \"jointeam 2\"".into())
        );
        assert_eq!(
            route_unknown(&toks(&["setinfo", "model", "gign"])),
            ConsoleRoute::Local("setinfo model gign".into())
        );
        for name in ["rcon_password", "RCON", "password", "sv_password", "exec", "bind"] {
            assert_eq!(route_unknown(&toks(&[name, "x"])), ConsoleRoute::Denied, "{name}");
        }
        assert_eq!(route_unknown(&[]), ConsoleRoute::Denied);
    }

    #[test]
    fn frame_times_carry_their_remainder() {
        let mut c = CmdClock::default();
        // Sub-millisecond frames return 0 ms and bank remainder until >= 1 ms.
        assert_eq!(c.msec(0.0006), 0);
        assert_eq!(c.msec(0.0006), 1);
        let mut c2 = CmdClock::default();
        // 6.9 ms frames: the sent milliseconds add up to the real time, within one ms.
        let total: u32 = (0..1000).map(|_| u32::from(c2.msec(0.0069))).sum();
        assert!((6899..=6900).contains(&total), "{total}");
        // Frames up to 255 ms preserve sub-ms carry; hitches > 255 ms clamp to 255 and drop excess.
        let mut c3 = CmdClock::default();
        assert_eq!(c3.msec(0.0008), 0);
        assert_eq!(c3.msec(0.25), 250);
        assert_eq!(c3.msec(0.0005), 1);
        assert_eq!(c3.msec(0.5), 255);
        assert_eq!(c3.msec(0.0005), 0);
        assert_eq!(c3.msec(-0.1), 0);
        assert_eq!(c3.msec(f64::NAN), 0);
    }

    #[test]
    fn non_finite_inputs_do_not_panic_or_leak_to_usercmd() {
        let mut c = CmdClock::default();
        assert_eq!(c.lerp_msec(0.1, -0.05), 0);
        assert_eq!(c.lerp_msec(f32::NAN, f32::NAN), 0);

        let mut actions = HashSet::new();
        actions.insert("forward".into());
        let held = Held {
            actions: Some(&actions),
            ..Default::default()
        };
        let cmd = build_usercmd(
            &held,
            Vec3::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY),
            CmdParams::default(),
            &MoveSpeeds {
                forward: f32::NAN,
                back: f32::INFINITY,
                side: f32::NAN,
                up: f32::NEG_INFINITY,
                walk_key: f32::NAN,
            },
        );
        assert!(cmd.viewangles.is_finite());
        assert!(cmd.forwardmove.is_finite() && cmd.sidemove.is_finite() && cmd.upmove.is_finite());
    }

    #[test]
    fn the_interpolation_amount_drifts_to_ex_interp() {
        let mut c = CmdClock::default();
        assert!(c.lerp_msec(0.1, 0.01) <= 1);
        let mut last = 0;
        for _ in 0..100_000 {
            last = c.lerp_msec(0.1, 0.01);
        }
        assert_eq!(last, 100);
        // Going down is bounded as well, and the value stays in 0..=100.
        assert!(c.lerp_msec(0.0, 0.01) >= 99);
        assert_eq!(c.lerp_msec(5.0, 0.01), 100);
    }

    fn info(name: &str, slot: u8, position: u8, id: u8) -> WeaponInfo {
        WeaponInfo {
            name: name.to_string(),
            ammo1_type: 0,
            ammo1_max: 0,
            ammo2_type: 0,
            ammo2_max: 0,
            slot,
            position,
            weapon_id: id,
            flags: 0,
        }
    }

    #[test]
    fn a_slot_cycles_the_weapons_in_it() {
        let mut table = BTreeMap::new();
        table.insert(1, info("weapon_p228", 1, 3, 1));
        table.insert(16, info("weapon_usp", 1, 4, 16));
        table.insert(26, info("weapon_glock18", 1, 5, 26));
        table.insert(29, info("weapon_knife", 2, 1, 29));
        let owned: HashSet<u8> = [16, 26, 29].into_iter().collect();
        // Slot 2 holds the pistols; the P228 is not owned.
        assert_eq!(weapon_for_slot(&table, &owned, None, 2), Some("weapon_usp"));
        assert_eq!(
            weapon_for_slot(&table, &owned, Some(16), 2),
            Some("weapon_glock18")
        );
        assert_eq!(weapon_for_slot(&table, &owned, Some(26), 2), Some("weapon_usp"));
        assert_eq!(weapon_for_slot(&table, &owned, None, 3), Some("weapon_knife"));
        assert_eq!(weapon_for_slot(&table, &owned, None, 4), None);
    }

    #[test]
    fn mwheelup_plus_jump_latches_frame_n_and_clears_frame_n_plus_1() {
        let mut reg = cs_cvar::CVarRegistry::new_with_defaults();
        reg.bind("mwheelup", "+jump");

        let mut state = InputActionState::default();
        let dispatch_notch = |reg: &mut cs_cvar::CVarRegistry, state: &mut InputActionState| {
            for pressed in [true, false] {
                for act in reg.handle_key_bind("mwheelup", pressed) {
                    if let cs_cvar::EngineAction::ActionInput { name, pressed } = act {
                        state.apply_action_input(name, pressed);
                    }
                }
            }
        };

        // Single wheel-up notch within frame N (press + release before build_usercmd).
        assert_eq!(state.accumulate_wheel(1.0, true), 1);
        dispatch_notch(&mut reg, &mut state);
        assert!(
            !state.active_actions.contains("jump"),
            "release already cleared held state"
        );

        let cmd_n = build_usercmd(&state.held(true), Vec3::ZERO, params(10), &MoveSpeeds::default());
        state.consume_impulses();
        assert_ne!(cmd_n.buttons & button::JUMP, 0, "frame N must emit JUMP");

        // Frame N+1 with no wheel input must clear JUMP.
        let cmd_n1 = build_usercmd(&state.held(true), Vec3::ZERO, params(10), &MoveSpeeds::default());
        state.consume_impulses();
        assert_eq!(cmd_n1.buttons & button::JUMP, 0, "frame N+1 must clear JUMP");

        // Accumulated 120px PixelDelta produces a fresh JUMP edge on frame N+2 (1 -> 0 -> 1).
        assert_eq!(state.accumulate_wheel(60.0, false), 0);
        assert_eq!(state.accumulate_wheel(60.0, false), 1);
        dispatch_notch(&mut reg, &mut state);
        let cmd_n2 = build_usercmd(&state.held(true), Vec3::ZERO, params(10), &MoveSpeeds::default());
        state.consume_impulses();
        assert_ne!(cmd_n2.buttons & button::JUMP, 0, "frame N+2 must re-trigger JUMP");
    }

    #[test]
    fn mouse4_bound_to_voicerecord_toggles_action() {
        let mut reg = cs_cvar::CVarRegistry::new_with_defaults();
        reg.bind("mouse4", "+voicerecord");
        let mut state = InputActionState::default();

        for act in reg.handle_key_bind("mouse4", true) {
            if let cs_cvar::EngineAction::ActionInput { name, pressed } = act {
                state.apply_action_input(name, pressed);
            }
        }
        assert!(state.active_actions.contains("voicerecord"));

        for act in reg.handle_key_bind("mouse4", false) {
            if let cs_cvar::EngineAction::ActionInput { name, pressed } = act {
                state.apply_action_input(name, pressed);
            }
        }
        assert!(!state.active_actions.contains("voicerecord"));
    }

    #[test]
    fn default_wheel_binds_and_invnext_invprev_cycle_respecting_fastswitch() {
        let mut reg = cs_cvar::CVarRegistry::new_with_defaults();
        assert_eq!(
            reg.handle_key_bind("mwheelup", true),
            vec![cs_cvar::EngineAction::InvPrev]
        );
        assert_eq!(
            reg.handle_key_bind("mwheeldown", true),
            vec![cs_cvar::EngineAction::InvNext]
        );

        let mut hud = cs_clientcore::hud::HudState::default();
        hud.weapons.insert(28, info("weapon_ak47", 0, 1, 28));
        hud.weapons.insert(16, info("weapon_usp", 1, 4, 16));
        hud.weapons.insert(29, info("weapon_knife", 2, 1, 29));
        hud.weapons.insert(4, info("weapon_hegrenade", 3, 1, 4));
        hud.current_weapon = Some(cs_clientcore::hud::CurrentWeapon {
            is_active: true,
            weapon_id: 16,
            clip: 12,
        });
        let owned: HashSet<u8> = [28, 16, 29, 4].into_iter().collect();

        // hud_fastswitch 1: immediate weapon select across slots and wrap-around.
        let mut buckets = crate::ui::hud_extras::WeaponBucketState::default();
        assert_eq!(
            step_inventory_cycle(&mut buckets, &hud, &owned, true, true).as_deref(),
            Some("weapon_knife")
        );
        assert_eq!(buckets.active_slot, None);
        assert_eq!(
            step_inventory_cycle(&mut buckets, &hud, &owned, false, true).as_deref(),
            Some("weapon_ak47")
        );
        hud.current_weapon = Some(cs_clientcore::hud::CurrentWeapon {
            is_active: true,
            weapon_id: 4,
            clip: -1,
        });
        assert_eq!(
            step_inventory_cycle(&mut buckets, &hud, &owned, true, true).as_deref(),
            Some("weapon_ak47")
        );

        // hud_fastswitch 0: opens/steps bucket highlight without immediate switch until confirm.
        hud.current_weapon = Some(cs_clientcore::hud::CurrentWeapon {
            is_active: true,
            weapon_id: 16,
            clip: 12,
        });
        assert_eq!(step_inventory_cycle(&mut buckets, &hud, &owned, true, false), None);
        assert_eq!(buckets.active_slot, Some(3));
        assert_eq!(buckets.highlighted_weapon_id, Some(29));
        assert_eq!(step_inventory_cycle(&mut buckets, &hud, &owned, true, false), None);
        assert_eq!(buckets.active_slot, Some(4));
        assert_eq!(buckets.highlighted_weapon_id, Some(4));
        assert_eq!(
            buckets.confirm_selection(&hud).as_deref(),
            Some("weapon_hegrenade")
        );
        assert_eq!(buckets.active_slot, None);
    }
}
