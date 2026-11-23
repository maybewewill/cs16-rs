//! Complete Counter-Strike 1.6 match state machine: players, combat, wallbangs, C4, economy,
//! buy system, grenades, dropped weapons, killfeed, and bots.

use std::collections::VecDeque;

use cs_assets::bsp::{BspMap, BrushVolumeKind, SpawnPoint, SpawnTeam};
use cs_assets::materials::{MaterialTable, SurfaceMaterial};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::bots::{BotBrain, BotObservation, CLASSIC_BOT_NAMES, NavGraph, think_bot};
use crate::grenades::{ActiveGrenade, GrenadeEvent, GrenadeKind, SmokeCloud, step_grenades};
use crate::pmove::{
    MoveInputCmd, MoveParams, PlayerMoveState, snap_spawn_to_floor, step_player_move,
};
use crate::trace::{BspCollision, HitGroup, Hull, raycast_player_hitboxes};
use crate::weapons::{
    TeamRestriction, WeaponId, WeaponInstance, WeaponSlot, apply_regamedll_kickback,
    compute_cs16_damage, compute_shot_spread, pm_drop_punch_angle,
};

pub const TICK_RATE: f32 = 100.0;
pub const TICK_DT: f32 = 1.0 / TICK_RATE;
pub const MAX_MONEY: i32 = 16000;
pub const KEVLAR_PRICE: i32 = 650;
pub const KEVLAR_HELMET_PRICE: i32 = 1000;
pub const DEFUSER_PRICE: i32 = 200;
pub const NVG_PRICE: i32 = 1250;
pub const C4_PLANT_TIME: f32 = 3.2;
pub const C4_DEFUSE_TIME: f32 = 10.0;
pub const C4_DEFUSE_KIT_TIME: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerModelSkin {
    Urban,
    Gsg9,
    Sas,
    Gign,
    Terror,
    Leet,
    Arctic,
    Guerilla,
}

