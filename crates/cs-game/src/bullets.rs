//! Bullets: what a shot does between the muzzle and the victim. Ported from ReGameDLL_CS
//! `cbase.cpp` (`CBaseEntity::FireBullets`, `FireBullets3`, `HitShield`, `TraceBleed`,
//! `CBaseEntity::TraceAttack`), `weapons.cpp` (`ClearMultiDamage`, `AddMultiDamage`,
//! `ApplyMultiDamage`, `SpawnBlood`, `DecalGunshot`), `util.cpp` (`UTIL_TextureHit`,
//! `UTIL_BloodDrips`, `UTIL_BloodDecalTrace`, `UTIL_DecalTrace`, `UTIL_GunshotDecalTrace`,
//! `UTIL_Sparks`) and `player.cpp` (`CBasePlayer::TraceAttack`, in [`trace_attack`]), read
//! 2026-11-22, vanilla build (not `REGAMEDLL_FIXES`/`REGAMEDLL_ADD`).
//!
//! What a bullet hits is decided by the engine's traces ([`EngineApi::trace_line`]): the studio
//! hitboxes of a player give the hit group, the texture under a brush hit comes from
//! [`EngineApi::trace_texture`] and the material table of `pm` ([`pm_find_texture_type`]).
//!
//! Left out, on purpose:
//! - The server's `DecalGunshot` is an empty function in the source (the client's event script
//!   paints the bullet decals and plays the impact sound; `TEXTURETYPE_PlaySound` has no server
//!   body either), so a bullet sends no decal message of its own. [`decal_gunshot`] exists for
//!   the call sites and does nothing, as the original. The decals the server does send are the
//!   blood ones ([`blood_decal_trace`], from `TraceBleed`).
//! - Tracers (`iTracerFreq` is 0 for every player weapon) and `UTIL_BubbleTrail`.
//! - `FireBullets` of the monster bullet kinds (`gSkillData`, no monsters here) and the
//!   `BULLET_NONE` club hit.
//! - `IsPenetrableEntity` (a `REGAMEDLL_API` hook chain whose default says yes) and
//!   `SetDmgPenetrationLevel` (empty without `REGAMEDLL_API`).
//! - The shield (`HasShield` is false until the shield task): the shield hit branches are ported,
//!   `IsHittingShield` never says yes yet.
//! - Entities that are not players do not take damage yet (the game has no damageable map
//!   entities), so `CBaseEntity::TraceAttack` has nothing to add for them.

use cs_engine::consts::{MOVETYPE_PUSHSTEP, SOLID_BSP};
use cs_engine::{EdictId, EngineApi, EntTrace, MoveFilter, MsgDest};
use cs_proto48::tempent;
use cs_sim::pm::math::angle_vectors;
use cs_sim::pm::pm_find_texture_type;
use glam::Vec3;

use crate::player::{self, HITGROUP_SHIELD, dmg, player_slot};
use crate::weapons::{BulletType, WeaponId, shared_random_float};
use crate::GameState;

/// `DMG_NEVERGIB`, `DMG_ALWAYSGIB` (`common/const.h`).
pub(crate) const DMG_NEVERGIB: u32 = 1 << 12;
const DMG_ALWAYSGIB: u32 = 1 << 13;
/// `DMG_SLASH`, `DMG_CLUB`, `DMG_MORTAR` (`TraceBleed` lists them).
const DMG_SLASH: u32 = 1 << 2;
const DMG_CLUB: u32 = 1 << 7;
const DMG_MORTAR: u32 = 1 << 11;

/// `CHAR_TEX_*` (`pm_materials.h`).
pub const CHAR_TEX_CONCRETE: u8 = b'C';
pub const CHAR_TEX_METAL: u8 = b'M';
pub const CHAR_TEX_VENT: u8 = b'V';
pub const CHAR_TEX_GRATE: u8 = b'G';
pub const CHAR_TEX_TILE: u8 = b'T';
pub const CHAR_TEX_WOOD: u8 = b'W';
pub const CHAR_TEX_COMPUTER: u8 = b'P';
pub const CHAR_TEX_FLESH: u8 = b'F';

/// `BLOOD_COLOR_RED` (`util.h`), the player's `m_bloodColor`.
pub const BLOOD_COLOR_RED: u8 = 247;

