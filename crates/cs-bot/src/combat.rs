// SPDX-License-Identifier: GPL-3.0-or-later
//! ZBot aim and combat controller (`BotCombatState` + `step_combat`).
//!
//! Ported from ReGameDLL_CS (`bot/cs_bot_aim.cpp`, `bot/cs_bot_weapon.cpp`, `bot/cs_bot_combat.cpp`)
//! as pure, `#![forbid(unsafe_code)]`, `wasm32`-clean Rust.

use glam::Vec3;

use crate::buy::WeaponRestrictions;
use crate::profile::{BotDifficulty, BotProfile, BotWeapon, LookSpringParams, WeaponCategory, mix64};

/// Enemy target visible to the combat controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombatTarget {
    pub id: u32,
    pub pos: Vec3,
    pub eye: Vec3,
    pub vel: Vec3,
    pub threat_score: f32,
    pub is_moving: bool,
    pub has_primary: bool,
}

/// Carried weapons and ammo snapshot for combat decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CombatInventory {
    pub primary: Option<BotWeapon>,
    pub primary_clip: u16,
    pub primary_reserve: u16,
    pub secondary: Option<BotWeapon>,
    pub secondary_clip: u16,
    pub secondary_reserve: u16,
    pub active_weapon: Option<BotWeapon>,
    pub has_knife: bool,
    pub has_shield: bool,
    pub shield_drawn: bool,
    pub he_count: u8,
    pub flash_count: u8,
    pub smoke_count: u8,
}

impl Default for CombatInventory {
    fn default() -> Self {
        Self {
            primary: None,
            primary_clip: 0,
            primary_reserve: 0,
            secondary: Some(BotWeapon::Usp),
            secondary_clip: 12,
            secondary_reserve: 24,
            active_weapon: Some(BotWeapon::Usp),
            has_knife: true,
            has_shield: false,
            shield_drawn: false,
            he_count: 0,
            flash_count: 0,
            smoke_count: 0,
        }
    }
}

/// Request from the tactical layer to throw a grenade at `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrenadeThrowRequest {
    pub grenade: BotWeapon,
    pub target: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ThrowPhase {
    PullingPin { timer: f32 },
    Releasing { timer: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveGrenadeThrow {
    grenade: BotWeapon,
    target: Vec3,
    phase: ThrowPhase,
}

/// Persistent per-bot aim, reaction, burst, scope, and grenade throw state.
#[derive(Debug, Clone, PartialEq)]
pub struct BotCombatState {
    pub target_id: Option<u32>,
    pub reaction_elapsed: f32,
    pub required_reaction: f32,
    pub target_lost_timer: f32,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub yaw_vel: f32,
    pub pitch_vel: f32,
    pub aim_focus: f32,
    pub aim_focus_timer: f32,
    pub focus_offset_yaw: f32,
    pub focus_offset_pitch: f32,
    pub burst_timer: f32,
    pub burst_pause_timer: f32,
    pub shots_in_burst: u16,
    pub zoom_cooldown: f32,
    pub strafe_dir: f32,
    pub strafe_timer: f32,
    pub blind_scatter_yaw: f32,
    pub blind_scatter_pitch: f32,
    pub needs_reload_in_cover: bool,
    active_throw: Option<ActiveGrenadeThrow>,
}

impl Default for BotCombatState {
    fn default() -> Self {
        Self {
            target_id: None,
            reaction_elapsed: 0.0,
            required_reaction: 0.3,
            target_lost_timer: 0.0,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            yaw_vel: 0.0,
            pitch_vel: 0.0,
            aim_focus: 20.0,
            aim_focus_timer: 0.0,
            focus_offset_yaw: 0.0,
            focus_offset_pitch: 0.0,
            burst_timer: 0.0,
            burst_pause_timer: 0.0,
            shots_in_burst: 0,
            zoom_cooldown: 0.0,
            strafe_dir: 1.0,
            strafe_timer: 0.4,
            blind_scatter_yaw: 0.0,
            blind_scatter_pitch: 0.0,
            needs_reload_in_cover: false,
            active_throw: None,
        }
    }
}

/// Inputs to [`step_combat`].
#[derive(Debug, Clone)]
pub struct CombatInput<'a> {
    pub dt: f32,
    pub self_pos: Vec3,
    pub self_eye: Vec3,
    pub self_vel: Vec3,
    pub current_yaw_deg: f32,
    pub current_pitch_deg: f32,
    /// Current `pev->punchangle` in Half-Life degrees (negative `x` kicks view up).
    pub punch_angle_deg: Vec3,
    pub current_fov: u8,
    pub on_ground: bool,
    pub is_crouching: bool,
    pub health: f32,
    pub flash_alpha: f32,
    pub inventory: CombatInventory,
    pub visible_enemies: &'a [CombatTarget],
    pub grenade_request: Option<GrenadeThrowRequest>,
    pub look_at_override: Option<Vec3>,
    pub profile: &'a BotProfile,
    pub difficulty: BotDifficulty,
    pub restrictions: &'a WeaponRestrictions,
    pub rng_seed: u64,
}

/// Output commands emitted by [`step_combat`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombatOutput {
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub desired_weapon: Option<BotWeapon>,
    pub primary_attack: bool,
    pub secondary_attack: bool,
    pub reload: bool,
    pub want_crouch: bool,
    pub want_stop_to_shoot: bool,
    pub strafe_bias: f32,
    pub needs_reload_in_cover: bool,
    pub threw_grenade: Option<BotWeapon>,
    pub engaged_target: Option<u32>,
}

