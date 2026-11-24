//! Client-side weapon prediction (`cl_lw 1`), matching ReGameDLL_CS / HL client weapon
//! state progression (`ItemPostFrame`, shared random spread, recoil punch, scope zoom,
//! silencer/burst toggles, reload, dryfire, and tactical shield draw/undraw).
//!
//! State transitions (`clip`, timers, `punchangle`, `fov`, `weapon_state`, `weaponanim`) run on
//! every prediction step (including replays so server ack reconciliation stays exact). Visual
//! and audio emissions (`PredictedWeaponEvent` and dryfire `PredictedSound`) are strictly gated
//! on `runfuncs && cl_lw` so prediction replays never double-fire effects.

use std::collections::BTreeMap;

use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::event::EventArgs;
use glam::Vec3;

use crate::predict::{LocalState, PredictedSound};

/// Button bitmasks (`in_buttons.h`).
pub const IN_ATTACK: u16 = 1 << 0;
pub const IN_ATTACK2: u16 = 1 << 11;
pub const IN_RELOAD: u16 = 1 << 13;

/// `PLAYER_HOLDING_SHIELD` bit in `clientdata.iuser3`.
pub const PLAYER_HOLDING_SHIELD: i32 = 1 << 2;

/// `WPNSTATE_*` bits in `weapon_data_t.m_iWeaponState`.
pub const WPNSTATE_USP_SILENCED: i32 = 1 << 0;
pub const WPNSTATE_GLOCK18_BURST_MODE: i32 = 1 << 1;
pub const WPNSTATE_M4A1_SILENCED: i32 = 1 << 2;
pub const WPNSTATE_ELITE_LEFT: i32 = 1 << 3;
pub const WPNSTATE_FAMAS_BURST_MODE: i32 = 1 << 4;
pub const WPNSTATE_SHIELD_DRAWN: i32 = 1 << 5;

/// `DEFAULT_FOV` (`weapons.h`).
pub const DEFAULT_FOV: f32 = 90.0;
/// `SHIELD_MAX_SPEED` while shield is drawn.
pub const SHIELD_DRAWN_MAX_SPEED: f32 = 180.0;

/// Numeric `WeaponIdType` values matching ReGameDLL_CS `weapontype.h`.
pub mod weapon_id {
    pub const P228: i32 = 1;
    pub const SCOUT: i32 = 3;
    pub const HEGRENADE: i32 = 4;
    pub const XM1014: i32 = 5;
    pub const C4: i32 = 6;
    pub const MAC10: i32 = 7;
    pub const AUG: i32 = 8;
    pub const SMOKEGRENADE: i32 = 9;
    pub const ELITE: i32 = 10;
    pub const FIVESEVEN: i32 = 11;
    pub const UMP45: i32 = 12;
    pub const SG550: i32 = 13;
    pub const GALIL: i32 = 14;
    pub const FAMAS: i32 = 15;
    pub const USP: i32 = 16;
    pub const GLOCK18: i32 = 17;
    pub const AWP: i32 = 18;
    pub const MP5N: i32 = 19;
    pub const M249: i32 = 20;
    pub const M3: i32 = 21;
    pub const M4A1: i32 = 22;
    pub const TMP: i32 = 23;
    pub const G3SG1: i32 = 24;
    pub const FLASHBANG: i32 = 25;
    pub const DEAGLE: i32 = 26;
    pub const SG552: i32 = 27;
    pub const AK47: i32 = 28;
    pub const KNIFE: i32 = 29;
    pub const P90: i32 = 30;
}

/// A local weapon fire/swing event predicted by `cl_lw 1`, ready to queue via `EventQueue::push_local`.
#[derive(Debug, Clone, PartialEq)]
pub struct PredictedWeaponEvent {
    pub event_name: &'static str,
    pub args: EventArgs,
}

/// Cross-step accuracy/recoil bookkeeping per weapon predicted locally between server updates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PredictWeaponExtras {
    pub accuracy: f32,
    pub shots_fired: i32,
    pub direction: i32,
    pub last_fire: f32,
    pub play_empty_sound: bool,
    pub resume_zoom: bool,
    pub last_zoom: f32,
}

impl Default for PredictWeaponExtras {
    fn default() -> Self {
        Self {
            accuracy: 0.2,
            shots_fired: 0,
            direction: 0,
            last_fire: 0.0,
            play_empty_sound: true,
            resume_zoom: false,
            last_zoom: DEFAULT_FOV,
        }
    }
}

/// `seed_table[256]` from ReGameDLL_CS `util.cpp` (`UTIL_SharedRandomLong`/`Float`), verbatim.
const SEED_TABLE: [u32; 256] = [
    28985, 27138, 26457, 9451, 17764, 10909, 28790, 8716, 6361, 4853, 17798, 21977, 19643, 20662, 10834, 20103,
    27067, 28634, 18623, 25849, 8576, 26234, 23887, 18228, 32587, 4836, 3306, 1811, 3035, 24559, 18399, 315,
    26766, 907, 24102, 12370, 9674, 2972, 10472, 16492, 22683, 11529, 27968, 30406, 13213, 2319, 23620, 16823,
    10013, 23772, 21567, 1251, 19579, 20313, 18241, 30130, 8402, 20807, 27354, 7169, 21211, 17293, 5410, 19223,
    10255, 22480, 27388, 9946, 15628, 24389, 17308, 2370, 9530, 31683, 25927, 23567, 11694, 26397, 32602, 15031,
    18255, 17582, 1422, 28835, 23607, 12597, 20602, 10138, 5212, 1252, 10074, 23166, 19823, 31667, 5902, 24630,
    18948, 14330, 14950, 8939, 23540, 21311, 22428, 22391, 3583, 29004, 30498, 18714, 4278, 2437, 22430, 3439,
    28313, 23161, 25396, 13471, 19324, 15287, 2563, 18901, 13103, 16867, 9714, 14322, 15197, 26889, 19372, 26241,
    31925, 14640, 11497, 8941, 10056, 6451, 28656, 10737, 13874, 17356, 8281, 25937, 1661, 4850, 7448, 12744,
    21826, 5477, 10167, 16705, 26897, 8839, 30947, 27978, 27283, 24685, 32298, 3525, 12398, 28726, 9475, 10208,
    617, 13467, 22287, 2376, 6097, 26312, 2974, 9114, 21787, 28010, 4725, 15387, 3274, 10762, 31695, 17320,
    18324, 12441, 16801, 27376, 22464, 7500, 5666, 18144, 15314, 31914, 31627, 6495, 5226, 31203, 2331, 4668,
    12650, 18275, 351, 7268, 31319, 30119, 7600, 2905, 13826, 11343, 13053, 15583, 30055, 31093, 5067, 761,
    9685, 11070, 21369, 27155, 3663, 26542, 20169, 12161, 15411, 30401, 7580, 31784, 8985, 29367, 20989, 14203,
    29694, 21167, 10337, 1706, 28578, 887, 3373, 19477, 14382, 675, 7033, 15111, 26138, 12252, 30996, 21409,
    25678, 18555, 13256, 23316, 22407, 16727, 991, 9236, 5373, 29402, 6117, 15241, 27715, 19291, 19888, 19847,
];

