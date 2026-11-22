//! Constants of `pm_shared` and the headers it includes (`pm_shared.h`, `pm_defs.h`, `const.h`,
//! `in_buttons.h`, `pm_materials.h`). Values read 2026-11-22 from ReGameDLL_CS
//! `regamedll/pm_shared/pm_shared.h`, `pm_defs.h`, `regamedll/dlls/const.h` (via the survey digest
//! section 3). Constants the headers of the fetched copies did not contain are marked "(HL SDK)":
//! they come from the standard Half-Life SDK and are an assumption until checked.

use glam::Vec3;

// pm_shared.h
/// `PM_DEAD_VIEWHEIGHT`: view offset of a dead player.
pub const PM_DEAD_VIEWHEIGHT: f32 = -8.0;
/// `OBS_NONE`..`OBS_MAP_CHASE`: observer modes (`iuser1`).
pub const OBS_NONE: i32 = 0;
pub const OBS_CHASE_LOCKED: i32 = 1;
pub const OBS_CHASE_FREE: i32 = 2;
pub const OBS_ROAMING: i32 = 3;
pub const OBS_IN_EYE: i32 = 4;
pub const OBS_MAP_FREE: i32 = 5;
pub const OBS_MAP_CHASE: i32 = 6;
/// `STEP_*`: footstep sound classes.
pub const STEP_CONCRETE: i32 = 0;
pub const STEP_METAL: i32 = 1;
pub const STEP_DIRT: i32 = 2;
pub const STEP_VENT: i32 = 3;
pub const STEP_GRATE: i32 = 4;
pub const STEP_TILE: i32 = 5;
pub const STEP_SLOSH: i32 = 6;
pub const STEP_WADE: i32 = 7;
pub const STEP_LADDER: i32 = 8;
pub const STEP_SNOW: i32 = 9;
/// `WJ_HEIGHT`: height of the water-jump probe.
pub const WJ_HEIGHT: f32 = 8.0;
/// `STOP_EPSILON` (a `double` literal in C).
pub const STOP_EPSILON: f64 = 0.1;
/// `MAX_CLIMB_SPEED`.
pub const MAX_CLIMB_SPEED: f32 = 200.0;
/// `PLAYER_DUCKING_MULTIPLIER` (a `double` literal in C). `PM_Duck` first stores it in a
/// `real_t` (`float`) local; `PM_LadderMove` multiplies a `float` by it in `double`.
pub const PLAYER_DUCKING_MULTIPLIER: f64 = 0.333;
/// `PM_CHECKSTUCK_MINTIME` seconds (a `double` literal in C).
pub const PM_CHECKSTUCK_MINTIME: f64 = 0.05;
/// `TIME_TO_DUCK` seconds.
pub const TIME_TO_DUCK: f32 = 0.4;
/// `STUCK_MOVEUP`.
pub const STUCK_MOVEUP: i32 = 1;
/// `PM_VEC_DUCK_HULL_MIN` / `PM_VEC_HULL_MIN` (z of the hull minimum).
pub const PM_VEC_DUCK_HULL_MIN: f32 = -18.0;
pub const PM_VEC_HULL_MIN: f32 = -36.0;
/// `PM_VEC_DUCK_VIEW` / `PM_VEC_VIEW`.
pub const PM_VEC_DUCK_VIEW: f32 = 12.0;
pub const PM_VEC_VIEW: f32 = 17.0;
/// `PM_PLAYER_MAX_SAFE_FALL_SPEED`, `PM_PLAYER_MIN_BOUNCE_SPEED`, `PM_PLAYER_FALL_PUNCH_THRESHHOLD`.
pub const PM_PLAYER_MAX_SAFE_FALL_SPEED: i32 = 580;
pub const PM_PLAYER_MIN_BOUNCE_SPEED: i32 = 350;
pub const PM_PLAYER_FALL_PUNCH_THRESHHOLD: i32 = 250;
/// `BUNNYJUMP_MAX_SPEED_FACTOR`.
pub const BUNNYJUMP_MAX_SPEED_FACTOR: f32 = 1.2;
/// `HalfHumanHeight`: `const float HalfHumanHeight = 36.0f;` in ReGameDLL `game_shared/bot/nav.h:145`
/// (read 2026-11-22). The source loop `for (i = 0; i < HalfHumanHeight; i++)` runs 36 times.
pub const HALF_HUMAN_HEIGHT: i32 = 36;

// pm_defs.h
/// `MAX_PHYSENTS`, `MAX_MOVEENTS`, `MAX_CLIP_PLANES`.
pub const MAX_PHYSENTS: usize = 600;
pub const MAX_MOVEENTS: usize = 64;
pub const MAX_CLIP_PLANES: usize = 5;
/// `PM_NORMAL` trace flags.
pub const PM_NORMAL: i32 = 0;
pub const PM_STUDIO_IGNORE: i32 = 1;
pub const PM_STUDIO_BOX: i32 = 2;
pub const PM_GLASS_IGNORE: i32 = 4;
pub const PM_WORLD_ONLY: i32 = 8;
/// `MAX_CLIENTS` (`const.h`).
pub const MAX_CLIENTS: usize = 32;

// const.h: FL_*
pub const FL_CONVEYOR: i32 = 1 << 2;
pub const FL_CLIENT: i32 = 1 << 3;
pub const FL_ONGROUND: i32 = 1 << 9;
pub const FL_WATERJUMP: i32 = 1 << 11;
pub const FL_FROZEN: i32 = 1 << 12;
pub const FL_DUCKING: i32 = 1 << 14;
pub const FL_BASEVELOCITY: i32 = 1 << 22;
pub const FL_MONSTERCLIP: i32 = 1 << 23;
pub const FL_ONTRAIN: i32 = 1 << 24;
pub const FL_SPECTATOR: i32 = 1 << 26;