/// `DECAL_*` (`decals.h`): the number of a decal in the game's table.
pub mod decal_number {
    pub const BLOOD1: usize = 13;
    pub const YBLOOD1: usize = 19;
    pub const GLASSBREAK1: usize = 25;
}

/// The names of the decal table `gDecals` (`decals.h` order; `util.cpp`/`globals.cpp` hold the
/// names, which the fetched sources do not include: assumption, the Half-Life SDK's table).
const DECAL_NAMES: [&str; 42] = [
    "{shot1", "{shot2", "{shot3", "{shot4", "{shot5", "{lambda01", "{lambda02", "{lambda03",
    "{lambda04", "{lambda05", "{lambda06", "{scorch1", "{scorch2", "{blood1", "{blood2",
    "{blood3", "{blood4", "{blood5", "{blood6", "{yblood1", "{yblood2", "{yblood3", "{yblood4",
    "{yblood5", "{yblood6", "{break1", "{break2", "{break3", "{bigshot1", "{bigshot2",
    "{bigshot3", "{bigshot4", "{bigshot5", "{spit1", "{spit2", "{bproof1", "{gargstomp",
    "{smscorch1", "{smscorch2", "{smscorch3", "{mommablob", "{mommablob2",
];

/// `RANDOM_FLOAT(low, high)` of the engine, from the game's generator (assumption: the engine's
/// own sequence is not reproducible, only its range and distribution matter). Shared with
/// `hud_misc`.
pub(crate) fn random_float(g: &mut GameState, low: f32, high: f32) -> f32 {
    low + (high - low) * (g.random_range(0, 65535) as f32 / 65535.0)
}

/// `Vector::Normalize()` of the Half-Life SDK: the zero vector gives `(0, 0, 1)`.
pub(crate) fn normalize_hl(v: Vec3) -> Vec3 {
    let len = v.length();
    if len == 0.0 { Vec3::Z } else { v / len }
}

/// `gMultiDamage`: the damage one shot has gathered for one entity. `weapon` is the active
/// weapon of the attacker (what `TakeDamage` reads from `pAttack->m_pActiveItem`).
#[derive(Debug, Clone, Copy)]
pub struct MultiDamage {
    entity: Option<EdictId>,
    amount: f32,
    bits: u32,
    weapon: Option<WeaponId>,
}

impl MultiDamage {
    /// A cleared accumulator for an attacker holding `weapon`.
    pub fn new(weapon: Option<WeaponId>) -> Self {
        Self { entity: None, amount: 0.0, bits: 0, weapon }
    }

    /// `ClearMultiDamage`.
    pub fn clear(&mut self) {
        self.entity = None;
        self.amount = 0.0;
        self.bits = 0;
    }

    /// The entity the damage is gathered for.
    pub fn entity(&self) -> Option<EdictId> {
        self.entity
    }

    /// The damage gathered.
    pub fn amount(&self) -> f32 {
        self.amount
    }
}

/// `ApplyMultiDamage(pevInflictor, pevAttacker)`: hurts the gathered entity. The accumulator is
/// not cleared (the source does not), so adding to the same entity again carries on from the
/// amount already applied.
pub fn apply_multi_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    mm: &MultiDamage,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
) {
    let Some(victim) = mm.entity else { return };
    if let Some(slot) = player_slot(g, eng, victim) {
        player::take_damage_with(g, eng, slot as u8, inflictor, attacker, mm.weapon, mm.amount, mm.bits);
    }
}

/// `AddMultiDamage(pevInflictor, pEntity, flDamage, bitsDamageType)`. QUIRK ("UNDONE: wrong
/// attacker!" in the source): when the entity changes, the previous one is hurt first with the
/// caller's first argument as both inflictor and attacker; the type bits are not reset then.
pub fn add_multi_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    mm: &mut MultiDamage,
    inflictor: Option<EdictId>,
    entity: EdictId,
    damage: f32,
    bits: u32,
) {
    mm.bits |= bits;
    if mm.entity != Some(entity) {
        apply_multi_damage(g, eng, mm, inflictor, inflictor);
        mm.entity = Some(entity);
        mm.amount = 0.0;
    }
    mm.amount += damage;
}

