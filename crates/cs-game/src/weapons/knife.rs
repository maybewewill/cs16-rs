//! The knife (ReGameDLL_CS `wpn_shared/wpn_knife.cpp`, read 2026-11-22), the vanilla branch:
//! no `FTRACE_KNIFE`, no wall sparks, no `CLASS_VEHICLE` test, and the stab does not set the
//! idle time (`REGAMEDLL_FIXES`).
//!
//! The primary attack is the slash ([`Knife::swing`]), the secondary the stab
//! ([`Knife::stab`]) or, with a shield, the shield up and down. A hit asks the engine for a
//! line trace of the reach, then a hull trace (`head_hull`), then [`find_hull_intersection`]
//! when the hull found something that is no player (a brush); the victim takes the damage
//! through `TraceAttack` and `ApplyMultiDamage` ([`crate::bullets`]).
//!
//! Not ported, and named:
//! - `CKnife::Smack` (the decal 0.2 s after a wall hit) and its `SetThink`/`nextthink`: it
//!   calls `DecalGunshot`, which has no body on the server ([`crate::bullets::decal_gunshot`]),
//!   so it does nothing.
//! - `TEXTURETYPE_PlaySound`, which has no body on the server either (the client plays the
//!   impact sound); its `RANDOM_LONG(0, 1) > 1` test (never true) is kept, because it draws a
//!   number from the generator.
//! - `UpdateShieldCrosshair` (the shield task), `m_iWeaponVolume`, `WeaponAnimation` (marked
//!   `NOXREF`), `TheBots`.
//!
//! The weapon's clip must be `WEAPON_NOCLIP` (-1), which `CKnife::Spawn` sets; whoever creates
//! the knife sets it (the inventory task).

use cs_engine::collision::Hull;
use cs_engine::{EdictId, EngineApi, EntTrace, MoveFilter};
use cs_sim::pm::math::angle_vectors;
use glam::{Vec2, Vec3};

use super::core::{CHAN_WEAPON, PlayerAnim, WeaponCtx, gun_position, precache_model, precache_sound};
use super::info::{WPNSTATE_SHIELD_DRAWN, WeaponId};
use super::weapon::{Weapon, WeaponBehavior};
use crate::bullets::{DMG_NEVERGIB, MultiDamage, apply_multi_damage, is_bsp_model, trace_attack};
use crate::player::{ATTN_NORM, CHAN_ITEM, DEAD_NO, PITCH_NORM, dmg, player_slot};

/// `KNIFE_*` (`weapons.h`).
pub const KNIFE_BODYHIT_VOLUME: f32 = 128.0;
pub const KNIFE_WALLHIT_VOLUME: f32 = 512.0;
pub const KNIFE_MAX_SPEED: f32 = 250.0;
pub const KNIFE_MAX_SPEED_SHIELD: f32 = 180.0;
pub const KNIFE_STAB_DAMAGE: f32 = 65.0;
pub const KNIFE_SWING_DAMAGE: f32 = 15.0;
pub const KNIFE_SWING_DAMAGE_FAST: f32 = 20.0;
pub const KNIFE_STAB_DISTANCE: f32 = 32.0;
pub const KNIFE_SWING_DISTANCE: f32 = 48.0;
pub const KNIFE_BACKSTAB_MULTIPLIER: f32 = 3.0;

/// `knife_e`.
const KNIFE_IDLE: i32 = 0;
const KNIFE_DRAW: i32 = 3;
const KNIFE_STABHIT: i32 = 4;
const KNIFE_STABMISS: i32 = 5;
const KNIFE_MIDATTACK1HIT: i32 = 6;
const KNIFE_MIDATTACK2HIT: i32 = 7;
/// `knife_shield_e`.
const KNIFE_SHIELD_ATTACKHIT: i32 = 2;
const KNIFE_SHIELD_DRAW: i32 = 3;
const KNIFE_SHIELD_UP: i32 = 5;
const KNIFE_SHIELD_DOWN: i32 = 6;

/// `VEC_DUCK_HULL_MIN` and `VEC_DUCK_HULL_MAX` (`util.h`): the corners `FindHullIntersection`
/// tries.
const VEC_DUCK_HULL_MIN: Vec3 = Vec3::new(-16.0, -16.0, -18.0);
const VEC_DUCK_HULL_MAX: Vec3 = Vec3::new(16.0, 16.0, 32.0);

