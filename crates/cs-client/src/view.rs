//! The frame the shell draws, built from a [`Connection`] without a window or a GPU: the entity
//! scene (every entity by `modelindex`, then the temp entities), the camera, the view model and
//! the flat effects. The renderer and the HUD take the result; the headless tests read it.
//!
//! One code for every world: nothing here knows whether the server is on this machine.

use std::collections::{BTreeMap, HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::cell::RefCell;

use cs_assets::bsp::BspMap;
use cs_assets::materials::{MaterialTable, SurfaceMaterial};
use cs_assets::mdl::{MdlSeqEvent, StudioModel};
use cs_assets::spr::GoldSrcSprite;
use cs_clientcore::cvars::ClientCvars;
use cs_clientcore::entities::ef;
use cs_clientcore::events::{EventFx, FiredEvent, ViewAnim};
use cs_clientcore::hud::{CustomDecalInfo, evaluate_lightstyles};
use cs_clientcore::tempents::{
    DynamicLight, FxTrace, FxWorld, ModelRef, RenderEffect, RenderMode, TempEnt, ftent,
};
use cs_clientcore::{
    ClientEffects, Connection, DemoPlayer, EmitInputs, EntityKind,
    ModelKind as CoreModelKind, PrecacheTables, Prediction, SoundStart, VisibleEntity,
};
use cs_pm::collision::{CONTENTS_SOLID, Hull, WorldCollision};
use cs_pm::pm::consts::{
    CHAR_TEX_COMPUTER, CHAR_TEX_CONCRETE, CHAR_TEX_DIRT, CHAR_TEX_GRATE, CHAR_TEX_METAL,
    CHAR_TEX_SLOSH, CHAR_TEX_SNOW, CHAR_TEX_TILE, CHAR_TEX_VENT, CHAR_TEX_WOOD,
};
use cs_proto48::tempent::TempEffect;
use cs_transport::PacketTransport;
use cs_vfs::Vfs;
use glam::Vec3;

use crate::render::decal_renderer::{DecalSpawn, query_submodel_faces_in_sphere};
use crate::render::fx_renderer::{FxBeam, FxPoint};
use crate::render::lighting::{
    DLightPool, GpuLightsUniform, SpotLight, compute_flashlight_spot,
};
use crate::render::mdl_renderer::{STUDIO_LOOPING, studio_frame};
use crate::render::particles::{BeamSystem, ParticleSystem};
use crate::render::scene::{
    BrushCollider, EF_NODRAW, EntRenderMode, ModelHandle, ModelKind, ModelRegistry,
    RENDERFX_DEAD_PLAYER, RenderEntity, SOLID_BSP, SceneFrame, ViewBasis,
    collect_opaque_brush_colliders, collect_solid_brush_colliders, trace_brush_collider,
    trace_glow_occlusion, trace_studio_shadow, world_to_brush_local,
};
use crate::view_effects::{
    BobParams, BobState, StairSmoothState, ViewModelLagState, calc_roll, calc_screen_shake,
    underwater_tint_and_fov,
};

/// The first model index the shell hands to models the client loads by itself (shells, the
/// effect sprites); the server's table ends well below (`MAX_MODELS` is 512).
pub const CLIENT_MODEL_BASE: u16 = 1024;

/// Default flat `entity_positions` capacity matching GoldSrc's edict table ceiling.
pub const DEFAULT_MAX_EDICTS: usize = 1024;

/// ReHLDS / GoldSrc edict table formula (`900 + 15 * (max_clients - 1)`), floored at
/// [`DEFAULT_MAX_EDICTS`] and capped at `2048`.
#[inline]
fn goldsrc_max_edicts(max_clients: u8) -> usize {
    let clients = usize::from(max_clients.max(1));
    (900 + 15 * (clients - 1)).clamp(DEFAULT_MAX_EDICTS, 2048)
}

/// A tracer or beam lives this long when the message gives no time of its own.
const TRACER_LIFE: f64 = 0.12;
/// Beams and glows kept at once.
const MAX_ACTIVE_FX: usize = 256;

/// The world the temp entities collide with and the names they animate by: the map's hull
/// collision, and the model names of the precache table (kept current by the shell).
pub struct MapFxWorld {
    collision: Option<Arc<WorldCollision>>,
    vfs: Option<Vfs>,
    names: Rc<RefCell<BTreeMap<u16, String>>>,
    frames: RefCell<HashMap<String, u32>>,
}

impl MapFxWorld {
    pub fn new(
        collision: Option<Arc<WorldCollision>>,
        vfs: Option<Vfs>,
        names: Rc<RefCell<BTreeMap<u16, String>>>,
    ) -> Self {
        Self {
            collision,
            vfs,
            names,
            frames: RefCell::new(HashMap::new()),
        }
    }

    fn path_of(&self, model: &ModelRef) -> Option<String> {
        match model {
            ModelRef::None => None,
            ModelRef::Precached(i) => self.names.borrow().get(i).cloned(),
            ModelRef::Client(path) => Some((*path).to_string()),
        }
    }
}

impl FxWorld for MapFxWorld {
    fn frame_count(&self, model: &ModelRef) -> u32 {
        let Some(path) = self.path_of(model) else {
            return 1;
        };
        if !path.to_ascii_lowercase().ends_with(".spr") {
            return 1;
        }
        if let Some(n) = self.frames.borrow().get(&path) {
            return *n;
        }
        let count = self
            .vfs
            .as_ref()
            .and_then(|vfs| GoldSrcSprite::load(vfs, &path).ok())
            .map_or(1, |s| s.frames.len().max(1) as u32);
        self.frames.borrow_mut().insert(path, count);
        count
    }

    fn trace_line(&self, from: Vec3, to: Vec3) -> Option<FxTrace> {
        let tr = self.collision.as_ref()?.trace_world(from, to, Hull::Point);
        (tr.fraction < 1.0).then_some(FxTrace {
            fraction: tr.fraction,
            end: tr.end_pos,
            normal: tr.plane_normal,
        })
    }

    fn point_solid(&self, point: Vec3) -> bool {
        self.collision
            .as_ref()
            .is_some_and(|c| c.point_contents(point, Hull::Point) == CONTENTS_SOLID)
    }
}

/// What the shell knows that the connection does not.
#[derive(Debug, Clone, Copy)]
pub struct ViewInputs {
    /// The session clock, the one the connection was pumped with.
    pub now: f64,
    /// Real seconds since the last frame.
    pub frametime: f64,
    /// `cl_righthand`.
    pub right_hand: bool,
    /// `r_drawviewmodel`.
    pub draw_viewmodel: bool,
    /// The fov when the server does not send one.
    pub default_fov: f32,
    pub viewmodel_fov: f32,
    pub cl_bob: f32,
    pub cl_bobcycle: f32,
    pub cl_bobup: f32,
    pub cl_rollangle: f32,
    pub cl_rollspeed: f32,
    pub cl_viewmodel_lag: f32,
    pub cl_weather: bool,
    pub sv_cheats: bool,
    pub sv_showimpacts: i32,
    pub sv_showhitboxes: bool,
    /// Spectator / demo X-Ray (`spec_xray`), strictly gated in `ViewBuilder`.
    pub spec_xray: bool,
}

impl Default for ViewInputs {
    fn default() -> Self {
        Self {
            now: 0.0,
            frametime: 0.016,
            right_hand: true,
            draw_viewmodel: true,
            default_fov: 90.0,
            viewmodel_fov: 90.0,
            cl_bob: 0.01,
            cl_bobcycle: 0.8,
            cl_bobup: 0.5,
            cl_rollangle: 2.0,
            cl_rollspeed: 200.0,
            cl_viewmodel_lag: 1.0,
            cl_weather: true,
            sv_cheats: false,
            sv_showimpacts: 0,
            sv_showhitboxes: false,
            spec_xray: false,
        }
    }
}

/// The first-person weapon.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewmodelView {
    pub path: String,
    pub sequence: i32,
    pub body: i32,
    /// Client time the sequence started.
    pub animtime: f32,
    /// Mirrors the viewmodel for right-handed display (`cl_righthand`); CS 1.6 `v_*.mdl`
    /// models are authored left-handed.
    pub right_hand: bool,
}

/// Counts of what a frame holds, for the net graph and the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewStats {
    pub entities: usize,
    pub brush: usize,
    pub studio: usize,
    pub sprites: usize,
    pub tempents: usize,
    pub beams: usize,
}

/// Everything a frame draws, apart from the HUD.
#[derive(Debug, Clone)]
pub struct FrameView {
    pub scene: SceneFrame,
    pub eye: Vec3,
    pub punch: Vec3,
    pub fov: f32,
    pub bob: f32,
    pub roll_deg: f32,
    pub shake_angles: Vec3,
    pub shake_z: f32,
    pub viewmodel_lag: Vec3,
    pub underwater_tint: Option<[f32; 4]>,
    pub underwater_fov_delta: f32,
    pub viewmodel: Option<ViewmodelView>,
    pub beams: Vec<FxBeam>,
    pub points: Vec<FxPoint>,
    pub lights: GpuLightsUniform,
    pub elights: Vec<DynamicLight>,
    pub decals: Vec<DecalSpawn>,
    pub lightstyles: [f32; 64],
    pub fog: Option<([f32; 3], f32)>,
    pub anim_sounds: Vec<String>,
    pub listener_right: Vec3,
    pub waterlevel: u8,
    /// Flat `entity_index -> Option<Vec3>` position table sized to `max_edicts` (`O(1)` lookup, zero hashing).
    pub entity_positions: Vec<Option<Vec3>>,
    pub impact_sounds: Vec<SoundStart>,
    pub radio_audio: Vec<(u8, String, u8)>,
    pub reload_sounds: Vec<(u8, u8)>,
    pub custom_decals: HashMap<u8, CustomDecalInfo>,
    pub alive: bool,
    pub observer_mode: i32,
    /// Observed target entity index (`cd.iuser2`), 0 when roaming or alive.
    pub observed_target: u8,
    /// Whether this frame was built from `DemoPlayer` (`.dem` playback).
    pub is_demo: bool,
    /// Active weapon short names per 1-based player entity index (`p_*.mdl` -> e.g. `"ak47"`).
    pub player_weapons: BTreeMap<u8, String>,
    pub stats: ViewStats,
}

fn empty_scene_frame() -> SceneFrame {
    SceneFrame {
        time: 0.0,
        view: ViewBasis::from_angles(Vec3::ZERO, Vec3::ZERO),
        entities: Vec::new(),
        shadows: Vec::new(),
    }
}

impl Default for FrameView {
    fn default() -> Self {
        Self {
            scene: empty_scene_frame(),
            eye: Vec3::ZERO,
            punch: Vec3::ZERO,
            fov: 90.0,
            bob: 0.0,
            roll_deg: 0.0,
            shake_angles: Vec3::ZERO,
            shake_z: 0.0,
            viewmodel_lag: Vec3::ZERO,
            underwater_tint: None,
            underwater_fov_delta: 0.0,
            viewmodel: None,
            beams: Vec::new(),
            points: Vec::new(),
            lights: GpuLightsUniform::default(),
            elights: Vec::new(),
            decals: Vec::new(),
            lightstyles: [1.0; 64],
            fog: None,
            anim_sounds: Vec::new(),
            listener_right: Vec3::Y,
            waterlevel: 0,
            entity_positions: Vec::new(),
            impact_sounds: Vec::new(),
            radio_audio: Vec::new(),
            reload_sounds: Vec::new(),
            custom_decals: HashMap::new(),
            alive: false,
            observer_mode: 0,
            observed_target: 0,
            is_demo: false,
            player_weapons: BTreeMap::new(),
            stats: ViewStats::default(),
        }
    }
}

impl FrameView {
    /// `O(1)` lookup of an entity's world position for this frame (zero hashing).
    #[inline]
    pub fn entity_position(&self, entity: u16) -> Option<Vec3> {
        self.entity_positions
            .get(usize::from(entity))
            .copied()
            .flatten()
    }

    /// Moves `self.scene` out for submission to [`Renderer::set_entity_scene`](crate::render::Renderer::set_entity_scene)
    /// without allocating a replacement buffer. Pair with [`Self::restore_scene`] after rendering
    /// so `entities` / `shadows` `Vec` capacities persist across frames.
    pub fn take_scene(&mut self) -> SceneFrame {
        std::mem::replace(&mut self.scene, empty_scene_frame())
    }

    /// Reinstalls a previously taken [`SceneFrame`] so its `Vec` capacities are reused next frame.
    pub fn restore_scene(&mut self, scene: SceneFrame) {
        self.scene = scene;
    }

    /// Clears per-frame vectors while preserving heap allocations for zero-alloc reuse.
    fn clear_for_reuse(&mut self) {
        self.scene.entities.clear();
        self.scene.shadows.clear();
        self.beams.clear();
        self.points.clear();
        self.elights.clear();
        self.decals.clear();
        self.anim_sounds.clear();
        self.entity_positions.fill(None);
        self.impact_sounds.clear();
        self.radio_audio.clear();
        self.reload_sounds.clear();
        self.stats = ViewStats::default();
    }
}

#[derive(Debug, Clone)]
struct ActiveBeam {
    die: f64,
    start: Vec3,
    end: Vec3,
    width: f32,
    color: [f32; 4],
}

#[derive(Debug, Clone)]
struct ActiveGlow {
    die: f64,
    origin: Vec3,
    size: f32,
    color: [f32; 4],
}

#[derive(Debug, Clone, Copy)]
struct ViewAnimState {
    sequence: i32,
    body: i32,
    started: f32,
}

struct CachedSeqMeta {
    fps: f32,
    num_frames: usize,
    looping: bool,
    events: Vec<MdlSeqEvent>,
}