/// Advances the ZBot aim, weapon selection, burst/recoil, scope, shield, and grenade throw
/// state machine by `input.dt` seconds.
pub fn step_combat(state: &mut BotCombatState, input: &CombatInput<'_>) -> CombatOutput {
    let dt = input.dt.clamp(0.001, 0.25);
    state.yaw_deg = input.current_yaw_deg.rem_euclid(360.0);
    state.pitch_deg = input.current_pitch_deg.clamp(-89.0, 89.0);
    state.zoom_cooldown = (state.zoom_cooldown - dt).max(0.0);

    let mut out = CombatOutput {
        yaw_deg: state.yaw_deg,
        pitch_deg: state.pitch_deg,
        desired_weapon: input.inventory.active_weapon,
        primary_attack: false,
        secondary_attack: false,
        reload: false,
        want_crouch: false,
        want_stop_to_shoot: false,
        strafe_bias: 0.0,
        needs_reload_in_cover: false,
        threw_grenade: None,
        engaged_target: None,
    };

    // 1. Flashblind aim scatter overrides normal combat tracking.
    if input.flash_alpha >= 0.55 {
        state.aim_focus = input.profile.aim_focus_initial;
        state.aim_focus_timer += dt;
        if state.aim_focus_timer >= 0.25 {
            state.aim_focus_timer = 0.0;
            let h1 = mix64(input.rng_seed ^ 0xF1A5_0001);
            let h2 = mix64(input.rng_seed ^ 0xF1A5_0002);
            state.blind_scatter_yaw = signed_unit(h1) * 55.0 * input.flash_alpha;
            state.blind_scatter_pitch = signed_unit(h2) * 22.0 * input.flash_alpha;
        }
        let target_yaw = (state.yaw_deg + state.blind_scatter_yaw * dt * 2.0).rem_euclid(360.0);
        let target_pitch = (state.pitch_deg + state.blind_scatter_pitch * dt * 2.0).clamp(-45.0, 45.0);
        step_look_spring(
            state,
            target_yaw,
            target_pitch,
            dt,
            false,
            input.difficulty,
            &input.profile.look_spring,
        );
        out.yaw_deg = state.yaw_deg;
        out.pitch_deg = state.pitch_deg;
        if input.inventory.has_shield && !input.inventory.shield_drawn {
            out.secondary_attack = true;
        } else if input.flash_alpha < 0.78 && !input.visible_enemies.is_empty() && has_active_clip(&input.inventory) {
            // Wild panic burst when partially flashed.
            out.primary_attack = (mix64(input.rng_seed) & 3) == 0;
        }
        state.needs_reload_in_cover = needs_reload(&input.inventory);
        out.needs_reload_in_cover = state.needs_reload_in_cover;
        return out;
    }

    // 2. Active grenade throw takes precedence when no immediate close threat interrupts it.
    let closest_dist = input
        .visible_enemies
        .iter()
        .map(|e| input.self_eye.distance(e.eye))
        .fold(f32::INFINITY, f32::min);

    if state.active_throw.is_none()
        && closest_dist > 260.0
        && let Some(req) = input.grenade_request
        && input.restrictions.allow_grenades
        && has_grenade(&input.inventory, req.grenade)
    {
        state.active_throw = Some(ActiveGrenadeThrow {
            grenade: req.grenade,
            target: req.target,
            phase: ThrowPhase::PullingPin { timer: 0.45 },
        });
    }

    if let Some(mut throw) = state.active_throw {
        if closest_dist < 180.0 {
            // Cancel throw to defend against rushing enemy.
            state.active_throw = None;
        } else {
            let (aim_yaw, aim_pitch) =
                compute_grenade_throw_angles(input.self_eye, throw.target, input.self_vel);
            step_look_spring(
                state,
                aim_yaw,
                aim_pitch,
                dt,
                true,
                input.difficulty,
                &input.profile.look_spring,
            );
            out.yaw_deg = state.yaw_deg;
            out.pitch_deg = state.pitch_deg;
            out.desired_weapon = Some(throw.grenade);

            let aligned = angle_diff(state.yaw_deg, aim_yaw).abs() < 12.0
                && (state.pitch_deg - aim_pitch).abs() < 12.0;

            match &mut throw.phase {
                ThrowPhase::PullingPin { timer } => {
                    if input.inventory.active_weapon == Some(throw.grenade) && aligned {
                        out.primary_attack = true;
                        *timer -= dt;
                        if *timer <= 0.0 {
                            throw.phase = ThrowPhase::Releasing { timer: 0.15 };
                        }
                    }
                    state.active_throw = Some(throw);
                }
                ThrowPhase::Releasing { timer } => {
                    out.primary_attack = false;
                    *timer -= dt;
                    if *timer <= 0.0 {
                        out.threw_grenade = Some(throw.grenade);
                        state.active_throw = None;
                    } else {
                        state.active_throw = Some(throw);
                    }
                }
            }
            return out;
        }
    }

    // 3. Pick primary combat target.
    let best_target = select_best_target(input.self_eye, input.visible_enemies, state.target_id);
    let Some(target) = best_target else {
        state.target_lost_timer += dt;
        if state.target_lost_timer > 0.35 {
            state.target_id = None;
            state.reaction_elapsed = 0.0;
        }
        state.burst_timer = 0.0;
        state.burst_pause_timer = 0.0;
        state.shots_in_burst = 0;

        // Unzoom sniper/scoped rifle when out of combat.
        if input.current_fov < 90
            && state.zoom_cooldown <= 0.0
            && input.inventory.active_weapon.is_some_and(|w| w.has_zoom())
        {
            out.secondary_attack = true;
            state.zoom_cooldown = 0.25;
        }

        // Select best ready weapon and reload if needed while out of combat.
        let ready_weapon = select_out_of_combat_weapon(&input.inventory, input.restrictions);
        out.desired_weapon = Some(ready_weapon);
        let active_needs_reload = active_clip_and_reserve(&input.inventory, ready_weapon)
            .is_some_and(|(clip, max_clip, reserve)| reserve > 0 && clip < (max_clip / 3).max(1));
        if active_needs_reload && input.inventory.active_weapon == Some(ready_weapon) {
            out.reload = true;
        }
        state.needs_reload_in_cover = needs_reload(&input.inventory);
        out.needs_reload_in_cover = state.needs_reload_in_cover;

        if let Some(look_pt) = input.look_at_override {
            let (ty, tp) = angles_to(input.self_eye, look_pt);
            step_look_spring(
                state,
                ty,
                tp,
                dt,
                false,
                input.difficulty,
                &input.profile.look_spring,
            );
            out.yaw_deg = state.yaw_deg;
            out.pitch_deg = state.pitch_deg;
        }
        return out;
    };

    // 4. Target acquired or retained -> update reaction & AimFocus.
    state.target_lost_timer = 0.0;
    if state.target_id != Some(target.id) {
        state.target_id = Some(target.id);
        state.reaction_elapsed = 0.0;
        state.required_reaction =
            compute_reaction_delay(input.profile, input.difficulty, state.yaw_deg, input.self_eye, target.eye);
        state.aim_focus = input.profile.aim_focus_initial;
        state.aim_focus_timer = 0.0;
        state.shots_in_burst = 0;
        state.burst_pause_timer = 0.0;
        resample_focus_offset(state, input.rng_seed ^ u64::from(target.id));
    } else {
        state.reaction_elapsed += dt;
        // Aim focus decays while steadily tracking; widens slightly if bot is moving fast.
        let move_penalty = (input.self_vel.truncate().length() / 250.0) * 1.5;
        let decay = input.profile.aim_focus_decay.clamp(0.1, 0.99).powf(dt * 3.0);
        state.aim_focus = (state.aim_focus * decay + move_penalty * dt).clamp(0.15, 40.0);
        state.aim_focus_timer += dt;
        if state.aim_focus_timer >= input.profile.aim_focus_interval.max(0.05) {
            state.aim_focus_timer = 0.0;
            resample_focus_offset(
                state,
                input.rng_seed ^ ((state.reaction_elapsed * 1000.0) as u64),
            );
        }
    }

    // 5. Choose engagement weapon for distance & ammo.
    let aim_point = compute_aim_point(target, input.profile, input.difficulty);
    let dist = input.self_eye.distance(aim_point);
    let chosen_weapon = select_engagement_weapon(&input.inventory, input.restrictions, dist);
    out.desired_weapon = Some(chosen_weapon);
    out.engaged_target = Some(target.id);

    // 6. Reload-in-cover trigger & shield protection.
    let (active_clip, _max_clip, active_reserve) =
        active_clip_and_reserve(&input.inventory, chosen_weapon).unwrap_or((0, 0, 0));
    state.needs_reload_in_cover = needs_reload(&input.inventory);
    out.needs_reload_in_cover = state.needs_reload_in_cover;

    if input.inventory.has_shield
        && (chosen_weapon == BotWeapon::ShieldGun || (active_clip == 0 && active_reserve > 0))
        && !input.inventory.shield_drawn
    {
        out.secondary_attack = true;
    }

    // 7. Compute desired angles with AimFocus spread + vertical recoil compensation.
    let (base_yaw, base_pitch) = angles_to(input.self_eye, aim_point);
    let spread_scale = state.aim_focus * input.profile.aim_focus_offset_scale;
    let recoil_gain = 0.40 + 0.55 * input.profile.skill;
    // In Half-Life/CS, negative punch_angle.x kicks view up; positive pitch looks down.
    let recoil_comp_pitch = -input.punch_angle_deg.x * recoil_gain;

    let desired_yaw = (base_yaw + state.focus_offset_yaw * spread_scale).rem_euclid(360.0);
    let desired_pitch = (base_pitch + state.focus_offset_pitch * spread_scale + recoil_comp_pitch)
        .clamp(-85.0, 85.0);

    step_look_spring(
        state,
        desired_yaw,
        desired_pitch,
        dt,
        true,
        input.difficulty,
        &input.profile.look_spring,
    );
    out.yaw_deg = state.yaw_deg;
    out.pitch_deg = state.pitch_deg;

    // 8. Sniper / AUG / SG552 scope control.
    if chosen_weapon.has_zoom() && input.inventory.active_weapon == Some(chosen_weapon) {
        let desired_fov = desired_scope_fov(chosen_weapon, dist);
        if input.current_fov != desired_fov && state.zoom_cooldown <= 0.0 {
            out.secondary_attack = true;
            state.zoom_cooldown = 0.28;
        }
    }

    // 9. Movement posture & strafe during combat.
    state.strafe_timer -= dt;
    if state.strafe_timer <= 0.0 {
        state.strafe_timer = 0.35 + ((mix64(input.rng_seed) % 45) as f32) * 0.01;
        state.strafe_dir = match mix64(input.rng_seed ^ 0x57AA) % 3 {
            0 => -1.0,
            1 => 1.0,
            _ => 0.0,
        };
    }

    let is_rifle_or_sniper = matches!(
        chosen_weapon.category(),
        WeaponCategory::Rifle | WeaponCategory::Sniper | WeaponCategory::MachineGun
    );
    if is_rifle_or_sniper && dist > 420.0 {
        out.want_stop_to_shoot = true;
        out.want_crouch = input.profile.crouch || (input.profile.skill >= 0.55 && dist > 700.0);
    } else if chosen_weapon == BotWeapon::Knife {
        out.strafe_bias = 0.0;
    } else {
        out.strafe_bias = state.strafe_dir * 0.75;
    }

    // 10. Firing gate: weapon equipped, aligned, reaction elapsed, burst/tap cadence.
    let yaw_err = angle_diff(state.yaw_deg, base_yaw).abs();
    let pitch_err = (state.pitch_deg - base_pitch).abs();
    let aim_tolerance = match chosen_weapon {
        w if w.is_sniper() => 3.5 + (1.0 - input.profile.skill) * 4.0,
        BotWeapon::Knife => 28.0,
        _ => 9.0 + (1.0 - input.profile.skill) * 8.0,
    };

    if chosen_weapon == BotWeapon::Knife {
        if dist <= 82.0 && yaw_err < aim_tolerance {
            out.primary_attack = true;
        }
        return out;
    }

    if active_clip == 0 {
        if active_reserve > 0 && dist > 600.0 {
            out.reload = true;
        }
        return out;
    }

    let weapon_ready = input.inventory.active_weapon == Some(chosen_weapon)
        || input.inventory.active_weapon.is_none();

    if weapon_ready
        && state.reaction_elapsed >= state.required_reaction
        && yaw_err <= aim_tolerance
        && pitch_err <= aim_tolerance
    {
        if input.inventory.shield_drawn {
            // Lower shield briefly or wait before pistol shot.
            out.secondary_attack = true;
            return out;
        }
        out.primary_attack = step_burst_cadence(state, chosen_weapon, dist, input.profile, dt);
    }

    out
}