/// `FindHullIntersection(vecSrc, tr, mins, maxs, pEntity)` (`wpn_knife.cpp`, read 2026-11-22):
/// when the hull trace found something, looks for the point on its surface a line would hit.
/// A line from `src` through twice the way to the hull's end position: if it hits, that is
/// the result; else the same line to each of the eight corners of the box around that point
/// (`mins` and `maxs` of the hull, in the order x, y, z of the loops) and the nearest hit
/// (strictly nearer than the best so far, from `1e6`) wins. `tr` is left alone when nothing
/// hit. All traces are `dont_ignore_monsters` and pass `skip` (the player).
pub fn find_hull_intersection(
    eng: &EngineApi<'_>,
    src: Vec3,
    tr: &mut EntTrace,
    mins: Vec3,
    maxs: Vec3,
    skip: EdictId,
) {
    let minmaxs = [mins, maxs];
    let mut distance = 1e6_f32;
    let hull_end = src + ((tr.trace.end_pos - src) * 2.0);
    let tmp = eng.trace_line(src, hull_end, MoveFilter::Normal, Some(skip));
    if tmp.trace.fraction < 1.0 {
        *tr = tmp;
        return;
    }
    for i in 0..2 {
        for j in 0..2 {
            for k in 0..2 {
                let end = Vec3::new(
                    hull_end.x + minmaxs[i].x,
                    hull_end.y + minmaxs[j].y,
                    hull_end.z + minmaxs[k].z,
                );
                let tmp = eng.trace_line(src, end, MoveFilter::Normal, Some(skip));
                if tmp.trace.fraction < 1.0 {
                    let this_distance = (tmp.trace.end_pos - src).length();
                    if this_distance < distance {
                        *tr = tmp;
                        distance = this_distance;
                    }
                }
            }
        }
    }
}

/// The sample of a slash that hit a player: `switch (RANDOM_LONG(0, 3))` (0 to 3 give
/// `knife_hit1` to `knife_hit4`; the source has no `default`, the draw never leaves 0..=3).
pub fn knife_hit_sample(n: i32) -> &'static str {
    match n {
        0 => "weapons/knife_hit1.wav",
        1 => "weapons/knife_hit2.wav",
        2 => "weapons/knife_hit3.wav",
        _ => "weapons/knife_hit4.wav",
    }
}

/// The knife (`CKnife`).
#[derive(Debug, Clone, Default)]
pub struct Knife;

impl Knife {
    /// `CKnife::SetPlayerShieldAnim`.
    fn set_player_shield_anim(w: &mut WeaponCtx<'_, '_>) {
        if !w.has_shield() {
            return;
        }
        let drawn = w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0;
        w.player_mut().anim_extension = if drawn { "shield" } else { "shieldknife" }.to_string();
    }