/// `UTIL_ShouldShowBlood(BLOOD_COLOR_RED)`: the `violence_hblood` cvar. A server that has not
/// registered it shows blood (assumption: the engine registers it with the default 1).
fn show_blood(eng: &EngineApi<'_>) -> bool {
    eng.cvar_str("violence_hblood")
        .is_none_or(|v| v.trim().parse::<f32>() != Ok(0.0))
}

/// `UTIL_BloodDrips` for a player's blood (`TE_BLOODSPRITE` to the point's PVS); `amount` is
/// multiplied by two in multiplayer and capped at 255. The sprite indices are those of
/// `sprites/bloodspray.spr` and `sprites/blood.spr` (0 when the game has not precached them).
pub fn blood_drips(eng: &mut EngineApi<'_>, origin: Vec3, color: u8, amount: i32) {
    if !show_blood(eng) || amount == 0 {
        return;
    }
    let amount = (amount.wrapping_mul(2)).min(255);
    let spray = eng.model_index("sprites/bloodspray.spr").unwrap_or(0);
    let drop = eng.model_index("sprites/blood.spr").unwrap_or(0);
    let scale = (amount / 10).clamp(3, 16) as u8;
    let te = tempent::blood_sprite(origin, spray, drop, color, scale);
    eng.send_temp_entity(MsgDest::Pvs(origin), &te);
}

/// `SpawnBlood(vecSpot, bloodColor, flDamage)`.
pub fn spawn_blood(eng: &mut EngineApi<'_>, spot: Vec3, color: u8, damage: f32) {
    blood_drips(eng, spot, color, damage as i32);
}

/// `CBaseEntity::IsBSPModel` (`rg_cbase.h:103`, read 2026-11-22): `pev->solid == SOLID_BSP ||
/// pev->movetype == MOVETYPE_PUSHSTEP`. (Not `MOVETYPE_PUSH`: the source says PUSHSTEP.)
pub(crate) fn is_bsp_model_vars(solid: u8, movetype: u8) -> bool {
    solid == SOLID_BSP || movetype == MOVETYPE_PUSHSTEP
}

/// `IsBSPModel` of the entity `id`.
pub(crate) fn is_bsp_model(eng: &EngineApi<'_>, id: EdictId) -> bool {
    eng.ent(id).is_some_and(|v| is_bsp_model_vars(v.solid, v.movetype))
}

/// `UTIL_DecalTrace(pTrace, decalNumber)`: a decal where a trace ended, on the world or on a
/// brush entity (`TE_WORLDDECAL`/`TE_DECAL`, the `HIGH` kinds for indices over 255), to everyone.
/// Nothing for a number the table does not have, a decal the server does not know, a trace that
/// hit nothing, or a hit that is not a brush model.
pub fn decal_trace(eng: &mut EngineApi<'_>, trace: &EntTrace, decal_number: usize) {
    let Some(name) = DECAL_NAMES.get(decal_number) else { return };
    let index = eng.decal_index(name);
    if index < 0 || trace.trace.fraction == 1.0 {
        return;
    }
    let entity = match trace.ent {
        Some(hit) => {
            if !is_bsp_model(eng, hit) {
                return;
            }
            hit.index
        }
        None => 0,
    };
    let pos = trace.trace.end_pos;
    let te = if entity != 0 {
        tempent::decal_for(pos, index, entity)
    } else {
        tempent::world_decal_for(pos, index)
    };
    if let Some(te) = te {
        eng.send_temp_entity(MsgDest::Broadcast, &te);
    }
}

/// `UTIL_GunshotDecalTrace(pTrace, decalNumber, ClientOnly, pShooter)`: `TE_GUNSHOTDECAL`, to
/// the shooter only (`ClientOnly`, `MSG_ONE`) or to everyone who can hear the point.
pub fn gunshot_decal_trace(
    eng: &mut EngineApi<'_>,
    trace: &EntTrace,
    decal_number: usize,
    client_only_to: Option<u8>,
) {
    let Some(name) = DECAL_NAMES.get(decal_number) else { return };
    let index = eng.decal_index(name);
    if !(0..=255).contains(&index) || trace.trace.fraction == 1.0 {
        return;
    }
    let entity = trace.ent.map_or(0, |e| e.index);
    let te = tempent::gunshot_decal(trace.trace.end_pos, entity, index as u8);
    let dest = match client_only_to {
        Some(slot) => MsgDest::One(slot),
        None => MsgDest::Pas(trace.trace.end_pos),
    };
    eng.send_temp_entity(dest, &te);
}