#[derive(Debug, Clone, Copy)]
struct URandom {
    seed: u32,
}

impl URandom {
    fn new(seed: u32) -> Self {
        Self {
            seed: SEED_TABLE[(seed & 0xFF) as usize],
        }
    }

    fn random(&mut self) -> u32 {
        self.seed = self.seed.wrapping_mul(69069);
        self.seed = self
            .seed
            .wrapping_add(SEED_TABLE[(self.seed & 0xFF) as usize].wrapping_add(1));
        self.seed & 0x0FFF_FFFF
    }
}

/// `UTIL_SharedRandomLong` matching ReGameDLL_CS `util.cpp` bit-for-bit.
pub fn shared_random_long(seed: u32, low: i32, high: i32) -> i32 {
    let range = (high.wrapping_sub(low).wrapping_add(1)) as u32;
    let mut r = URandom::new((high as u32).wrapping_add(low as u32).wrapping_add(seed));
    if range != 1 {
        if range == 0 {
            return low;
        }
        let rnum = r.random();
        let offset = rnum % range;
        return low.wrapping_add(offset as i32);
    }
    low
}

/// `UTIL_SharedRandomFloat` matching ReGameDLL_CS `util.cpp` bit-for-bit.
pub fn shared_random_float(seed: u32, low: f32, high: f32) -> f32 {
    let range = ((high - low) as i64) as u32;
    let mut r = URandom::new(seed.wrapping_add(low.to_bits()).wrapping_add(high.to_bits()));
    r.random();
    r.random();
    if range != 0 {
        let tensixrand = r.random() & 0xFFFF;
        let offset = tensixrand as f32 / 65536.0_f32;
        return low + offset * range as f32;
    }
    low
}

/// Computes the exact 2D shot displacement `(x * spread, y * spread)` matching `FireBullets3`
/// when `shot == 1` (`UTIL_SharedRandomFloat` x4 with `seed..seed+3`).
pub fn fire_bullets3_spread_offset(spread: f32, random_seed: u32) -> (f32, f32) {
    let x = shared_random_float(random_seed, -0.5, 0.5)
        + shared_random_float(random_seed.wrapping_add(1), -0.5, 0.5);
    let y = shared_random_float(random_seed.wrapping_add(2), -0.5, 0.5)
        + shared_random_float(random_seed.wrapping_add(3), -0.5, 0.5);
    (x * spread, y * spread)
}

#[derive(Debug, Clone, Copy)]
struct KickParams {
    up_base: f32,
    lateral_base: f32,
    up_modifier: f32,
    lateral_modifier: f32,
    up_max: f32,
    lateral_max: f32,
    direction_change: i32,
}

#[derive(Debug, Clone, Copy)]
enum WeaponKind {
    Pistol {
        range_coeffs: [f64; 4],
        cycle_time: f32,
        silencer_bit: Option<i32>,
        burst_bit: Option<i32>,
    },
    Shotgun {
        reload_step_anim: i32,
        cycle_time: f32,
    },
    Automatic {
        accuracy_divisor: i32,
        acc_base: f32,
        acc_cap: f32,
        mp5_f64_acc: bool,
        cycle_time: f32,
        zoom_cycle_time: Option<f32>,
        silencer_bit: Option<i32>,
        burst_bit: Option<i32>,
        stand_k: f64,
        duck_k: f64,
        move_k: f64,
        air_k: f64,
        move_base: f64,
        air_base: f64,
        move_speed_thresh: f32,
        kick_stand: KickParams,
        kick_duck: KickParams,
        kick_move: KickParams,
        kick_air: KickParams,
    },
    Sniper {
        cycle_time: f32,
        two_step_zoom_second: f32,
        zoom_max_speed: f32,
    },
    Knife,
    Grenade,
    C4,
}

#[derive(Debug, Clone, Copy)]
struct WeaponSpec {
    event_name: &'static str,
    max_clip: i32,
    reload_time: f32,
    reload_anim: i32,
    max_speed: f32,
    allows_shield: bool,
    is_pistol: bool,
    kind: WeaponKind,
}