    /// `CKnife::ResetPlayerShieldAnim`.
    fn reset_player_shield_anim(w: &mut WeaponCtx<'_, '_>) {
        if !w.has_shield() {
            return;
        }
        if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
            w.player_mut().anim_extension = "shieldknife".to_string();
        }
    }

    /// `CKnife::ShieldSecondaryFire(iUpAnim, iDownAnim)`: with a shield the secondary button
    /// raises or lowers it instead of stabbing. `UpdateShieldCrosshair` is not ported.
    fn shield_secondary_fire(
        &mut self,
        w: &mut WeaponCtx<'_, '_>,
        up_anim: i32,
        down_anim: i32,
    ) -> bool {
        if !w.has_shield() {
            return false;
        }
        if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
            w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
            w.send_weapon_anim(down_anim, true);
            w.player_mut().anim_extension = "shieldknife".to_string();
            w.weapon.max_speed = KNIFE_MAX_SPEED;
            w.player_mut().shield_drawn = false;
        } else {
            w.weapon.weapon_state |= WPNSTATE_SHIELD_DRAWN;
            w.send_weapon_anim(up_anim, true);
            w.player_mut().anim_extension = "shielded".to_string();
            w.weapon.max_speed = KNIFE_MAX_SPEED_SHIELD;
            w.player_mut().shield_drawn = true;
        }
        w.reset_max_speed(&*self);
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.4);
        w.weapon.next_secondary_attack = 0.4;
        w.weapon.time_weapon_idle = 0.6;
        true
    }

    /// The first half of `Swing` and `Stab`: the line trace of the reach, then the hull trace
    /// when the line found nothing, then `FindHullIntersection` when the hull hit no player.
    /// `UTIL_MakeVectors(v_angle)` gives the forward vector (no punch, unlike the guns).
    fn reach(w: &mut WeaponCtx<'_, '_>, distance: f32) -> (Vec3, Vec3, EntTrace) {
        let id = w.edict();
        let angles = w.eng.ent(id).map_or(Vec3::ZERO, |v| v.v_angle);
        let forward = angle_vectors(angles).0;
        let src = gun_position(w);
        let end = src + forward * distance;
        let mut tr = w.eng.trace_line(src, end, MoveFilter::Normal, Some(id));
        if tr.trace.fraction >= 1.0 {
            tr = w.eng.trace_hull_ents(src, end, Hull::Crouched, MoveFilter::Normal, Some(id));
            if tr.trace.fraction < 1.0 {
                // Calculate the point of intersection of the line (or hull) and the object we
                // hit: an approximation of the "best" intersection.
                let hit = tr.ent.unwrap_or_else(|| w.eng.world());
                if hit == w.eng.world() || is_bsp_model(w.eng, hit) {
                    find_hull_intersection(w.eng, src, &mut tr, VEC_DUCK_HULL_MIN, VEC_DUCK_HULL_MAX, id);
                }
            }
        }
        (src, forward, tr)
    }

    /// `ClearMultiDamage(); pEntity->TraceAttack(...); ApplyMultiDamage(...)` of the player
    /// (a hit on anything that is no player does nothing yet: no other entity takes damage).
    fn hurt(w: &mut WeaponCtx<'_, '_>, hit: EdictId, tr: &EntTrace, damage: f32, forward: Vec3) {
        let shooter = w.edict();
        let mut mm = MultiDamage::new(Some(WeaponId::Knife));
        mm.clear();
        trace_attack(
            w.g, w.eng, hit, Some(shooter), &mut mm, damage, forward, tr, dmg::BULLET | DMG_NEVERGIB,
        );
        apply_multi_damage(w.g, w.eng, &mm, Some(shooter), Some(shooter));
    }

    /// The end of a hit on something that is no player (or a player that lives): the
    /// `fHitWorld` branch. `TEXTURETYPE_PlaySound` has no body, but `RANDOM_LONG(0, 1) > 1`
    /// is evaluated; a hit that stays "the world" plays the wall strike with a pitch of 98 to
    /// 101.
    fn hit_world_tail(w: &mut WeaponCtx<'_, '_>, mut hit_world: bool) {
        if hit_world && w.random_long(0, 1) > 1 {
            hit_world = false;
        }
        if !hit_world {
            // Delay the decal a bit (`Smack`, which does nothing on the server).
            Self::reset_player_shield_anim(w);
        } else {
            // Also play knife strike.
            let pitch = w.random_long(0, 3) + 98;
            let id = w.edict();
            w.eng.emit_sound_flags(
                id, i32::from(CHAN_ITEM), "weapons/knife_hitwall1.wav", 1.0, ATTN_NORM, 0, pitch,
            );
        }
    }

    /// `PLAY` of the miss sound: `RANDOM_LONG(0, 1) ? slash1 : slash2` on the weapon channel,
    /// pitch 94.
    fn slash_sound(w: &mut WeaponCtx<'_, '_>) {
        let sample = if w.random_long(0, 1) != 0 {
            "weapons/knife_slash1.wav"
        } else {
            "weapons/knife_slash2.wav"
        };
        let id = w.edict();
        w.eng.emit_sound_flags(id, i32::from(CHAN_WEAPON), sample, 1.0, ATTN_NORM, 0, 94);
    }

    /// Whether the player `hit` is alive (`IsAlive`: `deadflag == DEAD_NO`).
    fn is_alive(w: &WeaponCtx<'_, '_>, hit: EdictId) -> bool {
        w.eng.ent(hit).is_some_and(|v| v.deadflag == DEAD_NO)
    }

    /// `CKnife::Swing(fFirst)`: returns whether it hit. A miss on the first swing waits 0.35 s
    /// (1.0 with a shield); a hit waits 0.4 s (1.0 with a shield), the secondary 0.5 s (1.2).
    /// The animation alternates between two by `m_iSwing`. The damage is 15, or 20 when the
    /// primary timer plus 0.4 is below the time base (`KnifeSwingDamage(fast)`: never at the
    /// time the timer has just been set). QUIRKS kept: the idle is 2 s on a hit or a first
    /// miss; a killed victim ends the swing before the texture sound.
    pub fn swing(&mut self, w: &mut WeaponCtx<'_, '_>, first: bool) -> bool {
        let mut did_hit = false;
        let (_src, forward, tr) = Self::reach(w, KNIFE_SWING_DISTANCE);

        if tr.trace.fraction >= 1.0 {
            if first {
                if !w.has_shield() {
                    let anim = Self::next_swing_anim(w);
                    w.send_weapon_anim(anim, true);
                    // Miss.
                    w.weapon.next_primary_attack = w.get_next_attack_delay(0.35);
                    w.weapon.next_secondary_attack = 0.5;
                } else {
                    w.send_weapon_anim(KNIFE_SHIELD_ATTACKHIT, true);
                    w.weapon.next_primary_attack = w.get_next_attack_delay(1.0);
                    w.weapon.next_secondary_attack = 1.2;
                }
                w.weapon.time_weapon_idle = 2.0;

                // Play wiff or swish sound.
                Self::slash_sound(w);
                // Player "shoot" animation.
                w.set_animation(PlayerAnim::Attack1);
            }
        } else {
            // Hit.
            did_hit = true;
            if !w.has_shield() {
                let anim = Self::next_swing_anim(w);
                w.send_weapon_anim(anim, true);
                w.weapon.next_primary_attack = w.get_next_attack_delay(0.4);
                w.weapon.next_secondary_attack = 0.5;
            } else {
                w.send_weapon_anim(KNIFE_SHIELD_ATTACKHIT, true);
                w.weapon.next_primary_attack = w.get_next_attack_delay(1.0);
                w.weapon.next_secondary_attack = 1.2;
            }
            w.weapon.time_weapon_idle = 2.0;

            // Play thwack, smack, or dong sound.
            let hit = tr.ent.unwrap_or_else(|| w.eng.world());
            let mut hit_world = true;
            Self::set_player_shield_anim(w);
            // Player "shoot" animation.
            w.set_animation(PlayerAnim::Attack1);

            let fast = w.weapon.next_primary_attack + 0.4 < 0.0;
            let damage = if fast { KNIFE_SWING_DAMAGE_FAST } else { KNIFE_SWING_DAMAGE };
            Self::hurt(w, hit, &tr, damage, forward);

            // `pEntity->Classify() != CLASS_NONE && != CLASS_MACHINE`: a player (the only
            // classified entity of the game so far).
            if player_slot(w.g, w.eng, hit).is_some() {
                // Play thwack or smack sound.
                let sample = knife_hit_sample(w.random_long(0, 3));
                let id = w.edict();
                w.eng.emit_sound(id, CHAN_WEAPON, sample, 1.0, ATTN_NORM, PITCH_NORM);
                if !Self::is_alive(w, hit) {
                    return true;
                }
                hit_world = false;
            }
            Self::hit_world_tail(w, hit_world);
        }
        did_hit
    }

    /// `switch ((m_iSwing++) % 2)`: the animation of this swing.
    fn next_swing_anim(w: &mut WeaponCtx<'_, '_>) -> i32 {
        let n = w.weapon.swing;
        w.weapon.swing += 1;
        if n % 2 == 0 { KNIFE_MIDATTACK1HIT } else { KNIFE_MIDATTACK2HIT }
    }

    /// `CKnife::Stab(fFirst)`: returns whether it hit. 65 damage, tripled when the victim is
    /// stabbed in the back (the stabber's flat view direction, normalised, against the flat
    /// forward vector of the victim's `pev->angles`, which is not normalised, is above 0.80).
    /// A hit waits 1.1 s on both timers, a miss 1.0 s. The idle time is not set (vanilla).
    pub fn stab(&mut self, w: &mut WeaponCtx<'_, '_>, first: bool) -> bool {
        let mut did_hit = false;
        let (_src, forward, tr) = Self::reach(w, KNIFE_STAB_DISTANCE);

        if tr.trace.fraction >= 1.0 {
            if first {
                w.send_weapon_anim(KNIFE_STABMISS, true);
                w.weapon.next_primary_attack = w.get_next_attack_delay(1.0);
                w.weapon.next_secondary_attack = 1.0;

                // Play wiff or swish sound.
                Self::slash_sound(w);
                // Player "shoot" animation.
                w.set_animation(PlayerAnim::Attack1);
            }
        } else {
            // Hit.
            did_hit = true;
            w.send_weapon_anim(KNIFE_STABHIT, true);
            w.weapon.next_primary_attack = w.get_next_attack_delay(1.1);
            w.weapon.next_secondary_attack = 1.1;

            // Play thwack, smack, or dong sound.
            let hit = tr.ent.unwrap_or_else(|| w.eng.world());
            let mut hit_world = true;
            // Player "shoot" animation.
            w.set_animation(PlayerAnim::Attack1);

            let mut damage = KNIFE_STAB_DAMAGE;
            let is_player = player_slot(w.g, w.eng, hit).is_some();
            if is_player {
                let victim_angles = w.eng.ent(hit).map_or(Vec3::ZERO, |v| v.angles);
                let victim_forward = angle_vectors(victim_angles).0;
                let my_forward = Vec2::new(forward.x, forward.y);
                // `Vector2D::Normalize`: the zero vector stays zero.
                let len = (my_forward.x * my_forward.x + my_forward.y * my_forward.y).sqrt();
                let los = if len == 0.0 {
                    Vec2::ZERO
                } else {
                    let inv = 1.0 / len;
                    Vec2::new(my_forward.x * inv, my_forward.y * inv)
                };
                let dot = los.x * victim_forward.x + los.y * victim_forward.y;
                // Multiply the damage if we are stabbing them in the back.
                if dot > 0.80 {
                    damage *= KNIFE_BACKSTAB_MULTIPLIER;
                }
            }
            Self::hurt(w, hit, &tr, damage, forward);

            if is_player {
                let id = w.edict();
                w.eng.emit_sound(id, CHAN_WEAPON, "weapons/knife_stab.wav", 1.0, ATTN_NORM, PITCH_NORM);
                if !Self::is_alive(w, hit) {
                    return true;
                }
                hit_world = false;
            }
            Self::hit_world_tail(w, hit_world);
        }
        did_hit
    }
}