/// `UTIL_BloodDecalTrace(pTrace, BLOOD_COLOR_RED)`: one of the six red blood decals.
pub fn blood_decal_trace(g: &mut GameState, eng: &mut EngineApi<'_>, trace: &EntTrace) {
    if show_blood(eng) {
        let n = decal_number::BLOOD1 + g.random_range(0, 5) as usize;
        decal_trace(eng, trace, n);
    }
}

/// `DecalGunshot`: empty on the server (see the module notes).
pub fn decal_gunshot() {}

/// `UTIL_Sparks`: `TE_SPARKS` to the point's PVS.
pub fn sparks(eng: &mut EngineApi<'_>, position: Vec3) {
    eng.send_temp_entity(MsgDest::Pvs(position), &tempent::sparks(position));
}

/// `CBaseEntity::TraceBleed` for a bleeding entity of red blood: blood decals on the wall behind
/// the victim. `victim` is the entity the traces pass.
pub fn trace_bleed(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    victim: EdictId,
    damage: f32,
    dir: Vec3,
    end_pos: Vec3,
    bits: u32,
) {
    if damage == 0.0 {
        return;
    }
    if bits & (dmg::CRUSH | dmg::BULLET | DMG_SLASH | dmg::BLAST | DMG_CLUB | DMG_MORTAR) == 0 {
        return;
    }
    let (noise, count) = if damage < 10.0 {
        (0.1_f32, 1)
    } else if damage < 25.0 {
        (0.2, 2)
    } else {
        (0.3, 4)
    };
    for _ in 0..count {
        // Trace in the opposite direction the shot came from.
        let mut trace_dir = dir * -1.0;
        trace_dir.x += random_float(g, -noise, noise);
        trace_dir.y += random_float(g, -noise, noise);
        trace_dir.z += random_float(g, -noise, noise);
        let blood = eng.trace_line(end_pos, end_pos + trace_dir * -172.0, MoveFilter::NoMonsters, Some(victim));
        if blood.trace.fraction != 1.0 {
            // QUIRK (vanilla): only one in three decals is made (`REGAMEDLL_FIXES` flips a coin
            // before the trace instead).
            if g.random_range(0, 2) == 0 {
                blood_decal_trace(g, eng, &blood);
            }
        }
    }
}

/// `CBaseEntity::HitShield(flDamage, ptr)` on the entity `ent` that was hit: a ricochet sound
/// (`RANDOM_LONG(0, 1)`), sparks, and a punch of the entity's view. QUIRK (vanilla, kept):
/// `punchangle.x` is `flDamage * RANDOM_FLOAT(-0.15, 0.15)` and then set to -4 whenever it is
/// below 4, which is always, so it ends as -4 (ReGameDLL pull 919).
pub fn hit_shield(g: &mut GameState, eng: &mut EngineApi<'_>, ent: EdictId, damage: f32, end_pos: Vec3) {
    let sample = if g.random_range(0, 1) != 0 {
        "weapons/ric_metal-1.wav"
    } else {
        "weapons/ric_metal-2.wav"
    };
    eng.emit_sound(ent, player::CHAN_VOICE, sample, 1.0, player::ATTN_NORM, player::PITCH_NORM);
    sparks(eng, end_pos);
    let x = damage * random_float(g, -0.15, 0.15);
    let z = (damage * random_float(g, -0.15, 0.15)).clamp(-5.0, 5.0);
    if let Some(v) = eng.ent_mut(ent) {
        v.punchangle.x = if x < 4.0 { -4.0 } else { x };
        v.punchangle.z = z;
    }
}