/// Computes CS 1.6 bent grenade throw `(yaw_deg, pitch_deg)` matching `wpn_shared/wpn_*grenade.cpp`.
///
/// In CS 1.6 (`throw_it`):
/// - `bent = -10.0 + pitch * (80.0 / 90.0)` when `pitch < 0` (above horizon)
/// - `bent = -10.0 + pitch * (100.0 / 90.0)` when `pitch >= 0` (below horizon)
/// - `speed = ((90.0 - bent) * 6.0).min(750.0)`
pub fn compute_grenade_throw_angles(eye: Vec3, target: Vec3, shooter_vel: Vec3) -> (f32, f32) {
    let delta = target - eye;
    let horiz_dist = (delta.x * delta.x + delta.y * delta.y).sqrt().max(1.0);
    let yaw = delta.y.atan2(delta.x).to_degrees().rem_euclid(360.0);

    // Search pitch in [-65..=45] degrees for closest impact to `target` under CS gravity (800 u/s^2).
    let mut best_pitch = -18.0_f32;
    let mut best_err = f32::INFINITY;
    let gravity = 800.0_f32;

    for step in -65..=45 {
        let pitch = step as f32;
        let bent = if pitch < 0.0 {
            -10.0 + pitch * (80.0 / 90.0)
        } else {
            -10.0 + pitch * (100.0 / 90.0)
        };
        let speed = ((90.0 - bent) * 6.0).min(750.0);
        let rad = bent.to_radians();
        let vx = rad.cos() * speed + shooter_vel.truncate().length() * 0.35;
        let vz = -rad.sin() * speed;
        if vx <= 10.0 {
            continue;
        }
        let t = (horiz_dist / vx).min(2.2);
        let z_at_t = vz * t - 0.5 * gravity * t * t;
        let err = (z_at_t - delta.z).abs();
        if err < best_err {
            best_err = err;
            best_pitch = pitch;
        }
    }

    (yaw, best_pitch.clamp(-75.0, 60.0))
}