/// Builds frames. Keeps what outlives one: the model registry, running beams, particles, dlights,
/// decals, player gait animation state, and the viewmodel animation + lag state.
pub struct ViewBuilder {
    models: ModelRegistry,
    models_key: u64,
    client_models: Vec<String>,
    brush_bounds: Vec<(Vec3, Vec3)>,
    names: Rc<RefCell<BTreeMap<u16, String>>>,
    beams: Vec<ActiveBeam>,
    glows: Vec<ActiveGlow>,
    dlights: DLightPool,
    particles: ParticleSystem,
    beam_sys: BeamSystem,
    pending_decals: Vec<DecalSpawn>,
    lag_state: ViewModelLagState,
    stair_smooth: StairSmoothState,
    bob_state: BobState,
    player_gaits: HashMap<u16, (f32, Vec3)>,
    vm_seq_cache: HashMap<String, Vec<CachedSeqMeta>>,
    hitbox_models: HashMap<String, Option<Arc<StudioModel>>>,
    last_vm_frame: f32,
    last_vm_path: String,
    last_vm_animtime: f32,
    vfs: Option<Vfs>,
    collision: Option<Arc<WorldCollision>>,
    bsp: Option<Arc<BspMap>>,
    materials: MaterialTable,
    anim: Option<ViewAnimState>,
    last_weaponanim: i32,
    last_viewmodel: i32,
    // Persistent scratch buffers reused across frames (zero steady-state heap allocations).
    spotlights_scratch: Vec<SpotLight>,
    shadow_origins_scratch: Vec<Vec3>,
    player_model_handles: [Option<ModelHandle>; 32],
    player_model_extra: Vec<(u8, ModelHandle)>,
    player_model_path_buf: String,
    vm_path_scratch: String,
    vm_key_scratch: String,
    pmodel_short_cache: HashMap<ModelHandle, Option<String>>,
}

#[derive(Debug, Clone, Copy)]
struct ViewCvarSnapshot {
    cl_minmodels: bool,
    cl_min_ct: i32,
    cl_min_t: i32,
    cl_shadows: bool,
    spec_xray: bool,
}

impl Default for ViewCvarSnapshot {
    fn default() -> Self {
        Self {
            cl_minmodels: false,
            cl_min_ct: 2,
            cl_min_t: 1,
            cl_shadows: true,
            spec_xray: false,
        }
    }
}