fn weapon_spec(id: i32) -> Option<WeaponSpec> {
    use weapon_id::*;
    Some(match id {
        P228 => WeaponSpec {
            event_name: "events/p228.sc",
            max_clip: 13,
            reload_time: 2.7,
            reload_anim: 5,
            max_speed: 250.0,
            allows_shield: true,
            is_pistol: true,
            kind: WeaponKind::Pistol {
                range_coeffs: [1.5, 0.255, 0.075, 0.15],
                cycle_time: 0.15,
                silencer_bit: None,
                burst_bit: None,
            },
        },
        GLOCK18 => WeaponSpec {
            event_name: "events/glock18.sc",
            max_clip: 20,
            reload_time: 2.2,
            reload_anim: 7,
            max_speed: 250.0,
            allows_shield: true,
            is_pistol: true,
            kind: WeaponKind::Pistol {
                range_coeffs: [1.0, 0.165, 0.075, 0.1],
                cycle_time: 0.15,
                silencer_bit: None,
                burst_bit: Some(WPNSTATE_GLOCK18_BURST_MODE),
            },
        },
        USP => WeaponSpec {
            event_name: "events/usp.sc",
            max_clip: 12,
            reload_time: 2.7,
            reload_anim: 5,
            max_speed: 250.0,
            allows_shield: true,
            is_pistol: true,
            kind: WeaponKind::Pistol {
                range_coeffs: [1.2, 0.225, 0.08, 0.1],
                cycle_time: 0.15,
                silencer_bit: Some(WPNSTATE_USP_SILENCED),
                burst_bit: None,
            },
        },
        DEAGLE => WeaponSpec {
            event_name: "events/deagle.sc",
            max_clip: 7,
            reload_time: 2.2,
            reload_anim: 4,
            max_speed: 250.0,
            allows_shield: true,
            is_pistol: true,
            kind: WeaponKind::Pistol {
                range_coeffs: [1.5, 0.25, 0.115, 0.13],
                cycle_time: 0.225,
                silencer_bit: None,
                burst_bit: None,
            },
        },
        FIVESEVEN => WeaponSpec {
            event_name: "events/fiveseven.sc",
            max_clip: 20,
            reload_time: 2.7,
            reload_anim: 4,
            max_speed: 250.0,
            allows_shield: true,
            is_pistol: true,
            kind: WeaponKind::Pistol {
                range_coeffs: [1.5, 0.255, 0.075, 0.15],
                cycle_time: 0.15,
                silencer_bit: None,
                burst_bit: None,
            },
        },
        ELITE => WeaponSpec {
            event_name: "events/elite_left.sc",
            max_clip: 30,
            reload_time: 4.5,
            reload_anim: 14,
            max_speed: 250.0,
            allows_shield: false,
            is_pistol: true,
            kind: WeaponKind::Pistol {
                range_coeffs: [1.3, 0.175, 0.08, 0.1],
                cycle_time: 0.075,
                silencer_bit: None,
                burst_bit: None,
            },
        },
        M3 => WeaponSpec {
            event_name: "events/m3.sc",
            max_clip: 8,
            reload_time: 0.55,
            reload_anim: 3,
            max_speed: 230.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Shotgun {
                reload_step_anim: 5,
                cycle_time: 0.875,
            },
        },
        XM1014 => WeaponSpec {
            event_name: "events/xm1014.sc",
            max_clip: 7,
            reload_time: 0.55,
            reload_anim: 3,
            max_speed: 240.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Shotgun {
                reload_step_anim: 5,
                cycle_time: 0.25,
            },
        },
        MAC10 => WeaponSpec {
            event_name: "events/mac10.sc",
            max_clip: 30,
            reload_time: 3.15,
            reload_anim: 1,
            max_speed: 250.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 200,
                acc_base: 0.6,
                acc_cap: 1.65,
                mp5_f64_acc: false,
                cycle_time: 0.07,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.03,
                duck_k: 0.03,
                move_k: 0.03,
                air_k: 0.375,
                move_base: 0.0,
                air_base: 0.0,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.5, lateral_base: 0.275, up_modifier: 0.15, lateral_modifier: 0.03, up_max: 3.0, lateral_max: 2.0, direction_change: 10 },
                kick_duck: KickParams { up_base: 0.45, lateral_base: 0.225, up_modifier: 0.125, lateral_modifier: 0.025, up_max: 2.75, lateral_max: 1.75, direction_change: 10 },
                kick_move: KickParams { up_base: 0.9, lateral_base: 0.45, up_modifier: 0.25, lateral_modifier: 0.055, up_max: 4.5, lateral_max: 3.0, direction_change: 6 },
                kick_air: KickParams { up_base: 1.3, lateral_base: 0.55, up_modifier: 0.4, lateral_modifier: 0.05, up_max: 4.75, lateral_max: 3.75, direction_change: 5 },
            },
        },
        TMP => WeaponSpec {
            event_name: "events/tmp.sc",
            max_clip: 30,
            reload_time: 2.12,
            reload_anim: 1,
            max_speed: 250.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 200,
                acc_base: 0.55,
                acc_cap: 1.4,
                mp5_f64_acc: false,
                cycle_time: 0.07,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.03,
                duck_k: 0.03,
                move_k: 0.03,
                air_k: 0.25,
                move_base: 0.0,
                air_base: 0.0,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.45, lateral_base: 0.25, up_modifier: 0.15, lateral_modifier: 0.03, up_max: 2.75, lateral_max: 2.0, direction_change: 10 },
                kick_duck: KickParams { up_base: 0.4, lateral_base: 0.2, up_modifier: 0.125, lateral_modifier: 0.025, up_max: 2.5, lateral_max: 1.75, direction_change: 10 },
                kick_move: KickParams { up_base: 0.8, lateral_base: 0.4, up_modifier: 0.2, lateral_modifier: 0.03, up_max: 3.75, lateral_max: 2.5, direction_change: 6 },
                kick_air: KickParams { up_base: 1.1, lateral_base: 0.5, up_modifier: 0.35, lateral_modifier: 0.04, up_max: 4.5, lateral_max: 3.5, direction_change: 5 },
            },
        },
        MP5N => WeaponSpec {
            event_name: "events/mp5n.sc",
            max_clip: 30,
            reload_time: 2.63,
            reload_anim: 1,
            max_speed: 250.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 220,
                acc_base: 0.45,
                acc_cap: 0.75,
                mp5_f64_acc: true,
                cycle_time: 0.075,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.04,
                duck_k: 0.04,
                move_k: 0.04,
                air_k: 0.2,
                move_base: 0.0,
                air_base: 0.0,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.45, lateral_base: 0.2, up_modifier: 0.11, lateral_modifier: 0.03, up_max: 2.25, lateral_max: 1.5, direction_change: 10 },
                kick_duck: KickParams { up_base: 0.4, lateral_base: 0.18, up_modifier: 0.1, lateral_modifier: 0.025, up_max: 2.0, lateral_max: 1.35, direction_change: 10 },
                kick_move: KickParams { up_base: 0.7, lateral_base: 0.35, up_modifier: 0.2, lateral_modifier: 0.05, up_max: 3.25, lateral_max: 2.0, direction_change: 7 },
                kick_air: KickParams { up_base: 0.9, lateral_base: 0.45, up_modifier: 0.35, lateral_modifier: 0.04, up_max: 4.25, lateral_max: 3.25, direction_change: 5 },
            },
        },
        UMP45 => WeaponSpec {
            event_name: "events/ump45.sc",
            max_clip: 25,
            reload_time: 3.5,
            reload_anim: 1,
            max_speed: 250.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 210,
                acc_base: 0.5,
                acc_cap: 1.0,
                mp5_f64_acc: false,
                cycle_time: 0.1,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.04,
                duck_k: 0.04,
                move_k: 0.04,
                air_k: 0.24,
                move_base: 0.0,
                air_base: 0.0,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.5, lateral_base: 0.275, up_modifier: 0.15, lateral_modifier: 0.03, up_max: 3.0, lateral_max: 2.0, direction_change: 10 },
                kick_duck: KickParams { up_base: 0.45, lateral_base: 0.225, up_modifier: 0.125, lateral_modifier: 0.025, up_max: 2.75, lateral_max: 1.75, direction_change: 10 },
                kick_move: KickParams { up_base: 0.9, lateral_base: 0.45, up_modifier: 0.25, lateral_modifier: 0.055, up_max: 4.5, lateral_max: 3.0, direction_change: 6 },
                kick_air: KickParams { up_base: 1.3, lateral_base: 0.55, up_modifier: 0.4, lateral_modifier: 0.05, up_max: 4.75, lateral_max: 3.75, direction_change: 5 },
            },
        },
        P90 => WeaponSpec {
            event_name: "events/p90.sc",
            max_clip: 50,
            reload_time: 3.4,
            reload_anim: 1,
            max_speed: 245.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 175,
                acc_base: 0.45,
                acc_cap: 1.0,
                mp5_f64_acc: false,
                cycle_time: 0.066,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.045,
                duck_k: 0.045,
                move_k: 0.115,
                air_k: 0.3,
                move_base: 0.0,
                air_base: 0.0,
                move_speed_thresh: 170.0,
                kick_stand: KickParams { up_base: 0.4, lateral_base: 0.25, up_modifier: 0.12, lateral_modifier: 0.03, up_max: 2.0, lateral_max: 2.0, direction_change: 9 },
                kick_duck: KickParams { up_base: 0.35, lateral_base: 0.2, up_modifier: 0.1, lateral_modifier: 0.02, up_max: 1.75, lateral_max: 1.75, direction_change: 10 },
                kick_move: KickParams { up_base: 0.65, lateral_base: 0.4, up_modifier: 0.2, lateral_modifier: 0.045, up_max: 3.0, lateral_max: 2.5, direction_change: 6 },
                kick_air: KickParams { up_base: 0.85, lateral_base: 0.5, up_modifier: 0.35, lateral_modifier: 0.045, up_max: 4.0, lateral_max: 3.5, direction_change: 5 },
            },
        },
        GALIL => WeaponSpec {
            event_name: "events/galil.sc",
            max_clip: 35,
            reload_time: 2.45,
            reload_anim: 1,
            max_speed: 240.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 200,
                acc_base: 0.35,
                acc_cap: 1.25,
                mp5_f64_acc: false,
                cycle_time: 0.0875,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.0375,
                duck_k: 0.0375,
                move_k: 0.07,
                air_k: 0.3,
                move_base: 0.04,
                air_base: 0.04,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.7, lateral_base: 0.325, up_modifier: 0.175, lateral_modifier: 0.035, up_max: 5.0, lateral_max: 2.0, direction_change: 8 },
                kick_duck: KickParams { up_base: 0.6, lateral_base: 0.3, up_modifier: 0.15, lateral_modifier: 0.025, up_max: 4.75, lateral_max: 1.75, direction_change: 9 },
                kick_move: KickParams { up_base: 1.25, lateral_base: 0.45, up_modifier: 0.22, lateral_modifier: 0.05, up_max: 6.0, lateral_max: 2.75, direction_change: 7 },
                kick_air: KickParams { up_base: 1.75, lateral_base: 0.8, up_modifier: 0.4, lateral_modifier: 0.3, up_max: 8.5, lateral_max: 5.5, direction_change: 5 },
            },
        },
        FAMAS => WeaponSpec {
            event_name: "events/famas.sc",
            max_clip: 25,
            reload_time: 3.3,
            reload_anim: 1,
            max_speed: 240.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 215,
                acc_base: 0.3,
                acc_cap: 1.0,
                mp5_f64_acc: false,
                cycle_time: 0.0825,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: Some(WPNSTATE_FAMAS_BURST_MODE),
                stand_k: 0.02,
                duck_k: 0.02,
                move_k: 0.07,
                air_k: 0.3,
                move_base: 0.03,
                air_base: 0.03,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.65, lateral_base: 0.35, up_modifier: 0.25, lateral_modifier: 0.015, up_max: 3.5, lateral_max: 2.25, direction_change: 7 },
                kick_duck: KickParams { up_base: 0.6, lateral_base: 0.3, up_modifier: 0.2, lateral_modifier: 0.0125, up_max: 3.25, lateral_max: 2.0, direction_change: 7 },
                kick_move: KickParams { up_base: 1.0, lateral_base: 0.45, up_modifier: 0.28, lateral_modifier: 0.045, up_max: 3.75, lateral_max: 3.0, direction_change: 7 },
                kick_air: KickParams { up_base: 1.2, lateral_base: 0.5, up_modifier: 0.23, lateral_modifier: 0.15, up_max: 5.5, lateral_max: 3.5, direction_change: 6 },
            },
        },
        AK47 => WeaponSpec {
            event_name: "events/ak47.sc",
            max_clip: 30,
            reload_time: 2.45,
            reload_anim: 1,
            max_speed: 221.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 200,
                acc_base: 0.35,
                acc_cap: 1.25,
                mp5_f64_acc: false,
                cycle_time: 0.0955,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.0275,
                duck_k: 0.0275,
                move_k: 0.07,
                air_k: 0.4,
                move_base: 0.04,
                air_base: 0.04,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 1.0, lateral_base: 0.375, up_modifier: 0.175, lateral_modifier: 0.0375, up_max: 5.75, lateral_max: 1.75, direction_change: 8 },
                kick_duck: KickParams { up_base: 0.9, lateral_base: 0.35, up_modifier: 0.15, lateral_modifier: 0.025, up_max: 5.5, lateral_max: 1.5, direction_change: 9 },
                kick_move: KickParams { up_base: 1.5, lateral_base: 0.45, up_modifier: 0.225, lateral_modifier: 0.05, up_max: 6.5, lateral_max: 2.5, direction_change: 7 },
                kick_air: KickParams { up_base: 2.0, lateral_base: 1.0, up_modifier: 0.5, lateral_modifier: 0.35, up_max: 9.0, lateral_max: 6.0, direction_change: 5 },
            },
        },
        M4A1 => WeaponSpec {
            event_name: "events/m4a1.sc",
            max_clip: 30,
            reload_time: 3.05,
            reload_anim: 4,
            max_speed: 230.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 220,
                acc_base: 0.3,
                acc_cap: 1.0,
                mp5_f64_acc: false,
                cycle_time: 0.0875,
                zoom_cycle_time: None,
                silencer_bit: Some(WPNSTATE_M4A1_SILENCED),
                burst_bit: None,
                stand_k: 0.02,
                duck_k: 0.02,
                move_k: 0.07,
                air_k: 0.4,
                move_base: 0.035,
                air_base: 0.035,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.65, lateral_base: 0.35, up_modifier: 0.25, lateral_modifier: 0.015, up_max: 3.5, lateral_max: 2.25, direction_change: 7 },
                kick_duck: KickParams { up_base: 0.6, lateral_base: 0.3, up_modifier: 0.2, lateral_modifier: 0.0125, up_max: 3.25, lateral_max: 2.0, direction_change: 7 },
                kick_move: KickParams { up_base: 1.0, lateral_base: 0.45, up_modifier: 0.28, lateral_modifier: 0.045, up_max: 3.75, lateral_max: 3.0, direction_change: 7 },
                kick_air: KickParams { up_base: 1.2, lateral_base: 0.5, up_modifier: 0.23, lateral_modifier: 0.15, up_max: 5.5, lateral_max: 3.5, direction_change: 6 },
            },
        },
        AUG => WeaponSpec {
            event_name: "events/aug.sc",
            max_clip: 30,
            reload_time: 3.3,
            reload_anim: 1,
            max_speed: 240.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 215,
                acc_base: 0.3,
                acc_cap: 1.0,
                mp5_f64_acc: false,
                cycle_time: 0.0825,
                zoom_cycle_time: Some(0.135),
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.02,
                duck_k: 0.02,
                move_k: 0.07,
                air_k: 0.4,
                move_base: 0.035,
                air_base: 0.035,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.625, lateral_base: 0.375, up_modifier: 0.25, lateral_modifier: 0.0125, up_max: 3.5, lateral_max: 2.25, direction_change: 8 },
                kick_duck: KickParams { up_base: 0.575, lateral_base: 0.325, up_modifier: 0.2, lateral_modifier: 0.011, up_max: 3.25, lateral_max: 2.0, direction_change: 8 },
                kick_move: KickParams { up_base: 1.0, lateral_base: 0.45, up_modifier: 0.275, lateral_modifier: 0.05, up_max: 4.0, lateral_max: 2.5, direction_change: 7 },
                kick_air: KickParams { up_base: 1.25, lateral_base: 0.45, up_modifier: 0.22, lateral_modifier: 0.18, up_max: 5.5, lateral_max: 4.0, direction_change: 5 },
            },
        },
        SG552 => WeaponSpec {
            event_name: "events/sg552.sc",
            max_clip: 30,
            reload_time: 3.0,
            reload_anim: 1,
            max_speed: 235.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 220,
                acc_base: 0.3,
                acc_cap: 1.0,
                mp5_f64_acc: false,
                cycle_time: 0.0825,
                zoom_cycle_time: Some(0.135),
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.02,
                duck_k: 0.02,
                move_k: 0.075,
                air_k: 0.45,
                move_base: 0.035,
                air_base: 0.035,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.625, lateral_base: 0.375, up_modifier: 0.25, lateral_modifier: 0.0125, up_max: 4.0, lateral_max: 2.25, direction_change: 9 },
                kick_duck: KickParams { up_base: 0.6, lateral_base: 0.35, up_modifier: 0.2, lateral_modifier: 0.0125, up_max: 3.7, lateral_max: 2.0, direction_change: 10 },
                kick_move: KickParams { up_base: 1.0, lateral_base: 0.45, up_modifier: 0.28, lateral_modifier: 0.04, up_max: 4.25, lateral_max: 2.5, direction_change: 7 },
                kick_air: KickParams { up_base: 1.25, lateral_base: 0.45, up_modifier: 0.22, lateral_modifier: 0.18, up_max: 6.0, lateral_max: 4.0, direction_change: 5 },
            },
        },
        M249 => WeaponSpec {
            event_name: "events/m249.sc",
            max_clip: 100,
            reload_time: 4.7,
            reload_anim: 3,
            max_speed: 220.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Automatic {
                accuracy_divisor: 175,
                acc_base: 0.4,
                acc_cap: 0.9,
                mp5_f64_acc: false,
                cycle_time: 0.1,
                zoom_cycle_time: None,
                silencer_bit: None,
                burst_bit: None,
                stand_k: 0.03,
                duck_k: 0.03,
                move_k: 0.095,
                air_k: 0.5,
                move_base: 0.045,
                air_base: 0.045,
                move_speed_thresh: 140.0,
                kick_stand: KickParams { up_base: 0.8, lateral_base: 0.35, up_modifier: 0.3, lateral_modifier: 0.03, up_max: 3.75, lateral_max: 3.0, direction_change: 9 },
                kick_duck: KickParams { up_base: 0.75, lateral_base: 0.325, up_modifier: 0.25, lateral_modifier: 0.025, up_max: 3.5, lateral_max: 2.5, direction_change: 9 },
                kick_move: KickParams { up_base: 1.1, lateral_base: 0.5, up_modifier: 0.3, lateral_modifier: 0.06, up_max: 4.0, lateral_max: 3.0, direction_change: 8 },
                kick_air: KickParams { up_base: 1.8, lateral_base: 0.65, up_modifier: 0.45, lateral_modifier: 0.125, up_max: 5.0, lateral_max: 3.5, direction_change: 8 },
            },
        },
        SCOUT => WeaponSpec {
            event_name: "events/scout.sc",
            max_clip: 10,
            reload_time: 2.0,
            reload_anim: 3,
            max_speed: 260.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Sniper {
                cycle_time: 1.25,
                two_step_zoom_second: 15.0,
                zoom_max_speed: 220.0,
            },
        },
        AWP => WeaponSpec {
            event_name: "events/awp.sc",
            max_clip: 10,
            reload_time: 2.5,
            reload_anim: 4,
            max_speed: 210.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Sniper {
                cycle_time: 1.45,
                two_step_zoom_second: 10.0,
                zoom_max_speed: 150.0,
            },
        },
        G3SG1 => WeaponSpec {
            event_name: "events/g3sg1.sc",
            max_clip: 20,
            reload_time: 3.5,
            reload_anim: 3,
            max_speed: 210.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Sniper {
                cycle_time: 0.25,
                two_step_zoom_second: 15.0,
                zoom_max_speed: 150.0,
            },
        },
        SG550 => WeaponSpec {
            event_name: "events/sg550.sc",
            max_clip: 30,
            reload_time: 3.35,
            reload_anim: 3,
            max_speed: 210.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::Sniper {
                cycle_time: 0.25,
                two_step_zoom_second: 15.0,
                zoom_max_speed: 150.0,
            },
        },
        KNIFE => WeaponSpec {
            event_name: "events/knife.sc",
            max_clip: -1,
            reload_time: 0.0,
            reload_anim: 0,
            max_speed: 250.0,
            allows_shield: true,
            is_pistol: false,
            kind: WeaponKind::Knife,
        },
        HEGRENADE | FLASHBANG | SMOKEGRENADE => WeaponSpec {
            event_name: "",
            max_clip: -1,
            reload_time: 0.0,
            reload_anim: 0,
            max_speed: 250.0,
            allows_shield: true,
            is_pistol: false,
            kind: WeaponKind::Grenade,
        },
        C4 => WeaponSpec {
            event_name: "",
            max_clip: -1,
            reload_time: 0.0,
            reload_anim: 0,
            max_speed: 250.0,
            allows_shield: false,
            is_pistol: false,
            kind: WeaponKind::C4,
        },
        _ => return None,
    })
}