/// `UTIL_TextureHit(ptr, vecSrc, vecEnd)`: the `CHAR_TEX_*` letter of what the trace hit. A
/// player is flesh (`Classify()` is not `CLASS_NONE`); for the rest the texture name comes from
/// `TRACE_TEXTURE`, loses a leading `-` or `+` and the character after it, then one of `{!~ `,
/// is cut to 16 characters and looked up in the material table. `0` when there is no texture.
pub fn texture_hit(
    g: &GameState,
    eng: &EngineApi<'_>,
    hit: EdictId,
    src: Vec3,
    end: Vec3,
) -> u8 {
    if player_slot(g, eng, hit).is_some() {
        return CHAR_TEX_FLESH;
    }
    let Some(name) = eng.trace_texture(hit, src, end) else { return 0 };
    let mut name = name.as_str();
    if name.starts_with(['-', '+']) {
        name = name.get(2..).unwrap_or("");
    }
    if name.starts_with(['{', '!', '~', ' ']) {
        name = &name[1..];
    }
    let name: String = name.chars().take(16).collect();
    pm_find_texture_type(&g.pm_shared, &name)
}

/// What a texture does to a bullet passing through it (the `switch` of `FireBullets3`):
/// the factor of `iPenetrationPower` (a `double` multiplied then truncated to `int`), the new
/// `flDamageModifier` if the material sets one, and whether it is metal (sparks, `bHitMetal`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenetrationMaterial {
    pub power_factor: Option<f64>,
    pub damage_modifier: Option<f32>,
    pub metal: bool,
}

/// The effect of the texture type `c` on a bullet (see [`PenetrationMaterial`]).
pub fn penetration_material(c: u8) -> PenetrationMaterial {
    let (power_factor, damage_modifier, metal) = match c {
        CHAR_TEX_METAL => (Some(0.15), Some(0.2), true),
        CHAR_TEX_CONCRETE => (Some(0.25), None, false),
        CHAR_TEX_GRATE => (Some(0.5), Some(0.4), true),
        CHAR_TEX_VENT => (Some(0.5), Some(0.45), true),
        CHAR_TEX_TILE => (Some(0.65), Some(0.3), false),
        CHAR_TEX_COMPUTER => (Some(0.4), Some(0.45), true),
        CHAR_TEX_WOOD => (None, Some(0.6), false),
        _ => (None, None, false),
    };
    PenetrationMaterial { power_factor, damage_modifier, metal }
}

/// The penetration power and distance of a bullet kind (the first `switch` of `FireBullets3`);
/// `(0, 0)` for any other.
pub fn bullet_penetration(kind: BulletType) -> (i32, f32) {
    match kind {
        BulletType::Player9mm => (21, 800.0),
        BulletType::Player45acp => (15, 500.0),
        BulletType::Player50ae => (30, 1000.0),
        BulletType::Player762mm => (39, 5000.0),
        BulletType::Player556mm => (35, 4000.0),
        BulletType::Player338mag => (45, 8000.0),
        BulletType::Player57mm => (30, 2000.0),
        BulletType::Player357sig => (25, 800.0),
        _ => (0, 0.0),
    }
}

/// The arguments of `FireBullets3` besides the shooter.
#[derive(Debug, Clone, Copy)]
pub struct Bullet {
    /// `vecSrc`: the gun position.
    pub src: Vec3,
    /// `vecDirShooting`: `v_forward` of the view.
    pub dir: Vec3,
    /// `vecSpread`.
    pub spread: f32,
    /// `flDistance`.
    pub distance: f32,
    /// `iPenetration`: how many surfaces the bullet may go through.
    pub penetration: i32,
    pub kind: BulletType,
    /// `iDamage`.
    pub damage: i32,
    /// `flRangeModifier`.
    pub range_modifier: f32,
    /// `bPistol`.
    pub pistol: bool,
    /// `shared_rand`: the command's random seed.
    pub shared_rand: u32,
}