impl ViewCvarSnapshot {
    fn from_cvars(cvars: Option<&ClientCvars>) -> Self {
        match cvars {
            Some(c) => Self {
                cl_minmodels: c.float("cl_minmodels") != 0.0,
                cl_min_ct: c.float("cl_min_ct") as i32,
                cl_min_t: c.float("cl_min_t") as i32,
                cl_shadows: c.float("cl_shadows") != 0.0,
                spec_xray: c.float("spec_xray") != 0.0,
            },
            None => Self::default(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ClientDataSnapshot {
    origin: Vec3,
    view_ofs: Vec3,
    velocity: Vec3,
    punchangle: Vec3,
    fov: f32,
    weaponanim: i32,
    viewmodel: i32,
    alive: bool,
    observer_mode: i32,
    observed_target: u8,
}

impl ClientDataSnapshot {
    fn from_cd(cd: &cs_proto48::delta::GoldSrcClientData) -> Self {
        Self {
            origin: cd.origin,
            view_ofs: cd.view_ofs,
            velocity: cd.velocity,
            punchangle: cd.punchangle,
            fov: cd.fov,
            weaponanim: cd.weaponanim,
            viewmodel: cd.viewmodel,
            alive: is_client_alive(cd),
            observer_mode: cd.iuser1,
            observed_target: u8::try_from(cd.iuser2.max(0)).unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PredictionSnapshot {
    predicted: bool,
    simorg: Vec3,
    viewheight: Vec3,
    simvel: Vec3,
    punchangle: Vec3,
    onground: i32,
    watertype: i32,
    waterlevel: i32,
    effects: i32,
    weapon_fov: f32,
    weapon_anim: i32,
    weapon_viewmodel: i32,
    standing_on: Option<u16>,
}

impl PredictionSnapshot {
    fn from_prediction(p: &Prediction) -> Self {
        Self {
            predicted: p.predicted,
            simorg: p.simorg,
            viewheight: p.viewheight,
            simvel: p.simvel,
            punchangle: p.punchangle,
            onground: p.onground,
            watertype: p.watertype,
            waterlevel: p.waterlevel,
            effects: p.effects,
            weapon_fov: p.weapon.fov,
            weapon_anim: p.weapon.weaponanim,
            weapon_viewmodel: p.weapon.viewmodel,
            standing_on: p.standing_on(),
        }
    }
}

/// Zero-alloc lookup for `\key\value` pairs in a GoldSrc info string (`InfoString::parse` parity).
fn info_value<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    let trimmed = raw.strip_prefix('\\').unwrap_or(raw);
    let mut parts = trimmed.split('\\');
    while let Some(k) = parts.next() {
        if k.is_empty() {
            break;
        }
        let v = parts.next().unwrap_or("");
        if k.eq_ignore_ascii_case(key) {
            return Some(v);
        }
    }
    None
}

fn register_client_model_slot(client_models: &mut Vec<String>, path: &str) -> Option<usize> {
    if let Some(slot) = client_models.iter().position(|p| p == path) {
        return Some(slot);
    }
    if client_models.len() >= 256 {
        return None;
    }
    client_models.push(path.to_string());
    Some(client_models.len() - 1)
}

fn resolve_eye_and_vm(
    cd: Option<ClientDataSnapshot>,
    prediction: Option<PredictionSnapshot>,
) -> (Vec3, Vec3, f32, bool, i32, i32, i32) {
    match cd {
        Some(cd) => {
            let alive = cd.alive;
            let observer_mode = cd.observer_mode;
            let (pred_eye, pred_punch, pred_fov, pred_anim, pred_vm) = match prediction {
                Some(p) if p.predicted && alive && observer_mode == 0 => (
                    p.simorg + p.viewheight,
                    p.punchangle,
                    if p.weapon_fov > 0.0 {
                        p.weapon_fov
                    } else {
                        cd.fov
                    },
                    p.weapon_anim,
                    if p.weapon_viewmodel > 0 {
                        p.weapon_viewmodel
                    } else {
                        cd.viewmodel
                    },
                ),
                Some(p) if p.predicted => (
                    p.simorg + p.viewheight,
                    p.punchangle,
                    cd.fov,
                    cd.weaponanim,
                    cd.viewmodel,
                ),
                _ => (
                    cd.origin + cd.view_ofs,
                    cd.punchangle,
                    cd.fov,
                    cd.weaponanim,
                    cd.viewmodel,
                ),
            };
            (
                pred_eye,
                pred_punch,
                pred_fov,
                alive,
                observer_mode,
                pred_anim,
                pred_vm,
            )
        }
        None => (Vec3::ZERO, Vec3::ZERO, 0.0, false, 0, 0, 0),
    }
}

fn contains_ascii_ignore_case(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() {
        return true;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

impl Default for ViewBuilder {
    fn default() -> Self {
        Self::new()
    }
}

const FL_SPECTATOR: u32 = 1 << 12;
const WEAPON_SUIT_BIT: u32 = 1 << 31;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SpecXrayPolicy {
    Disabled,
    AllTeams,
    TeamOnly(String),
}

impl SpecXrayPolicy {
    fn allows(&self, team: &str, team_id: i16) -> bool {
        let norm = normalize_player_team(team, team_id);
        match self {
            Self::Disabled => false,
            Self::AllTeams => matches!(norm, Some("CT" | "TERRORIST")),
            Self::TeamOnly(own) => norm.is_some_and(|t| t == own),
        }
    }
}

fn normalize_player_team(team: &str, team_id: i16) -> Option<&'static str> {
    if team.eq_ignore_ascii_case("CT") {
        Some("CT")
    } else if team.eq_ignore_ascii_case("TERRORIST") {
        Some("TERRORIST")
    } else if team.eq_ignore_ascii_case("SPECTATOR") {
        Some("SPECTATOR")
    } else if !team.is_empty() {
        None
    } else {
        match team_id {
            1 => Some("TERRORIST"),
            2 => Some("CT"),
            3 => Some("SPECTATOR"),
            _ => None,
        }
    }
}

fn evaluate_spec_xray_policy(
    enabled: bool,
    is_demo: bool,
    alive: bool,
    observer_mode: i32,
    local_number: Option<u16>,
    fx: &ClientEffects,
) -> SpecXrayPolicy {
    if !enabled {
        return SpecXrayPolicy::Disabled;
    }
    if is_demo {
        return SpecXrayPolicy::AllTeams;
    }
    // Strict anti-cheat gate: NEVER render X-Ray while local player is alive or not observing.
    if alive || observer_mode == 0 {
        return SpecXrayPolicy::Disabled;
    }
    let Some(local_id) = local_number.and_then(|n| u8::try_from(n).ok()) else {
        return SpecXrayPolicy::Disabled;
    };
    let local_team = fx
        .hud
        .players
        .get(&local_id)
        .and_then(|s| normalize_player_team(&s.team, s.team_id));
    match local_team {
        Some("SPECTATOR") => SpecXrayPolicy::AllTeams,
        Some(team @ ("CT" | "TERRORIST")) => {
            let (forcecam, forcechasecam, fadetoblack) = fx.hud.force_cam.unwrap_or((0, 0, 0));
            if fadetoblack != 0 {
                SpecXrayPolicy::Disabled
            } else if forcecam != 0 || forcechasecam != 0 {
                SpecXrayPolicy::TeamOnly(team.to_string())
            } else {
                SpecXrayPolicy::AllTeams
            }
        }
        _ => SpecXrayPolicy::Disabled,
    }
}

fn weapon_short_from_pmodel(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next()?;
    let stem = file.strip_prefix("p_")?.strip_suffix(".mdl")?;
    let mapped = match stem {
        "mp5" => "mp5navy",
        other => other,
    };
    Some(mapped.to_string())
}

/// Resolve the forced player studio model path when `cl_minmodels != 0` from `TeamInfo` /
/// `ScoreAttrib` VIP bit and `cl_min_ct` / `cl_min_t` indices (`client.dll` `0x10105f08` table).
pub fn resolve_minmodel_path(
    team: &str,
    team_id: i16,
    attrib: u8,
    cl_min_ct: i32,
    cl_min_t: i32,
) -> Option<&'static str> {
    const SCORE_STATUS_VIP: u8 = 4;
    let is_t = team.eq_ignore_ascii_case("TERRORIST") || (team.is_empty() && team_id == 1);
    let is_ct = team.eq_ignore_ascii_case("CT") || (team.is_empty() && team_id == 2);
    if is_t {
        Some(match cl_min_t {
            5 => "models/player/guerilla/guerilla.mdl",
            6 => "models/player/arctic/arctic.mdl",
            8 => "models/player/terror/terror.mdl",
            11 => "models/player/militia/militia.mdl",
            _ => "models/player/leet/leet.mdl",
        })
    } else if is_ct {
        if (attrib & SCORE_STATUS_VIP) != 0 {
            return Some("models/player/vip/vip.mdl");
        }
        Some(match cl_min_ct {
            3 => "models/player/vip/vip.mdl",
            4 => "models/player/gsg9/gsg9.mdl",
            7 => "models/player/sas/sas.mdl",
            9 => "models/player/urban/urban.mdl",
            10 => "models/player/spetsnaz/spetsnaz.mdl",
            _ => "models/player/gign/gign.mdl",
        })
    } else {
        None
    }
}

/// GoldSrc `UpdateClientData` leaves `cd.deadflag == 0` and `cd.health == 100.0` while a
/// joining player sits on the intro camera, marking them with `FL_SPECTATOR` (`1 << 12`)
/// and omitting the HEV suit bit (`1 << WEAPON_SUIT`, bit 31). Observers set `cd.iuser1 != 0`.
pub(crate) fn is_client_alive(cd: &cs_proto48::delta::GoldSrcClientData) -> bool {
    cd.deadflag == 0
        && cd.health > 0.0
        && (cd.flags & FL_SPECTATOR) == 0
        && cd.iuser1 == 0
        && (cd.weapons & WEAPON_SUIT_BIT) != 0
}

impl ViewBuilder {
    pub fn new() -> Self {
        Self {
            models: ModelRegistry::default(),
            models_key: 0,
            client_models: Vec::new(),
            brush_bounds: Vec::new(),
            names: Rc::new(RefCell::new(BTreeMap::new())),
            beams: Vec::new(),
            glows: Vec::new(),
            dlights: DLightPool::default(),
            particles: ParticleSystem::new(),
            beam_sys: BeamSystem::new(),
            pending_decals: Vec::new(),
            lag_state: ViewModelLagState::default(),
            stair_smooth: StairSmoothState::default(),
            bob_state: BobState::default(),
            player_gaits: HashMap::new(),
            vm_seq_cache: HashMap::new(),
            hitbox_models: HashMap::new(),
            last_vm_frame: 0.0,
            last_vm_path: String::new(),
            last_vm_animtime: -1.0,
            vfs: None,
            collision: None,
            bsp: None,
            materials: MaterialTable::default(),
            anim: None,
            last_weaponanim: -1,
            last_viewmodel: -1,
            spotlights_scratch: Vec::new(),
            shadow_origins_scratch: Vec::new(),
            player_model_handles: [None; 32],
            player_model_extra: Vec::new(),
            player_model_path_buf: String::new(),
            vm_path_scratch: String::new(),
            vm_key_scratch: String::new(),
            pmodel_short_cache: HashMap::new(),
        }
    }

    /// Provide world collision and VFS so client view building can trace flashlights and read
    /// studio animation events.
    pub fn set_world_context(&mut self, collision: Option<Arc<WorldCollision>>, vfs: Option<Vfs>) {
        self.collision = collision;
        self.materials = vfs
            .as_ref()
            .map(MaterialTable::load)
            .unwrap_or_default();
        self.vfs = vfs;
        self.vm_seq_cache.clear();
        self.hitbox_models.clear();
    }

    pub fn set_map_bsp(&mut self, bsp: Option<Arc<BspMap>>) {
        self.bsp = bsp;
    }

    /// The shared model names a [`MapFxWorld`] reads.
    pub fn names(&self) -> Rc<RefCell<BTreeMap<u16, String>>> {
        Rc::clone(&self.names)
    }

    /// Current resolved model registry.
    pub fn models(&self) -> &ModelRegistry {
        &self.models
    }

    /// Drops per-map scene state on map change (`changelevel`).
    pub fn reset_map(&mut self) {
        self.beams.clear();
        self.glows.clear();
        self.dlights.clear();
        self.particles.clear();
        self.beam_sys.clear();
        self.pending_decals.clear();
        self.bsp = None;
        self.lag_state.reset();
        self.stair_smooth.reset();
        self.bob_state.reset();
        self.player_gaits.clear();
        self.hitbox_models.clear();
        self.client_models.clear();
        self.anim = None;
        self.last_weaponanim = -1;
        self.last_viewmodel = -1;
        self.last_vm_frame = 0.0;
        self.last_vm_path.clear();
        self.last_vm_animtime = -1.0;
        self.models_key = 0;
        self.pmodel_short_cache.clear();
    }

    /// The submodel bounds of the loaded map, for the translucent sort centres.
    pub fn set_map_bounds(&mut self, collision: Option<&WorldCollision>) {
        self.brush_bounds = collision.map_or_else(Vec::new, |c| {
            (0..c.model_count())
                .filter_map(|m| c.model_bounds(m))
                .collect()
        });
        self.models_key = 0;
    }

    fn ensure_registry(&mut self, precache: &cs_clientcore::PrecacheTables) -> bool {
        let mut h = DefaultHasher::new();
        precache.models.len().hash(&mut h);
        for (i, e) in &precache.models {
            i.hash(&mut h);
            e.name.hash(&mut h);
        }
        self.client_models.hash(&mut h);
        self.brush_bounds.len().hash(&mut h);
        let key = h.finish();
        if key == self.models_key && !self.models.is_empty() {
            return false;
        }
        self.models_key = key;
        let mut names: Vec<String> = vec![String::new(); usize::from(CLIENT_MODEL_BASE)];
        for (&i, e) in &precache.models {
            if usize::from(i) < names.len() {
                names[usize::from(i)] = e.name.clone();
            }
        }
        names.extend(self.client_models.iter().cloned());
        let mut registry = ModelRegistry::from_precache(&names);
        registry.set_brush_bounds(&self.brush_bounds);
        self.models = registry;
        self.pmodel_short_cache.clear();
        *self.names.borrow_mut() = precache
            .models
            .iter()
            .map(|(i, e)| (*i, e.name.clone()))
            .collect();
        true
    }

    /// The handle of a model a temp entity names; models the client loads itself get a slot
    /// past the server's table (the registry is rebuilt on the next frame).
    fn handle_of(&mut self, model: &ModelRef) -> Option<ModelHandle> {
        match model {
            ModelRef::None => None,
            ModelRef::Precached(i) => self.models.resolve(i32::from(*i)),
            ModelRef::Client(path) => self.client_handle(path),
        }
    }

    /// The handle of a model file the client loads by itself (a player's model by the userinfo
    /// `model`, the shells): a slot past the server's table. `None` the first time; the
    /// registry is rebuilt at the start of the next frame.
    fn client_handle(&mut self, path: &str) -> Option<ModelHandle> {
        let slot = register_client_model_slot(&mut self.client_models, path)?;
        self.models
            .resolve(i32::from(CLIENT_MODEL_BASE) + slot as i32)
    }

    fn player_model_handle(&self, slot: u8) -> Option<ModelHandle> {
        let idx = usize::from(slot);
        if idx < self.player_model_handles.len() {
            self.player_model_handles[idx]
        } else {
            self.player_model_extra
                .iter()
                .find(|&&(s, _)| s == slot)
                .map(|&(_, h)| h)
        }
    }

    /// Resolves per-player studio models from userinfo `model` and `cl_minmodels` into
    /// `self.player_model_handles` with zero steady-state allocations.
    fn resolve_player_model_handles(
        &mut self,
        userinfos: &BTreeMap<u8, String>,
        players: &BTreeMap<u8, cs_clientcore::hud::PlayerScore>,
        cvars: ViewCvarSnapshot,
        precache: &PrecacheTables,
    ) -> bool {
        let mut registry_changed = self.ensure_registry(precache);
        let prev_client_models = self.client_models.len();

        // Pass 1: register any newly seen userinfo / minmodel paths.
        for raw in userinfos.values() {
            if let Some(m) = info_value(raw, "model")
                && !m.is_empty()
                && m.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                self.player_model_path_buf.clear();
                self.player_model_path_buf.push_str("models/player/");
                self.player_model_path_buf.push_str(m);
                self.player_model_path_buf.push('/');
                self.player_model_path_buf.push_str(m);
                self.player_model_path_buf.push_str(".mdl");
                let _ = register_client_model_slot(
                    &mut self.client_models,
                    &self.player_model_path_buf,
                );
            }
        }
        if cvars.cl_minmodels {
            for score in players.values() {
                if let Some(path) = resolve_minmodel_path(
                    &score.team,
                    score.team_id,
                    score.attrib,
                    cvars.cl_min_ct,
                    cvars.cl_min_t,
                ) {
                    let _ = register_client_model_slot(&mut self.client_models, path);
                }
            }
        }
        if self.client_models.len() != prev_client_models && self.ensure_registry(precache) {
            registry_changed = true;
        }

        // Pass 2: resolve slot -> ModelHandle into fixed inline array (0 allocs).
        self.player_model_handles = [None; 32];
        self.player_model_extra.clear();
        for (&slot, raw) in userinfos {
            if let Some(m) = info_value(raw, "model")
                && !m.is_empty()
                && m.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                self.player_model_path_buf.clear();
                self.player_model_path_buf.push_str("models/player/");
                self.player_model_path_buf.push_str(m);
                self.player_model_path_buf.push('/');
                self.player_model_path_buf.push_str(m);
                self.player_model_path_buf.push_str(".mdl");
                if let Some(cm_slot) = self
                    .client_models
                    .iter()
                    .position(|p| p == &self.player_model_path_buf)
                    && let Some(h) = self
                        .models
                        .resolve(i32::from(CLIENT_MODEL_BASE) + cm_slot as i32)
                {
                    let idx = usize::from(slot);
                    if idx < self.player_model_handles.len() {
                        self.player_model_handles[idx] = Some(h);
                    } else {
                        self.player_model_extra.push((slot, h));
                    }
                }
            }
        }
        if cvars.cl_minmodels {
            for (&player_id, score) in players {
                if let Some(slot0) = player_id.checked_sub(1)
                    && let Some(path) = resolve_minmodel_path(
                        &score.team,
                        score.team_id,
                        score.attrib,
                        cvars.cl_min_ct,
                        cvars.cl_min_t,
                    )
                    && let Some(cm_slot) = self.client_models.iter().position(|p| p == path)
                    && let Some(h) = self
                        .models
                        .resolve(i32::from(CLIENT_MODEL_BASE) + cm_slot as i32)
                {
                    let idx = usize::from(slot0);
                    if idx < self.player_model_handles.len() {
                        self.player_model_handles[idx] = Some(h);
                    } else if let Some(entry) =
                        self.player_model_extra.iter_mut().find(|(s, _)| *s == slot0)
                    {
                        entry.1 = h;
                    } else {
                        self.player_model_extra.push((slot0, h));
                    }
                }
            }
        }
        registry_changed
    }

    fn capture_vm_path(&mut self, precache: &PrecacheTables, viewmodel_index: i32) {
        self.vm_path_scratch.clear();
        if let Ok(idx) = u16::try_from(viewmodel_index)
            && let Some(path) = precache.model(idx)
        {
            self.vm_path_scratch.push_str(path);
        }
    }

    /// Takes the frame of connection `conn` at session time `inputs.now`, reusing `out`'s
    /// internal `Vec` buffers across frames (`0` steady-state heap allocations per frame).
    ///
    /// Returns the registry too when it changed, so the renderer can install it.
    pub fn build_into<T: PacketTransport>(
        &mut self,
        out: &mut FrameView,
        conn: &mut Connection<T>,
        view_angles: Vec3,
        inputs: &ViewInputs,
    ) -> Option<ModelRegistry> {
        let now = inputs.now;
        conn.frames_mut().advance(inputs.frametime.clamp(0.0, 0.25));
        let weapon_state = {
            let p = conn.predictor().prediction();
            p.weapon
                .weapons
                .get(&(p.weapon.weapon_id.clamp(0, 255) as u8))
                .map_or(0, |w| w.m_i_weapon_state)
        };
        let (max_smokepuffs, fastsprites, cvar_snap) = {
            let cv = conn.cvars();
            (
                cv.float("max_smokepuffs").max(0.0) as usize,
                cv.float("fastsprites").clamp(0.0, 2.0) as u8,
                ViewCvarSnapshot::from_cvars(Some(cv)),
            )
        };
        conn.fx_mut().set_smoke_cvars(max_smokepuffs, fastsprites);
        conn.set_fx_local(weapon_state, false, inputs.right_hand);
        let fired = conn.update_effects(now);
        let cd_snap = conn.client_data().map(ClientDataSnapshot::from_cd);
        let slot = conn.player_slot();
        let pred_snap = PredictionSnapshot::from_prediction(conn.predictor().prediction());
        let max_edicts =
            goldsrc_max_edicts(conn.server_info().map_or(32, |info| info.max_clients));
        let (_, _, _, _, _, _, viewmodel_index) = resolve_eye_and_vm(cd_snap, Some(pred_snap));

        let mut registry_changed = self.resolve_player_model_handles(
            conn.userinfos(),
            &conn.fx().hud.players,
            cvar_snap,
            conn.precache(),
        );
        self.capture_vm_path(conn.precache(), viewmodel_index);

        let emitted = conn.frames_mut().emit(&EmitInputs {
            simorg: pred_snap
                .predicted
                .then_some(pred_snap.simorg)
                .or_else(|| cd_snap.map(|c| c.origin)),
            standing_on: pred_snap.standing_on,
        });
        let added_tempent_model = self.build_parts_into_inner(
            out,
            conn.fx_mut(),
            cd_snap,
            slot,
            Some(pred_snap),
            &emitted,
            &fired,
            view_angles,
            inputs,
            cvar_snap,
            false,
            max_edicts,
        );
        if added_tempent_model && self.ensure_registry(conn.precache()) {
            registry_changed = true;
        }
        registry_changed.then(|| self.models.clone())
    }

    /// Takes the frame of connection `conn` at session time `inputs.now`: advances the client
    /// clock, fires the due events, steps the temp entities, interpolates the entities.
    ///
    /// Returns the registry too when it changed, so the renderer can install it.
    pub fn build<T: PacketTransport>(
        &mut self,
        conn: &mut Connection<T>,
        view_angles: Vec3,
        inputs: &ViewInputs,
    ) -> (FrameView, Option<ModelRegistry>) {
        let mut out = FrameView::default();
        let reg = self.build_into(&mut out, conn, view_angles, inputs);
        (out, reg)
    }

    /// Builds a [`FrameView`] from a [`DemoPlayer`] session frame into `out` for buffer reuse.
    pub fn build_demo_into(
        &mut self,
        out: &mut FrameView,
        player: &mut DemoPlayer,
        fired: Vec<FiredEvent>,
        view_angles: Vec3,
        inputs: &ViewInputs,
    ) -> Option<ModelRegistry> {
        let cd_snap = player.client_data().map(ClientDataSnapshot::from_cd);
        let slot = player.server_info().map(|i| i.player_slot);
        let max_edicts =
            goldsrc_max_edicts(player.server_info().map_or(32, |info| info.max_clients));
        let (_, _, _, _, _, _, viewmodel_index) = resolve_eye_and_vm(cd_snap, None);
        let cvar_snap = ViewCvarSnapshot::default();
        let mut registry_changed = self.resolve_player_model_handles(
            player.userinfos(),
            &player.hud().players,
            cvar_snap,
            player.precache(),
        );
        self.capture_vm_path(player.precache(), viewmodel_index);
        let emitted = player.frames_mut().emit(&EmitInputs {
            simorg: cd_snap.map(|c| c.origin),
            standing_on: None,
        });
        let added_tempent_model = self.build_parts_into_inner(
            out,
            player.fx_mut(),
            cd_snap,
            slot,
            None,
            &emitted,
            &fired,
            view_angles,
            inputs,
            cvar_snap,
            true,
            max_edicts,
        );
        if added_tempent_model && self.ensure_registry(player.precache()) {
            registry_changed = true;
        }
        registry_changed.then(|| self.models.clone())
    }

    /// Builds a [`FrameView`] from a [`DemoPlayer`] session frame.
    pub fn build_demo(
        &mut self,
        player: &mut DemoPlayer,
        fired: Vec<FiredEvent>,
        view_angles: Vec3,
        inputs: &ViewInputs,
    ) -> (FrameView, Option<ModelRegistry>) {
        let mut out = FrameView::default();
        let reg = self.build_demo_into(&mut out, player, fired, view_angles, inputs);
        (out, reg)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_parts(
        &mut self,
        precache: &PrecacheTables,
        fx: &mut ClientEffects,
        userinfos: &BTreeMap<u8, String>,
        cd: Option<cs_proto48::delta::GoldSrcClientData>,
        slot: Option<u8>,
        prediction: Option<&Prediction>,
        emitted: cs_clientcore::interp::EmitResult,
        fired: Vec<FiredEvent>,
        view_angles: Vec3,
        inputs: &ViewInputs,
    ) -> (FrameView, Option<ModelRegistry>) {
        let mut out = FrameView::default();
        let reg = self.build_parts_with_is_demo_into(
            &mut out,
            precache,
            fx,
            userinfos,
            cd,
            slot,
            prediction,
            emitted,
            fired,
            view_angles,
            inputs,
            None,
            false,
        );
        (out, reg)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_parts_into(
        &mut self,
        out: &mut FrameView,
        precache: &PrecacheTables,
        fx: &mut ClientEffects,
        userinfos: &BTreeMap<u8, String>,
        cd: Option<cs_proto48::delta::GoldSrcClientData>,
        slot: Option<u8>,
        prediction: Option<&Prediction>,
        emitted: cs_clientcore::interp::EmitResult,
        fired: Vec<FiredEvent>,
        view_angles: Vec3,
        inputs: &ViewInputs,
    ) -> Option<ModelRegistry> {
        self.build_parts_with_is_demo_into(
            out,
            precache,
            fx,
            userinfos,
            cd,
            slot,
            prediction,
            emitted,
            fired,
            view_angles,
            inputs,
            None,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_parts_with_cvars(
        &mut self,
        precache: &PrecacheTables,
        fx: &mut ClientEffects,
        userinfos: &BTreeMap<u8, String>,
        cd: Option<cs_proto48::delta::GoldSrcClientData>,
        slot: Option<u8>,
        prediction: Option<&Prediction>,
        emitted: cs_clientcore::interp::EmitResult,
        fired: Vec<FiredEvent>,
        view_angles: Vec3,
        inputs: &ViewInputs,
        cvars: Option<&ClientCvars>,
    ) -> (FrameView, Option<ModelRegistry>) {
        let mut out = FrameView::default();
        let reg = self.build_parts_with_is_demo_into(
            &mut out,
            precache,
            fx,
            userinfos,
            cd,
            slot,
            prediction,
            emitted,
            fired,
            view_angles,
            inputs,
            cvars,
            false,
        );
        (out, reg)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_parts_with_is_demo(
        &mut self,
        precache: &PrecacheTables,
        fx: &mut ClientEffects,
        userinfos: &BTreeMap<u8, String>,
        cd: Option<cs_proto48::delta::GoldSrcClientData>,
        slot: Option<u8>,
        prediction: Option<&Prediction>,
        emitted: cs_clientcore::interp::EmitResult,
        fired: Vec<FiredEvent>,
        view_angles: Vec3,
        inputs: &ViewInputs,
        cvars: Option<&ClientCvars>,
        is_demo: bool,
    ) -> (FrameView, Option<ModelRegistry>) {
        let mut out = FrameView::default();
        let reg = self.build_parts_with_is_demo_into(
            &mut out,
            precache,
            fx,
            userinfos,
            cd,
            slot,
            prediction,
            emitted,
            fired,
            view_angles,
            inputs,
            cvars,
            is_demo,
        );
        (out, reg)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_parts_with_is_demo_into(
        &mut self,
        out: &mut FrameView,
        precache: &PrecacheTables,
        fx: &mut ClientEffects,
        userinfos: &BTreeMap<u8, String>,
        cd: Option<cs_proto48::delta::GoldSrcClientData>,
        slot: Option<u8>,
        prediction: Option<&Prediction>,
        emitted: cs_clientcore::interp::EmitResult,
        fired: Vec<FiredEvent>,
        view_angles: Vec3,
        inputs: &ViewInputs,
        cvars: Option<&ClientCvars>,
        is_demo: bool,
    ) -> Option<ModelRegistry> {
        let cd_snap = cd.as_ref().map(ClientDataSnapshot::from_cd);
        let pred_snap = prediction.map(PredictionSnapshot::from_prediction);
        let cvar_snap = ViewCvarSnapshot::from_cvars(cvars);
        let (_, _, _, _, _, _, viewmodel_index) = resolve_eye_and_vm(cd_snap, pred_snap);
        let mut registry_changed =
            self.resolve_player_model_handles(userinfos, &fx.hud.players, cvar_snap, precache);
        self.capture_vm_path(precache, viewmodel_index);
        let added_tempent_model = self.build_parts_into_inner(
            out,
            fx,
            cd_snap,
            slot,
            pred_snap,
            &emitted,
            &fired,
            view_angles,
            inputs,
            cvar_snap,
            is_demo,
            DEFAULT_MAX_EDICTS,
        );
        if added_tempent_model && self.ensure_registry(precache) {
            registry_changed = true;
        }
        registry_changed.then(|| self.models.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn build_parts_into_inner(
        &mut self,
        out: &mut FrameView,
        fx: &mut ClientEffects,
        cd: Option<ClientDataSnapshot>,
        slot: Option<u8>,
        prediction: Option<PredictionSnapshot>,
        emitted: &cs_clientcore::interp::EmitResult,
        fired: &[FiredEvent],
        view_angles: Vec3,
        inputs: &ViewInputs,
        cvars: ViewCvarSnapshot,
        is_demo: bool,
        max_edicts: usize,
    ) -> bool {
        out.clear_for_reuse();
        let now = inputs.now;
        let local_number = slot.map(|s| u16::from(s) + 1);
        let (mut eye, mut punch, mut fov, alive, observer_mode, weaponanim, viewmodel_index) =
            resolve_eye_and_vm(cd, prediction);
        if !eye.is_finite() {
            eye = Vec3::ZERO;
        }
        let dt_view = inputs.frametime.clamp(0.0, 0.25) as f32;
        let on_ground_pred = prediction.is_some_and(|p| p.predicted && p.onground >= 0 && alive);
        eye.z += self.stair_smooth.offset(eye.z, on_ground_pred, dt_view);
        // `client.dll` `0x1006b21b` (`V_CalcRefdef`): +1/32 unit vieworg nudge prevents BSP plane flicker.
        eye += Vec3::splat(0.03125);
        if !punch.is_finite() {
            punch = Vec3::ZERO;
        }
        let fallback_fov = if inputs.default_fov.is_finite() {
            inputs.default_fov.clamp(10.0, 120.0)
        } else {
            90.0
        };
        if !fov.is_finite() || !(10.0..=120.0).contains(&fov) {
            fov = fallback_fov;
        }

        let time = now as f32;
        out.scene.time = time;
        out.scene.view = ViewBasis::from_angles(eye, view_angles + punch);

        let dt = inputs.frametime.clamp(0.0, 0.25) as f32;
        self.dlights.step(now, dt);
        self.particles.step(now, dt);

        // Local player flashlight spotlight (`impulse 100` -> Flashlight HUD/prediction).
        self.spotlights_scratch.clear();
        let pred_effects = prediction.map_or(0, |p| p.effects);
        if alive && (fx.hud.flashlight_on || (pred_effects as u32 & ef::DIMLIGHT) != 0) {
            let (fwd, _, _) = crate::render::scene::angle_vectors(view_angles + punch);
            let (spot, bounce) = compute_flashlight_spot(
                eye,
                fwd,
                self.collision.as_deref(),
                now,
            );
            self.spotlights_scratch.push(spot);
            if let Some(b) = bounce {
                self.dlights.add_light(b);
            }
        }

        self.shadow_origins_scratch.clear();
        let observed_target = cd.map_or(0, |c| c.observed_target);
        let xray_enabled = inputs.spec_xray || cvars.spec_xray;
        let xray_policy = evaluate_spec_xray_policy(
            xray_enabled,
            is_demo,
            alive,
            observer_mode,
            local_number,
            fx,
        );

        // Ensure flat entity position table covers `max_edicts` and every active entity index (`O(1)`).
        let max_ent = emitted
            .entities
            .iter()
            .map(|e| usize::from(e.number) + 1)
            .max()
            .unwrap_or(0);
        let target_edicts = max_ent.max(max_edicts);
        if out.entity_positions.len() < target_edicts {
            out.entity_positions.resize(target_edicts, None);
        }

        // Entity loop: build RenderEntities + collect entity EF_* lights & flat world positions.
        let mut seen_weapon_players: u32 = 0;
        for e in &emitted.entities {
            out.entity_positions[usize::from(e.number)] = Some(e.origin);
            if e.kind == EntityKind::Player
                && let Some(pid) = u8::try_from(e.number).ok().filter(|&n| (1..=32).contains(&n))
                && e.state.weaponmodel != 0
                && let Some(h) = self.models.resolve(i32::from(e.state.weaponmodel))
            {
                let models = &self.models;
                let short = self.pmodel_short_cache.entry(h).or_insert_with(|| {
                    models.get(h).and_then(|m| weapon_short_from_pmodel(&m.name))
                });
                if let Some(wname) = short.as_deref() {
                    seen_weapon_players |= 1u32 << (pid - 1);
                    if let Some(existing) = out.player_weapons.get_mut(&pid) {
                        if existing != wname {
                            existing.clear();
                            existing.push_str(wname);
                        }
                    } else {
                        out.player_weapons.insert(pid, wname.to_string());
                    }
                }
            }
            if e.kind == EntityKind::Beam {
                self.push_beam_entity(e, now);
                continue;
            }
            if e.local_player || Some(e.number) == local_number {
                continue;
            }
            let eff = e.state.effects;
            if (eff & ef::BRIGHTLIGHT) != 0 {
                self.dlights.add_light(DynamicLight {
                    key: e.number,
                    entity_light: false,
                    origin: e.origin + Vec3::new(0.0, 0.0, 16.0),
                    radius: 400.0,
                    color: [250, 250, 200],
                    die: now + 0.05,
                    decay: 0.0,
                });
            }
            if (eff & ef::DIMLIGHT) != 0 {
                if e.kind == EntityKind::Player {
                    let (fwd, _, _) = crate::render::scene::angle_vectors(e.angles);
                    let (spot, bounce) = compute_flashlight_spot(
                        e.origin + Vec3::new(0.0, 0.0, 24.0),
                        fwd,
                        self.collision.as_deref(),
                        now,
                    );
                    self.spotlights_scratch.push(spot);
                    if let Some(b) = bounce {
                        self.dlights.add_light(b);
                    }
                } else {
                    self.dlights.add_light(DynamicLight {
                        key: e.number,
                        entity_light: false,
                        origin: e.origin,
                        radius: 200.0,
                        color: [220, 220, 200],
                        die: now + 0.05,
                        decay: 0.0,
                    });
                }
            }
            if (eff & ef::MUZZLEFLASH) != 0 {
                let (fwd, _, _) = crate::render::scene::angle_vectors(e.angles);
                self.dlights.add_light(DynamicLight {
                    key: e.number.wrapping_add(1024),
                    entity_light: false,
                    origin: e.origin + Vec3::new(0.0, 0.0, 20.0) + fwd * 24.0,
                    radius: 200.0,
                    color: [255, 190, 90],
                    die: now + 0.06,
                    decay: 1500.0,
                });
            }
            if (eff & ef::LIGHT) != 0 {
                self.dlights.add_light(DynamicLight {
                    key: e.number.wrapping_add(2048),
                    entity_light: false,
                    origin: e.origin,
                    radius: 200.0,
                    color: [180, 180, 160],
                    die: now + 0.05,
                    decay: 0.0,
                });
            }
            if (eff & ef::BRIGHTFIELD) != 0 {
                self.particles.spawn_effect(
                    now,
                    &cs_clientcore::tempents::ParticleEffect::Flicker { position: e.origin },
                );
            }
            if let Some(mut r) = self.render_entity(e, dt, time) {
                match e.model {
                    CoreModelKind::Brush => out.stats.brush += 1,
                    CoreModelKind::Studio => {
                        out.stats.studio += 1;
                        if is_shadow_eligible(e, &r, &self.models, &fx.hud.players) {
                            self.shadow_origins_scratch.push(e.origin);
                        }
                        if r.player
                            && r.renderfx != RENDERFX_DEAD_PLAYER
                            && (r.effects & EF_NODRAW) == 0
                            && let Some(pid) = u8::try_from(e.number).ok()
                            && let Some(score) = fx.hud.players.get(&pid)
                            && (score.attrib & 1) == 0
                            && xray_policy.allows(&score.team, score.team_id)
                        {
                            r.xray_color = match normalize_player_team(&score.team, score.team_id) {
                                Some("CT") => Some([0.25, 0.55, 1.0]),
                                Some("TERRORIST") => Some([1.0, 0.35, 0.20]),
                                _ => None,
                            };
                        }
                    }
                    CoreModelKind::Sprite => out.stats.sprites += 1,
                }
                out.scene.entities.push(r);
            }
        }
        if seen_weapon_players == 0 {
            out.player_weapons.clear();
        } else {
            out.player_weapons
                .retain(|&pid, _| (1..=32).contains(&pid) && (seen_weapon_players & (1u32 << (pid - 1))) != 0);
        }
        out.stats.entities = out.scene.entities.len();

        // Temp entities: shells, sprites, gibs (zero TempEnt cloning!).
        let prev_client_models = self.client_models.len();
        for t in fx.tempents.render_list() {
            if let Some(r) = self.render_tempent(t, out.scene.entities.len() as u16) {
                out.stats.tempents += 1;
                out.scene.entities.push(r);
            }
        }
        let added_tempent_client_model = self.client_models.len() != prev_client_models;
        if cvars.cl_shadows
            && !self.shadow_origins_scratch.is_empty()
            && let Some(world) = self.collision.as_deref()
        {
            let solid_colliders = collect_solid_brush_colliders(&out.scene.entities, &self.models);
            for &origin in &self.shadow_origins_scratch {
                if let Some(sh) = trace_studio_shadow(world, &solid_colliders, origin, 18.0) {
                    out.scene.shadows.push(sh);
                }
            }
        }
        self.apply_glow_occlusion(&mut out.scene);

        // Effects that are not entries of the pool (O(1) flat position lookup).
        let positions = &out.entity_positions;
        let pos_lookup = |id: u16| positions.get(usize::from(id)).copied().flatten();
        for effect in fx.tempents.take_effects() {
            self.take_effect(effect, now, &pos_lookup);
        }
        let weather = if inputs.cl_weather { fx.hud.weather } else { 0 };
        self.particles.step_weather(now, dt, weather, eye);
        self.beam_sys.step(now, &pos_lookup);
        self.beams.retain(|b| b.die > now);
        self.glows.retain(|g| g.die > now);
        let overflow = self.beams.len().saturating_sub(MAX_ACTIVE_FX);
        self.beams.drain(..overflow);
        let overflow = self.glows.len().saturating_sub(MAX_ACTIVE_FX);
        self.glows.drain(..overflow);

        let prev_vm_anim = self.anim;
        let show_client_impacts = inputs.sv_cheats && matches!(inputs.sv_showimpacts, 1 | 2);
        self.collect_bullet_impacts_into(
            fired,
            &out.scene.entities,
            now,
            show_client_impacts,
            &mut out.impact_sounds,
        );
        self.update_view_anim(fx, fired, weaponanim, viewmodel_index, time);
        self.populate_viewmodel_into(&mut out.viewmodel, alive, fov, viewmodel_index, inputs);
        if let Some(ref vm) = out.viewmodel {
            self.sample_viewmodel_events_into(
                vm,
                prev_vm_anim,
                time,
                now,
                eye,
                view_angles + punch,
                &mut out.anim_sounds,
            );
        }

        out.beams.extend(self.beams.iter().map(|b| FxBeam {
            start: b.start,
            end: b.end,
            width: b.width,
            color: b.color,
            additive: true,
        }));
        self.beam_sys.emit_fx(now, &mut out.beams);
        out.points.extend(self.glows.iter().map(|g| {
            let fade = ((g.die - now) / 0.25).clamp(0.0, 1.0) as f32;
            FxPoint {
                origin: g.origin,
                size: g.size,
                color: [g.color[0], g.color[1], g.color[2], g.color[3] * fade],
                additive: true,
            }
        }));
        self.particles.emit_fx(now, &mut out.beams, &mut out.points);
        if inputs.sv_cheats && inputs.sv_showhitboxes {
            self.emit_client_hitboxes(&out.scene.entities, &mut out.beams, time);
        }
        out.stats.beams = out.beams.len();

        let simvel = prediction
            .map(|p| p.simvel)
            .or_else(|| cd.map(|c| c.velocity))
            .unwrap_or(Vec3::ZERO);
        let onground = prediction.is_none_or(|p| p.onground >= 0);
        let watertype = prediction.map_or(0, |p| p.watertype);
        let waterlevel = prediction.map_or(0, |p| p.waterlevel.max(0) as u8);

        let bob = self.bob_state.calc(
            time,
            simvel,
            onground,
            &BobParams {
                cl_bob: inputs.cl_bob,
                cl_bobcycle: inputs.cl_bobcycle,
                cl_bobup: inputs.cl_bobup,
            },
        );
        let roll_deg = calc_roll(
            view_angles.y,
            simvel,
            inputs.cl_rollangle,
            inputs.cl_rollspeed,
        );
        let (shake_angles, shake_z) = calc_screen_shake(fx.hud.screen_shake.as_ref(), now);
        let viewmodel_lag =
            self.lag_state
                .update(out.scene.view.forward, dt, inputs.cl_viewmodel_lag);
        let (underwater_tint, underwater_fov_delta) =
            underwater_tint_and_fov(watertype, waterlevel, time);

        out.eye = eye;
        out.punch = punch;
        out.fov = fov;
        out.bob = bob;
        out.roll_deg = roll_deg;
        out.shake_angles = shake_angles;
        out.shake_z = shake_z;
        out.viewmodel_lag = viewmodel_lag;
        out.underwater_tint = underwater_tint;
        out.underwater_fov_delta = underwater_fov_delta;
        out.lights = self.dlights.pack_uniform(eye, &self.spotlights_scratch);
        out.elights.extend_from_slice(&self.dlights.elights);
        out.decals.append(&mut self.pending_decals);
        out.lightstyles = evaluate_lightstyles(&fx.hud.lightstyles, time);
        out.fog = fx.hud.fog.map(|f| {
            (
                [
                    f32::from(f.color[0]) / 255.0,
                    f32::from(f.color[1]) / 255.0,
                    f32::from(f.color[2]) / 255.0,
                ],
                f.density,
            )
        });
        out.listener_right = out.scene.view.right;
        out.waterlevel = waterlevel;
        out.radio_audio.extend(
            fx.hud
                .audio
                .drain(..)
                .map(|(sender, code, pitch)| (sender, code, pitch.clamp(0, 255) as u8)),
        );
        out.reload_sounds.extend(fx.hud.reload_sounds.drain(..));
        out.custom_decals.clear();
        if !fx.hud.custom_decals.is_empty() {
            out.custom_decals
                .extend(fx.hud.custom_decals.iter().map(|(&k, v)| (k, v.clone())));
        }
        out.alive = alive;
        out.observer_mode = observer_mode;
        out.observed_target = observed_target;
        out.is_demo = is_demo;

        added_tempent_client_model
    }

    #[cfg(test)]
    fn collect_bullet_impacts(
        &mut self,
        fired: &[FiredEvent],
        scene_entities: &[RenderEntity],
        now: f64,
        show_client_impacts: bool,
    ) -> Vec<SoundStart> {
        let mut out = Vec::new();
        self.collect_bullet_impacts_into(fired, scene_entities, now, show_client_impacts, &mut out);
        out
    }

    fn collect_bullet_impacts_into(
        &mut self,
        fired: &[FiredEvent],
        scene_entities: &[RenderEntity],
        now: f64,
        show_client_impacts: bool,
        sounds: &mut Vec<SoundStart>,
    ) {
        let Some(ref coll) = self.collision else {
            return;
        };
        if !fired
            .iter()
            .any(|ev| matches!(&ev.fx, EventFx::Weapon(w) if w.bullets.is_some()))
        {
            return;
        }
        let brush_targets: Vec<(u16, u16, BrushCollider)> = scene_entities
            .iter()
            .filter(|e| (e.effects & EF_NODRAW) == 0 && e.solid == SOLID_BSP)
            .filter_map(|e| {
                let entry = self.models.get(e.model)?;
                match entry.kind {
                    ModelKind::Brush(submodel) => Some((
                        e.index,
                        e.model.0,
                        BrushCollider {
                            submodel,
                            origin: e.origin,
                            angles: e.angles,
                        },
                    )),
                    _ => None,
                }
            })
            .collect();
        let base_seed = (now * 1000.0) as u32;
        for (ev_idx, ev) in fired.iter().enumerate() {
            let EventFx::Weapon(w) = &ev.fx else {
                continue;
            };
            let Some(ref fire) = w.bullets else {
                continue;
            };
            let shots = fire.shots.min(12);
            for shot in 0..shots {
                let seed = base_seed
                    .wrapping_add((ev_idx as u32).wrapping_mul(131))
                    .wrapping_add(shot.wrapping_mul(17));
                // Single-bullet events already carry exact (dx, dy) displacement in `fire.spread`;
                // shotguns pass a cone radius (`Spread::Fixed`) scaled per pellet.
                let (sx, sy) = if shots == 1 {
                    (fire.spread[0], fire.spread[1])
                } else {
                    let rx = (((seed & 0xFF) as f32) / 255.0 - 0.5) * 2.0 * fire.spread[0];
                    let ry = ((((seed >> 8) & 0xFF) as f32) / 255.0 - 0.5) * 2.0 * fire.spread[1];
                    (rx, ry)
                };
                let dir = (fire.forward + fire.right * sx + fire.up * sy).normalize_or_zero();
                let dist = fire.distance.clamp(64.0, 8192.0);
                let end = fire.origin + dir * dist;
                let world_tr = coll.trace_world(fire.origin, end, Hull::Point);

                let mut best_fraction = world_tr.fraction;
                let mut best_pos = world_tr.end_pos;
                let mut best_normal = world_tr.plane_normal;
                let mut hit_entity: u16 = 0;
                let mut hit_model: u16 = 0;
                let mut hit_brush: Option<&BrushCollider> = None;

                for (ent_idx, model_idx, collider) in &brush_targets {
                    let tr = trace_brush_collider(coll, collider, fire.origin, end);
                    if !tr.start_solid && !tr.all_solid && tr.fraction < best_fraction {
                        best_fraction = tr.fraction;
                        best_pos = tr.end_pos;
                        best_normal = tr.plane_normal;
                        hit_entity = *ent_idx;
                        hit_model = *model_idx;
                        hit_brush = Some(collider);
                    }
                }
                if best_fraction >= 1.0 || best_fraction <= 0.001 {
                    continue;
                }
                let tex_type = self.surface_char_at(best_pos, best_normal, hit_brush);
                sounds.extend(crate::audio::build_impact_sounds(best_pos, tex_type, seed));
                if show_client_impacts {
                    let half = Vec3::splat(2.0);
                    self.particles.spawn_effect(
                        now,
                        &cs_clientcore::tempents::ParticleEffect::Box {
                            min: best_pos - half,
                            max: best_pos + half,
                            life: 4.0,
                            color: [60, 140, 255],
                        },
                    );
                }
                if self.pending_decals.len() < 128 {
                    self.pending_decals.push(DecalSpawn {
                        origin: best_pos,
                        decal_name: format!("{{shot{}", (seed % 5) + 1),
                        half_size: 4.0,
                        permanent: false,
                        player_spray: None,
                        entity_index: hit_entity,
                        model_index: hit_model,
                    });
                }
            }
        }
    }

    fn emit_client_hitboxes(
        &mut self,
        entities: &[RenderEntity],
        beams: &mut Vec<FxBeam>,
        now_sec: f32,
    ) {
        let Some(ref vfs) = self.vfs else {
            return;
        };
        for ent in entities {
            if (ent.effects & EF_NODRAW) != 0 || !ent.player {
                continue;
            }
            let Some(entry) = self.models.get(ent.model) else {
                continue;
            };
            if entry.kind != ModelKind::Studio {
                continue;
            }
            let key = entry.name.to_ascii_lowercase();
            let mdl = self
                .hitbox_models
                .entry(key)
                .or_insert_with(|| StudioModel::load(vfs, &entry.name).ok().map(Arc::new))
                .clone();
            let Some(mdl) = mdl else {
                continue;
            };
            if mdl.hitboxes.is_empty() || mdl.sequences.is_empty() {
                continue;
            }
            let seq_idx = (ent.sequence.max(0) as usize).min(mdl.sequences.len() - 1);
            let seq = &mdl.sequences[seq_idx];
            let frame = studio_frame(
                ent.frame,
                ent.animtime,
                ent.framerate,
                now_sec,
                seq.fps,
                seq.num_frames,
                seq.flags & STUDIO_LOOPING != 0,
            );
            let bones = mdl.evaluate_bones_full(&cs_assets::mdl::BoneEvalParams {
                sequence: seq_idx,
                frame,
                loop_anim: false,
                blending: ent.blending,
                controller: ent.controllers,
                mouth: ent.mouth,
                prev_sequence: None,
                gait: ent.gait.map(|(gs, gf)| ((gs.max(0) as usize).min(mdl.sequences.len() - 1), gf)),
            });
            let ent_mat = crate::render::scene::studio_matrix(
                ent.origin,
                ent.angles,
                ent.scale,
                ent.player,
            );
            const EDGES: [(usize, usize); 12] = [
                (0, 1), (1, 2), (2, 3), (3, 0),
                (4, 5), (5, 6), (6, 7), (7, 4),
                (0, 4), (1, 5), (2, 6), (3, 7),
            ];
            for hb in &mdl.hitboxes {
                let Some(bm) = bones.get(hb.bone) else {
                    continue;
                };
                let m = ent_mat * *bm;
                let lo = hb.bbmin;
                let hi = hb.bbmax;
                let corners = [
                    m.transform_point3(Vec3::new(lo[0], lo[1], lo[2])),
                    m.transform_point3(Vec3::new(hi[0], lo[1], lo[2])),
                    m.transform_point3(Vec3::new(hi[0], hi[1], lo[2])),
                    m.transform_point3(Vec3::new(lo[0], hi[1], lo[2])),
                    m.transform_point3(Vec3::new(lo[0], lo[1], hi[2])),
                    m.transform_point3(Vec3::new(hi[0], lo[1], hi[2])),
                    m.transform_point3(Vec3::new(hi[0], hi[1], hi[2])),
                    m.transform_point3(Vec3::new(lo[0], hi[1], hi[2])),
                ];
                let color = match hb.group {
                    1 => [1.0, 0.2, 0.2, 0.95],
                    2 | 3 => [1.0, 0.55, 0.15, 0.95],
                    4 | 5 => [1.0, 0.95, 0.2, 0.95],
                    6 | 7 => [0.25, 0.9, 0.35, 0.95],
                    _ => [0.85, 0.85, 0.85, 0.9],
                };
                for (a, b) in EDGES {
                    beams.push(FxBeam {
                        start: corners[a],
                        end: corners[b],
                        width: 0.9,
                        color,
                        additive: true,
                    });
                }
            }
        }
    }

    fn surface_char_at(
        &self,
        pos: Vec3,
        normal: Vec3,
        brush: Option<&BrushCollider>,
    ) -> u8 {
        let Some(ref bsp) = self.bsp else {
            return CHAR_TEX_CONCRETE;
        };
        let (faces, cmp_normal) = if let Some(b) = brush {
            let local_pos = world_to_brush_local(pos, b.origin, b.angles);
            let local_norm = world_to_brush_local(normal, Vec3::ZERO, b.angles);
            (
                query_submodel_faces_in_sphere(bsp, b.submodel as usize, local_pos, 16.0),
                local_norm,
            )
        } else {
            (bsp.query_faces_in_sphere(pos, 16.0), normal)
        };
        let best = faces.iter().max_by(|a, b| {
            a.normal
                .dot(cmp_normal)
                .partial_cmp(&b.normal.dot(cmp_normal))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let Some(poly) = best else {
            return CHAR_TEX_CONCRETE;
        };
        let Some(face) = bsp.faces.get(poly.face_index) else {
            return CHAR_TEX_CONCRETE;
        };
        let Some(ti) = bsp.texinfos.get(usize::from(face.texinfo)) else {
            return CHAR_TEX_CONCRETE;
        };
        let Some(tex) = usize::try_from(ti.miptex)
            .ok()
            .and_then(|i| bsp.textures.get(i))
        else {
            return CHAR_TEX_CONCRETE;
        };
        surface_material_to_char(self.materials.lookup(&tex.name))
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_viewmodel_events_into(
        &mut self,
        vm: &ViewmodelView,
        prev_anim: Option<ViewAnimState>,
        now_sec: f32,
        now: f64,
        eye: Vec3,
        view_plus_punch: Vec3,
        out_anim_sounds: &mut Vec<String>,
    ) {
        self.vm_key_scratch.clear();
        self.vm_key_scratch
            .extend(vm.path.chars().map(|c| c.to_ascii_lowercase()));
        if !self.vm_seq_cache.contains_key(&self.vm_key_scratch) {
            let meta = self
                .vfs
                .as_ref()
                .and_then(|vfs| StudioModel::load(vfs, &self.vm_key_scratch).ok())
                .map(|m| {
                    m.sequences
                        .into_iter()
                        .map(|s| CachedSeqMeta {
                            fps: s.fps,
                            num_frames: s.num_frames,
                            looping: (s.flags & STUDIO_LOOPING) != 0,
                            events: s.events,
                        })
                        .collect()
                });
            let key = self.vm_key_scratch.clone();
            if let Some(list) = meta {
                self.vm_seq_cache.insert(key, list);
            } else {
                self.vm_seq_cache.insert(key, Vec::new());
            }
        }
        let seq_idx = vm.sequence.max(0) as usize;
        let Some(seq_meta) = self
            .vm_seq_cache
            .get(&self.vm_key_scratch)
            .and_then(|v| v.get(seq_idx))
        else {
            return;
        };
        let cur_frame = studio_frame(
            0.0,
            vm.animtime,
            1.0,
            now_sec,
            seq_meta.fps,
            seq_meta.num_frames,
            seq_meta.looping,
        );
        let switched_anim = self.last_vm_path != self.vm_key_scratch
            || prev_anim.is_none_or(|a| {
                a.sequence != vm.sequence || (a.started - vm.animtime).abs() > 1e-4
            })
            || (self.last_vm_animtime - vm.animtime).abs() > 1e-4;
        let prev_frame = if switched_anim { -0.01 } else { self.last_vm_frame };
        if self.last_vm_path != self.vm_key_scratch {
            self.last_vm_path.clear();
            self.last_vm_path.push_str(&self.vm_key_scratch);
        }
        self.last_vm_animtime = vm.animtime;
        self.last_vm_frame = cur_frame;
        if cur_frame > prev_frame {
            for ev in &seq_meta.events {
                let f = ev.frame as f32;
                if f > prev_frame && f <= cur_frame {
                    match ev.event {
                        5001 | 5011 | 5021 | 5031 => {
                            let (fwd, _, _) = crate::render::scene::angle_vectors(view_plus_punch);
                            self.dlights.add_light(DynamicLight {
                                key: 9999,
                                entity_light: false,
                                origin: eye + fwd * 26.0,
                                radius: 180.0,
                                color: [255, 195, 95],
                                die: now + 0.05,
                                decay: 1800.0,
                            });
                        }
                        5004 if !ev.options.is_empty() => {
                            out_anim_sounds.push(ev.options.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// `R_AddEntity`'s input for one entity of the frame.
    fn render_entity(
        &mut self,
        e: &VisibleEntity,
        dt: f32,
        time: f32,
    ) -> Option<RenderEntity> {
        let player_model = (e.kind == EntityKind::Player)
            .then(|| e.number.checked_sub(1))
            .flatten()
            .and_then(|slot| u8::try_from(slot).ok())
            .and_then(|slot| self.player_model_handle(slot));
        let model = match player_model {
            Some(h) => h,
            None => self.models.resolve(i32::from(e.modelindex))?,
        };
        let s = &e.state;
        let mut r = RenderEntity::new(e.number, model, e.origin);
        r.angles = e.angles;
        r.frame = e.frame;
        r.sequence = i32::from(e.sequence);
        r.animtime = s.animtime;
        r.framerate = s.framerate;
        r.skin = i32::from(s.skin);
        r.body = i32::from(s.body);
        r.effects = s.effects;
        r.player = e.kind == EntityKind::Player;
        r.rendermode = EntRenderMode::from_raw(e.rendermode);
        r.renderamt = e.renderamt;
        r.rendercolor = e.rendercolor;
        r.renderfx = e.renderfx;
        r.scale = s.scale;
        r.solid = s.solid as i32;
        r.controllers = s.controller;
        r.blending = s.blending;
        r.scroll_speed = 0.0;

        // Sequence crossfade (0.1s ramp from latched.prevsequence).
        let seq_elapsed = (time - e.latched.sequencetime).max(0.0);
        if i32::from(e.latched.prevsequence) != r.sequence
            && e.latched.sequencetime > 0.0
            && seq_elapsed < 0.1
        {
            r.prev_sequence = Some((
                i32::from(e.latched.prevsequence),
                e.latched.prevframe,
                (1.0 - seq_elapsed / 0.1).clamp(0.0, 1.0),
            ));
        }

        // Player lower-body gait sequence tracking (`gaitsequence` split at `"Bip01 Spine"`).
        if r.player && s.gaitsequence > 0 {
            let (g_frame, prev_org) = self
                .player_gaits
                .entry(e.number)
                .or_insert((e.latched.gaitframe, e.origin));
            let moved = (*prev_org - e.origin).truncate().length();
            *prev_org = e.origin;
            *g_frame = (*g_frame + dt * 24.0 + moved * 0.25) % 64.0;
            r.gait = Some((i32::from(s.gaitsequence), *g_frame));
        }

        if r.player && s.weaponmodel != 0 {
            r.attached = self.models.resolve(i32::from(s.weaponmodel));
        }
        Some(r)
    }

    fn render_tempent(&mut self, t: &TempEnt, index: u16) -> Option<RenderEntity> {
        if t.has(ftent::NOMODEL) {
            return None;
        }
        let model = self.handle_of(&t.model)?;
        let mut r = RenderEntity::new(index.wrapping_add(4096), model, t.origin);
        r.angles = t.angles;
        r.frame = t.frame;
        r.animtime = t.born as f32;
        r.framerate = t.framerate;
        r.body = t.body as i32;
        r.rendermode = EntRenderMode::from_raw(match t.render_mode {
            RenderMode::Normal => 0,
            RenderMode::TransColor => 1,
            RenderMode::TransTexture => 2,
            RenderMode::Glow => 3,
            RenderMode::TransAlpha => 4,
            RenderMode::TransAdd => 5,
        });
        r.renderamt = t.render_amt.clamp(0.0, 255.0) as u8;
        r.rendercolor = t.render_color;
        r.renderfx = t.render_fx;
        r.scale = t.scale;
        Some(r)
    }

    /// Suppress `EntRenderMode::Glow` entities and `TE_GLOWSPRITE` tempents occluded by world BSP
    /// or opaque `SOLID_BSP` brush entities (`R_SpriteGlowBlend`, `ref/gl/gl_sprite.c:220-242`,
    /// read 2026-11-25).
    fn apply_glow_occlusion(&self, scene: &mut SceneFrame) {
        let Some(world) = self.collision.as_deref() else {
            return;
        };
        if !scene
            .entities
            .iter()
            .any(|e| e.rendermode == EntRenderMode::Glow && (e.effects & EF_NODRAW) == 0)
        {
            return;
        }
        let colliders = collect_opaque_brush_colliders(&scene.entities, &self.models);
        let eye = scene.view.origin;
        for e in &mut scene.entities {
            if e.rendermode == EntRenderMode::Glow
                && (e.effects & EF_NODRAW) == 0
                && trace_glow_occlusion(world, &colliders, eye, e.origin)
            {
                e.renderamt = 0;
                e.effects |= EF_NODRAW;
            }
        }
    }

    /// A `ET_BEAM` entity: `origin` is where it starts and `angles` where it ends
    /// (`CBeam::SetEndPos`, assumption: the layout of the original game's `beam.cpp`).
    fn push_beam_entity(&mut self, e: &VisibleEntity, now: f64) {
        let amt = f32::from(e.renderamt) / 255.0;
        let c = e.rendercolor;
        self.beams.push(ActiveBeam {
            die: now + 0.0001,
            start: e.origin,
            end: e.state.angles,
            width: (e.state.scale * 0.25).max(1.0),
            color: [
                f32::from(c[0]) / 255.0,
                f32::from(c[1]) / 255.0,
                f32::from(c[2]) / 255.0,
                amt.max(0.05),
            ],
        });
    }

    fn take_effect(
        &mut self,
        effect: RenderEffect,
        now: f64,
        entity_pos: &dyn Fn(u16) -> Option<Vec3>,
    ) {
        match effect {
            RenderEffect::Beam { time, effect } => {
                self.beam_sys.spawn_from_temp_effect(time.max(now - 0.1), &effect, entity_pos);
                match effect {
                    TempEffect::BeamPoints { start, end, beam } => {
                        let life = if beam.life == 0 {
                            TRACER_LIFE
                        } else {
                            f64::from(beam.life) * 0.1
                        };
                        self.beams.push(ActiveBeam {
                            die: time.max(now - life) + life,
                            start,
                            end,
                            width: (f32::from(beam.width) * 0.1).max(0.5),
                            color: [
                                f32::from(beam.color[0]) / 255.0,
                                f32::from(beam.color[1]) / 255.0,
                                f32::from(beam.color[2]) / 255.0,
                                f32::from(beam.brightness) / 255.0,
                            ],
                        });
                    }
                    TempEffect::Tracer { start, end } => self.beams.push(ActiveBeam {
                        die: time + TRACER_LIFE,
                        start,
                        end,
                        width: 0.7,
                        color: [1.0, 0.88, 0.45, 0.75],
                    }),
                    _ => {}
                }
            }
            RenderEffect::Light(l) => {
                let origin = l.origin;
                let radius = l.radius;
                let color = l.color;
                let die = l.die.max(now);
                self.dlights.add_light(l);
                self.glows.push(ActiveGlow {
                    die,
                    origin,
                    size: (radius * 0.08).clamp(2.0, 24.0),
                    color: [
                        f32::from(color[0]) / 255.0,
                        f32::from(color[1]) / 255.0,
                        f32::from(color[2]) / 255.0,
                        0.35,
                    ],
                });
            }
            RenderEffect::Decal {
                position,
                decal,
                decal_name,
                entity,
                model,
                permanent,
            } => {
                if self.pending_decals.len() < 128 {
                    self.pending_decals.push(DecalSpawn {
                        origin: position,
                        decal_name: decal_name
                            .unwrap_or_else(|| format!("{{shot{}", (decal % 5) + 1)),
                        half_size: 16.0,
                        permanent,
                        player_spray: None,
                        entity_index: entity,
                        model_index: model,
                    });
                }
            }
            RenderEffect::PlayerDecal {
                player,
                position,
                entity,
                ..
            } => {
                if self.pending_decals.len() < 128 {
                    self.pending_decals.push(DecalSpawn {
                        origin: position,
                        decal_name: "spray".to_string(),
                        half_size: 24.0,
                        permanent: true,
                        player_spray: Some(player),
                        entity_index: entity,
                        model_index: 0,
                    });
                }
            }
            RenderEffect::Particles(pe) => {
                self.particles.spawn_effect(now, &pe);
                if let cs_clientcore::tempents::ParticleEffect::Gunshot { position, .. } = pe {
                    self.glows.push(ActiveGlow {
                        die: now + 0.12,
                        origin: position,
                        size: 1.5,
                        color: [1.0, 0.85, 0.4, 0.8],
                    });
                }
            }
            RenderEffect::TextMessage(_) => {}
        }
    }

    /// The view model's animation: an event or `svc_weaponanim` starts one; so does a change of
    /// the `weaponanim` the client data carries (the engine copies it to the view entity).
    fn update_view_anim(
        &mut self,
        fx: &mut ClientEffects,
        fired: &[FiredEvent],
        weaponanim: i32,
        viewmodel: i32,
        time: f32,
    ) {
        if viewmodel != self.last_viewmodel {
            self.last_viewmodel = viewmodel;
            self.anim = Some(ViewAnimState {
                sequence: weaponanim,
                body: 0,
                started: time,
            });
            self.last_weaponanim = weaponanim;
        } else if weaponanim != self.last_weaponanim {
            self.last_weaponanim = weaponanim;
            self.anim = Some(ViewAnimState {
                sequence: weaponanim,
                body: self.anim.map_or(0, |a| a.body),
                started: time,
            });
        }
        if let Some(ViewAnim { sequence, body }) = fx.hud.server_weapon_anim.take() {
            // Avoid restarting a deploy/weapon animation when the server ack arrives for the
            // same sequence that viewmodel switching or prediction already started.
            let already_started = self.anim.is_some_and(|a| {
                a.sequence == sequence
                    && a.body == body
                    && (time - a.started) >= 0.0
                    && (time - a.started) < 0.5
            });
            if !already_started {
                self.anim = Some(ViewAnimState {
                    sequence,
                    body,
                    started: time,
                });
            }
        }
        for ev in fired {
            let anim = match &ev.fx {
                EventFx::Weapon(w) if w.local => w.view_anim,
                EventFx::Knife {
                    local: true,
                    view_anim,
                    ..
                } => *view_anim,
                _ => None,
            };
            if let Some(ViewAnim { sequence, body }) = anim {
                self.anim = Some(ViewAnimState {
                    sequence,
                    body,
                    started: time,
                });
            }
        }
    }

    fn populate_viewmodel_into(
        &self,
        out: &mut Option<ViewmodelView>,
        alive: bool,
        fov: f32,
        viewmodel_index: i32,
        inputs: &ViewInputs,
    ) {
        // Hide the viewmodel at sniper scope zoom tiers (<= 40 deg), keeping AUG/SG552 (55 deg).
        if !alive
            || !inputs.draw_viewmodel
            || viewmodel_index <= 0
            || fov <= 40.0
            || self.vm_path_scratch.is_empty()
        {
            *out = None;
            return;
        }
        let anim = self.anim.unwrap_or(ViewAnimState {
            sequence: 0,
            body: 0,
            started: 0.0,
        });
        if let Some(existing) = out.as_mut() {
            if existing.path != self.vm_path_scratch {
                existing.path.clear();
                existing.path.push_str(&self.vm_path_scratch);
            }
            existing.sequence = anim.sequence;
            existing.body = anim.body;
            existing.animtime = anim.started;
            existing.right_hand = inputs.right_hand;
        } else {
            *out = Some(ViewmodelView {
                path: self.vm_path_scratch.clone(),
                sequence: anim.sequence,
                body: anim.body,
                animtime: anim.started,
                right_hand: inputs.right_hand,
            });
        }
    }
}

fn is_shadow_eligible(
    e: &VisibleEntity,
    r: &RenderEntity,
    models: &ModelRegistry,
    players: &BTreeMap<u8, cs_clientcore::hud::PlayerScore>,
) -> bool {
    if (r.effects & EF_NODRAW) != 0 || r.rendermode == EntRenderMode::TransAdd {
        return false;
    }
    if r.player {
        if r.renderfx == RENDERFX_DEAD_PLAYER {
            return false;
        }
        let dead = u8::try_from(e.number)
            .ok()
            .and_then(|id| players.get(&id))
            .is_some_and(|p| (p.attrib & 1) != 0);
        return !dead;
    }
    let Some(entry) = models.get(r.model) else {
        return false;
    };
    if entry.kind != ModelKind::Studio {
        return false;
    }
    let is_hostage = contains_ascii_ignore_case(&entry.name, "scientist")
        || contains_ascii_ignore_case(&entry.name, "hostage");
    // Dead hostages switch to MOVETYPE_TOSS (6) + SOLID_NOT (0); live ones stay SOLID_SLIDEBOX (3).
    is_hostage && !(e.state.solid == 0 && e.state.movetype != 0)
}

fn surface_material_to_char(mat: SurfaceMaterial) -> u8 {
    match mat {
        SurfaceMaterial::Metal => CHAR_TEX_METAL,
        SurfaceMaterial::Dirt | SurfaceMaterial::Grass => CHAR_TEX_DIRT,
        SurfaceMaterial::Vent => CHAR_TEX_VENT,
        SurfaceMaterial::Grate => CHAR_TEX_GRATE,
        SurfaceMaterial::Tile => CHAR_TEX_TILE,
        SurfaceMaterial::Slosh => CHAR_TEX_SLOSH,
        SurfaceMaterial::Wood => CHAR_TEX_WOOD,
        SurfaceMaterial::Computer => CHAR_TEX_COMPUTER,
        SurfaceMaterial::Glass => b'Y',
        SurfaceMaterial::Snow => CHAR_TEX_SNOW,
        SurfaceMaterial::Concrete => CHAR_TEX_CONCRETE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::Cell;
    use std::collections::VecDeque;

    use cs_clientcore::{ConnState, ConnectionConfig};
    use cs_proto48::crypto::md5_hex;
    use cs_transport::{PeerAddr, TransportError};

    /// A server that is a capture: each datagram arrives when the clock reaches its time.
    struct Replay {
        records: VecDeque<(f64, Vec<u8>)>,
        clock: Rc<Cell<f64>>,
    }

    impl PacketTransport for Replay {
        fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
            let due = self.records.front().is_some_and(|(t, _)| *t <= self.clock.get());
            due.then(|| self.records.pop_front())
                .flatten()
                .map(|(_, data)| (PeerAddr::Loopback(0), data))
        }

        fn send(&mut self, _: PeerAddr, _: &[u8]) -> Result<(), TransportError> {
            Ok(())
        }
    }

    fn inputs(now: f64) -> ViewInputs {
        ViewInputs {
            now,
            frametime: 0.01,
            right_hand: true,
            draw_viewmodel: true,
            default_fov: 90.0,
            ..ViewInputs::default()
        }
    }

    /// The scene of a golden capture of the stock server builds without a window or a GPU: the
    /// players and props resolve by `modelindex`, the frames are finite, and the camera is
    /// where the server put the player.
    #[test]
    fn a_golden_replay_builds_scenes_without_a_gpu() {
        let Some(dir) = std::env::var_os("CS16_GOLDEN_DIR") else {
            return;
        };
        let mut replayed = 0;
        for file in ["hlds_join.cs48cap", "hlds_office_join.cs48cap"] {
            let Ok(bytes) = std::fs::read(std::path::Path::new(&dir).join(file)) else {
                continue;
            };
            replayed += 1;
            let records: VecDeque<(f64, Vec<u8>)> = cs_probe::read_capture(&bytes)
                .expect("capture")
                .into_iter()
                .filter(|r| r.dir == cs_probe::Direction::Received)
                .map(|r| (r.time, r.data))
                .collect();
            let last = records.back().map_or(0.0, |r| r.0);
            let clock = Rc::new(Cell::new(0.0));
            let transport = Replay {
                records,
                clock: Rc::clone(&clock),
            };
            let cfg = ConnectionConfig::new("replay", 76_561_197_960_265_730, &md5_hex(b"replay"));
            let mut conn = Connection::new(transport, PeerAddr::Loopback(0), cfg);
            conn.connect();

            let mut builder = ViewBuilder::new();
            let mut best = ViewStats::default();
            let mut frames = 0usize;
            let mut now = 0.0;
            while now < last + 0.5 {
                now += 0.01;
                clock.set(now);
                conn.pump(now);
                if conn.state() != &ConnState::InGame {
                    continue;
                }
                let (view, _) = builder.build(&mut conn, Vec3::ZERO, &inputs(now));
                frames += 1;
                assert!(view.eye.is_finite(), "{file}: eye {:?}", view.eye);
                for e in &view.scene.entities {
                    assert!(e.origin.is_finite() && e.angles.is_finite(), "{file}: {e:?}");
                    assert!(builder.models.get(e.model).is_some(), "{file}: {e:?}");
                }
                if view.stats.entities > best.entities {
                    best = view.stats;
                }
            }
            eprintln!("{file}: {frames} frames, busiest {best:?}, state {:?}", conn.state());
            assert!(frames > 0, "{file}: never got in the game: {:?}", conn.state());
            assert!(best.entities > 0, "{file}: nothing was ever in the scene");
            // The local player is not drawn in first person, so a capture of one player has no
            // studio entity; its brush models (doors, plats) resolve by `modelindex`.
            assert!(best.brush > 0, "{file}: no brush model resolved by modelindex");
            assert!(!builder.models.is_empty());
        }
        assert!(replayed > 0 || std::env::var_os("CS16_GOLDEN_DIR").is_none());
    }

    #[test]
    fn client_models_get_slots_past_the_server_table() {
        let mut b = ViewBuilder::new();
        let mut precache = cs_clientcore::PrecacheTables::default();
        precache.add(&[cs_proto48::messages::ResourceEntry {
            res_type: cs_clientcore::res_type::MODEL,
            filename: "sprites/smokepuff.spr".to_string(),
            index: 5,
            download_size: 0,
            flags: 0,
            md5: None,
            reserved: None,
        }]);
        assert!(b.ensure_registry(&precache));
        let h = b
            .handle_of(&ModelRef::Precached(5))
            .expect("a precached model");
        assert_eq!(h, ModelHandle(5));
        assert_eq!(b.handle_of(&ModelRef::None), None);
        // A client model needs the registry rebuilt once, then resolves.
        assert_eq!(b.handle_of(&ModelRef::Client("models/shell.mdl")), None);
        assert!(b.ensure_registry(&precache));
        let h = b
            .handle_of(&ModelRef::Client("models/shell.mdl"))
            .expect("a client model");
        assert_eq!(h, ModelHandle(CLIENT_MODEL_BASE));
        assert_eq!(b.models.get(h).map(|e| e.name.as_str()), Some("models/shell.mdl"));
        // Unchanged: no rebuild.
        assert!(!b.ensure_registry(&precache));
    }

    #[test]
    fn intro_camera_client_data_is_not_alive() {
        let cd = cs_proto48::delta::GoldSrcClientData {
            deadflag: 0,
            health: 100.0,
            flags: (1 << 3) | FL_SPECTATOR,
            weapons: 0,
            iuser1: 0,
            ..Default::default()
        };
        assert!(!is_client_alive(&cd));
    }

    #[test]
    fn spawned_player_client_data_is_alive() {
        let cd = cs_proto48::delta::GoldSrcClientData {
            deadflag: 0,
            health: 100.0,
            flags: 1 << 3,
            weapons: WEAPON_SUIT_BIT,
            iuser1: 0,
            ..Default::default()
        };
        assert!(is_client_alive(&cd));
    }

    #[test]
    fn observer_client_data_is_not_alive() {
        let cd = cs_proto48::delta::GoldSrcClientData {
            deadflag: 0,
            health: 100.0,
            flags: 1 << 3,
            weapons: WEAPON_SUIT_BIT,
            iuser1: 4,
            ..Default::default()
        };
        assert!(!is_client_alive(&cd));
    }

    #[test]
    fn single_bullet_impact_uses_exact_spread_without_timestamp_jitter() {
        use cs_assets::bsp::{BspMap, BspModel, Leaf, LightmapAtlas, Node, Plane};
        use cs_clientcore::events::{BulletFire, BulletType, WeaponFireFx};
        use cs_clientcore::{EventFx, FiredEvent};
        use cs_pm::collision::{CONTENTS_EMPTY, CONTENTS_SOLID};

        let bsp = BspMap {
            name: "wall".into(),
            skyname: String::new(),
            entities: Vec::new(),
            spawns: Vec::new(),
            hostage_spawns: Vec::new(),
            brush_entities: Vec::new(),
            planes: vec![Plane {
                normal: Vec3::X,
                dist: 128.0,
                plane_type: 0,
            }],
            vertices_raw: Vec::new(),
            visibility: Vec::new(),
            nodes: vec![Node {
                plane: 0,
                children: [-1, -2],
                mins: [-512, -512, -512],
                maxs: [512, 512, 512],
                first_face: 0,
                num_faces: 0,
            }],
            texinfos: Vec::new(),
            faces: Vec::new(),
            lighting: Vec::new(),
            clipnodes: Vec::new(),
            leafs: vec![
                Leaf {
                    contents: CONTENTS_SOLID,
                    vis_offset: -1,
                    mins: [-512, -512, -512],
                    maxs: [512, 512, 512],
                    first_mark_surface: 0,
                    num_mark_surfaces: 0,
                    ambient_levels: [0; 4],
                },
                Leaf {
                    contents: CONTENTS_EMPTY,
                    vis_offset: -1,
                    mins: [-512, -512, -512],
                    maxs: [512, 512, 512],
                    first_mark_surface: 0,
                    num_mark_surfaces: 0,
                    ambient_levels: [0; 4],
                },
            ],
            mark_surfaces: Vec::new(),
            edges: Vec::new(),
            surf_edges: Vec::new(),
            models: vec![BspModel {
                mins: Vec3::splat(-512.0),
                maxs: Vec3::splat(512.0),
                origin: Vec3::ZERO,
                head_nodes: [0, -1, -1, -1],
                vis_leafs: 2,
                first_face: 0,
                num_faces: 0,
            }],
            textures: Vec::new(),
            lightmap_atlas: LightmapAtlas {
                width: 0,
                height: 0,
                rgba: Vec::new(),
                packed_faces: 0,
            },
            render_vertices: Vec::new(),
            render_indices: Vec::new(),
            batches: Vec::new(),
        };

        let mut builder = ViewBuilder::new();
        builder.set_world_context(Some(Arc::new(WorldCollision::from_bsp(&bsp))), None);
        let fired = [FiredEvent {
            index: 1,
            name: Some("events/ak47.sc".into()),
            flags: 0,
            args: cs_proto48::event::EventArgs::default(),
            velocity: Vec3::ZERO,
            fx: EventFx::Weapon(WeaponFireFx {
                weapon: "ak47",
                entity: 1,
                local: true,
                view_anim: None,
                muzzle_flash: false,
                shell: None,
                sound: None,
                bullets: Some(BulletFire {
                    entity: 1,
                    origin: Vec3::ZERO,
                    forward: Vec3::X,
                    right: Vec3::Y,
                    up: Vec3::Z,
                    shots: 1,
                    spread: [0.05, -0.025],
                    distance: 512.0,
                    bullet: BulletType::Mm762,
                    penetration: 2,
                }),
            }),
        }];

        let _ = builder.collect_bullet_impacts(&fired, &[], 1.234, false);
        assert_eq!(builder.pending_decals.len(), 1);
        let p1 = builder.pending_decals[0].origin;
        assert!((p1.x - 128.0).abs() < 0.1, "x={}", p1.x);
        assert!((p1.y - 6.4).abs() < 0.2, "y={}", p1.y);
        assert!((p1.z + 3.2).abs() < 0.2, "z={}", p1.z);

        builder.pending_decals.clear();
        let _ = builder.collect_bullet_impacts(&fired, &[], 99.876, false);
        let p2 = builder.pending_decals[0].origin;
        assert!((p1 - p2).length() < 1e-5, "single shot impact must be deterministic");
    }

    #[test]
    fn build_parts_nudges_vieworg_by_one_32nd_and_freezes_airborne_bob() {
        let mut builder = ViewBuilder::new();
        let precache = cs_clientcore::PrecacheTables::default();
        let mut fx = ClientEffects::default();
        let userinfos = BTreeMap::new();
        let mut pred = Prediction {
            predicted: true,
            simorg: Vec3::new(10.0, 20.0, 30.0),
            viewheight: Vec3::new(0.0, 0.0, 17.0),
            simvel: Vec3::new(250.0, 0.0, 0.0),
            onground: 0,
            ..Default::default()
        };

        let cd = cs_proto48::delta::GoldSrcClientData {
            origin: Vec3::new(10.0, 20.0, 30.0),
            view_ofs: Vec3::new(0.0, 0.0, 17.0),
            ..Default::default()
        };
        let emitted = cs_clientcore::interp::EmitResult::default();
        let (v_ground, _) = builder.build_parts(
            &precache,
            &mut fx,
            &userinfos,
            Some(cd.clone()),
            Some(0),
            Some(&pred),
            emitted,
            Vec::new(),
            Vec3::ZERO,
            &inputs(0.2),
        );
        assert_eq!(v_ground.eye, Vec3::new(10.03125, 20.03125, 47.03125));
        assert!(v_ground.bob > 0.0);

        pred.onground = -1;
        let emitted_air = cs_clientcore::interp::EmitResult::default();
        let (v_air, _) = builder.build_parts(
            &precache,
            &mut fx,
            &userinfos,
            Some(cd),
            Some(0),
            Some(&pred),
            emitted_air,
            Vec::new(),
            Vec3::ZERO,
            &inputs(0.4),
        );
        assert_eq!(v_air.bob, v_ground.bob);
    }

    #[test]
    fn occluded_glow_entity_and_te_glowsprite_are_suppressed_while_unoccluded_remains() {
        use cs_assets::bsp::{BspMap, BspModel, Leaf, LightmapAtlas, Node, Plane};
        use cs_clientcore::PrecacheEntry;
        use cs_clientcore::interp::{EmitResult, VisibleEntity};
        use cs_clientcore::sound::SoundQueue;
        use cs_clientcore::tempents::FxRandom;
        use cs_pm::collision::{CONTENTS_EMPTY, CONTENTS_SOLID};
        use cs_proto48::delta::GoldSrcEntityState;

        let bsp = BspMap {
            name: "glow_view".into(),
            skyname: String::new(),
            entities: Vec::new(),
            spawns: Vec::new(),
            hostage_spawns: Vec::new(),
            brush_entities: Vec::new(),
            planes: vec![
                Plane { normal: Vec3::X, dist: 128.0, plane_type: 0 },
                Plane { normal: Vec3::X, dist: 0.0, plane_type: 0 },
            ],
            vertices_raw: Vec::new(),
            visibility: Vec::new(),
            nodes: vec![
                Node { plane: 0, children: [-1, -2], mins: [-512; 3], maxs: [512; 3], first_face: 0, num_faces: 0 },
                Node { plane: 1, children: [-1, -2], mins: [-64; 3], maxs: [64; 3], first_face: 0, num_faces: 0 },
            ],
            texinfos: Vec::new(),
            faces: Vec::new(),
            lighting: Vec::new(),
            clipnodes: Vec::new(),
            leafs: vec![
                Leaf { contents: CONTENTS_SOLID, vis_offset: -1, mins: [-512; 3], maxs: [512; 3], first_mark_surface: 0, num_mark_surfaces: 0, ambient_levels: [0; 4] },
                Leaf { contents: CONTENTS_EMPTY, vis_offset: -1, mins: [-512; 3], maxs: [512; 3], first_mark_surface: 0, num_mark_surfaces: 0, ambient_levels: [0; 4] },
            ],
            mark_surfaces: Vec::new(),
            edges: Vec::new(),
            surf_edges: Vec::new(),
            models: vec![
                BspModel { mins: Vec3::splat(-512.0), maxs: Vec3::splat(512.0), origin: Vec3::ZERO, head_nodes: [0, -1, -1, -1], vis_leafs: 2, first_face: 0, num_faces: 0 },
                BspModel { mins: Vec3::splat(-64.0), maxs: Vec3::splat(64.0), origin: Vec3::ZERO, head_nodes: [1, -1, -1, -1], vis_leafs: 2, first_face: 0, num_faces: 0 },
            ],
            textures: Vec::new(),
            lightmap_atlas: LightmapAtlas { width: 0, height: 0, rgba: Vec::new(), packed_faces: 0 },
            render_vertices: Vec::new(),
            render_indices: Vec::new(),
            batches: Vec::new(),
        };

        let mut builder = ViewBuilder::new();
        let coll = Arc::new(WorldCollision::from_bsp(&bsp));
        builder.set_world_context(Some(coll.clone()), None);
        let mut precache = PrecacheTables::default();
        for (idx, name) in [
            (1u16, "maps/glow_view.bsp"),
            (2, "*1"),
            (3, "sprites/glow01.spr"),
            (4, "sprites/ledglow.spr"),
        ] {
            precache.models.insert(
                idx,
                PrecacheEntry {
                    name: name.to_string(),
                    download_size: 0,
                    flags: 0,
                },
            );
        }
        let fx_world = MapFxWorld::new(
            Some(coll),
            None,
            Rc::new(RefCell::new(BTreeMap::new())),
        );
        let mut rng = FxRandom::new(1);
        let mut sounds = SoundQueue::default();
        let mut fx = ClientEffects::default();
        // TE_GLOWSPRITE #1 unoccluded in open air (x=50), #2 occluded behind world wall (x=200),
        // #3 occluded behind SOLID_BSP func_wall at y=60 (x=80 is in front of world wall x=128).
        for pos in [
            Vec3::new(50.0, 0.0, 0.0),
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(80.0, 60.0, 0.0),
        ] {
            fx.tempents.spawn_effect(
                &TempEffect::GlowSprite {
                    position: pos,
                    model: 4,
                    life: 10,
                    scale: 10,
                    brightness: 255,
                },
                1.0,
                &precache,
                &fx_world,
                &mut rng,
                &mut sounds,
            );
        }

        let make_vis = |number, model, modelindex, origin, rendermode, solid| VisibleEntity {
            number,
            kind: EntityKind::Normal,
            model,
            modelindex,
            origin,
            angles: Vec3::ZERO,
            frame: 0.0,
            sequence: 0,
            rendermode,
            renderamt: 255,
            renderfx: 0,
            rendercolor: [255, 255, 255],
            local_player: false,
            studio_lerp: false,
            latched: Default::default(),
            state: GoldSrcEntityState {
                solid,
                ..Default::default()
            },
        };
        let emitted = EmitResult {
            entities: vec![
                // Opaque SOLID_BSP crate (*1) placed at (60, 60, 0) blocking (80, 60, 0) from (0, 0, 0).
                make_vis(10, CoreModelKind::Brush, 2, Vec3::new(60.0, 60.0, 0.0), 0, 4),
                // Unoccluded env_glow at (40, -40, 0).
                make_vis(11, CoreModelKind::Sprite, 3, Vec3::new(40.0, -40.0, 0.0), 3, 0),
                // Occluded env_glow behind world BSP at (180, -40, 0).
                make_vis(12, CoreModelKind::Sprite, 3, Vec3::new(180.0, -40.0, 0.0), 3, 0),
            ],
            ..Default::default()
        };

        let (frame_view, _) = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            None,
            None,
            None,
            emitted,
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.05),
        );
        let draw_lists = crate::render::scene::build_draw_lists(&frame_view.scene, &builder.models);
        assert_eq!(
            draw_lists.sprites.len(),
            2,
            "expected only the unoccluded env_glow and unoccluded TE_GLOWSPRITE, got {:?}",
            draw_lists.sprites.iter().map(|s| s.origin).collect::<Vec<_>>()
        );
        let origins: Vec<Vec3> = draw_lists.sprites.iter().map(|s| s.origin).collect();
        assert!(origins.contains(&Vec3::new(40.0, -40.0, 0.0)));
        assert!(origins.contains(&Vec3::new(50.0, 0.0, 0.0)));
    }

    #[test]
    fn quick_switch_resets_view_anim_and_plays_5004_once_without_ack_double_play() {
        use cs_clientcore::events::ViewAnim;
        use cs_clientcore::interp::EmitResult;
        use cs_clientcore::{PrecacheEntry, Prediction, WeaponState};
        use cs_proto48::delta::GoldSrcClientData;

        let mut builder = ViewBuilder::new();
        if let Ok(vfs) = cs_vfs::Vfs::discover(None, cs_vfs::VfsConfig::default()) {
            builder.set_world_context(None, Some(vfs));
        }
        let mut precache = PrecacheTables::default();
        for (idx, name) in [
            (10u16, "models/v_awp.mdl"),
            (11u16, "models/v_knife.mdl"),
            (12u16, "models/v_deagle.mdl"),
        ] {
            precache.models.insert(
                idx,
                PrecacheEntry {
                    name: name.to_string(),
                    download_size: 0,
                    flags: 0,
                },
            );
        }

        let mut fx = ClientEffects::default();
        let make_cd = |fov: f32, vm: i32, anim: i32| GoldSrcClientData {
            health: 100.0,
            weapons: 1 << 31,
            fov,
            viewmodel: vm,
            weaponanim: anim,
            ..Default::default()
        };
        let make_pred = |fov: f32, vm: i32, anim: i32| Prediction {
            predicted: true,
            viewmodel: vm,
            weapon: WeaponState {
                fov,
                viewmodel: vm,
                weaponanim: anim,
                ..Default::default()
            },
            ..Default::default()
        };

        // Frame 1: AWP zoomed (fov = 40) -> viewmodel is hidden while scoped, server cd still stale at 40.
        let cd_zoomed = make_cd(40.0, 10, 0);
        let pred_zoomed = make_pred(40.0, 10, 0);
        let (fv1, _) = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            Some(cd_zoomed.clone()),
            Some(0),
            Some(&pred_zoomed),
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.0),
        );
        assert!(fv1.viewmodel.is_none());

        // Frame 2: Predicted quick-switch AWP -> Knife while cd is still stale (fov=40, vm=10):
        // Prediction immediately cancels FOV to 90 and switches ViewAnim to Knife deploy (seq=3, animtime=1.02).
        let pred_knife = make_pred(90.0, 11, 3);
        let (fv2, _) = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            Some(cd_zoomed),
            Some(0),
            Some(&pred_knife),
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.02),
        );
        assert_eq!(fv2.fov, 90.0);
        let vm2 = fv2.viewmodel.expect("knife viewmodel visible immediately on unzoom");
        assert_eq!(vm2.path, "models/v_knife.mdl");
        assert_eq!(vm2.sequence, 3);
        assert!((vm2.animtime - 1.02).abs() < 1e-4);

        // Frame 3: Server ack arrives 30ms later with svc_weaponanim(3, 0) & updated cd:
        // Must NOT restart animtime!
        fx.hud.server_weapon_anim = Some(ViewAnim { sequence: 3, body: 0 });
        let cd_knife = make_cd(90.0, 11, 3);
        let (fv3, _) = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            Some(cd_knife),
            Some(0),
            Some(&pred_knife),
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.05),
        );
        let vm3 = fv3.viewmodel.expect("knife viewmodel");
        assert!(
            (vm3.animtime - 1.02).abs() < 1e-4,
            "server ack must not reset deploy animtime: {}",
            vm3.animtime
        );

        // Frame 4: Quick-switch Knife -> AWP: resets ViewAnim to AWP deploy (seq=5, animtime=1.10).
        let pred_awp = make_pred(90.0, 10, 5);
        let (fv4, _) = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            Some(make_cd(90.0, 10, 5)),
            Some(0),
            Some(&pred_awp),
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.10),
        );
        let vm4 = fv4.viewmodel.expect("awp viewmodel");
        assert_eq!(vm4.path, "models/v_awp.mdl");
        assert_eq!(vm4.sequence, 5);
        assert!((vm4.animtime - 1.10).abs() < 1e-4);

        // Frame 5-7: Switch to Deagle (seq 5 draw has MDL 5004 event "weapons/de_deploy.wav" at frame 1):
        // plays 5004 once as the deploy sequence passes frame 1, and server svc_weaponanim ack must NOT replay it.
        let pred_deagle = make_pred(90.0, 12, 5);
        let cd_deagle = make_cd(90.0, 12, 5);
        let _ = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            Some(cd_deagle.clone()),
            Some(0),
            Some(&pred_deagle),
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.20),
        );
        let (fv6, _) = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            Some(cd_deagle.clone()),
            Some(0),
            Some(&pred_deagle),
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.25),
        );
        if builder.vfs.is_some() {
            assert_eq!(fv6.anim_sounds, vec!["weapons/de_deploy.wav".to_string()]);
        }
        fx.hud.server_weapon_anim = Some(ViewAnim { sequence: 5, body: 0 });
        let (fv7, _) = builder.build_parts(
            &precache,
            &mut fx,
            &BTreeMap::new(),
            Some(cd_deagle),
            Some(0),
            Some(&pred_deagle),
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.28),
        );
        let vm7 = fv7.viewmodel.expect("deagle viewmodel");
        assert!(
            (vm7.animtime - 1.20).abs() < 1e-4,
            "server ack must not reset deagle deploy animtime: {}",
            vm7.animtime
        );
        if builder.vfs.is_some() {
            assert!(fv7.anim_sounds.is_empty(), "5004 must not double-play on server ack");
        }
    }

    #[test]
    fn build_into_reuses_frame_view_buffers_and_flat_entity_positions() {
        use cs_clientcore::PrecacheEntry;
        use cs_clientcore::interp::{EmitResult, VisibleEntity};
        use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState};

        let mut builder = ViewBuilder::new();
        let mut precache = PrecacheTables::default();
        precache.models.insert(
            1,
            PrecacheEntry {
                name: "maps/de_dust2.bsp".into(),
                download_size: 0,
                flags: 0,
            },
        );
        precache.models.insert(
            2,
            PrecacheEntry {
                name: "models/player.mdl".into(),
                download_size: 0,
                flags: 0,
            },
        );
        precache.models.insert(
            3,
            PrecacheEntry {
                name: "models/v_ak47.mdl".into(),
                download_size: 0,
                flags: 0,
            },
        );
        precache.models.insert(
            4,
            PrecacheEntry {
                name: "models/p_ak47.mdl".into(),
                download_size: 0,
                flags: 0,
            },
        );

        let mut userinfos = BTreeMap::new();
        userinfos.insert(1, r"\name\Bot1\model\urban\".to_string());
        userinfos.insert(2, r"\name\Bot2\model\leet\".to_string());

        let cd = GoldSrcClientData {
            health: 100.0,
            weapons: WEAPON_SUIT_BIT,
            fov: 90.0,
            viewmodel: 3,
            weaponanim: 0,
            ..Default::default()
        };
        let make_emitted = |t: f32| EmitResult {
            entities: (2u16..=17)
                .map(|num| VisibleEntity {
                    number: num,
                    kind: EntityKind::Player,
                    model: CoreModelKind::Studio,
                    modelindex: 2,
                    origin: Vec3::new(num as f32 * 10.0 + t, 20.0, 0.0),
                    angles: Vec3::ZERO,
                    frame: t,
                    sequence: 4,
                    rendermode: 0,
                    renderamt: 255,
                    renderfx: 0,
                    rendercolor: [255, 255, 255],
                    local_player: false,
                    studio_lerp: true,
                    latched: Default::default(),
                    state: GoldSrcEntityState {
                        weaponmodel: 4,
                        ..Default::default()
                    },
                })
                .collect(),
            ..Default::default()
        };

        let mut fx = ClientEffects::default();
        let mut frame_view = FrameView::default();

        // Warm-up frame allocates initial capacities.
        let _ = builder.build_parts_into(
            &mut frame_view,
            &precache,
            &mut fx,
            &userinfos,
            Some(cd.clone()),
            Some(0),
            None,
            make_emitted(0.0),
            Vec::new(),
            Vec3::ZERO,
            &inputs(1.0),
        );
        assert!(frame_view.entity_positions.len() >= DEFAULT_MAX_EDICTS);
        assert_eq!(frame_view.entity_position(2), Some(Vec3::new(20.0, 20.0, 0.0)));
        assert_eq!(frame_view.entity_position(17), Some(Vec3::new(170.0, 20.0, 0.0)));
        assert_eq!(frame_view.entity_position(999), None);
        assert_eq!(frame_view.player_weapons.get(&2).map(String::as_str), Some("ak47"));

        let ent_ptr = frame_view.scene.entities.as_ptr();
        let ent_cap = frame_view.scene.entities.capacity();
        let pos_ptr = frame_view.entity_positions.as_ptr();
        let vm_path_ptr = frame_view
            .viewmodel
            .as_ref()
            .expect("viewmodel present")
            .path
            .as_ptr();

        // Steady-state frames reuse exact heap buffers (zero reallocations).
        for step in 1..=10 {
            let t = step as f32;
            // Simulate app.rs take_scene / restore_scene round-trip across render_frame.
            let taken = frame_view.take_scene();
            frame_view.restore_scene(taken);
            let _ = builder.build_parts_into(
                &mut frame_view,
                &precache,
                &mut fx,
                &userinfos,
                Some(cd.clone()),
                Some(0),
                None,
                make_emitted(t),
                Vec::new(),
                Vec3::ZERO,
                &inputs(1.0 + f64::from(step) * 0.01),
            );
            assert_eq!(frame_view.scene.entities.as_ptr(), ent_ptr);
            assert_eq!(frame_view.scene.entities.capacity(), ent_cap);
            assert_eq!(frame_view.entity_positions.as_ptr(), pos_ptr);
            assert_eq!(
                frame_view.viewmodel.as_ref().unwrap().path.as_ptr(),
                vm_path_ptr
            );
            assert_eq!(
                frame_view.entity_position(2),
                Some(Vec3::new(20.0 + t, 20.0, 0.0))
            );
        }

        // Entity #17 removed on next frame: flat table clears stale slot to None.
        let _ = builder.build_parts_into(
            &mut frame_view,
            &precache,
            &mut fx,
            &userinfos,
            Some(cd),
            Some(0),
            None,
            EmitResult::default(),
            Vec::new(),
            Vec3::ZERO,
            &inputs(2.0),
        );
        assert_eq!(frame_view.entity_position(2), None);
        assert_eq!(frame_view.entity_position(17), None);
        assert!(frame_view.player_weapons.is_empty());
    }
}