/// Runs one client-predicted weapon step (`cl_lw`) on `state` for `cmd` at `random_seed`.
/// Must run after `pm_move` and before `WeaponState::post_run_cmd(cmd.msec)` so timer checks
/// match ReGameDLL_CS `PlayerPostThink -> ItemPostFrame -> decay_timers`.
#[allow(clippy::too_many_arguments)]
pub fn predict_weapon_step(
    state: &mut LocalState,
    extras: &mut BTreeMap<i32, PredictWeaponExtras>,
    cmd: &GoldSrcUserCmd,
    old_buttons: u16,
    random_seed: u32,
    on_ground: bool,
    ducking: bool,
    runfuncs: bool,
    cl_lw: bool,
    out_events: &mut Vec<PredictedWeaponEvent>,
    out_sounds: &mut Vec<PredictedSound>,
) {
    let wid = state.weapon.weapon_id;
    let Some(spec) = weapon_spec(wid) else {
        return;
    };
    let wid_u8 = match u8::try_from(wid) {
        Ok(v) => v,
        Err(_) => return,
    };

    let holding_shield = state.iuser[2] & PLAYER_HOLDING_SHIELD != 0;
    let emit_fx = runfuncs && cl_lw;
    let pressed_attack = cmd.buttons & IN_ATTACK != 0;
    let pressed_attack2 = cmd.buttons & IN_ATTACK2 != 0;
    let pressed_reload = cmd.buttons & IN_RELOAD != 0;
    let attack_edge = pressed_attack && (old_buttons & IN_ATTACK == 0);
    let attack2_edge = pressed_attack2 && (old_buttons & IN_ATTACK2 == 0);

    let extra = extras.entry(wid).or_default();
    let w = state.weapon.weapons.entry(wid_u8).or_default();
    if w.m_i_id == 0 {
        w.m_i_id = wid;
    }

    // Resume sniper zoom after bolt/fire cooldown finishes.
    if extra.resume_zoom && state.weapon.next_attack <= 0.0 && w.m_fl_next_primary_attack <= 0.0 {
        state.weapon.fov = extra.last_zoom;
        w.m_f_in_zoom = i32::from(extra.last_zoom < DEFAULT_FOV);
        extra.resume_zoom = false;
    }

    // Complete pending magazine reload when next_attack reaches <= 0.
    if w.m_f_in_reload != 0 && state.weapon.next_attack <= 0.0 && spec.max_clip > 0 {
        w.m_i_clip = spec.max_clip;
        w.m_f_in_reload = 0;
        extra.accuracy = 0.2;
        extra.shots_fired = 0;
    }

    // Complete pending shotgun special reload shell insertion step when timer expires.
    if w.m_f_in_special_reload != 0 && state.weapon.next_attack <= 0.0 && spec.max_clip > 0 {
        if w.m_i_clip < spec.max_clip {
            w.m_i_clip += 1;
            w.m_f_in_special_reload = 2;
            state.weapon.next_attack = spec.reload_time;
        } else {
            w.m_f_in_special_reload = 0;
        }
    }

    // Shield bearer pressing IN_ATTACK2 cuts reload short (`player.cpp:7435`).
    if holding_shield && w.m_f_in_reload != 0 && pressed_attack2 {
        state.weapon.next_attack = 0.0;
        w.m_f_in_reload = 0;
    }

    if state.weapon.next_attack > 0.0 {
        return;
    }

    // Secondary attack (shield toggle, zoom, silencer, burst, knife stab).
    if pressed_attack2 && w.m_fl_next_secondary_attack <= 0.0 {
        if holding_shield && spec.allows_shield {
            if w.m_i_weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
                w.m_i_weapon_state &= !WPNSTATE_SHIELD_DRAWN;
                state.maxspeed = spec.max_speed;
            } else {
                w.m_i_weapon_state |= WPNSTATE_SHIELD_DRAWN;
                state.maxspeed = SHIELD_DRAWN_MAX_SPEED;
            }
            w.m_fl_next_primary_attack = 0.4;
            w.m_fl_next_secondary_attack = 0.4;
            w.m_fl_time_weapon_idle = 0.6;
            return;
        }
        match spec.kind {
            WeaponKind::Sniper {
                two_step_zoom_second,
                zoom_max_speed,
                ..
            } => {
                if attack2_edge || w.m_fl_next_secondary_attack <= 0.0 {
                    let cur = state.weapon.fov;
                    let next_fov = if (cur - DEFAULT_FOV).abs() < 0.5 || cur <= 0.0 {
                        40.0
                    } else if (cur - 40.0).abs() < 0.5 {
                        two_step_zoom_second
                    } else {
                        DEFAULT_FOV
                    };
                    state.weapon.fov = next_fov;
                    w.m_f_in_zoom = i32::from(next_fov < DEFAULT_FOV);
                    state.maxspeed = if next_fov < DEFAULT_FOV {
                        zoom_max_speed
                    } else {
                        spec.max_speed
                    };
                    w.m_fl_next_secondary_attack = 0.3;
                    if emit_fx {
                        out_sounds.push(PredictedSound {
                            channel: 3,
                            sample: "weapons/zoom.wav".to_string(),
                            volume: 0.2,
                            attenuation: 2.4,
                            flags: 0,
                            pitch: 100,
                        });
                    }
                    return;
                }
            }
            WeaponKind::Automatic {
                zoom_cycle_time: Some(_),
                ..
            } => {
                let next_fov = if (state.weapon.fov - DEFAULT_FOV).abs() < 0.5
                    || state.weapon.fov <= 0.0
                {
                    55.0
                } else {
                    DEFAULT_FOV
                };
                state.weapon.fov = next_fov;
                w.m_f_in_zoom = i32::from(next_fov < DEFAULT_FOV);
                w.m_fl_next_secondary_attack = 0.3;
                return;
            }
            WeaponKind::Automatic {
                silencer_bit: Some(bit),
                ..
            }
            | WeaponKind::Pistol {
                silencer_bit: Some(bit),
                ..
            } => {
                w.m_i_weapon_state ^= bit;
                let sil_delay = if wid == weapon_id::USP { 3.0 } else { 2.0 };
                w.m_fl_next_secondary_attack = sil_delay;
                w.m_fl_next_primary_attack = sil_delay;
                w.m_fl_time_weapon_idle = sil_delay;
                return;
            }
            WeaponKind::Automatic {
                burst_bit: Some(bit),
                ..
            }
            | WeaponKind::Pistol {
                burst_bit: Some(bit),
                ..
            } => {
                w.m_i_weapon_state ^= bit;
                w.m_fl_next_secondary_attack = 0.3;
                return;
            }
            WeaponKind::Knife => {
                w.m_fl_next_primary_attack = 1.1;
                w.m_fl_next_secondary_attack = 1.1;
                w.m_fl_time_weapon_idle = 2.0;
                if emit_fx {
                    out_events.push(PredictedWeaponEvent {
                        event_name: spec.event_name,
                        args: EventArgs {
                            origin: state.origin,
                            angles: cmd.viewangles,
                            ducking: i32::from(ducking),
                            bparam1: 1,
                            ..Default::default()
                        },
                    });
                }
                return;
            }
            _ => {}
        }
    }

    let shield_drawn = w.m_i_weapon_state & WPNSTATE_SHIELD_DRAWN != 0;

    // Primary attack.
    if pressed_attack && !shield_drawn && w.m_fl_next_primary_attack <= 0.0 {
        match spec.kind {
            WeaponKind::C4 | WeaponKind::Grenade => {}
            WeaponKind::Knife => {
                w.m_fl_next_primary_attack = 0.4;
                w.m_fl_next_secondary_attack = 0.5;
                w.m_fl_time_weapon_idle = 2.0;
                if emit_fx {
                    out_events.push(PredictedWeaponEvent {
                        event_name: spec.event_name,
                        args: EventArgs {
                            origin: state.origin,
                            angles: cmd.viewangles,
                            ducking: i32::from(ducking),
                            ..Default::default()
                        },
                    });
                }
                return;
            }
            WeaponKind::Shotgun { cycle_time, .. } => {
                if w.m_i_clip <= 0 {
                    w.m_fl_next_primary_attack = 0.2;
                    if emit_fx && attack_edge {
                        out_sounds.push(dryfire_sound(false));
                    }
                    return;
                }
                w.m_f_in_special_reload = 0;
                w.m_i_clip -= 1;
                w.m_fl_next_primary_attack = cycle_time;
                w.m_fl_next_secondary_attack = cycle_time;
                w.m_fl_time_weapon_idle = 2.5;
                state.punchangle.x -= if on_ground { 4.0 } else { 8.0 };
                if emit_fx {
                    let spread = if wid == weapon_id::M3 { 0.0675 } else { 0.0725 };
                    let (dx, dy) = fire_bullets3_spread_offset(spread, random_seed);
                    out_events.push(PredictedWeaponEvent {
                        event_name: spec.event_name,
                        args: EventArgs {
                            origin: state.origin,
                            angles: cmd.viewangles,
                            ducking: i32::from(ducking),
                            fparam1: dx,
                            fparam2: dy,
                            iparam1: (state.punchangle.x * 100.0) as i32,
                            iparam2: (state.punchangle.y * 100.0) as i32,
                            ..Default::default()
                        },
                    });
                }
                return;
            }
            WeaponKind::Pistol {
                range_coeffs,
                cycle_time,
                ..
            } => {
                extra.shots_fired += 1;
                if extra.shots_fired > 1 && wid != weapon_id::GLOCK18 {
                    return;
                }
                if w.m_i_clip <= 0 {
                    w.m_fl_next_primary_attack = 0.2;
                    if emit_fx && attack_edge && extra.play_empty_sound {
                        out_sounds.push(dryfire_sound(true));
                    }
                    return;
                }
                let speed_2d = state.velocity.truncate().length();
                let k = if !on_ground {
                    range_coeffs[0]
                } else if speed_2d > 0.0 {
                    range_coeffs[1]
                } else if ducking {
                    range_coeffs[2]
                } else {
                    range_coeffs[3]
                };
                let spread = (k * f64::from(1.0_f32 - extra.accuracy)) as f32;
                let adj_cycle = cycle_time - 0.075;
                w.m_fl_next_primary_attack = adj_cycle.max(0.05);
                w.m_fl_next_secondary_attack = adj_cycle.max(0.05);
                w.m_i_clip -= 1;
                w.m_fl_time_weapon_idle = 2.0;

                let (dx, dy) = fire_bullets3_spread_offset(spread, random_seed);
                let mut event_name = spec.event_name;
                if wid == weapon_id::ELITE {
                    if w.m_i_weapon_state & WPNSTATE_ELITE_LEFT != 0 {
                        w.m_i_weapon_state &= !WPNSTATE_ELITE_LEFT;
                        event_name = "events/elite_left.sc";
                    } else {
                        w.m_i_weapon_state |= WPNSTATE_ELITE_LEFT;
                        event_name = "events/elite_right.sc";
                    }
                }
                let clip_empty = i32::from(w.m_i_clip == 0);
                let silenced = i32::from(
                    wid == weapon_id::USP && (w.m_i_weapon_state & WPNSTATE_USP_SILENCED != 0),
                );
                if emit_fx {
                    out_events.push(PredictedWeaponEvent {
                        event_name,
                        args: EventArgs {
                            origin: state.origin,
                            angles: cmd.viewangles,
                            ducking: i32::from(ducking),
                            fparam1: dx,
                            fparam2: dy,
                            iparam1: (state.punchangle.x * 100.0) as i32,
                            iparam2: (state.punchangle.y * 100.0) as i32,
                            bparam1: clip_empty,
                            bparam2: silenced,
                            ..Default::default()
                        },
                    });
                }
                state.punchangle.x -= 2.0;
                return;
            }
            WeaponKind::Automatic {
                accuracy_divisor,
                acc_base,
                acc_cap,
                mp5_f64_acc,
                cycle_time,
                zoom_cycle_time,
                silencer_bit,
                stand_k,
                duck_k,
                move_k,
                air_k,
                move_base,
                air_base,
                move_speed_thresh,
                kick_stand,
                kick_duck,
                kick_move,
                kick_air,
                ..
            } => {
                let speed_2d = state.velocity.truncate().length();
                let acc = f64::from(extra.accuracy);
                let spread = if !on_ground {
                    (air_base + air_k * acc) as f32
                } else if speed_2d > move_speed_thresh {
                    (move_base + move_k * acc) as f32
                } else if ducking {
                    (duck_k * acc) as f32
                } else {
                    (stand_k * acc) as f32
                };

                extra.shots_fired += 1;
                extra.accuracy = if mp5_f64_acc {
                    let s = f64::from(extra.shots_fired);
                    ((s * s * s / f64::from(accuracy_divisor)) + f64::from(acc_base)) as f32
                } else {
                    let s = extra.shots_fired;
                    let cubed = s.wrapping_mul(s).wrapping_mul(s);
                    ((cubed / accuracy_divisor) as f32) + acc_base
                }
                .min(acc_cap);

                if w.m_i_clip <= 0 {
                    w.m_fl_next_primary_attack = 0.2;
                    if emit_fx && attack_edge && extra.play_empty_sound {
                        out_sounds.push(dryfire_sound(false));
                    }
                    return;
                }

                w.m_i_clip -= 1;
                let effective_cycle = match zoom_cycle_time {
                    Some(zc) if state.weapon.fov < DEFAULT_FOV && state.weapon.fov > 0.0 => zc,
                    _ => cycle_time,
                };
                w.m_fl_next_primary_attack = effective_cycle;
                w.m_fl_next_secondary_attack = effective_cycle;
                w.m_fl_time_weapon_idle = 1.9;

                let (dx, dy) = fire_bullets3_spread_offset(spread, random_seed);
                let silenced = i32::from(
                    silencer_bit.is_some_and(|b| w.m_i_weapon_state & b != 0),
                );
                if emit_fx {
                    out_events.push(PredictedWeaponEvent {
                        event_name: spec.event_name,
                        args: EventArgs {
                            origin: state.origin,
                            angles: cmd.viewangles,
                            ducking: i32::from(ducking),
                            fparam1: dx,
                            fparam2: dy,
                            iparam1: (state.punchangle.x * 100.0) as i32,
                            iparam2: (state.punchangle.y * 100.0) as i32,
                            bparam1: silenced,
                            bparam2: 0,
                            ..Default::default()
                        },
                    });
                }

                let params = if speed_2d > 0.0 {
                    kick_move
                } else if !on_ground {
                    kick_air
                } else if ducking {
                    kick_duck
                } else {
                    kick_stand
                };
                apply_kick_back(state, extra, params, random_seed);
                return;
            }
            WeaponKind::Sniper { cycle_time, .. } => {
                let zoomed = state.weapon.fov > 0.0 && (state.weapon.fov - DEFAULT_FOV).abs() > 0.5;
                if zoomed && (wid == weapon_id::SCOUT || wid == weapon_id::AWP) {
                    extra.resume_zoom = true;
                    extra.last_zoom = state.weapon.fov;
                    state.weapon.fov = DEFAULT_FOV;
                    w.m_f_in_zoom = 0;
                    state.maxspeed = spec.max_speed;
                }
                if w.m_i_clip <= 0 {
                    w.m_fl_next_primary_attack = 0.2;
                    if emit_fx && attack_edge && extra.play_empty_sound {
                        out_sounds.push(dryfire_sound(false));
                    }
                    return;
                }
                let speed_2d = state.velocity.truncate().length();
                let mut spread = match wid {
                    weapon_id::AWP => {
                        if !on_ground {
                            0.85
                        } else if speed_2d > 140.0 {
                            0.25
                        } else if speed_2d > 10.0 {
                            0.1
                        } else if ducking {
                            0.0
                        } else {
                            0.001
                        }
                    }
                    weapon_id::SCOUT => {
                        if !on_ground {
                            0.2
                        } else if speed_2d > 170.0 {
                            0.075
                        } else if ducking {
                            0.0
                        } else {
                            0.007
                        }
                    }
                    weapon_id::G3SG1 => {
                        if !on_ground {
                            0.45
                        } else if speed_2d > 0.0 {
                            0.15
                        } else if ducking {
                            0.035
                        } else {
                            0.055
                        }
                    }
                    _ => {
                        let one_minus = f64::from(1.0_f32 - extra.accuracy);
                        if !on_ground {
                            (0.45 * one_minus) as f32
                        } else if speed_2d > 0.0 {
                            0.15
                        } else if ducking {
                            (0.04 * one_minus) as f32
                        } else {
                            (0.05 * one_minus) as f32
                        }
                    }
                };
                if !zoomed && matches!(wid, weapon_id::AWP | weapon_id::SCOUT | weapon_id::G3SG1 | weapon_id::SG550)
                {
                    let penalty = if matches!(wid, weapon_id::SCOUT) { 0.025 } else { 0.08 };
                    if matches!(wid, weapon_id::AWP | weapon_id::SCOUT) {
                        spread += penalty;
                    }
                }
                w.m_i_clip -= 1;
                w.m_fl_next_primary_attack = cycle_time;
                w.m_fl_next_secondary_attack = cycle_time;
                w.m_fl_time_weapon_idle = 2.0;

                let (dx, dy) = fire_bullets3_spread_offset(spread, random_seed);
                if emit_fx {
                    out_events.push(PredictedWeaponEvent {
                        event_name: spec.event_name,
                        args: EventArgs {
                            origin: state.origin,
                            angles: Vec3::ZERO,
                            ducking: i32::from(ducking),
                            fparam1: dx,
                            fparam2: dy,
                            iparam1: (state.punchangle.x * 100.0) as i32,
                            iparam2: (state.punchangle.x * 100.0) as i32,
                            ..Default::default()
                        },
                    });
                }
                match wid {
                    weapon_id::AWP | weapon_id::SCOUT => state.punchangle.x -= 2.0,
                    weapon_id::G3SG1 => {
                        state.punchangle.x -=
                            shared_random_float(random_seed.wrapping_add(4), 0.75, 1.75)
                                + state.punchangle.x * 0.25;
                        state.punchangle.y +=
                            shared_random_float(random_seed.wrapping_add(5), -0.75, 0.75);
                    }
                    _ => {
                        let r = f64::from(shared_random_float(random_seed.wrapping_add(4), 0.75, 1.25));
                        state.punchangle.x =
                            (f64::from(state.punchangle.x) - (r + f64::from(state.punchangle.x) * 0.25)) as f32;
                        state.punchangle.y +=
                            shared_random_float(random_seed.wrapping_add(5), -0.75, 0.75);
                    }
                }
                return;
            }
        }
    } else if !pressed_attack {
        if spec.is_pistol {
            extra.shots_fired = 0;
        }
        extra.play_empty_sound = true;
    }

    // Reload (`IN_RELOAD`).
    if pressed_reload && spec.max_clip > 0 && w.m_i_clip < spec.max_clip && w.m_f_in_reload == 0 {
        match spec.kind {
            WeaponKind::Shotgun { reload_step_anim, .. } => {
                if w.m_f_in_special_reload == 0 {
                    w.m_f_in_special_reload = 1;
                    state.weapon.next_attack = spec.reload_time;
                    state.weapon.weaponanim = reload_step_anim;
                }
            }
            _ => {
                w.m_f_in_reload = 1;
                w.m_i_weapon_state &= !WPNSTATE_SHIELD_DRAWN;
                state.weapon.fov = DEFAULT_FOV;
                w.m_f_in_zoom = 0;
                state.maxspeed = spec.max_speed;
                state.weapon.next_attack = spec.reload_time;
                w.m_fl_time_weapon_idle = spec.reload_time + 0.5;
                state.weapon.weaponanim = spec.reload_anim;
            }
        }
    }
}