/// `CBaseEntity::FireBullets3` (cbase.cpp, read 2026-11-22), the shot of every player weapon but
/// the shotguns and the knife. `shooter` is the entity firing (`pev`), `attacker` the one to
/// blame (`pevAttacker`, the shooter when `None`), `weapon` the shooter's active weapon,
/// `v_right` and `v_up` the view vectors the weapon made with `UTIL_MakeVectors(v_angle +
/// punchangle)` just before. Returns `(x * spread, y * spread, 0)`, the spread used (what the
/// weapons feed their event with).
///
/// The shot: a player's spread comes from `shared_random_float(seed .. seed + 3)`, any other
/// shooter's from the server's generator in a rejection loop. Then up to `penetration` passes:
/// the material under the hit lowers the penetration power and sets the damage modifier; the
/// damage falls off as `range_modifier ^ (distance / 500)`; a hit on the shield stops the bullet;
/// a surface that is no brush model or the last allowed pass sets power 42 and modifiers 0.75;
/// the bullet continues from the hit plus `dir * power`, and each pass applies its damage
/// separately. Quirks kept: `iCurrentDamage` is an `int` truncated at each product (the range
/// product is a `double`, assumption for `Q_pow`, the modifier product a `float`);
/// `iPenetrationPower` multiplies by a `double` and truncates, and it keeps the factors of the
/// earlier passes; the texture effects apply even on a pass that hits nothing;
/// `RANDOM_LONG(0, 3)` is drawn for `DecalGunshot`'s argument unless the weapon is a pistol,
/// before `TraceAttack`.
#[allow(clippy::too_many_arguments)]
pub fn fire_bullets3(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    shooter: EdictId,
    attacker: Option<EdictId>,
    weapon: Option<WeaponId>,
    v_right: Vec3,
    v_up: Vec3,
    bullet: &Bullet,
) -> Vec3 {
    let attacker = attacker.unwrap_or(shooter);
    let (mut power, pen_distance) = bullet_penetration(bullet.kind);
    let mut current_damage = bullet.damage;
    // Hostile input: a sane number of passes (the weapons use 1 to 3).
    let mut penetration = bullet.penetration.clamp(0, 16);
    let mut src = bullet.src;
    let mut distance = bullet.distance;
    let mut mm = MultiDamage::new(weapon);
    mm.bits = dmg::BULLET | DMG_NEVERGIB;

    let (x, y);
    if player_slot(g, eng, shooter).is_some() {
        // Use player's random seed: a circular gaussian spread.
        let seed = bullet.shared_rand;
        x = shared_random_float(seed, -0.5, 0.5) + shared_random_float(seed.wrapping_add(1), -0.5, 0.5);
        y = shared_random_float(seed.wrapping_add(2), -0.5, 0.5) + shared_random_float(seed.wrapping_add(3), -0.5, 0.5);
    } else {
        loop {
            let a = random_float(g, -0.5, 0.5) + random_float(g, -0.5, 0.5);
            let b = random_float(g, -0.5, 0.5) + random_float(g, -0.5, 0.5);
            if a * a + b * b <= 1.0 {
                x = a;
                y = b;
                break;
            }
        }
    }

    if !(bullet.src.is_finite() && bullet.dir.is_finite() && bullet.spread.is_finite()
        && bullet.distance.is_finite() && bullet.range_modifier.is_finite())
    {
        return Vec3::ZERO;
    }
    let dir = bullet.dir + (x * bullet.spread) * v_right + (y * bullet.spread) * v_up;
    let mut end = src + dir * distance;
    let mut damage_modifier = 0.5_f32;

    while penetration != 0 {
        mm.clear();
        let tr = eng.trace_line(src, end, MoveFilter::Normal, Some(shooter));
        let hit = tr.ent.unwrap_or_else(|| eng.world());
        let tex = texture_hit(g, eng, hit, src, end);
        let material = penetration_material(tex);
        if let Some(factor) = material.power_factor {
            power = (f64::from(power) * factor) as i32;
        }
        if let Some(modifier) = material.damage_modifier {
            damage_modifier = modifier;
        }

        if tr.trace.fraction != 1.0 {
            penetration -= 1;
            let current_distance = tr.trace.fraction * distance;
            current_damage = (f64::from(current_damage)
                * f64::from(bullet.range_modifier).powf(f64::from(current_distance / 500.0)))
                as i32;
            if current_distance > pen_distance {
                penetration = 0;
            }
            if tr.trace.hitgroup == HITGROUP_SHIELD {
                // Stop on shield hit.
                hit_shield(g, eng, hit, current_damage as f32, tr.trace.end_pos);
                break;
            }
            let distance_modifier;
            if eng.ent(hit).is_none_or(|v| v.solid != SOLID_BSP) || penetration == 0 {
                power = 42;
                damage_modifier = 0.75;
                distance_modifier = 0.75;
            } else {
                distance_modifier = 0.5;
            }
            // `DecalGunshot(&tr, type, (!bPistol && RANDOM_LONG(0, 3)), pev, bHitMetal)`.
            if !bullet.pistol {
                g.random_range(0, 3);
            }
            decal_gunshot();

            src = tr.trace.end_pos + dir * power as f32;
            distance = (distance - current_distance) * distance_modifier;
            end = src + dir * distance;

            trace_attack(
                g,
                eng,
                hit,
                Some(attacker),
                &mut mm,
                current_damage as f32,
                dir,
                &tr,
                dmg::BULLET | DMG_NEVERGIB,
            );
            current_damage = (current_damage as f32 * damage_modifier) as i32;
        } else {
            penetration = 0;
        }
        apply_multi_damage(g, eng, &mm, Some(shooter), Some(attacker));
    }
    Vec3::new(x * bullet.spread, y * bullet.spread, 0.0)
}