/// Selects the best combat weapon for `dist` given carried inventory and `bot_allow_*` flags.
pub fn select_engagement_weapon(
    inv: &CombatInventory,
    restrictions: &WeaponRestrictions,
    dist: f32,
) -> BotWeapon {
    if restrictions.is_knives_only() {
        return BotWeapon::Knife;
    }

    let primary_usable = inv.primary.filter(|&w| {
        restrictions.allows_weapon(w) && inv.primary_clip > 0
    });
    let secondary_usable = inv.secondary.filter(|&w| {
        restrictions.allows_weapon(w) && inv.secondary_clip > 0
    });

    if let Some(p) = primary_usable {
        // Shotgun at long range -> fall back to pistol if available.
        if p.category() == WeaponCategory::Shotgun
            && dist > 950.0
            && let Some(s) = secondary_usable
        {
            return s;
        }
        // Bolt sniper at point-blank range -> fall back to pistol if available.
        if matches!(p, BotWeapon::Awp | BotWeapon::Scout)
            && dist < 140.0
            && let Some(s) = secondary_usable
        {
            return s;
        }
        return p;
    }

    if let Some(s) = secondary_usable {
        return s;
    }

    // Both primary and secondary clips are empty; pick weapon with reserve to reload, or knife.
    if let Some(p) = inv.primary.filter(|&w| restrictions.allows_weapon(w) && inv.primary_reserve > 0) && dist > 180.0 {
        return p;
    }
    if let Some(s) = inv.secondary.filter(|&w| restrictions.allows_weapon(w) && inv.secondary_reserve > 0) && dist > 120.0 {
        return s;
    }
    if inv.has_shield && restrictions.allow_shield {
        return BotWeapon::ShieldGun;
    }

    BotWeapon::Knife
}