impl WeaponBehavior for Knife {
    fn id(&self) -> WeaponId {
        WeaponId::Knife
    }

    /// `CKnife::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_knife.mdl");
        precache_model(eng, "models/shield/v_shield_knife.mdl");
        precache_model(eng, "models/w_knife.mdl");
        for s in [
            "weapons/knife_deploy1.wav",
            "weapons/knife_hit1.wav",
            "weapons/knife_hit2.wav",
            "weapons/knife_hit3.wav",
            "weapons/knife_hit4.wav",
            "weapons/knife_slash1.wav",
            "weapons/knife_slash2.wav",
            "weapons/knife_stab.wav",
            "weapons/knife_hitwall1.wav",
        ] {
            precache_sound(eng, s);
        }
        eng.precache_event("events/knife.sc");
    }

    /// `CKnife::Deploy`: the deploy sound (0.3, attenuation 2.4), the swing counter and the
    /// speed reset, the shield down; with a shield the shield knife.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        let id = w.edict();
        w.eng.emit_sound(id, CHAN_ITEM, "weapons/knife_deploy1.wav", 0.3, 2.4, PITCH_NORM);
        w.weapon.swing = 0;
        w.weapon.max_speed = KNIFE_MAX_SPEED;
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        w.player_mut().shield_drawn = false;
        if w.has_shield() {
            w.default_deploy(
                &*self,
                "models/shield/v_shield_knife.mdl",
                "models/shield/p_shield_knife.mdl",
                KNIFE_SHIELD_DRAW,
                "shieldknife",
                true,
            )
        } else {
            w.default_deploy(&*self, "models/v_knife.mdl", "models/p_knife.mdl", KNIFE_DRAW, "knife", true)
        }
    }

    /// `CKnife::Holster`: only `m_flNextAttack = 0.5`. QUIRK: unlike the base holster it does
    /// not cancel a reload or hide the models.
    fn holster(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.player_mut().next_attack = 0.5;
    }

    /// `CKnife::PrimaryAttack`: `Swing(TRUE)`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        self.swing(w, true);
    }

    /// `CKnife::SecondaryAttack`: the shield, or `Stab(TRUE)`. (The `pev->nextthink` it sets
    /// belongs to the `Smack` think, which is not ported.)
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if !self.shield_secondary_fire(w, KNIFE_SHIELD_UP, KNIFE_SHIELD_DOWN) {
            self.stab(w, true);
        }
    }

    /// `CKnife::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle > 0.0 {
            return;
        }
        if w.player().shield_drawn {
            return;
        }
        w.weapon.time_weapon_idle = 20.0;
        // Only idle if the slid isn't back.
        w.send_weapon_anim(KNIFE_IDLE, true);
    }

    /// `CKnife::GetMaxSpeed`: `m_fMaxSpeed`, set by the deploy.
    fn max_speed(&self, w: &Weapon) -> f32 {
        w.max_speed
    }

    /// `CKnife::CanDrop`: no.
    fn can_drop(&self, _w: &WeaponCtx<'_, '_>) -> bool {
        false
    }
}