// const.h: MOVETYPE_*
pub const MOVETYPE_NONE: i32 = 0;
pub const MOVETYPE_WALK: i32 = 3;
pub const MOVETYPE_STEP: i32 = 4;
pub const MOVETYPE_FLY: i32 = 5;
pub const MOVETYPE_TOSS: i32 = 6;
pub const MOVETYPE_PUSH: i32 = 7;
pub const MOVETYPE_NOCLIP: i32 = 8;
pub const MOVETYPE_FLYMISSILE: i32 = 9;
pub const MOVETYPE_BOUNCE: i32 = 10;
pub const MOVETYPE_BOUNCEMISSILE: i32 = 11;
pub const MOVETYPE_FOLLOW: i32 = 12;
pub const MOVETYPE_PUSHSTEP: i32 = 13;

// const.h: DEAD_*
pub const DEAD_NO: i32 = 0;
pub const DEAD_DYING: i32 = 1;
pub const DEAD_DEAD: i32 = 2;

// const.h: CONTENTS_*
pub const CONTENTS_EMPTY: i32 = -1;
pub const CONTENTS_SOLID: i32 = -2;
pub const CONTENTS_WATER: i32 = -3;
pub const CONTENTS_SLIME: i32 = -4;
pub const CONTENTS_LAVA: i32 = -5;
pub const CONTENTS_SKY: i32 = -6;
/// (HL SDK) `CONTENTS_CURRENT_0` .. `CONTENTS_CURRENT_DOWN`, `CONTENTS_TRANSLUCENT`.
pub const CONTENTS_CURRENT_0: i32 = -9;
pub const CONTENTS_CURRENT_90: i32 = -10;
pub const CONTENTS_CURRENT_180: i32 = -11;
pub const CONTENTS_CURRENT_270: i32 = -12;
pub const CONTENTS_CURRENT_UP: i32 = -13;
pub const CONTENTS_CURRENT_DOWN: i32 = -14;
pub const CONTENTS_TRANSLUCENT: i32 = -15;
pub const CONTENTS_LADDER: i32 = -16;

// in_buttons.h
pub const IN_ATTACK: i32 = 1 << 0;
pub const IN_JUMP: i32 = 1 << 1;
pub const IN_DUCK: i32 = 1 << 2;
pub const IN_FORWARD: i32 = 1 << 3;
pub const IN_BACK: i32 = 1 << 4;
pub const IN_USE: i32 = 1 << 5;
pub const IN_CANCEL: i32 = 1 << 6;
pub const IN_LEFT: i32 = 1 << 7;
pub const IN_RIGHT: i32 = 1 << 8;
pub const IN_MOVELEFT: i32 = 1 << 9;
pub const IN_MOVERIGHT: i32 = 1 << 10;
pub const IN_ATTACK2: i32 = 1 << 11;
pub const IN_RUN: i32 = 1 << 12;
pub const IN_RELOAD: i32 = 1 << 13;

// cdll_dll.h: iuser3 bits
pub const PLAYER_CAN_SHOOT: i32 = 1 << 0;
pub const PLAYER_FREEZE_TIME_OVER: i32 = 1 << 1;
pub const PLAYER_IN_BOMB_ZONE: i32 = 1 << 2;
pub const PLAYER_HOLDING_SHIELD: i32 = 1 << 3;

// pm_materials.h
pub const MAX_TEXTURES: usize = 1024;
/// `MAX_TEXTURENAME_LENGHT` (sic): 16 characters plus the terminator.
pub const MAX_TEXTURENAME_LENGHT: usize = 17;
pub const CHAR_TEX_CONCRETE: u8 = b'C';
pub const CHAR_TEX_METAL: u8 = b'M';
pub const CHAR_TEX_DIRT: u8 = b'D';
pub const CHAR_TEX_VENT: u8 = b'V';
pub const CHAR_TEX_GRATE: u8 = b'G';
pub const CHAR_TEX_TILE: u8 = b'T';
pub const CHAR_TEX_SLOSH: u8 = b'S';
pub const CHAR_TEX_WOOD: u8 = b'W';
pub const CHAR_TEX_COMPUTER: u8 = b'P';
pub const CHAR_TEX_GRASS: u8 = b'X';
pub const CHAR_TEX_GLASS: u8 = b'Y';
pub const CHAR_TEX_FLESH: u8 = b'F';
pub const CHAR_TEX_SNOW: u8 = b'N';

// Sound constants (const.h / pm_shared.cpp)
/// `CHAN_BODY`.
pub const CHAN_BODY: i32 = 4;
/// `VOL_NORM`, `ATTN_NORM` (0.8 in HL), `PITCH_NORM`.
pub const VOL_NORM: f32 = 1.0;
pub const ATTN_NORM: f32 = 0.8;
pub const PITCH_NORM: i32 = 100;

/// The four player hulls the engine copies into `playermove_t::player_mins/maxs`
/// (ReHLDS `pmove.cpp` `pm_hullmins`/`pm_hullmaxs`, read 2026-11-22): standing, ducked, point, large.
pub const PLAYER_HULL_MINS: [Vec3; 4] = [
    Vec3::new(-16.0, -16.0, -36.0),
    Vec3::new(-16.0, -16.0, -18.0),
    Vec3::new(0.0, 0.0, 0.0),
    Vec3::new(-32.0, -32.0, -32.0),
];
pub const PLAYER_HULL_MAXS: [Vec3; 4] = [
    Vec3::new(16.0, 16.0, 36.0),
    Vec3::new(16.0, 16.0, 18.0),
    Vec3::new(0.0, 0.0, 0.0),
    Vec3::new(32.0, 32.0, 32.0),
];