fn select_out_of_combat_weapon(
    inv: &CombatInventory,
    restrictions: &WeaponRestrictions,
) -> BotWeapon {
    if restrictions.is_knives_only() {
        return BotWeapon::Knife;
    }
    if let Some(p) = inv.primary.filter(|&w| {
        restrictions.allows_weapon(w) && (inv.primary_clip > 0 || inv.primary_reserve > 0)
    }) {
        return p;
    }
    if let Some(s) = inv.secondary.filter(|&w| {
        restrictions.allows_weapon(w) && (inv.secondary_clip > 0 || inv.secondary_reserve > 0)
    }) {
        return s;
    }
    BotWeapon::Knife
}

fn desired_scope_fov(weapon: BotWeapon, dist: f32) -> u8 {
    match weapon {
        BotWeapon::Awp | BotWeapon::Scout | BotWeapon::G3sg1 | BotWeapon::Sg550 => {
            if dist > 1350.0 {
                15
            } else if dist > 420.0 {
                40
            } else {
                90
            }
        }
        BotWeapon::Aug | BotWeapon::Sg552 if dist > 850.0 => 55,
        BotWeapon::Aug | BotWeapon::Sg552 => 90,
        _ => 90,
    }
}

fn step_burst_cadence(
    state: &mut BotCombatState,
    weapon: BotWeapon,
    dist: f32,
    profile: &BotProfile,
    dt: f32,
) -> bool {
    if weapon.is_sniper() {
        // Sniper rifles fire single deliberate shots.
        if state.burst_pause_timer > 0.0 {
            state.burst_pause_timer = (state.burst_pause_timer - dt).max(0.0);
            return false;
        }
        state.burst_pause_timer = if matches!(weapon, BotWeapon::G3sg1 | BotWeapon::Sg550) {
            0.28 + (1.0 - profile.skill) * 0.15
        } else {
            0.35
        };
        return true;
    }

    if weapon.is_pistol() {
        if state.burst_pause_timer > 0.0 {
            state.burst_pause_timer = (state.burst_pause_timer - dt).max(0.0);
            return false;
        }
        state.burst_pause_timer = if dist > 700.0 {
            0.22 + (1.0 - profile.skill) * 0.12
        } else {
            0.12 + (1.0 - profile.skill) * 0.08
        };
        return true;
    }

    // Close range spray for rifles/SMGs/M249.
    if dist < 380.0
        || (weapon.category() == WeaponCategory::Smg && dist < 580.0)
        || weapon.category() == WeaponCategory::MachineGun && dist < 550.0
    {
        return true;
    }

    if state.burst_pause_timer > 0.0 {
        state.burst_pause_timer = (state.burst_pause_timer - dt).max(0.0);
        return false;
    }

    let max_burst_time = if dist > 1050.0 {
        0.16 // 1-2 round tap at long range
    } else {
        0.34 // 3-5 round burst at mid range
    };
    state.burst_timer += dt;
    if state.burst_timer >= max_burst_time {
        state.burst_timer = 0.0;
        state.burst_pause_timer = if dist > 1050.0 {
            0.26 + (1.0 - profile.skill) * 0.14
        } else {
            0.18 + (1.0 - profile.skill) * 0.10
        };
        return false;
    }

    true
}

fn step_look_spring(
    state: &mut BotCombatState,
    target_yaw: f32,
    target_pitch: f32,
    dt: f32,
    attacking: bool,
    difficulty: BotDifficulty,
    spring: &LookSpringParams,
) {
    let diff_scale = match difficulty {
        BotDifficulty::Easy => 0.65,
        BotDifficulty::Normal => 0.85,
        BotDifficulty::Hard => 1.05,
        BotDifficulty::Expert => 1.30,
    };
    let (max_accel, stiffness, damping) = if attacking {
        (
            spring.max_accel_attacking * diff_scale,
            spring.stiffness_attacking * diff_scale,
            spring.damping_attacking,
        )
    } else {
        (
            spring.max_accel_normal * diff_scale,
            spring.stiffness_normal * diff_scale,
            spring.damping_normal,
        )
    };

    let yaw_err = angle_diff(state.yaw_deg, target_yaw);
    let pitch_err = target_pitch - state.pitch_deg;

    let yaw_accel = (yaw_err * stiffness - state.yaw_vel * damping).clamp(-max_accel, max_accel);
    let pitch_accel =
        (pitch_err * stiffness - state.pitch_vel * damping).clamp(-max_accel, max_accel);

    state.yaw_vel += yaw_accel * dt;
    state.pitch_vel += pitch_accel * dt;

    // Prevent overshoot oscillation when nearly aligned.
    if yaw_err.abs() < 0.25 && state.yaw_vel.abs() < 8.0 {
        state.yaw_deg = target_yaw;
        state.yaw_vel = 0.0;
    } else {
        state.yaw_deg = (state.yaw_deg + state.yaw_vel * dt).rem_euclid(360.0);
    }

    if pitch_err.abs() < 0.25 && state.pitch_vel.abs() < 8.0 {
        state.pitch_deg = target_pitch;
        state.pitch_vel = 0.0;
    } else {
        state.pitch_deg = (state.pitch_deg + state.pitch_vel * dt).clamp(-89.0, 89.0);
    }
}