fn apply_kick_back(
    state: &mut LocalState,
    extra: &mut PredictWeaponExtras,
    p: KickParams,
    random_seed: u32,
) {
    let shots = extra.shots_fired as f32;
    let (kick_up, kick_lateral) = if extra.shots_fired == 1 {
        (p.up_base, p.lateral_base)
    } else {
        (
            shots * p.up_modifier + p.up_base,
            shots * p.lateral_modifier + p.lateral_base,
        )
    };
    state.punchangle.x -= kick_up;
    if state.punchangle.x < -p.up_max {
        state.punchangle.x = -p.up_max;
    }
    if extra.direction == 1 {
        state.punchangle.y += kick_lateral;
        if state.punchangle.y > p.lateral_max {
            state.punchangle.y = p.lateral_max;
        }
    } else {
        state.punchangle.y -= kick_lateral;
        if state.punchangle.y < -p.lateral_max {
            state.punchangle.y = -p.lateral_max;
        }
    }
    if shared_random_long(random_seed, 0, p.direction_change) == 0 {
        extra.direction = i32::from(extra.direction == 0);
    }
}

fn dryfire_sound(is_pistol: bool) -> PredictedSound {
    PredictedSound {
        channel: 3,
        sample: if is_pistol {
            "weapons/dryfire_pistol.wav".to_string()
        } else {
            "weapons/dryfire_rifle.wav".to_string()
        },
        volume: 0.8,
        attenuation: 0.8,
        flags: 0,
        pitch: 100,
    }
}