impl PlayerModelSkin {
    pub fn model_path(self) -> &'static str {
        match self {
            Self::Urban => "models/player/urban/urban.mdl",
            Self::Gsg9 => "models/player/gsg9/gsg9.mdl",
            Self::Sas => "models/player/sas/sas.mdl",
            Self::Gign => "models/player/gign/gign.mdl",
            Self::Terror => "models/player/terror/terror.mdl",
            Self::Leet => "models/player/leet/leet.mdl",
            Self::Arctic => "models/player/arctic/arctic.mdl",
            Self::Guerilla => "models/player/guerilla/guerilla.mdl",
        }
    }

    pub fn for_team_index(team: SpawnTeam, idx: usize) -> Self {
        match team {
            SpawnTeam::CounterTerrorist => [Self::Urban, Self::Sas, Self::Gsg9, Self::Gign][idx % 4],
            SpawnTeam::Terrorist => [Self::Terror, Self::Leet, Self::Arctic, Self::Guerilla][idx % 4],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundPhase {
    FreezeTime,
    Live,
    BombPlanted,
    RoundEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewAnimKind {
    Idle,
    Draw,
    Shoot,
    Reload,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PlayerInput {
    pub move_cmd: MoveInputCmd,
    pub attack: bool,
    pub attack2: bool,
    pub reload: bool,
    pub use_key: bool,
    pub select_slot: Option<u8>,
    pub last_inv: bool,
    pub drop_weapon: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: u32,
    pub name: String,
    pub is_bot: bool,
    pub team: SpawnTeam,
    pub skin: PlayerModelSkin,
    pub alive: bool,
    pub health: i32,
    pub armor: i32,
    pub has_helmet: bool,
    pub has_defuser: bool,
    pub has_nvg: bool,
    pub nvg_active: bool,
    pub money: i32,
    pub frags: i32,
    pub deaths: i32,
    pub mv: PlayerMoveState,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub punch_pitch: f32,
    pub punch_yaw: f32,
    pub recoil_dir: i8,
    pub weapons: Vec<WeaponInstance>,
    pub active_slot: usize,
    pub prev_weapon: Option<WeaponId>,
    pub attack_cooldown: f32,
    pub reload_timer: f32,
    pub draw_timer: f32,
    pub shots_fired: u32,
    pub burst_remaining: u8,
    pub burst_timer: f32,
    pub zoom_level: u8,
    pub old_attack: bool,
    pub old_attack2: bool,
    pub old_use: bool,
    pub flash_alpha: f32,
    pub damage_flash: f32,
    pub plant_progress: f32,
    pub defuse_progress: f32,
    pub view_anim: ViewAnimKind,
    pub view_anim_time: f32,
    pub god_mode: bool,
}

impl PlayerState {
    pub fn active_weapon(&self) -> Option<&WeaponInstance> {
        self.weapons.get(self.active_slot)
    }

    pub fn active_weapon_mut(&mut self) -> Option<&mut WeaponInstance> {
        self.weapons.get_mut(self.active_slot)
    }

    pub fn active_weapon_id(&self) -> WeaponId {
        self.active_weapon().map(|w| w.id).unwrap_or(WeaponId::Knife)
    }

    pub fn has_weapon(&self, id: WeaponId) -> bool {
        self.weapons.iter().any(|w| w.id == id)
    }

    pub fn eye_pos(&self) -> Vec3 {
        self.mv.eye_pos()
    }

    pub fn forward_dir(&self) -> Vec3 {
        let yaw = (self.yaw_deg + self.punch_yaw).to_radians();
        let pitch = (self.pitch_deg + self.punch_pitch).clamp(-89.0, 89.0).to_radians();
        Vec3::new(
            yaw.cos() * pitch.cos(),
            yaw.sin() * pitch.cos(),
            -pitch.sin(),
        )
        .normalize()
    }

    pub fn weapon_max_speed(&self) -> f32 {
        let spec = self.active_weapon_id().spec();
        if self.zoom_level > 0 {
            spec.scoped_max_speed
        } else {
            spec.max_speed
        }
    }

    pub fn select_weapon(&mut self, id: WeaponId) -> bool {
        if let Some(idx) = self.weapons.iter().position(|w| w.id == id) {
            if idx != self.active_slot {
                self.prev_weapon = Some(self.active_weapon_id());
                self.active_slot = idx;
                self.reload_timer = 0.0;
                self.zoom_level = 0;
                self.shots_fired = 0;
                self.draw_timer = id.spec().draw_time;
                self.view_anim = ViewAnimKind::Draw;
                self.view_anim_time = 0.0;
            }
            true
        } else {
            false
        }
    }

    pub fn select_slot(&mut self, slot_num: u8) -> bool {
        let target_slot = match slot_num {
            1 => WeaponSlot::Primary,
            2 => WeaponSlot::Secondary,
            3 => WeaponSlot::Melee,
            4 => WeaponSlot::Grenade,
            5 => WeaponSlot::C4,
            _ => return false,
        };
        // Cycle among weapons in the requested slot (important for Slot 4 grenades!)
        let matching: Vec<usize> = self
            .weapons
            .iter()
            .enumerate()
            .filter(|(_, w)| w.id.spec().slot == target_slot)
            .map(|(i, _)| i)
            .collect();
        if matching.is_empty() {
            return false;
        }
        let next_idx = if let Some(pos) = matching.iter().position(|&i| i == self.active_slot) {
            matching[(pos + 1) % matching.len()]
        } else {
            matching[0]
        };
        let id = self.weapons[next_idx].id;
        self.select_weapon(id)
    }

    pub fn give_weapon(&mut self, id: WeaponId) {
        let slot = id.spec().slot;
        if matches!(slot, WeaponSlot::Primary | WeaponSlot::Secondary) {
            self.weapons.retain(|w| w.id.spec().slot != slot);
        } else if self.has_weapon(id) {
            // Refill ammo
            if let Some(w) = self.weapons.iter_mut().find(|w| w.id == id) {
                let spec = id.spec();
                w.clip = spec.clip_size;
                w.reserve = spec.max_reserve;
            }
            return;
        }
        self.weapons.push(WeaponInstance::new(id));
        self.weapons.sort_by_key(|w| w.id.spec().slot as u8);
        self.select_weapon(id);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlantedC4 {
    pub position: Vec3,
    pub timer: f32,
    pub beep_accum: f32,
    pub defuser_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldWeapon {
    pub id: WeaponId,
    pub clip: u16,
    pub reserve: u16,
    pub position: Vec3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactDecal {
    pub position: Vec3,
    pub normal: Vec3,
    pub is_blood: bool,
    pub is_spray: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Corpse {
    pub position: Vec3,
    pub yaw_deg: f32,
    pub skin: PlayerModelSkin,
    pub seq_name: String,
    pub anim_time: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostageState {
    pub id: u32,
    pub position: Vec3,
    pub yaw_deg: f32,
    pub health: i32,
    pub alive: bool,
    pub rescued: bool,
    pub following_player: Option<u32>,
    pub moving: bool,
}

#[derive(Debug, Clone)]
pub struct BreakableState {
    pub model_index: usize,
    pub max_health: f32,
    pub health: f32,
    pub material: i32,
    pub broken: bool,
}

#[derive(Debug, Clone)]
pub struct DoorState {
    pub model_index: usize,
    pub center: Vec3,
    pub travel: Vec3,
    pub speed: f32,
    pub wait: f32,
    pub open_frac: f32,
    pub opening: bool,
    pub hold_timer: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulletTracer {
    pub start: Vec3,
    pub end: Vec3,
    pub remaining: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillFeedEntry {
    pub killer_name: String,
    pub killer_team: SpawnTeam,
    pub victim_name: String,
    pub victim_team: SpawnTeam,
    pub weapon: String,
    pub headshot: bool,
    pub remaining: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub sender: String,
    pub team: Option<SpawnTeam>,
    pub text: String,
    pub remaining: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SimEvent {
    Sound {
        path: String,
        position: Option<Vec3>,
        volume: f32,
    },
    MuzzleFlash {
        player_id: u32,
        position: Vec3,
    },
    Explosion {
        position: Vec3,
        is_c4: bool,
    },
    Banner {
        text: String,
        duration: f32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MatchConfig {
    pub round_time_sec: f32,
    pub freeze_time_sec: f32,
    pub c4_timer_sec: f32,
    pub start_money: i32,
    pub friendly_fire: bool,
    pub bot_difficulty: u8,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            round_time_sec: 105.0,
            freeze_time_sec: 4.0,
            c4_timer_sec: 45.0,
            start_money: 800,
            friendly_fire: false,
            bot_difficulty: 1,
        }
    }
}

/// Complete authoritative CS 1.6 match world (shared by offline singleplayer, `cs-server`, and tests).
pub struct MatchWorld {
    pub map_name: String,
    pub collision: BspCollision,
    pub spawns: Vec<SpawnPoint>,
    pub hostage_spawns: Vec<(Vec3, f32)>,
    pub nav: NavGraph,
    pub move_params: MoveParams,
    pub config: MatchConfig,

    pub tick: u64,
    pub phase: RoundPhase,
    pub phase_timer: f32,
    pub round_number: u32,
    pub ct_score: u32,
    pub t_score: u32,
    ct_loss_streak: u32,
    t_loss_streak: u32,

    pub players: Vec<PlayerState>,
    bot_brains: Vec<(u32, BotBrain)>,
    next_id: u32,
    next_grenade_id: u32,

    pub planted_c4: Option<PlantedC4>,
    pub grenades: Vec<ActiveGrenade>,
    pub smokes: Vec<SmokeCloud>,
    pub dropped_weapons: Vec<WorldWeapon>,
    pub corpses: Vec<Corpse>,
    pub hostages: Vec<HostageState>,
    pub breakables: Vec<BreakableState>,
    pub doors: Vec<DoorState>,
    pub broken_models: Vec<usize>,
    pub door_offsets: Vec<(usize, Vec3)>,
    pub decals: VecDeque<ImpactDecal>,
    pub tracers: Vec<BulletTracer>,
    pub killfeed: Vec<KillFeedEntry>,
    pub chat: Vec<ChatMessage>,
    pub banner: Option<(String, f32)>,
    pub events: Vec<SimEvent>,

    rng_state: u64,
    /// Optional callback/table for floor textures (copied lightly as `(leaf_idx -> SurfaceMaterial)`).
    leaf_materials: Vec<SurfaceMaterial>,
    map_nodes_leafs_for_mat: BspMapLightRef,
}

struct BspMapLightRef {
    planes: Vec<cs_assets::bsp::Plane>,
    nodes: Vec<cs_assets::bsp::Node>,
    head_node: i32,
}

impl BspMapLightRef {
    fn from_map(map: &BspMap) -> Self {
        Self {
            planes: map.planes.clone(),
            nodes: map.nodes.clone(),
            head_node: map.models.first().map(|m| m.head_nodes[0]).unwrap_or(0),
        }
    }

    fn find_leaf(&self, p: Vec3) -> usize {
        if self.nodes.is_empty() {
            return 0;
        }
        let mut idx = self.head_node;
        let mut guard = 0;
        while idx >= 0 && guard < 2048 {
            guard += 1;
            let Some(n) = self.nodes.get(idx as usize) else {
                return 0;
            };
            let Some(pl) = self.planes.get(n.plane as usize) else {
                return 0;
            };
            let d = pl.normal.dot(p) - pl.dist;
            idx = if d >= 0.0 {
                n.children[0] as i32
            } else {
                n.children[1] as i32
            };
        }
        (-idx - 1).max(0) as usize
    }
}

impl MatchWorld {
    pub fn new(map: &BspMap, materials: MaterialTable, config: MatchConfig) -> Self {
        // Precompute floor SurfaceMaterial per BSP leaf for O(1) footstep lookup!
        let mut leaf_materials = vec![SurfaceMaterial::Concrete; map.leafs.len()];
        for (i, leaf) in map.leafs.iter().enumerate() {
            let first = leaf.first_mark_surface as usize;
            let count = leaf.num_mark_surfaces as usize;
            let end = first.saturating_add(count).min(map.mark_surfaces.len());
            let mut best_up = 0.5f32;
            let mut mat = SurfaceMaterial::Concrete;
            for &surf_idx in &map.mark_surfaces[first..end] {
                if let Some(face) = map.faces.get(surf_idx as usize)
                    && let Some(plane) = map.planes.get(face.plane as usize)
                {
                    let n = if face.plane_side != 0 {
                        -plane.normal
                    } else {
                        plane.normal
                    };
                    if n.z > best_up
                        && let Some(ti) = map.texinfos.get(face.texinfo as usize)
                        && ti.miptex >= 0
                        && let Some(tex) = map.textures.get(ti.miptex as usize)
                    {
                        best_up = n.z;
                        mat = materials.lookup(&tex.name);
                    }
                }
            }
            leaf_materials[i] = mat;
        }

        let collision = BspCollision::from_map(map, materials);
        let mut nav_seeds: Vec<Vec3> = map.spawns.iter().map(|s| s.position).collect();
        nav_seeds.extend(collision.bomb_target_centers());
        nav_seeds.extend(collision.hostage_rescue_centers());
        nav_seeds.extend(map.hostage_spawns.iter().map(|(p, _)| *p));
        let nav = NavGraph::build(&collision, &nav_seeds);

        let breakables: Vec<BreakableState> = collision
            .breakables()
            .iter()
            .map(|b| {
                let (h, m) = match b.kind {
                    BrushVolumeKind::Breakable { health, material } => (health, material),
                    _ => (50.0, 0),
                };
                BreakableState {
                    model_index: b.model_index,
                    max_health: h,
                    health: h,
                    material: m,
                    broken: false,
                }
            })
            .collect();

        let doors: Vec<DoorState> = collision
            .doors()
            .iter()
            .map(|d| {
                let (speed, lip, move_dir, wait) = match d.kind {
                    BrushVolumeKind::Door {
                        speed,
                        lip,
                        move_dir,
                        wait,
                        ..
                    } => (speed, lip, move_dir, wait),
                    _ => (100.0, 8.0, Vec3::Z, 4.0),
                };
                let size = (d.maxs - d.mins).abs();
                let dist = (size.x * move_dir.x.abs()
                    + size.y * move_dir.y.abs()
                    + size.z * move_dir.z.abs()
                    - lip)
                    .max(32.0);
                DoorState {
                    model_index: d.model_index,
                    center: (d.mins + d.maxs) * 0.5,
                    travel: move_dir.normalize_or_zero() * dist,
                    speed,
                    wait,
                    open_frac: 0.0,
                    opening: false,
                    hold_timer: 0.0,
                }
            })
            .collect();

        Self {
            map_name: map.name.clone(),
            collision,
            spawns: map.spawns.clone(),
            hostage_spawns: map.hostage_spawns.clone(),
            nav,
            move_params: MoveParams::default(),
            config,
            tick: 0,
            phase: RoundPhase::Live,
            phase_timer: config.round_time_sec,
            round_number: 0,
            ct_score: 0,
            t_score: 0,
            ct_loss_streak: 0,
            t_loss_streak: 0,
            players: Vec::new(),
            bot_brains: Vec::new(),
            next_id: 1,
            next_grenade_id: 1,
            planted_c4: None,
            grenades: Vec::new(),
            smokes: Vec::new(),
            dropped_weapons: Vec::new(),
            corpses: Vec::new(),
            hostages: Vec::new(),
            breakables,
            doors,
            broken_models: Vec::new(),
            door_offsets: Vec::new(),
            decals: VecDeque::with_capacity(256),
            tracers: Vec::new(),
            killfeed: Vec::new(),
            chat: Vec::new(),
            banner: None,
            events: Vec::new(),
            rng_state: 0xC516_2026_1337_BEEF,
            leaf_materials,
            map_nodes_leafs_for_mat: BspMapLightRef::from_map(map),
        }
    }

    pub fn add_player(&mut self, name: impl Into<String>, team: SpawnTeam, is_bot: bool) -> u32 {
        let id = self.next_id;
        self.next_id += 1;

        let team_count = self.players.iter().filter(|p| p.team == team).count();
        let skin = PlayerModelSkin::for_team_index(team, team_count);
        let (spawn_pos, spawn_yaw) = self.pick_spawn(team, team_count);

        let mut p = PlayerState {
            id,
            name: name.into(),
            is_bot,
            team,
            skin,
            alive: true,
            health: 100,
            armor: 0,
            has_helmet: false,
            has_defuser: false,
            has_nvg: false,
            nvg_active: false,
            money: self.config.start_money,
            frags: 0,
            deaths: 0,
            mv: PlayerMoveState::at_spawn(spawn_pos),
            yaw_deg: spawn_yaw,
            pitch_deg: 0.0,
            punch_pitch: 0.0,
            punch_yaw: 0.0,
            recoil_dir: 1,
            weapons: Vec::new(),
            active_slot: 0,
            prev_weapon: None,
            attack_cooldown: 0.0,
            reload_timer: 0.0,
            draw_timer: 0.0,
            shots_fired: 0,
            burst_remaining: 0,
            burst_timer: 0.0,
            zoom_level: 0,
            old_attack: false,
            old_attack2: false,
            old_use: false,
            flash_alpha: 0.0,
            damage_flash: 0.0,
            plant_progress: 0.0,
            defuse_progress: 0.0,
            view_anim: ViewAnimKind::Draw,
            view_anim_time: 0.0,
            god_mode: false,
        };
        give_starting_loadout(&mut p);

        self.players.push(p);
        if is_bot {
            self.bot_brains.push((id, BotBrain::default()));
        }
        id
    }

    pub fn add_bot(&mut self, team: Option<SpawnTeam>) -> u32 {
        let ct = self
            .players
            .iter()
            .filter(|p| p.team == SpawnTeam::CounterTerrorist)
            .count();
        let t = self
            .players
            .iter()
            .filter(|p| p.team == SpawnTeam::Terrorist)
            .count();
        let chosen = team.unwrap_or(if ct <= t {
            SpawnTeam::CounterTerrorist
        } else {
            SpawnTeam::Terrorist
        });
        let bot_idx = self.players.iter().filter(|p| p.is_bot).count();
        let name = CLASSIC_BOT_NAMES[bot_idx % CLASSIC_BOT_NAMES.len()];
        let id = self.add_player(format!("[BOT] {name}"), chosen, true);
        if self.phase == RoundPhase::FreezeTime || self.phase == RoundPhase::Live {
            self.bot_buy_round(id);
        }
        id
    }

    pub fn kick_all_bots(&mut self) {
        self.players.retain(|p| !p.is_bot);
        self.bot_brains.clear();
    }

    pub fn set_bot_quota(&mut self, total_bots: usize) {
        let cur = self.players.iter().filter(|p| p.is_bot).count();
        if total_bots > cur {
            for _ in cur..total_bots {
                self.add_bot(None);
            }
        } else if total_bots < cur {
            let mut to_remove = cur - total_bots;
            self.players.retain(|p| {
                if p.is_bot && to_remove > 0 {
                    to_remove -= 1;
                    false
                } else {
                    true
                }
            });
            let alive_ids: Vec<u32> = self.players.iter().map(|p| p.id).collect();
            self.bot_brains.retain(|(id, _)| alive_ids.contains(id));
        }
    }

    pub fn player(&self, id: u32) -> Option<&PlayerState> {
        self.players.iter().find(|p| p.id == id)
    }

    pub fn player_mut(&mut self, id: u32) -> Option<&mut PlayerState> {
        self.players.iter_mut().find(|p| p.id == id)
    }

    /// Restart match (`sv_restart 1`): resets scores, money, and starts Round 1 cleanly.
    pub fn restart_match(&mut self) {
        self.ct_score = 0;
        self.t_score = 0;
        self.ct_loss_streak = 0;
        self.t_loss_streak = 0;
        self.round_number = 0;
        for p in &mut self.players {
            p.money = self.config.start_money;
            p.frags = 0;
            p.deaths = 0;
            p.armor = 0;
            p.has_helmet = false;
            p.has_defuser = false;
            p.weapons.clear();
            give_starting_loadout(p);
        }
        self.start_new_round();
    }

    pub fn start_new_round(&mut self) {
        self.round_number += 1;
        self.phase = if self.config.freeze_time_sec > 0.0 {
            RoundPhase::FreezeTime
        } else {
            RoundPhase::Live
        };
        self.phase_timer = if self.phase == RoundPhase::FreezeTime {
            self.config.freeze_time_sec
        } else {
            self.config.round_time_sec
        };
        self.planted_c4 = None;
        self.grenades.clear();
        self.smokes.clear();
        self.dropped_weapons.clear();
        self.corpses.clear();
        self.tracers.clear();

        // Reset breakable glass/vents & doors each round!
        self.collision.disabled_models.clear();
        self.broken_models.clear();
        self.door_offsets.clear();
        for b in &mut self.breakables {
            b.health = b.max_health;
            b.broken = false;
        }
        for d in &mut self.doors {
            d.open_frac = 0.0;
            d.opening = false;
            d.hold_timer = 0.0;
        }

        // Respawn hostages on `cs_*` maps!
        self.hostages.clear();
        for (i, &(pos, yaw)) in self.hostage_spawns.iter().enumerate() {
            let grounded = snap_spawn_to_floor(pos, &self.collision);
            self.hostages.push(HostageState {
                id: (i as u32) + 1,
                position: grounded,
                yaw_deg: yaw,
                health: 100,
                alive: true,
                rescued: false,
                following_player: None,
                moving: false,
            });
        }

        // Respawn all players cleanly at team spawns
        let mut ct_idx = 0usize;
        let mut t_idx = 0usize;
        let spawn_list: Vec<(u32, Vec3, f32)> = self
            .players
            .iter()
            .map(|p| {
                let idx = match p.team {
                    SpawnTeam::CounterTerrorist => {
                        let i = ct_idx;
                        ct_idx += 1;
                        i
                    }
                    SpawnTeam::Terrorist => {
                        let i = t_idx;
                        t_idx += 1;
                        i
                    }
                };
                let (pos, yaw) = self.pick_spawn(p.team, idx);
                (p.id, pos, yaw)
            })
            .collect();

        let round_num = self.round_number;
        for (id, pos, yaw) in spawn_list {
            if let Some(p) = self.player_mut(id) {
                let survived = p.alive && round_num > 1;
                p.alive = true;
                p.health = 100;
                p.mv = PlayerMoveState::at_spawn(pos);
                p.yaw_deg = yaw;
                p.pitch_deg = 0.0;
                p.punch_pitch = 0.0;
                p.punch_yaw = 0.0;
                p.flash_alpha = 0.0;
                p.damage_flash = 0.0;
                p.plant_progress = 0.0;
                p.defuse_progress = 0.0;
                p.reload_timer = 0.0;
                p.attack_cooldown = 0.0;
                p.zoom_level = 0;
                p.weapons.retain(|w| w.id != WeaponId::C4);

                if !survived {
                    p.armor = 0;
                    p.has_helmet = false;
                    p.has_defuser = false;
                    p.weapons.clear();
                    give_starting_loadout(p);
                } else {
                    // Refill clips of surviving weapons
                    for w in &mut p.weapons {
                        let spec = w.id.spec();
                        w.clip = spec.clip_size;
                    }
                }
            }
        }

        // Assign C4 to one Terrorist if the map has bomb targets
        if !self.collision.bomb_target_centers().is_empty() {
            let t_ids: Vec<u32> = self
                .players
                .iter()
                .filter(|p| p.team == SpawnTeam::Terrorist)
                .map(|p| p.id)
                .collect();
            if !t_ids.is_empty() {
                let carrier = t_ids[(self.round_number as usize) % t_ids.len()];
                if let Some(p) = self.player_mut(carrier) {
                    p.weapons.push(WeaponInstance::new(WeaponId::C4));
                    p.weapons.sort_by_key(|w| w.id.spec().slot as u8);
                }
            }
        }

        // Let all bots shop for the new round
        let bot_ids: Vec<u32> = self
            .players
            .iter()
            .filter(|p| p.is_bot)
            .map(|p| p.id)
            .collect();
        for bid in bot_ids {
            self.bot_buy_round(bid);
        }

        // Authentic CS 1.6 starts rounds cleanly without a big "Round N" center banner
        self.banner = None;
    }

    /// Process a buy command (weapon name or equipment: `vest`, `vesthelm`, `defuser`, `primammo`, `secammo`).
    pub fn buy_item(&mut self, player_id: u32, item: &str) -> Result<String, String> {
        let Some(p) = self.player(player_id) else {
            return Err("Player not found".into());
        };
        if !p.alive {
            return Err("Cannot buy while dead".into());
        }
        if !self.collision.is_in_buy_zone(p.mv.origin, p.team) {
            return Err("You are not in a buy zone!".into());
        }
        let team = p.team;
        let money = p.money;
        let item_l = item.trim().to_ascii_lowercase();

        match item_l.as_str() {
            "vest" | "kevlar" => {
                if money < KEVLAR_PRICE {
                    return Err("Insufficient funds ($650)".into());
                }
                if let Some(p) = self.player_mut(player_id) {
                    if p.armor >= 100 {
                        return Err("You already have full Kevlar".into());
                    }
                    p.money -= KEVLAR_PRICE;
                    p.armor = 100;
                }
                self.emit_sound("items/kevlar.wav", None, 0.8);
                return Ok("Bought Kevlar Vest".into());
            }
            "vesthelm" | "kevlarhelmet" => {
                let cost = if p.armor >= 100 && !p.has_helmet {
                    350
                } else {
                    KEVLAR_HELMET_PRICE
                };
                if money < cost {
                    return Err(format!("Insufficient funds (${cost})"));
                }
                if let Some(p) = self.player_mut(player_id) {
                    if p.armor >= 100 && p.has_helmet {
                        return Err("You already have Kevlar + Helmet".into());
                    }
                    p.money -= cost;
                    p.armor = 100;
                    p.has_helmet = true;
                }
                self.emit_sound("items/kevlar.wav", None, 0.8);
                return Ok("Bought Kevlar + Helmet".into());
            }
            "defuser" | "defusekit" => {
                if team != SpawnTeam::CounterTerrorist {
                    return Err("Only Counter-Terrorists can buy a Defuse Kit".into());
                }
                if money < DEFUSER_PRICE {
                    return Err("Insufficient funds ($200)".into());
                }
                if let Some(p) = self.player_mut(player_id) {
                    if p.has_defuser {
                        return Err("You already have a Defuse Kit".into());
                    }
                    p.money -= DEFUSER_PRICE;
                    p.has_defuser = true;
                }
                self.emit_sound("items/gunpickup2.wav", None, 0.8);
                return Ok("Bought Defuse Kit".into());
            }
            "nvgs" | "nightvision" => {
                if money < NVG_PRICE {
                    return Err("Insufficient funds ($1250)".into());
                }
                if let Some(p) = self.player_mut(player_id) {
                    if p.has_nvg {
                        return Err("You already have NightVision Goggles".into());
                    }
                    p.money -= NVG_PRICE;
                    p.has_nvg = true;
                }
                self.emit_sound("items/gunpickup2.wav", None, 0.8);
                return Ok("Bought NightVision Goggles (Press N)".into());
            }
            "primammo" | "secammo" => {
                let target_slot = if item_l == "primammo" {
                    WeaponSlot::Primary
                } else {
                    WeaponSlot::Secondary
                };
                if money < 60 {
                    return Err("Insufficient funds ($60)".into());
                }
                if let Some(p) = self.player_mut(player_id)
                    && let Some(w) = p
                        .weapons
                        .iter_mut()
                        .find(|w| w.id.spec().slot == target_slot)
                {
                    let max_r = w.id.spec().max_reserve;
                    if w.reserve >= max_r {
                        return Err("Ammo is already full".into());
                    }
                    w.reserve = max_r;
                    p.money -= 60;
                    self.emit_sound("items/ammopickup2.wav", None, 0.8);
                    return Ok("Refilled ammo".into());
                }
                return Err("No weapon in that slot".into());
            }
            _ => {}
        }

        let Some(wid) = WeaponId::from_console_name(&item_l) else {
            return Err(format!("Unknown buy item: {item}"));
        };
        let spec = wid.spec();
        if spec.price <= 0 {
            return Err(format!("{} cannot be bought", spec.display_name));
        }
        let team_ok = match spec.team {
            TeamRestriction::Any => true,
            TeamRestriction::Terrorist => team == SpawnTeam::Terrorist,
            TeamRestriction::CounterTerrorist => team == SpawnTeam::CounterTerrorist,
        };
        if !team_ok {
            return Err(format!("{} is not available for your team", spec.display_name));
        }
        if money < spec.price {
            return Err(format!("Insufficient funds (${} needed)", spec.price));
        }

        if let Some(p) = self.player_mut(player_id) {
            if wid.is_grenade() && p.has_weapon(wid) {
                return Err(format!("Already carrying {}", spec.display_name));
            }
            p.money -= spec.price;
            p.give_weapon(wid);
        }
        self.emit_sound("items/gunpickup2.wav", None, 0.85);
        Ok(format!("Bought {}", spec.display_name))
    }

    /// Give any weapon directly (cheat/testing `give ak47`, `give awp`, etc.).
    pub fn give_weapon_cheat(&mut self, player_id: u32, name: &str) -> Result<String, String> {
        let Some(wid) = WeaponId::from_console_name(name) else {
            return Err(format!("Unknown weapon: {name}"));
        };
        if let Some(p) = self.player_mut(player_id) {
            p.give_weapon(wid);
            Ok(format!("Gave {}", wid.spec().display_name))
        } else {
            Err("Player not found".into())
        }
    }

    pub fn toggle_nightvision(&mut self, player_id: u32) {
        let Some(p) = self.player_mut(player_id) else {
            return;
        };
        if !p.alive || !p.has_nvg {
            return;
        }
        p.nvg_active = !p.nvg_active;
        let snd = if p.nvg_active {
            "items/nvg_on.wav"
        } else {
            "items/nvg_off.wav"
        };
        self.emit_sound(snd, None, 0.8);
    }

    pub fn send_radio(&mut self, player_id: u32, message: &str, wav_path: &str) {
        let Some(p) = self.player(player_id) else {
            return;
        };
        if !p.alive {
            return;
        }
        let sender = format!("{} (RADIO)", p.name);
        let team = p.team;
        self.push_chat(sender, Some(team), message);
        self.emit_sound(wav_path, None, 0.9);
    }

    pub fn spray_logo(&mut self, player_id: u32) {
        let Some(p) = self.player(player_id) else {
            return;
        };
        if !p.alive {
            return;
        }
        let eye = p.eye_pos();
        let end = eye + p.forward_dir() * 128.0;
        let tr = self.collision.trace_hull(eye, end, Hull::Point);
        if tr.fraction < 1.0 && !tr.hit_sky {
            if self.decals.len() >= 256 {
                self.decals.pop_front();
            }
            self.decals.push_back(ImpactDecal {
                position: tr.end_pos + tr.plane_normal * 0.6,
                normal: tr.plane_normal,
                is_blood: false,
                is_spray: true,
            });
            self.emit_sound("player/sprayer.wav", Some(tr.end_pos), 0.85);
        }
    }

    pub fn push_chat(&mut self, sender: impl Into<String>, team: Option<SpawnTeam>, text: impl Into<String>) {
        self.chat.push(ChatMessage {
            sender: sender.into(),
            team,
            text: text.into(),
            remaining: 10.0,
        });
        if self.chat.len() > 8 {
            self.chat.remove(0);
        }
    }

    pub fn take_events(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.events)
    }

    /// Advance the entire match simulation by one fixed `dt` tick (`0.01s` at 100 Hz).
    pub fn step(&mut self, human_inputs: &[(u32, PlayerInput)], dt: f32) {
        self.tick += 1;

        // Decay transient visual lists
        for t in &mut self.tracers {
            t.remaining -= dt;
        }
        self.tracers.retain(|t| t.remaining > 0.0);

        for kf in &mut self.killfeed {
            kf.remaining -= dt;
        }
        self.killfeed.retain(|k| k.remaining > 0.0);

        for msg in &mut self.chat {
            msg.remaining -= dt;
        }
        self.chat.retain(|m| m.remaining > 0.0);

        if let Some((_, ref mut rem)) = self.banner {
            *rem -= dt;
            if *rem <= 0.0 {
                self.banner = None;
            }
        }

        // 1. Round phase timers + corpses + doors + hostages
        self.step_round_phase(dt);
        for c in &mut self.corpses {
            c.anim_time += dt;
        }
        self.step_doors(dt);
        self.step_hostages(dt);

        // 2. Collect inputs for every player (humans from caller, bots from AI)
        let mut all_inputs: Vec<(u32, PlayerInput)> = Vec::with_capacity(self.players.len());
        let bomb_sites = self.collision.bomb_target_centers();
        let bomb_targets = [bomb_sites.first().copied(), bomb_sites.get(1).copied()];
        let planted_pos = self.planted_c4.as_ref().map(|c| c.position);

        let player_snapshots: Vec<(u32, SpawnTeam, bool, Vec3, Vec3)> = self
            .players
            .iter()
            .map(|p| (p.id, p.team, p.alive, p.mv.origin, p.eye_pos()))
            .collect();

        for i in 0..self.players.len() {
            if !self.players[i].alive {
                continue;
            }
            let pid = self.players[i].id;
            if !self.players[i].is_bot {
                let inp = human_inputs
                    .iter()
                    .find(|(id, _)| *id == pid)
                    .map(|(_, inp)| *inp)
                    .unwrap_or_default();
                all_inputs.push((pid, inp));
            } else {
                let p_team = self.players[i].team;
                let enemies: Vec<(u32, Vec3, Vec3)> = player_snapshots
                    .iter()
                    .filter(|(_, team, alive, _, _)| *alive && *team != p_team)
                    .map(|&(id, _, _, pos, eye)| (id, pos, eye))
                    .collect();
                let p = &self.players[i];
                let obs = BotObservation {
                    self_id: p.id,
                    team: p.team,
                    pos: p.mv.origin,
                    eye: p.eye_pos(),
                    yaw_deg: p.yaw_deg,
                    pitch_deg: p.pitch_deg,
                    has_c4: p.has_weapon(WeaponId::C4),
                    in_bomb_zone: self.collision.is_in_bomb_target(p.mv.origin),
                    clip: p.active_weapon().map(|w| w.clip).unwrap_or(0),
                    reserve: p.active_weapon().map(|w| w.reserve).unwrap_or(0),
                    flash_alpha: p.flash_alpha,
                    bomb_planted_pos: planted_pos,
                    bomb_targets,
                    priority_goal: None,
                    difficulty: self.config.bot_difficulty,
                };
                let seed = self.next_rand_u64();
                let decision = if let Some((_, brain)) =
                    self.bot_brains.iter_mut().find(|(id, _)| *id == pid)
                {
                    think_bot(
                        brain,
                        &obs,
                        &enemies,
                        &self.nav,
                        &self.collision,
                        &self.smokes,
                        seed,
                        dt,
                    )
                } else {
                    Default::default()
                };

                all_inputs.push((
                    pid,
                    PlayerInput {
                        move_cmd: MoveInputCmd {
                            forward: decision.forward,
                            right: decision.right,
                            yaw_deg: decision.yaw_deg,
                            pitch_deg: decision.pitch_deg,
                            jump: decision.jump,
                            duck: decision.duck,
                            walk: false,
                        },
                        attack: decision.attack,
                        attack2: false,
                        reload: decision.reload,
                        use_key: decision.use_key,
                        select_slot: if decision.want_c4 { Some(5) } else { None },
                        last_inv: false,
                        drop_weapon: false,
                    },
                ));
            }
        }

        // 3. Step every alive player (movement, weapons, C4 plant/defuse, dropped pickup)
        let frozen = self.phase == RoundPhase::FreezeTime;
        for (pid, inp) in all_inputs {
            self.step_single_player(pid, inp, frozen, dt);
        }

        // 4. Step planted C4 countdown & beeps
        self.step_planted_c4(dt);

        // 5. Step active grenades & smoke clouds
        let g_events = step_grenades(
            &mut self.grenades,
            &mut self.smokes,
            &self.collision,
            self.move_params.gravity,
            dt,
        );
        for ev in g_events {
            match ev {
                GrenadeEvent::Bounce { pos, .. } => {
                    self.emit_sound("weapons/he_bounce-1.wav", Some(pos), 0.55);
                }
                GrenadeEvent::Detonate {
                    owner_id,
                    kind,
                    pos,
                } => self.handle_grenade_detonation(owner_id, kind, pos),
            }
        }

        // 6. Check win conditions (team elimination) when round is active
        if matches!(self.phase, RoundPhase::Live | RoundPhase::BombPlanted) {
            self.check_round_win_conditions();
        }
    }

    fn step_round_phase(&mut self, dt: f32) {
        self.phase_timer -= dt;
        match self.phase {
            RoundPhase::FreezeTime => {
                if self.phase_timer <= 0.0 {
                    self.phase = RoundPhase::Live;
                    self.phase_timer = self.config.round_time_sec;
                    let start_snd = ["radio/locknload.wav", "radio/letsgo.wav", "radio/moveout.wav"]
                        [(self.round_number as usize) % 3];
                    self.emit_sound(start_snd, None, 0.9);
                }
            }
            RoundPhase::Live => {
                if self.phase_timer <= 0.0 {
                    self.end_round(SpawnTeam::CounterTerrorist, "Target Has Been Saved!");
                }
            }
            RoundPhase::BombPlanted => {
                // Controlled by planted C4 timer
            }
            RoundPhase::RoundEnd => {
                if self.phase_timer <= 0.0 {
                    self.start_new_round();
                }
            }
        }
    }

    fn step_single_player(&mut self, pid: u32, inp: PlayerInput, frozen: bool, dt: f32) {
        let Some(idx) = self.players.iter().position(|p| p.id == pid) else {
            return;
        };

        // Update look angles, recoil recovery, flash fade, timers
        let (max_speed, mut move_state, old_weapon_id) = {
            let p = &mut self.players[idx];
            p.yaw_deg = inp.move_cmd.yaw_deg;
            p.pitch_deg = inp.move_cmd.pitch_deg.clamp(-89.0, 89.0);
            p.flash_alpha = (p.flash_alpha - dt * 0.28).max(0.0);
            p.damage_flash = (p.damage_flash - dt * 2.5).max(0.0);

            // Exact GoldSrc `PM_DropPunchAngle` smooth spring recovery
            pm_drop_punch_angle(&mut p.punch_pitch, &mut p.punch_yaw, dt);

            p.attack_cooldown = (p.attack_cooldown - dt).max(0.0);
            p.draw_timer = (p.draw_timer - dt).max(0.0);
            p.view_anim_time += dt;

            // Reset shots_fired when not firing for >0.25s
            if !inp.attack && p.attack_cooldown <= 0.0 && p.burst_remaining == 0 {
                p.shots_fired = 0;
            }

            // Finish reload timer (supports M3 / XM1014 shell-by-shell reload!)
            if p.reload_timer > 0.0 {
                p.reload_timer -= dt;
                if p.reload_timer <= 0.0 {
                    p.reload_timer = 0.0;
                    let mut continue_shell_reload = None;
                    if let Some(w) = p.active_weapon_mut() {
                        let spec = w.id.spec();
                        if spec.shell_reload {
                            if w.clip < spec.clip_size && w.reserve > 0 {
                                w.clip += 1;
                                w.reserve -= 1;
                                if w.clip < spec.clip_size && w.reserve > 0 {
                                    continue_shell_reload = Some(spec.reload_time);
                                }
                            }
                        } else {
                            let need = spec.clip_size.saturating_sub(w.clip);
                            let take = need.min(w.reserve);
                            w.clip += take;
                            w.reserve -= take;
                        }
                    }
                    if let Some(next_shell_t) = continue_shell_reload {
                        p.reload_timer = next_shell_t;
                        p.view_anim = ViewAnimKind::Reload;
                        p.view_anim_time = 0.0;
                    } else {
                        p.view_anim = ViewAnimKind::Idle;
                        p.view_anim_time = 0.0;
                    }
                }
            } else if p.draw_timer <= 0.0 && p.view_anim == ViewAnimKind::Draw {
                p.view_anim = ViewAnimKind::Idle;
                p.view_anim_time = 0.0;
            }

            // Weapon selection / quick-switch (`Q`)
            if let Some(slot) = inp.select_slot {
                p.select_slot(slot);
            } else if inp.last_inv
                && let Some(prev) = p.prev_weapon
            {
                p.select_weapon(prev);
            }

            (p.weapon_max_speed(), p.mv, p.active_weapon_id())
        };

        // Drop active primary/secondary/C4 weapon (`G`)
        if inp.drop_weapon && !frozen {
            self.drop_active_weapon(pid);
        }

        // Player movement (`pm_shared`) unless frozen during FreezeTime or actively planting/defusing
        let planting_or_defusing =
            self.players[idx].plant_progress > 0.0 || self.players[idx].defuse_progress > 0.0;

        if !frozen && !planting_or_defusing {
            let ev = step_player_move(
                &mut move_state,
                &inp.move_cmd,
                max_speed,
                &self.move_params,
                &self.collision,
                dt,
            );
            self.players[idx].mv = move_state;

            if ev.footstep {
                let mat = self.surface_material_at(move_state.origin - Vec3::new(0.0, 0.0, 40.0));
                let step_sound = footstep_sound_for_material(mat, self.next_rand_u64());
                self.emit_sound(step_sound, Some(move_state.origin), 0.45);
            }
            if ev.ladder_step {
                let s = ["player/pl_ladder1.wav", "player/pl_ladder2.wav"]
                    [(self.next_rand_u64() as usize) % 2];
                self.emit_sound(s, Some(move_state.origin), 0.5);
            }
            if ev.fall_damage > 0.0 && !self.players[idx].god_mode {
                let dmg = ev.fall_damage.round() as i32;
                self.players[idx].health -= dmg;
                self.players[idx].damage_flash = 0.7;
                self.emit_sound("player/pl_fallpain1.wav", Some(move_state.origin), 0.75);
                if self.players[idx].health <= 0 {
                    self.kill_player(pid, pid, "world", false);
                    return;
                }
            }
        }

        // Auto-pickup dropped weapons (primary/C4 if slot empty)
        self.try_pickup_dropped_weapons(pid);

        // Reload trigger (`R`)
        if inp.reload {
            let can_reload = {
                let p = &self.players[idx];
                p.reload_timer <= 0.0
                    && p.draw_timer <= 0.0
                    && p.active_weapon().is_some_and(|w| {
                        let spec = w.id.spec();
                        spec.clip_size > 0 && w.clip < spec.clip_size && w.reserve > 0
                    })
            };
            if can_reload {
                let (reload_time, reload_sound, pos) = {
                    let p = &mut self.players[idx];
                    let spec = p.active_weapon_id().spec();
                    p.reload_timer = spec.reload_time;
                    p.zoom_level = 0;
                    p.view_anim = ViewAnimKind::Reload;
                    p.view_anim_time = 0.0;
                    (spec.reload_time, spec.reload_sound, p.mv.origin)
                };
                if reload_time > 0.0 && !reload_sound.is_empty() {
                    self.emit_sound(reload_sound, Some(pos), 0.7);
                }
            }
        }

        // Secondary attack (`MOUSE2`): Zoom / Silencer / Knife heavy stab
        let attack2_edge = inp.attack2 && !self.players[idx].old_attack2;
        if attack2_edge && !frozen {
            let wid = self.players[idx].active_weapon_id();
            let spec = wid.spec();
            if spec.has_scope {
                let p = &mut self.players[idx];
                let max_zoom = if spec.double_zoom { 2 } else { 1 };
                p.zoom_level = (p.zoom_level + 1) % (max_zoom + 1);
                let pos = p.mv.origin;
                self.emit_sound("weapons/zoom.wav", Some(pos), 0.6);
            } else if spec.has_silencer {
                let (pos, new_sil) = {
                    let p = &mut self.players[idx];
                    let origin = p.mv.origin;
                    if let Some(w) = p.active_weapon_mut() {
                        w.silenced = !w.silenced;
                        (origin, w.silenced)
                    } else {
                        (origin, false)
                    }
                };
                let snd = match (wid, new_sil) {
                    (WeaponId::M4a1, true) => "weapons/m4a1_silencer_on.wav",
                    (WeaponId::M4a1, false) => "weapons/m4a1_silencer_off.wav",
                    (WeaponId::Usp, true) => "weapons/usp_silencer_on.wav",
                    _ => "weapons/usp_silencer_off.wav",
                };
                self.emit_sound(snd, Some(pos), 0.65);
            } else if spec.has_burst {
                let (pos, now_burst) = {
                    let p = &mut self.players[idx];
                    let origin = p.mv.origin;
                    if let Some(w) = p.active_weapon_mut() {
                        w.burst_mode = !w.burst_mode;
                        (origin, w.burst_mode)
                    } else {
                        (origin, false)
                    }
                };
                self.emit_sound("weapons/dryfire_pistol.wav", Some(pos), 0.5);
                let msg = if now_burst {
                    "Switched to burst-fire mode"
                } else if wid == WeaponId::Famas {
                    "Switched to automatic"
                } else {
                    "Switched to semi-automatic"
                };
                self.emit_banner(msg.to_string(), 1.8);
            } else if wid == WeaponId::Knife && self.players[idx].attack_cooldown <= 0.0 {
                self.perform_knife_attack(pid, true);
            }
        }

        // `+use` edge (`E`): interact with Doors & Hostages!
        let use_edge = inp.use_key && !self.players[idx].old_use;
        if use_edge && !frozen {
            self.try_use_door_or_hostage(pid);
        }

        // Continue 3-round Burst Fire shots (Glock-18 / FAMAS)
        if !frozen && self.players[idx].burst_remaining > 0 {
            self.players[idx].burst_timer -= dt;
            if self.players[idx].burst_timer <= 0.0 {
                self.players[idx].burst_remaining -= 1;
                self.players[idx].burst_timer = 0.065;
                self.fire_gun(pid, true);
            }
        }

        // C4 Defuse (`E` / `+use` near planted C4 for CT)
        if !frozen {
            self.step_player_defuse(pid, inp.use_key, dt);
        }

        // Primary attack (`MOUSE1`): C4 plant, Grenade throw, Knife slash, or Gunfire
        let wid = self.players[idx].active_weapon_id();
        if !frozen && wid == WeaponId::C4 {
            self.step_player_plant_c4(pid, inp.attack || inp.use_key, dt);
        } else {
            self.players[idx].plant_progress = 0.0;
            if !frozen && inp.attack {
                let spec = wid.spec();
                // Shotguns (M3 / XM1014) can interrupt shell-by-shell reload by pressing fire!
                if spec.shell_reload
                    && self.players[idx].reload_timer > 0.0
                    && self.players[idx].active_weapon().is_some_and(|w| w.clip > 0)
                {
                    self.players[idx].reload_timer = 0.0;
                }
                let burst_on = self.players[idx]
                    .active_weapon()
                    .is_some_and(|w| w.burst_mode);
                let trigger_ok = (spec.automatic && !burst_on) || !self.players[idx].old_attack;
                if trigger_ok
                    && self.players[idx].attack_cooldown <= 0.0
                    && self.players[idx].reload_timer <= 0.0
                    && self.players[idx].draw_timer <= 0.0
                {
                    if wid == WeaponId::Knife {
                        self.perform_knife_attack(pid, false);
                    } else if wid.is_grenade() {
                        self.throw_grenade(pid, wid);
                    } else {
                        self.fire_gun(pid, false);
                        if burst_on && let Some(p) = self.player_mut(pid) {
                            p.burst_remaining = 2;
                            p.burst_timer = 0.065;
                            p.attack_cooldown = 0.45;
                        }
                    }
                }
            }
        }

        let _ = old_weapon_id;
        if let Some(p) = self.player_mut(pid) {
            p.old_attack = inp.attack;
            p.old_attack2 = inp.attack2;
            p.old_use = inp.use_key;
        }
    }

    fn perform_knife_attack(&mut self, attacker_id: u32, heavy_stab: bool) {
        let Some(p) = self.player_mut(attacker_id) else {
            return;
        };
        p.attack_cooldown = if heavy_stab { 1.1 } else { 0.4 };
        p.view_anim = ViewAnimKind::Shoot;
        p.view_anim_time = 0.0;
        let eye = p.eye_pos();
        let dir = p.forward_dir();
        let team = p.team;
        let range = if heavy_stab { 56.0 } else { 68.0 };
        let end = eye + dir * range;

        // Check players first
        let mut best_hit: Option<(u32, f32, Vec3, f32)> = None;
        for victim in &self.players {
            if !victim.alive || victim.id == attacker_id {
                continue;
            }
            if !self.config.friendly_fire && victim.team == team {
                continue;
            }
            if let Some((frac, pos, _)) = raycast_player_hitboxes(
                eye,
                end,
                victim.mv.origin,
                victim.yaw_deg,
                victim.mv.ducking,
            ) && best_hit.is_none_or(|(_, bf, _, _)| frac < bf)
            {
                let v_fwd = Vec3::new(
                    victim.yaw_deg.to_radians().cos(),
                    victim.yaw_deg.to_radians().sin(),
                    0.0,
                );
                let backstab = dir.dot(v_fwd) > 0.5;
                let dmg = if heavy_stab {
                    if backstab { 195.0 } else { 65.0 }
                } else {
                    20.0
                };
                best_hit = Some((victim.id, frac, pos, dmg));
            }
        }

        let wall_tr = self.collision.trace_hull(eye, end, Hull::Point);
        if let Some((vid, frac, pos, dmg)) = best_hit
            && frac <= wall_tr.fraction
        {
            self.emit_sound(
                if heavy_stab {
                    "weapons/knife_stab.wav"
                } else {
                    "weapons/knife_hit1.wav"
                },
                Some(pos),
                0.85,
            );
            self.apply_damage(attacker_id, vid, dmg, 1.7, HitGroup::Chest, "knife", pos);
            return;
        }

        if wall_tr.fraction < 1.0 {
            if let Some((mid, b_frac, hit_pos, mat)) = self.collision.raycast_breakable(eye, end)
                && b_frac <= wall_tr.fraction + 0.01
            {
                let dmg = if heavy_stab { 65.0 } else { 25.0 };
                self.damage_breakable(mid, dmg, hit_pos, mat);
            }
            self.emit_sound("weapons/knife_hitwall1.wav", Some(wall_tr.end_pos), 0.75);
            self.push_decal(wall_tr.end_pos, wall_tr.plane_normal, false);
        } else {
            self.emit_sound("weapons/knife_slash1.wav", Some(eye), 0.7);
        }
    }

    fn throw_grenade(&mut self, owner_id: u32, wid: WeaponId) {
        let kind = match wid {
            WeaponId::HeGrenade => GrenadeKind::He,
            WeaponId::Flashbang => GrenadeKind::Flashbang,
            WeaponId::SmokeGrenade => GrenadeKind::Smoke,
            _ => return,
        };
        let (eye, dir, player_vel) = {
            let Some(p) = self.player_mut(owner_id) else {
                return;
            };
            let eye = p.eye_pos();
            let dir = p.forward_dir();
            let vel = p.mv.velocity;
            p.weapons.retain(|w| w.id != wid);
            p.active_slot = 0;
            let fallback = p
                .weapons
                .first()
                .map(|w| w.id)
                .unwrap_or(WeaponId::Knife);
            p.select_weapon(fallback);
            (eye, dir, vel)
        };

        let gid = self.next_grenade_id;
        self.next_grenade_id += 1;
        let launch_vel = dir * 680.0 + Vec3::new(0.0, 0.0, 120.0) + player_vel * 0.4;
        self.grenades.push(ActiveGrenade {
            id: gid,
            owner_id,
            kind,
            position: eye + dir * 18.0,
            velocity: launch_vel,
            fuse_remaining: match kind {
                GrenadeKind::He => 1.65,
                GrenadeKind::Flashbang => 1.5,
                GrenadeKind::Smoke => 1.75,
            },
        });

        self.emit_sound("weapons/pinpull.wav", Some(eye), 0.7);
        self.emit_sound("radio/ct_fireinhole.wav", Some(eye), 0.7);
    }

    fn fire_gun(&mut self, shooter_id: u32, is_burst_followup: bool) {
        let rand_dir_u32 = self.next_rand_u64() as u32;
        let (spec, silenced, eye, base_dir, horiz_speed, on_ground, ducking, zoom, shots_fired, team) = {
            let Some(p) = self.player_mut(shooter_id) else {
                return;
            };
            let wid = p.active_weapon_id();
            let spec = wid.spec();
            let (has_ammo, silenced) = match p.active_weapon_mut() {
                Some(w) if w.clip > 0 => {
                    w.clip -= 1;
                    (true, w.silenced)
                }
                _ => (false, false),
            };
            if !has_ammo {
                if !is_burst_followup {
                    p.attack_cooldown = 0.2;
                    let dry = if spec.slot == WeaponSlot::Secondary {
                        "weapons/dryfire_pistol.wav"
                    } else {
                        "weapons/dryfire_rifle.wav"
                    };
                    let pos = p.mv.origin;
                    self.emit_sound(dry, Some(pos), 0.7);
                }
                return;
            }

            let shots = p.shots_fired;
            p.shots_fired = p.shots_fired.saturating_add(1);
            if !is_burst_followup {
                p.attack_cooldown = spec.cycle_time;
            }
            p.view_anim = ViewAnimKind::Shoot;
            p.view_anim_time = 0.0;

            let horiz = Vec3::new(p.mv.velocity.x, p.mv.velocity.y, 0.0).length();

            // Exact ReGameDLL_CS `CBasePlayerWeapon::KickBack` camera recoil!
            apply_regamedll_kickback(
                spec.kick,
                p.shots_fired,
                horiz,
                p.mv.on_ground,
                p.mv.ducking,
                &mut p.punch_pitch,
                &mut p.punch_yaw,
                &mut p.recoil_dir,
                rand_dir_u32,
            );

            // Sniper rifles unscope briefly after firing (bolt-action AWP/Scout)
            if matches!(wid, WeaponId::Awp | WeaponId::Scout) && p.zoom_level > 0 {
                p.zoom_level = 0;
            }

            (
                spec,
                silenced,
                p.eye_pos(),
                p.forward_dir(),
                horiz,
                p.mv.on_ground,
                p.mv.ducking,
                p.zoom_level,
                shots,
                p.team,
            )
        };

        let fire_snd = if silenced {
            spec.silenced_sound.unwrap_or(spec.fire_sound)
        } else {
            spec.fire_sound
        };
        self.emit_sound(fire_snd, Some(eye), if silenced { 0.6 } else { 0.95 });
        if !silenced {
            self.events.push(SimEvent::MuzzleFlash {
                player_id: shooter_id,
                position: eye + base_dir * 26.0,
            });
        }

        let spread = compute_shot_spread(&spec, horiz_speed, on_ground, ducking, zoom, shots_fired);
        let up_ref = if base_dir.z.abs() > 0.95 { Vec3::X } else { Vec3::Z };
        let right = base_dir.cross(up_ref).normalize();
        let up = right.cross(base_dir).normalize();

        for _ in 0..spec.pellets.max(1) {
            let sx = (self.next_rand_f32() + self.next_rand_f32() - 1.0) * spread;
            let sy = (self.next_rand_f32() + self.next_rand_f32() - 1.0) * spread;
            let shot_dir = (base_dir + right * sx + up * sy).normalize();
            self.trace_bullet_with_wallbang(shooter_id, team, eye, shot_dir, &spec);
        }
    }

    fn trace_bullet_with_wallbang(
        &mut self,
        shooter_id: u32,
        shooter_team: SpawnTeam,
        start: Vec3,
        dir: Vec3,
        spec: &crate::weapons::WeaponSpec,
    ) {
        let mut cur_start = start;
        let mut remaining_dist = 8192.0f32;
        let mut damage = spec.damage;
        let mut penetrations_left = if spec.penetration_power >= 2.0 { 2 } else if spec.penetration_power >= 1.0 { 1 } else { 0 };
        let mut total_dist = 0.0f32;

        loop {
            let end = cur_start + dir * remaining_dist;
            let wall_tr = self.collision.trace_hull(cur_start, end, Hull::Point);

            // Check player hitbox intersections up to `wall_tr.fraction`
            let mut best_player: Option<(u32, f32, Vec3, HitGroup)> = None;
            for victim in &self.players {
                if !victim.alive || victim.id == shooter_id {
                    continue;
                }
                if !self.config.friendly_fire && victim.team == shooter_team {
                    continue;
                }
                if let Some((frac, pos, hg)) = raycast_player_hitboxes(
                    cur_start,
                    end,
                    victim.mv.origin,
                    victim.yaw_deg,
                    victim.mv.ducking,
                ) && frac < wall_tr.fraction
                    && best_player.is_none_or(|(_, bf, _, _)| frac < bf)
                {
                    best_player = Some((victim.id, frac, pos, hg));
                }
            }

            if let Some((vid, frac, hit_pos, hitgroup)) = best_player {
                let seg_dist = remaining_dist * frac;
                total_dist += seg_dist;
                self.tracers.push(BulletTracer {
                    start: cur_start,
                    end: hit_pos,
                    remaining: 0.06,
                });
                let dmg_after_dist = damage * spec.range_modifier.powf(total_dist / 500.0);
                self.apply_damage(
                    shooter_id,
                    vid,
                    dmg_after_dist,
                    spec.armor_ratio,
                    hitgroup,
                    spec.console_name,
                    hit_pos,
                );
                return;
            }

            // Bullet hit world wall / sky / max range
            self.tracers.push(BulletTracer {
                start: cur_start,
                end: wall_tr.end_pos,
                remaining: 0.06,
            });
            if wall_tr.fraction >= 1.0 || wall_tr.hit_sky {
                return;
            }
            // Damage & shatter `func_breakable` glass/vents if hit!
            if let Some((mid, b_frac, hit_pos, mat)) = self.collision.raycast_breakable(cur_start, end)
                && b_frac <= wall_tr.fraction + 0.005
            {
                self.damage_breakable(mid, damage, hit_pos, mat);
            }
            self.push_decal(wall_tr.end_pos, wall_tr.plane_normal, false);

            // Try classic CS 1.6 wallbang penetration through crates/doors/thin walls!
            if penetrations_left == 0 {
                return;
            }
            let mat = self.surface_material_at(wall_tr.end_pos - wall_tr.plane_normal * 4.0);
            let max_thickness = 18.0 * spec.penetration_power * mat.penetration_modifier();
            let mut exit_point = None;
            let mut step = 6.0f32;
            while step <= max_thickness {
                let probe = wall_tr.end_pos + dir * step;
                if self.collision.point_contents(probe, Hull::Point) != crate::trace::CONTENTS_SOLID {
                    exit_point = Some((probe, step));
                    break;
                }
                step += 6.0;
            }

            if let Some((exit, thickness)) = exit_point {
                penetrations_left -= 1;
                let seg_dist = (wall_tr.end_pos - cur_start).length() + thickness;
                total_dist += seg_dist;
                remaining_dist = (remaining_dist - seg_dist).max(0.0);
                damage *= mat.penetration_modifier() * 0.72;
                if damage < 5.0 || remaining_dist < 16.0 {
                    return;
                }
                cur_start = exit;
            } else {
                return;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_damage(
        &mut self,
        attacker_id: u32,
        victim_id: u32,
        base_damage: f32,
        armor_ratio: f32,
        hitgroup: HitGroup,
        weapon_name: &str,
        hit_pos: Vec3,
    ) {
        let Some(v_idx) = self.players.iter().position(|p| p.id == victim_id) else {
            return;
        };
        if !self.players[v_idx].alive || self.players[v_idx].god_mode {
            return;
        }

        let (hp_dmg, armor_dmg) = compute_cs16_damage(
            base_damage,
            0.0,
            1.0,
            hitgroup,
            armor_ratio,
            self.players[v_idx].armor as f32,
            self.players[v_idx].has_helmet,
        );

        let headshot = hitgroup == HitGroup::Head;
        let hit_helmet = headshot && self.players[v_idx].has_helmet && self.players[v_idx].armor > 0;
        self.players[v_idx].health -= hp_dmg.round() as i32;
        self.players[v_idx].armor = (self.players[v_idx].armor - armor_dmg.round() as i32).max(0);
        self.players[v_idx].damage_flash = 0.85;
        // Tagging slowdown (authentic CS 1.6 movement slowdown on bullet impact!)
        self.players[v_idx].mv.velocity *= 0.45;

        let sound = if hit_helmet {
            "player/bhit_helmet-1.wav"
        } else if headshot {
            "player/headshot1.wav"
        } else if armor_dmg > 0.0 {
            "player/bhit_kevlar-1.wav"
        } else {
            "player/bhit_flesh-1.wav"
        };
        self.emit_sound(sound, Some(hit_pos), 0.85);
        self.push_decal(hit_pos, Vec3::Z, true);

        if self.players[v_idx].health <= 0 {
            self.kill_player(attacker_id, victim_id, weapon_name, headshot);
        }
    }

    fn kill_player(&mut self, attacker_id: u32, victim_id: u32, weapon_name: &str, headshot: bool) {
        let Some(v_idx) = self.players.iter().position(|p| p.id == victim_id) else {
            return;
        };
        if !self.players[v_idx].alive {
            return;
        }
        self.players[v_idx].alive = false;
        self.players[v_idx].health = 0;
        self.players[v_idx].deaths += 1;
        self.players[v_idx].zoom_level = 0;
        self.players[v_idx].plant_progress = 0.0;
        self.players[v_idx].defuse_progress = 0.0;

        let victim_name = self.players[v_idx].name.clone();
        let victim_team = self.players[v_idx].team;
        let victim_skin = self.players[v_idx].skin;
        let victim_yaw = self.players[v_idx].yaw_deg;
        let drop_pos = self.players[v_idx].mv.origin;

        // Spawn persistent round corpse playing its real Valve death sequence!
        let seq_name = if headshot {
            "headshot"
        } else {
            ["death1", "death2", "death3"][(self.next_rand_u64() as usize) % 3]
        };
        let corpse_pos = snap_spawn_to_floor(drop_pos, &self.collision);
        self.corpses.push(Corpse {
            position: corpse_pos,
            yaw_deg: victim_yaw,
            skin: victim_skin,
            seq_name: seq_name.to_string(),
            anim_time: 0.0,
        });

        // Drop primary weapon and C4 upon death!
        let dropped: Vec<WeaponInstance> = self.players[v_idx]
            .weapons
            .iter()
            .filter(|w| matches!(w.id.spec().slot, WeaponSlot::Primary | WeaponSlot::C4))
            .copied()
            .collect();
        for w in dropped {
            let floor = snap_spawn_to_floor(drop_pos, &self.collision);
            self.dropped_weapons.push(WorldWeapon {
                id: w.id,
                clip: w.clip,
                reserve: w.reserve,
                position: floor - Vec3::new(0.0, 0.0, 28.0),
            });
        }

        let (killer_name, killer_team) = if attacker_id != victim_id {
            if let Some(a) = self.player_mut(attacker_id) {
                if a.team != victim_team {
                    a.frags += 1;
                    a.money = (a.money + 300).min(MAX_MONEY);
                } else {
                    a.frags -= 1;
                }
                (a.name.clone(), a.team)
            } else {
                ("World".to_string(), victim_team)
            }
        } else {
            (victim_name.clone(), victim_team)
        };

        let die_snd = ["player/die1.wav", "player/die2.wav", "player/die3.wav"]
            [(self.next_rand_u64() as usize) % 3];
        self.emit_sound(die_snd, Some(drop_pos), 0.9);

        self.killfeed.push(KillFeedEntry {
            killer_name,
            killer_team,
            victim_name,
            victim_team,
            weapon: weapon_name.to_string(),
            headshot,
            remaining: 6.5,
        });
        if self.killfeed.len() > 6 {
            self.killfeed.remove(0);
        }
    }

    fn step_player_plant_c4(&mut self, pid: u32, holding: bool, dt: f32) {
        let Some(idx) = self.players.iter().position(|p| p.id == pid) else {
            return;
        };
        let can_plant = self.phase == RoundPhase::Live
            && self.planted_c4.is_none()
            && self.players[idx].mv.on_ground
            && self.collision.is_in_bomb_target(self.players[idx].mv.origin);

        if holding && can_plant {
            self.players[idx].plant_progress += dt / C4_PLANT_TIME;
            if self.players[idx].plant_progress >= 1.0 {
                let pos = self.players[idx].mv.origin - Vec3::new(0.0, 0.0, 32.0);
                self.players[idx].plant_progress = 0.0;
                self.players[idx].weapons.retain(|w| w.id != WeaponId::C4);
                self.players[idx].active_slot = 0;
                self.players[idx].money = (self.players[idx].money + 300).min(MAX_MONEY);

                self.planted_c4 = Some(PlantedC4 {
                    position: pos,
                    timer: self.config.c4_timer_sec,
                    beep_accum: 0.0,
                    defuser_id: None,
                });
                self.phase = RoundPhase::BombPlanted;
                self.phase_timer = self.config.c4_timer_sec;

                self.emit_sound("weapons/c4_plant.wav", Some(pos), 0.9);
                self.emit_sound("radio/bombpl.wav", None, 0.95);
                self.emit_banner("The bomb has been planted.".to_string(), 3.5);
            }
        } else {
            self.players[idx].plant_progress = 0.0;
        }
    }

    fn step_player_defuse(&mut self, pid: u32, holding_use: bool, dt: f32) {
        let Some(c4_pos) = self.planted_c4.as_ref().map(|c| c.position) else {
            return;
        };
        let Some(idx) = self.players.iter().position(|p| p.id == pid) else {
            return;
        };
        let can_defuse = self.players[idx].team == SpawnTeam::CounterTerrorist
            && self.players[idx].alive
            && self.players[idx].mv.origin.distance(c4_pos) < 85.0
            && self.phase == RoundPhase::BombPlanted;

        if holding_use && can_defuse {
            let started_now = self.players[idx].defuse_progress == 0.0;
            if started_now {
                self.emit_sound("weapons/c4_disarm.wav", Some(c4_pos), 0.85);
            }
            let total_time = if self.players[idx].has_defuser {
                C4_DEFUSE_KIT_TIME
            } else {
                C4_DEFUSE_TIME
            };
            self.players[idx].defuse_progress += dt / total_time;
            if let Some(c4) = &mut self.planted_c4 {
                c4.defuser_id = Some(pid);
            }
            if self.players[idx].defuse_progress >= 1.0 {
                self.players[idx].defuse_progress = 0.0;
                self.players[idx].frags += 3;
                self.players[idx].money = (self.players[idx].money + 300).min(MAX_MONEY);
                self.planted_c4 = None;
                self.emit_sound("weapons/c4_disarmed.wav", Some(c4_pos), 0.9);
                self.emit_sound("radio/bombdef.wav", None, 0.95);
                self.end_round(SpawnTeam::CounterTerrorist, "Bomb has been defused!");
            }
        } else {
            self.players[idx].defuse_progress = 0.0;
            if let Some(c4) = &mut self.planted_c4
                && c4.defuser_id == Some(pid)
            {
                c4.defuser_id = None;
            }
        }
    }

    fn step_planted_c4(&mut self, dt: f32) {
        let mut beep_to_play: Option<(&'static str, Vec3)> = None;
        let mut explode_pos: Option<Vec3> = None;

        if let Some(c4) = &mut self.planted_c4 {
            c4.timer -= dt;
            self.phase_timer = c4.timer.max(0.0);

            // Classic accelerating C4 beep cadence
            let beep_interval = (c4.timer / self.config.c4_timer_sec).clamp(0.08, 1.0) * 1.15;
            c4.beep_accum += dt;
            if c4.beep_accum >= beep_interval {
                c4.beep_accum = 0.0;
                let snd = if c4.timer < 5.0 {
                    "weapons/c4_beep5.wav"
                } else if c4.timer < 12.0 {
                    "weapons/c4_beep4.wav"
                } else if c4.timer < 22.0 {
                    "weapons/c4_beep3.wav"
                } else {
                    "weapons/c4_beep1.wav"
                };
                beep_to_play = Some((snd, c4.position));
            }
            if c4.timer <= 0.0 {
                explode_pos = Some(c4.position);
            }
        } else {
            return;
        }

        if let Some((snd, pos)) = beep_to_play {
            self.emit_sound(snd, Some(pos), 0.85);
        }

        if let Some(pos) = explode_pos {
            self.planted_c4 = None;
            self.emit_sound("weapons/c4_explode1.wav", Some(pos), 1.0);
            self.events.push(SimEvent::Explosion {
                position: pos,
                is_c4: true,
            });
            // Blast radius damage (~1200 units)
            let targets: Vec<(u32, f32)> = self
                .players
                .iter()
                .filter(|p| p.alive)
                .map(|p| (p.id, p.mv.origin.distance(pos)))
                .collect();
            for (vid, dist) in targets {
                if dist < 1100.0 {
                    let dmg = (1.0 - dist / 1100.0) * 450.0;
                    self.apply_damage(vid, vid, dmg, 1.0, HitGroup::Chest, "c4", pos);
                }
            }
            self.end_round(SpawnTeam::Terrorist, "Target Successfully Bombed!");
        }
    }

    fn handle_grenade_detonation(&mut self, owner_id: u32, kind: GrenadeKind, pos: Vec3) {
        match kind {
            GrenadeKind::He => {
                self.emit_sound("weapons/hegrenade-1.wav", Some(pos), 1.0);
                self.events.push(SimEvent::Explosion {
                    position: pos,
                    is_c4: false,
                });
                let targets: Vec<(u32, Vec3)> = self
                    .players
                    .iter()
                    .filter(|p| p.alive)
                    .map(|p| (p.id, p.mv.origin))
                    .collect();
                for (vid, vpos) in targets {
                    let dist = vpos.distance(pos);
                    if dist < 360.0 {
                        let occluded = !self.collision.has_line_of_sight(pos + Vec3::Z * 8.0, vpos + Vec3::Z * 12.0);
                        let mult = if occluded { 0.45 } else { 1.0 };
                        let dmg = (1.0 - dist / 360.0) * 98.0 * mult;
                        if dmg >= 1.0 {
                            self.apply_damage(owner_id, vid, dmg, 1.2, HitGroup::Chest, "hegrenade", vpos);
                        }
                    }
                }
            }
            GrenadeKind::Flashbang => {
                self.emit_sound("weapons/flashbang-1.wav", Some(pos), 1.0);
                let flash_pt = pos + Vec3::Z * 8.0;
                for p in &mut self.players {
                    if !p.alive {
                        continue;
                    }
                    let dist = p.eye_pos().distance(flash_pt);
                    if dist > 1400.0 {
                        continue;
                    }
                    if !self.collision.has_line_of_sight(p.eye_pos(), flash_pt) {
                        continue;
                    }
                    let to_flash = (flash_pt - p.eye_pos()).normalize();
                    let facing = p.forward_dir().dot(to_flash).clamp(0.0, 1.0);
                    let intensity = (1.0 - dist / 1400.0) * (0.25 + 0.75 * facing);
                    p.flash_alpha = (p.flash_alpha + intensity * 1.4).clamp(0.0, 1.45);
                }
            }
            GrenadeKind::Smoke => {
                self.emit_sound("weapons/sg_explode.wav", Some(pos), 0.9);
            }
        }
    }

    fn drop_active_weapon(&mut self, pid: u32) {
        let Some(p) = self.player_mut(pid) else {
            return;
        };
        let wid = p.active_weapon_id();
        if !matches!(wid.spec().slot, WeaponSlot::Primary | WeaponSlot::Secondary | WeaponSlot::C4) {
            return;
        }
        let dropped = p.weapons.remove(p.active_slot);
        let drop_pos = p.mv.origin + p.forward_dir() * 36.0;
        p.active_slot = 0;
        let fallback = p.weapons.first().map(|w| w.id).unwrap_or(WeaponId::Knife);
        p.select_weapon(fallback);

        let floor = snap_spawn_to_floor(drop_pos, &self.collision);
        self.dropped_weapons.push(WorldWeapon {
            id: dropped.id,
            clip: dropped.clip,
            reserve: dropped.reserve,
            position: floor - Vec3::new(0.0, 0.0, 28.0),
        });
    }

    fn try_pickup_dropped_weapons(&mut self, pid: u32) {
        let Some(p_idx) = self.players.iter().position(|p| p.id == pid) else {
            return;
        };
        let pos = self.players[p_idx].mv.origin;
        let team = self.players[p_idx].team;

        let mut i = 0;
        while i < self.dropped_weapons.len() {
            let w = &self.dropped_weapons[i];
            if w.position.distance(pos) < 48.0 {
                let slot = w.id.spec().slot;
                let can_take = match slot {
                    WeaponSlot::C4 => team == SpawnTeam::Terrorist && !self.players[p_idx].has_weapon(WeaponId::C4),
                    WeaponSlot::Primary | WeaponSlot::Secondary => {
                        !self.players[p_idx].weapons.iter().any(|ew| ew.id.spec().slot == slot)
                    }
                    _ => false,
                };
                if can_take {
                    let taken = self.dropped_weapons.swap_remove(i);
                    let p = &mut self.players[p_idx];
                    p.weapons.push(WeaponInstance {
                        id: taken.id,
                        clip: taken.clip,
                        reserve: taken.reserve,
                        silenced: false,
                        burst_mode: false,
                    });
                    p.weapons.sort_by_key(|x| x.id.spec().slot as u8);
                    self.emit_sound("items/gunpickup2.wav", Some(pos), 0.7);
                    continue;
                }
            }
            i += 1;
        }
    }

    fn check_round_win_conditions(&mut self) {
        let total_ct = self
            .players
            .iter()
            .filter(|p| p.team == SpawnTeam::CounterTerrorist)
            .count();
        let total_t = self
            .players
            .iter()
            .filter(|p| p.team == SpawnTeam::Terrorist)
            .count();
        // Don't end rounds immediately in solo exploration mode (0 enemies)
        if total_ct == 0 || total_t == 0 {
            return;
        }

        let alive_ct = self
            .players
            .iter()
            .filter(|p| p.team == SpawnTeam::CounterTerrorist && p.alive)
            .count();
        let alive_t = self
            .players
            .iter()
            .filter(|p| p.team == SpawnTeam::Terrorist && p.alive)
            .count();

        if alive_ct == 0 {
            self.end_round(SpawnTeam::Terrorist, "Terrorists Win!");
        } else if alive_t == 0 && self.planted_c4.is_none() {
            self.end_round(SpawnTeam::CounterTerrorist, "Counter-Terrorists Win!");
        }
    }

    fn end_round(&mut self, winner: SpawnTeam, banner: &str) {
        if self.phase == RoundPhase::RoundEnd {
            return;
        }
        self.phase = RoundPhase::RoundEnd;
        self.phase_timer = 5.0;

        match winner {
            SpawnTeam::CounterTerrorist => {
                self.ct_score += 1;
                self.ct_loss_streak = 0;
                self.t_loss_streak = (self.t_loss_streak + 1).min(4);
                self.emit_sound("radio/ctwin.wav", None, 0.95);
            }
            SpawnTeam::Terrorist => {
                self.t_score += 1;
                self.t_loss_streak = 0;
                self.ct_loss_streak = (self.ct_loss_streak + 1).min(4);
                self.emit_sound("radio/terwin.wav", None, 0.95);
            }
        }

        // Authentic CS 1.6 round-end economy payouts
        let win_bonus = 3250;
        let ct_lose = 1400 + (self.ct_loss_streak.saturating_sub(1) as i32) * 500;
        let t_lose = 1400 + (self.t_loss_streak.saturating_sub(1) as i32) * 500;

        for p in &mut self.players {
            let reward = if p.team == winner {
                win_bonus
            } else if p.team == SpawnTeam::CounterTerrorist {
                ct_lose
            } else {
                t_lose
            };
            p.money = (p.money + reward).min(MAX_MONEY);
        }

        self.emit_banner(banner.to_string(), 4.5);
    }

    fn bot_buy_round(&mut self, bot_id: u32) {
        let Some(p) = self.player(bot_id) else {
            return;
        };
        let team = p.team;
        let money = p.money;

        // Buy Kevlar+Helmet when affordable
        if money >= 2000 {
            let _ = self.buy_item(bot_id, "vesthelm");
        } else if money >= 800 {
            let _ = self.buy_item(bot_id, "vest");
        }

        let money = self.player(bot_id).map(|p| p.money).unwrap_or(0);
        let choice = self.next_rand_u64() % 10;
        match team {
            SpawnTeam::Terrorist => {
                if money >= 4750 && choice == 0 {
                    let _ = self.buy_item(bot_id, "awp");
                } else if money >= 2500 {
                    let _ = self.buy_item(bot_id, "ak47");
                } else if money >= 2000 {
                    let _ = self.buy_item(bot_id, "galil");
                } else if money >= 1500 {
                    let _ = self.buy_item(bot_id, "mp5");
                } else if money >= 650 {
                    let _ = self.buy_item(bot_id, "deagle");
                }
            }
            SpawnTeam::CounterTerrorist => {
                if money >= 4750 && choice == 0 {
                    let _ = self.buy_item(bot_id, "awp");
                } else if money >= 3100 {
                    let _ = self.buy_item(bot_id, "m4a1");
                } else if money >= 2250 {
                    let _ = self.buy_item(bot_id, "famas");
                } else if money >= 1500 {
                    let _ = self.buy_item(bot_id, "mp5");
                } else if money >= 650 {
                    let _ = self.buy_item(bot_id, "deagle");
                }
                if self.player(bot_id).is_some_and(|p| p.money >= 200) {
                    let _ = self.buy_item(bot_id, "defuser");
                }
            }
        }
    }

    fn pick_spawn(&self, team: SpawnTeam, index: usize) -> (Vec3, f32) {
        let team_spawns: Vec<&SpawnPoint> =
            self.spawns.iter().filter(|s| s.team == team).collect();
        let chosen = if !team_spawns.is_empty() {
            team_spawns[index % team_spawns.len()]
        } else if let Some(any) = self.spawns.first() {
            any
        } else {
            return (Vec3::new(0.0, 0.0, 64.0), 0.0);
        };
        let snapped = snap_spawn_to_floor(chosen.position, &self.collision);
        (snapped, chosen.yaw_deg)
    }

    fn surface_material_at(&self, pos: Vec3) -> SurfaceMaterial {
        let leaf = self.map_nodes_leafs_for_mat.find_leaf(pos);
        self.leaf_materials
            .get(leaf)
            .copied()
            .unwrap_or(SurfaceMaterial::Concrete)
    }

    fn damage_breakable(&mut self, model_index: usize, dmg: f32, hit_pos: Vec3, material: i32) {
        let mut shattered = false;
        for b in &mut self.breakables {
            if b.model_index == model_index && !b.broken {
                b.health -= dmg;
                if b.health <= 0.0 {
                    b.broken = true;
                    shattered = true;
                }
                break;
            }
        }
        if shattered {
            self.collision.disabled_models.insert(model_index);
            if !self.broken_models.contains(&model_index) {
                self.broken_models.push(model_index);
            }
            let snd = match material {
                0 | 5 => "debris/bustglass1.wav",
                1 => "debris/bustcrate1.wav",
                2 => "debris/bustmetal1.wav",
                _ => "debris/bustconcrete1.wav",
            };
            self.emit_sound(snd, Some(hit_pos), 0.9);
        }
    }

    fn try_use_door_or_hostage(&mut self, pid: u32) {
        let Some(p) = self.player(pid) else {
            return;
        };
        let pos = p.mv.origin;
        let team = p.team;

        // 1. Open nearest `func_door` within 110 units
        let mut opened_door = None;
        for d in &mut self.doors {
            if d.center.distance(pos) < 110.0 && !d.opening {
                d.opening = true;
                d.hold_timer = d.wait.max(2.5);
                opened_door = Some(d.center);
                break;
            }
        }
        if let Some(dpos) = opened_door {
            self.emit_sound("doors/doormove1.wav", Some(dpos), 0.8);
            return;
        }

        // 2. Toggle nearest Hostage following (CT only on `cs_*` maps!)
        if team == SpawnTeam::CounterTerrorist {
            let mut voice_pos = None;
            for h in &mut self.hostages {
                if h.alive && !h.rescued && h.position.distance(pos) < 92.0 {
                    if h.following_player == Some(pid) {
                        h.following_player = None;
                    } else {
                        h.following_player = Some(pid);
                        voice_pos = Some(h.position);
                    }
                    break;
                }
            }
            if let Some(hpos) = voice_pos {
                let snd = ["hostage/hos1.wav", "hostage/hos2.wav", "hostage/hos3.wav"]
                    [(self.next_rand_u64() as usize) % 3];
                self.emit_sound(snd, Some(hpos), 0.9);
            }
        }
    }

    fn step_doors(&mut self, dt: f32) {
        self.door_offsets.clear();
        for d in &mut self.doors {
            let travel_len = d.travel.length().max(1.0);
            let rate = d.speed / travel_len;
            if d.opening {
                if d.open_frac < 1.0 {
                    d.open_frac = (d.open_frac + rate * dt).min(1.0);
                } else {
                    d.hold_timer -= dt;
                    if d.hold_timer <= 0.0 {
                        d.opening = false;
                    }
                }
            } else if d.open_frac > 0.0 {
                d.open_frac = (d.open_frac - rate * dt).max(0.0);
            }

            if d.open_frac > 0.25 {
                self.collision.disabled_models.insert(d.model_index);
            } else if !self.broken_models.contains(&d.model_index) {
                self.collision.disabled_models.remove(&d.model_index);
            }

            if d.open_frac > 0.001 {
                self.door_offsets.push((d.model_index, d.travel * d.open_frac));
            }
        }
    }

    fn step_hostages(&mut self, dt: f32) {
        if self.hostages.is_empty() {
            return;
        }
        let mut rescued_by: Vec<u32> = Vec::new();
        for h in &mut self.hostages {
            h.moving = false;
            if !h.alive || h.rescued {
                continue;
            }
            if let Some(leader_id) = h.following_player {
                if let Some(leader) = self.players.iter().find(|p| p.id == leader_id && p.alive) {
                    let mut delta = leader.mv.origin - h.position;
                    delta.z = 0.0;
                    let dist = delta.length();
                    if dist > 64.0 && dist < 520.0 {
                        let dir = delta / dist;
                        let target = h.position + dir * 185.0 * dt;
                        let tr = self.collision.trace_hull(h.position, target, Hull::Standing);
                        if tr.fraction > 0.2 {
                            h.position = snap_spawn_to_floor(tr.end_pos, &self.collision);
                        }
                        h.yaw_deg = dir.y.atan2(dir.x).to_degrees().rem_euclid(360.0);
                        h.moving = true;
                    } else if dist >= 520.0 {
                        h.following_player = None;
                    }
                    if self.collision.is_in_hostage_rescue(h.position) {
                        h.rescued = true;
                        h.following_player = None;
                        rescued_by.push(leader_id);
                    }
                } else {
                    h.following_player = None;
                }
            }
        }

        for pid in rescued_by {
            if let Some(p) = self.player_mut(pid) {
                p.money = (p.money + 1000).min(MAX_MONEY);
            }
            self.emit_sound("radio/rescued.wav", None, 0.95);
            self.emit_banner("Hostage has been rescued!".to_string(), 3.0);
        }

        if self.phase == RoundPhase::Live
            && !self.hostages.is_empty()
            && self.hostages.iter().all(|h| !h.alive || h.rescued)
            && self.hostages.iter().any(|h| h.rescued)
        {
            self.end_round(SpawnTeam::CounterTerrorist, "All Hostages have been rescued!");
        }
    }

    fn push_decal(&mut self, pos: Vec3, normal: Vec3, is_blood: bool) {
        if self.decals.len() >= 256 {
            self.decals.pop_front();
        }
        self.decals.push_back(ImpactDecal {
            position: pos + normal * 0.4,
            normal,
            is_blood,
            is_spray: false,
        });
    }

    fn emit_sound(&mut self, path: &str, position: Option<Vec3>, volume: f32) {
        self.events.push(SimEvent::Sound {
            path: path.to_string(),
            position,
            volume,
        });
    }

    fn emit_banner(&mut self, text: String, duration: f32) {
        self.banner = Some((text.clone(), duration));
        self.events.push(SimEvent::Banner { text, duration });
    }

    fn next_rand_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    fn next_rand_f32(&mut self) -> f32 {
        ((self.next_rand_u64() >> 40) as f32) / ((1u64 << 24) as f32)
    }
}

fn give_starting_loadout(p: &mut PlayerState) {
    let pistol = match p.team {
        SpawnTeam::CounterTerrorist => WeaponId::Usp,
        SpawnTeam::Terrorist => WeaponId::Glock18,
    };
    p.weapons.push(WeaponInstance::new(pistol));
    p.weapons.push(WeaponInstance::new(WeaponId::Knife));
    p.weapons.sort_by_key(|w| w.id.spec().slot as u8);
    p.select_weapon(pistol);
}

fn footstep_sound_for_material(mat: SurfaceMaterial, rng: u64) -> &'static str {
    let idx = (rng as usize) % 4;
    match mat {
        SurfaceMaterial::Dirt | SurfaceMaterial::Grass | SurfaceMaterial::Snow => {
            ["player/pl_dirt1.wav", "player/pl_dirt2.wav", "player/pl_dirt3.wav", "player/pl_dirt4.wav"][idx]
        }
        SurfaceMaterial::Metal | SurfaceMaterial::Computer => {
            ["player/pl_metal1.wav", "player/pl_metal2.wav", "player/pl_metal3.wav", "player/pl_metal4.wav"][idx]
        }
        SurfaceMaterial::Grate => {
            ["player/pl_grate1.wav", "player/pl_grate2.wav", "player/pl_grate3.wav", "player/pl_grate4.wav"][idx]
        }
        SurfaceMaterial::Vent => {
            ["player/pl_duct1.wav", "player/pl_duct2.wav", "player/pl_duct3.wav", "player/pl_duct4.wav"][idx]
        }
        SurfaceMaterial::Tile => {
            ["player/pl_tile1.wav", "player/pl_tile2.wav", "player/pl_tile3.wav", "player/pl_tile4.wav"][idx]
        }
        SurfaceMaterial::Slosh => {
            ["player/pl_slosh1.wav", "player/pl_slosh2.wav", "player/pl_slosh3.wav", "player/pl_slosh4.wav"][idx]
        }
        _ => {
            ["player/pl_step1.wav", "player/pl_step2.wav", "player/pl_step3.wav", "player/pl_step4.wav"][idx]
        }
    }
}