fn select_best_target(
    self_eye: Vec3,
    enemies: &[CombatTarget],
    current_target: Option<u32>,
) -> Option<CombatTarget> {
    enemies
        .iter()
        .copied()
        .min_by(|a, b| {
            let sa = target_priority_cost(self_eye, *a, current_target);
            let sb = target_priority_cost(self_eye, *b, current_target);
            sa.total_cmp(&sb)
        })
}

fn target_priority_cost(self_eye: Vec3, t: CombatTarget, current_target: Option<u32>) -> f32 {
    let dist = self_eye.distance(t.eye);
    let sticky_bonus = if current_target == Some(t.id) { -180.0 } else { 0.0 };
    let threat_bonus = -t.threat_score * 120.0;
    dist + sticky_bonus + threat_bonus
}

fn compute_reaction_delay(
    profile: &BotProfile,
    difficulty: BotDifficulty,
    current_yaw: f32,
    self_eye: Vec3,
    target_eye: Vec3,
) -> f32 {
    let diff_mult = match difficulty {
        BotDifficulty::Easy => 1.35,
        BotDifficulty::Normal => 1.0,
        BotDifficulty::Hard => 0.78,
        BotDifficulty::Expert => 0.55,
    };
    let (target_yaw, _) = angles_to(self_eye, target_eye);
    let off_axis = angle_diff(current_yaw, target_yaw).abs();
    let surprise = if off_axis > 60.0 { 0.08 } else { 0.0 };
    ((profile.reaction_time + profile.attack_delay) * diff_mult + surprise).clamp(0.04, 1.25)
}

fn compute_aim_point(target: CombatTarget, profile: &BotProfile, difficulty: BotDifficulty) -> Vec3 {
    // High-skill bots aim higher toward chest/head; lower-skill bots aim center-mass.
    let height_t = match difficulty {
        BotDifficulty::Expert => 0.88,
        BotDifficulty::Hard => 0.80,
        BotDifficulty::Normal => 0.72,
        BotDifficulty::Easy => 0.65,
    };
    let lead = target.vel * (0.04 * profile.skill);
    target.pos.lerp(target.eye, height_t) + lead
}

fn resample_focus_offset(state: &mut BotCombatState, seed: u64) {
    state.focus_offset_yaw = signed_unit(mix64(seed ^ 0xA1E1));
    state.focus_offset_pitch = signed_unit(mix64(seed ^ 0xB2F2)) * 0.65;
}

fn has_grenade(inv: &CombatInventory, w: BotWeapon) -> bool {
    match w {
        BotWeapon::HeGrenade => inv.he_count > 0,
        BotWeapon::Flashbang => inv.flash_count > 0,
        BotWeapon::SmokeGrenade => inv.smoke_count > 0,
        _ => false,
    }
}

fn has_active_clip(inv: &CombatInventory) -> bool {
    inv.primary_clip > 0 || inv.secondary_clip > 0
}

fn needs_reload(inv: &CombatInventory) -> bool {
    let primary_empty = inv.primary.is_some() && inv.primary_clip == 0 && inv.primary_reserve > 0;
    let secondary_empty =
        inv.primary.is_none() && inv.secondary.is_some() && inv.secondary_clip == 0 && inv.secondary_reserve > 0;
    primary_empty || secondary_empty
}

fn active_clip_and_reserve(inv: &CombatInventory, w: BotWeapon) -> Option<(u16, u16, u16)> {
    if inv.primary == Some(w) {
        Some((inv.primary_clip, 30, inv.primary_reserve))
    } else if inv.secondary == Some(w) {
        Some((inv.secondary_clip, 12, inv.secondary_reserve))
    } else {
        None
    }
}

#[inline]
fn signed_unit(h: u64) -> f32 {
    let u = ((h >> 16) as u32) as f32 / (u32::MAX as f32);
    u * 2.0 - 1.0
}

#[inline]
pub(crate) fn angles_to(from: Vec3, to: Vec3) -> (f32, f32) {
    let d = to - from;
    let horiz = (d.x * d.x + d.y * d.y).sqrt().max(0.001);
    let yaw = d.y.atan2(d.x).to_degrees().rem_euclid(360.0);
    let pitch = (-d.z.atan2(horiz).to_degrees()).clamp(-89.0, 89.0);
    (yaw, pitch)
}

