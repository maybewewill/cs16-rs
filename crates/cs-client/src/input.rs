//! Input to `clc_move`: the held actions and the view angles become one `usercmd`, and the
//! console commands that only the server understands become `clc_stringcmd`.

use std::collections::{BTreeMap, HashSet};

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

/// `cl_forwardspeed`, `cl_sidespeed` and `cl_backspeed` defaults.
pub const MOVE_SPEED: f32 = 400.0;
/// `cl_movespeedkey`: the factor while `+speed` (walk) is held.
pub const WALK_FACTOR: f32 = 0.52;

/// What the player holds down this frame.
#[derive(Debug, Clone, Default)]
pub struct Held<'a> {
    pub actions: Option<&'a HashSet<String>>,
    pub mouse1: bool,
    pub mouse2: bool,
}

impl Held<'_> {
    fn on(&self, name: &str) -> bool {
        self.actions.is_some_and(|a| a.contains(name))
    }
}

/// One usercmd for a frame of `msec` milliseconds. `impulse` is sent once (the spray).
pub fn build_usercmd(held: &Held<'_>, view_angles: Vec3, msec: u8, impulse: u8) -> GoldSrcUserCmd {
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

    let axis = |plus: &str, minus: &str| f32::from(held.on(plus)) - f32::from(held.on(minus));
    let walk = if held.on("speed") { WALK_FACTOR } else { 1.0 };
    GoldSrcUserCmd {
        lerp_msec: 0,
        msec,
        viewangles: view_angles,
        forwardmove: axis("forward", "back") * MOVE_SPEED * walk,
        sidemove: axis("moveright", "moveleft") * MOVE_SPEED * walk,
        upmove: 0.0,
        lightlevel: 128,
        buttons,
        impulse,
        weaponselect: 0,
        impact_index: 0,
        impact_position: Vec3::ZERO,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn held_actions_become_buttons_and_moves() {
        let a = actions(&["forward", "moveleft", "jump", "attack"]);
        let held = Held {
            actions: Some(&a),
            ..Held::default()
        };
        let cmd = build_usercmd(&held, Vec3::new(10.0, 90.0, 0.0), 10, 0);
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
    fn walking_scales_the_move_and_opposite_keys_cancel() {
        let a = actions(&["forward", "back", "moveright", "speed"]);
        let held = Held {
            actions: Some(&a),
            mouse1: true,
            mouse2: false,
        };
        let cmd = build_usercmd(&held, Vec3::ZERO, 10, 201);
        assert_eq!(cmd.forwardmove, 0.0);
        assert!((cmd.sidemove - 400.0 * WALK_FACTOR).abs() < 1e-3);
        assert!(cmd.buttons & button::ATTACK != 0 && cmd.buttons & button::RUN != 0);
        assert_eq!(cmd.impulse, 201);
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
}