/// `CBaseEntity::FireBullets` (cbase.cpp, read 2026-11-22) as the shotguns call it
/// (`FireBullets(9, src, aim, cone, 3000, BULLET_PLAYER_BUCKSHOT, 0, 0, NULL)`): `shots` pellets,
/// each with a circular gaussian spread from the server's generator (`RANDOM_FLOAT` in a
/// rejection loop, `x` then `y` per attempt), one trace each, the damage of all gathered into one
/// `TakeDamage` at the end. With `damage == 0` and `BULLET_PLAYER_BUCKSHOT` a pellet does
/// `int((1 - fraction) * 20)`; with a `damage` it does that, `DMG_ALWAYSGIB` over 16 and
/// `DMG_NEVERGIB` otherwise. Other bullet kinds with no damage use `gSkillData`, which does not
/// exist here: they do nothing. The accumulator starts as `DMG_BULLET | DMG_NEVERGIB`.
#[allow(clippy::too_many_arguments)]
pub fn fire_bullets(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    shooter: EdictId,
    attacker: Option<EdictId>,
    weapon: Option<WeaponId>,
    v_right: Vec3,
    v_up: Vec3,
    shots: u32,
    src: Vec3,
    dir_shooting: Vec3,
    spread: Vec3,
    distance: f32,
    kind: BulletType,
    damage: i32,
) {
    let attacker = attacker.unwrap_or(shooter);
    let mut mm = MultiDamage::new(weapon);
    mm.clear();
    mm.bits = dmg::BULLET | DMG_NEVERGIB;
    for _ in 0..shots {
        // Get circular gaussian spread.
        let (x, y) = loop {
            let x = random_float(g, -0.5, 0.5) + random_float(g, -0.5, 0.5);
            let y = random_float(g, -0.5, 0.5) + random_float(g, -0.5, 0.5);
            if x * x + y * y <= 1.0 {
                break (x, y);
            }
        };
        let dir = dir_shooting + (x * spread.x) * v_right + (y * spread.y) * v_up;
        let end = src + dir * distance;
        let tr = eng.trace_line(src, end, MoveFilter::Normal, Some(shooter));
        if tr.trace.fraction != 1.0 {
            let hit = tr.ent.unwrap_or_else(|| eng.world());
            if damage != 0 {
                let gib = if damage > 16 { DMG_ALWAYSGIB } else { DMG_NEVERGIB };
                trace_attack(g, eng, hit, Some(attacker), &mut mm, damage as f32, dir, &tr, dmg::BULLET | gib);
                decal_gunshot();
            } else if kind == BulletType::PlayerBuckshot {
                let d = (1.0 - tr.trace.fraction) * 20.0;
                trace_attack(g, eng, hit, Some(attacker), &mut mm, d.trunc(), dir, &tr, dmg::BULLET);
            }
        }
    }
    apply_multi_damage(g, eng, &mm, Some(shooter), Some(attacker));
}