#[inline]
pub(crate) fn angle_diff(from: f32, to: f32) -> f32 {
    let mut d = (to - from).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enemy(id: u32, pos: Vec3) -> CombatTarget {
        CombatTarget {
            id,
            pos,
            eye: pos + Vec3::new(0.0, 0.0, 64.0),
            vel: Vec3::ZERO,
            threat_score: 0.5,
            is_moving: false,
            has_primary: true,
        }
    }

    #[test]
    fn reaction_delay_gates_fire_and_aim_focus_decays() {
        let easy_prof = BotProfile::fallback("Shawn", BotDifficulty::Easy);
        let expert_prof = BotProfile::fallback("Cliffe", BotDifficulty::Expert);
        let restrictions = WeaponRestrictions::default();

        let mut easy_state = BotCombatState::default();
        let mut expert_state = BotCombatState::default();
        let inv = CombatInventory {
            primary: Some(BotWeapon::M4a1),
            primary_clip: 30,
            primary_reserve: 90,
            active_weapon: Some(BotWeapon::M4a1),
            ..Default::default()
        };
        let target = [enemy(1, Vec3::new(300.0, 0.0, 0.0))];

        let mut easy_first_shot = None;
        let mut expert_first_shot = None;
        let initial_focus = easy_prof.aim_focus_initial;

        for tick in 0..30 {
            let t = tick as f32 * 0.05;
            let (ey, ep) = (easy_state.yaw_deg, easy_state.pitch_deg);
            let e_out = step_combat(
                &mut easy_state,
                &CombatInput {
                    dt: 0.05,
                    self_pos: Vec3::ZERO,
                    self_eye: Vec3::new(0.0, 0.0, 64.0),
                    self_vel: Vec3::ZERO,
                    current_yaw_deg: ey,
                    current_pitch_deg: ep,
                    punch_angle_deg: Vec3::ZERO,
                    current_fov: 90,
                    on_ground: true,
                    is_crouching: false,
                    health: 100.0,
                    flash_alpha: 0.0,
                    inventory: inv,
                    visible_enemies: &target,
                    grenade_request: None,
                    look_at_override: None,
                    profile: &easy_prof,
                    difficulty: BotDifficulty::Easy,
                    restrictions: &restrictions,
                    rng_seed: 11,
                },
            );
            if e_out.primary_attack && easy_first_shot.is_none() {
                easy_first_shot = Some(t);
            }

            let (xy, xp) = (expert_state.yaw_deg, expert_state.pitch_deg);
            let x_out = step_combat(
                &mut expert_state,
                &CombatInput {
                    dt: 0.05,
                    self_pos: Vec3::ZERO,
                    self_eye: Vec3::new(0.0, 0.0, 64.0),
                    self_vel: Vec3::ZERO,
                    current_yaw_deg: xy,
                    current_pitch_deg: xp,
                    punch_angle_deg: Vec3::ZERO,
                    current_fov: 90,
                    on_ground: true,
                    is_crouching: false,
                    health: 100.0,
                    flash_alpha: 0.0,
                    inventory: inv,
                    visible_enemies: &target,
                    grenade_request: None,
                    look_at_override: None,
                    profile: &expert_prof,
                    difficulty: BotDifficulty::Expert,
                    restrictions: &restrictions,
                    rng_seed: 11,
                },
            );
            if x_out.primary_attack && expert_first_shot.is_none() {
                expert_first_shot = Some(t);
            }
        }

        assert!(expert_first_shot.unwrap() < easy_first_shot.unwrap());
        assert!(easy_state.aim_focus < initial_focus);
        assert!(expert_state.yaw_deg < 10.0 || expert_state.yaw_deg > 350.0);
    }

    #[test]
    fn engagement_weapon_selection_sniper_scope_and_fallbacks() {
        let restrictions = WeaponRestrictions::default();
        let awp_inv = CombatInventory {
            primary: Some(BotWeapon::Awp),
            primary_clip: 10,
            primary_reserve: 20,
            secondary: Some(BotWeapon::Deagle),
            secondary_clip: 7,
            secondary_reserve: 28,
            active_weapon: Some(BotWeapon::Awp),
            ..Default::default()
        };

        assert_eq!(select_engagement_weapon(&awp_inv, &restrictions, 1400.0), BotWeapon::Awp);
        assert_eq!(select_engagement_weapon(&awp_inv, &restrictions, 100.0), BotWeapon::Deagle);

        let empty_all = CombatInventory {
            primary: Some(BotWeapon::Ak47),
            primary_clip: 0,
            primary_reserve: 0,
            secondary: Some(BotWeapon::Glock18),
            secondary_clip: 0,
            secondary_reserve: 0,
            active_weapon: Some(BotWeapon::Ak47),
            ..Default::default()
        };
        assert_eq!(select_engagement_weapon(&empty_all, &restrictions, 60.0), BotWeapon::Knife);

        // Sniper triggers secondary_attack to zoom when enemy is far.
        let prof = BotProfile::fallback("Phoenix", BotDifficulty::Expert);
        let mut st = BotCombatState::default();
        let out = step_combat(
            &mut st,
            &CombatInput {
                dt: 0.05,
                self_pos: Vec3::ZERO,
                self_eye: Vec3::new(0.0, 0.0, 64.0),
                self_vel: Vec3::ZERO,
                current_yaw_deg: 0.0,
                current_pitch_deg: 0.0,
                punch_angle_deg: Vec3::ZERO,
                current_fov: 90,
                on_ground: true,
                is_crouching: false,
                health: 100.0,
                flash_alpha: 0.0,
                inventory: awp_inv,
                visible_enemies: &[enemy(2, Vec3::new(1500.0, 0.0, 0.0))],
                grenade_request: None,
                look_at_override: None,
                profile: &prof,
                difficulty: BotDifficulty::Expert,
                restrictions: &restrictions,
                rng_seed: 1,
            },
        );
        assert!(out.secondary_attack);
        assert!(out.want_stop_to_shoot);
    }

    #[test]
    fn recoil_compensation_and_reload_in_cover_and_flash_scatter() {
        let prof = BotProfile::fallback("Cliffe", BotDifficulty::Expert);
        let restrictions = WeaponRestrictions::default();
        let mut st = BotCombatState {
            target_id: Some(1),
            reaction_elapsed: 1.0,
            required_reaction: 0.05,
            ..Default::default()
        };
        let inv = CombatInventory {
            primary: Some(BotWeapon::Ak47),
            primary_clip: 20,
            primary_reserve: 60,
            active_weapon: Some(BotWeapon::Ak47),
            ..Default::default()
        };

        // Upward kick (negative punchangle.x) should pull aim pitch downward (positive).
        for _ in 0..8 {
            let (sy, sp) = (st.yaw_deg, st.pitch_deg);
            step_combat(
                &mut st,
                &CombatInput {
                    dt: 0.05,
                    self_pos: Vec3::ZERO,
                    self_eye: Vec3::new(0.0, 0.0, 64.0),
                    self_vel: Vec3::ZERO,
                    current_yaw_deg: sy,
                    current_pitch_deg: sp,
                    punch_angle_deg: Vec3::new(-4.0, 0.0, 0.0),
                    current_fov: 90,
                    on_ground: true,
                    is_crouching: false,
                    health: 100.0,
                    flash_alpha: 0.0,
                    inventory: inv,
                    visible_enemies: &[enemy(1, Vec3::new(500.0, 0.0, 0.0))],
                    grenade_request: None,
                    look_at_override: None,
                    profile: &prof,
                    difficulty: BotDifficulty::Expert,
                    restrictions: &restrictions,
                    rng_seed: 3,
                },
            );
        }
        assert!(st.pitch_deg > 1.0, "expected downward pitch recoil compensation, got {}", st.pitch_deg);

        // Empty primary clip triggers needs_reload_in_cover.
        let mut empty_inv = inv;
        empty_inv.primary_clip = 0;
        let (sy, sp) = (st.yaw_deg, st.pitch_deg);
        let out_empty = step_combat(
            &mut st,
            &CombatInput {
                dt: 0.05,
                self_pos: Vec3::ZERO,
                self_eye: Vec3::new(0.0, 0.0, 64.0),
                self_vel: Vec3::ZERO,
                current_yaw_deg: sy,
                current_pitch_deg: sp,
                punch_angle_deg: Vec3::ZERO,
                current_fov: 90,
                on_ground: true,
                is_crouching: false,
                health: 100.0,
                flash_alpha: 0.0,
                inventory: empty_inv,
                visible_enemies: &[enemy(1, Vec3::new(500.0, 0.0, 0.0))],
                grenade_request: None,
                look_at_override: None,
                profile: &prof,
                difficulty: BotDifficulty::Expert,
                restrictions: &restrictions,
                rng_seed: 4,
            },
        );
        assert!(out_empty.needs_reload_in_cover);
        assert_eq!(out_empty.desired_weapon, Some(BotWeapon::Usp));

        // Flashblind scatters aim away from 0 yaw.
        let mut flash_st = BotCombatState::default();
        for i in 0..10 {
            let (fy, fp) = (flash_st.yaw_deg, flash_st.pitch_deg);
            step_combat(
                &mut flash_st,
                &CombatInput {
                    dt: 0.05,
                    self_pos: Vec3::ZERO,
                    self_eye: Vec3::new(0.0, 0.0, 64.0),
                    self_vel: Vec3::ZERO,
                    current_yaw_deg: fy,
                    current_pitch_deg: fp,
                    punch_angle_deg: Vec3::ZERO,
                    current_fov: 90,
                    on_ground: true,
                    is_crouching: false,
                    health: 100.0,
                    flash_alpha: 0.95,
                    inventory: inv,
                    visible_enemies: &[enemy(1, Vec3::new(500.0, 0.0, 0.0))],
                    grenade_request: None,
                    look_at_override: None,
                    profile: &prof,
                    difficulty: BotDifficulty::Expert,
                    restrictions: &restrictions,
                    rng_seed: 100 + i,
                },
            );
        }
        assert!(flash_st.blind_scatter_yaw.abs() > 1.0);
    }

    #[test]
    fn grenade_throw_completes_pin_pull_and_release() {
        let prof = BotProfile::fallback("Viper", BotDifficulty::Expert);
        let restrictions = WeaponRestrictions::default();
        let mut st = BotCombatState::default();
        let inv = CombatInventory {
            primary: Some(BotWeapon::M4a1),
            primary_clip: 30,
            primary_reserve: 90,
            active_weapon: Some(BotWeapon::HeGrenade),
            he_count: 1,
            ..Default::default()
        };

        let mut thrown = None;
        for tick in 0..25 {
            let (sy, sp) = (st.yaw_deg, st.pitch_deg);
            let out = step_combat(
                &mut st,
                &CombatInput {
                    dt: 0.05,
                    self_pos: Vec3::ZERO,
                    self_eye: Vec3::new(0.0, 0.0, 64.0),
                    self_vel: Vec3::ZERO,
                    current_yaw_deg: sy,
                    current_pitch_deg: sp,
                    punch_angle_deg: Vec3::ZERO,
                    current_fov: 90,
                    on_ground: true,
                    is_crouching: false,
                    health: 100.0,
                    flash_alpha: 0.0,
                    inventory: inv,
                    visible_enemies: &[],
                    grenade_request: if tick == 0 {
                        Some(GrenadeThrowRequest {
                            grenade: BotWeapon::HeGrenade,
                            target: Vec3::new(450.0, 0.0, 0.0),
                        })
                    } else {
                        None
                    },
                    look_at_override: None,
                    profile: &prof,
                    difficulty: BotDifficulty::Expert,
                    restrictions: &restrictions,
                    rng_seed: 9,
                },
            );
            if let Some(g) = out.threw_grenade {
                thrown = Some(g);
                break;
            }
        }
        assert_eq!(thrown, Some(BotWeapon::HeGrenade));
    }
}