/// `TraceAttack` of the entity `victim` (a virtual call in the source): `CBasePlayer::TraceAttack`
/// for a player, `CBaseEntity::TraceAttack` (nothing to do: no other entity takes damage yet)
/// for the rest. `inflictor` is the first argument `AddMultiDamage` gets (the source passes the
/// attacker there), `tr` the trace that hit.
#[allow(clippy::too_many_arguments)]
pub fn trace_attack(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    victim: EdictId,
    attacker: Option<EdictId>,
    mm: &mut MultiDamage,
    damage: f32,
    dir: Vec3,
    tr: &EntTrace,
    bits: u32,
) {
    let Some(slot) = player_slot(g, eng, victim) else { return };
    player::trace_attack(g, eng, slot, attacker, mm, damage, dir, tr, bits);
}

/// Entry points for the tests (cargo feature `test-hooks`): the weapons that fire bullets come
/// with later tasks, so the tests fire them by hand.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
impl crate::CsGame {
    /// `FireBullets3` for the player of `slot` holding `weapon`, looking along `angles`
    /// (`UTIL_MakeVectors` of them gives `v_right` and `v_up`). Returns the spread it used.
    pub fn test_fire_bullets3(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        weapon: Option<WeaponId>,
        angles: Vec3,
        bullet: &Bullet,
    ) -> Vec3 {
        let shooter = eng.client_edict(slot);
        let (_, right, up) = angle_vectors(angles);
        fire_bullets3(&mut self.state, eng, shooter, None, weapon, right, up, bullet)
    }

    /// `FireBullets` of the shotguns for the player of `slot`.
    #[allow(clippy::too_many_arguments)]
    pub fn test_fire_buckshot(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        angles: Vec3,
        shots: u32,
        src: Vec3,
        spread: Vec3,
        distance: f32,
    ) {
        let shooter = eng.client_edict(slot);
        let (forward, right, up) = angle_vectors(angles);
        fire_bullets(
            &mut self.state, eng, shooter, None, None, right, up, shots, src, forward, spread,
            distance, BulletType::PlayerBuckshot, 0,
        );
    }

    /// `TraceAttack` on the player of `slot` as a bullet of `attacker` (a player's slot) holding
    /// `weapon` that hit `hitgroup`, then `ApplyMultiDamage`.
    pub fn test_trace_attack(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        attacker: u8,
        weapon: Option<WeaponId>,
        damage: f32,
        hitgroup: i32,
    ) {
        let victim = eng.client_edict(slot);
        let shooter = eng.client_edict(attacker);
        let origin = eng.ent(victim).map_or(Vec3::ZERO, |v| v.origin);
        let mut trace = eng.trace_line(origin, origin, MoveFilter::Normal, None);
        trace.ent = Some(victim);
        trace.trace.fraction = 0.5;
        trace.trace.end_pos = origin;
        trace.trace.hitgroup = hitgroup;
        let mut mm = MultiDamage::new(weapon);
        mm.bits = dmg::BULLET | DMG_NEVERGIB;
        trace_attack(
            &mut self.state, eng, victim, Some(shooter), &mut mm, damage, Vec3::X, &trace,
            dmg::BULLET | DMG_NEVERGIB,
        );
        apply_multi_damage(&mut self.state, eng, &mm, Some(shooter), Some(shooter));
    }

    /// `TakeDamage` on the player of `slot` from the player `attacker` (inflictor and attacker
    /// both) holding `weapon`.
    pub fn test_damage_with_weapon(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        attacker: Option<u8>,
        weapon: Option<WeaponId>,
        damage: f32,
        bits: u32,
    ) -> bool {
        let attacker = attacker.map(|a| eng.client_edict(a));
        player::take_damage_with(&mut self.state, eng, slot, attacker, attacker, weapon, damage, bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_engine::consts::{MOVETYPE_NONE, MOVETYPE_PUSH, SOLID_BBOX};

    /// `IsBSPModel`: `SOLID_BSP` or `MOVETYPE_PUSHSTEP`; a `MOVETYPE_PUSH` box is not one.
    #[test]
    fn is_bsp_model_predicate() {
        assert!(is_bsp_model_vars(SOLID_BSP, MOVETYPE_NONE));
        assert!(is_bsp_model_vars(SOLID_BBOX, MOVETYPE_PUSHSTEP));
        assert!(!is_bsp_model_vars(SOLID_BBOX, MOVETYPE_PUSH));
        assert!(!is_bsp_model_vars(SOLID_BBOX, MOVETYPE_NONE));
    }
}
