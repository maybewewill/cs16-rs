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

use cs_assets::mdl::{MdlSeqEvent, StudioModel};
use cs_assets::spr::GoldSrcSprite;
use cs_clientcore::entities::ef;
use cs_clientcore::events::{EventFx, FiredEvent, ViewAnim};
use cs_clientcore::hud::{CustomDecalInfo, evaluate_lightstyles};
use cs_clientcore::tempents::{
    DynamicLight, FxTrace, FxWorld, ModelRef, RenderEffect, RenderMode, TempEnt, ftent,
};
use cs_clientcore::{
    ClientEffects, Connection, EmitInputs, EntityKind, ModelKind as CoreModelKind, VisibleEntity,
};
use cs_pm::collision::{CONTENTS_SOLID, Hull, WorldCollision};
use cs_proto48::tempent::TempEffect;
use cs_transport::PacketTransport;
use cs_vfs::Vfs;
use glam::Vec3;

use crate::render::decal_renderer::DecalSpawn;
use crate::render::fx_renderer::{FxBeam, FxPoint};
use crate::render::lighting::{
    DLightPool, GpuLightsUniform, SpotLight, compute_flashlight_spot,
};
use crate::render::mdl_renderer::{STUDIO_LOOPING, studio_frame};
use crate::render::particles::{BeamSystem, ParticleSystem};
use crate::render::scene::{
    EntRenderMode, ModelHandle, ModelRegistry, RenderEntity, SceneFrame, ViewBasis,
};
use crate::view_effects::{
    BobParams, ViewModelLagState, calc_bob, calc_roll, calc_screen_shake, underwater_tint_and_fov,
};

/// The first model index the shell hands to models the client loads by itself (shells, the
/// effect sprites); the server's table ends well below (`MAX_MODELS` is 512).
pub const CLIENT_MODEL_BASE: u16 = 1024;

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
    pub custom_decals: HashMap<u8, CustomDecalInfo>,
    pub alive: bool,
    pub stats: ViewStats,
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
    player_gaits: HashMap<u16, (f32, Vec3)>,
    vm_seq_cache: HashMap<String, Vec<CachedSeqMeta>>,
    last_vm_frame: f32,
    vfs: Option<Vfs>,
    collision: Option<Arc<WorldCollision>>,
    anim: Option<ViewAnimState>,
    last_weaponanim: i32,
    last_viewmodel: i32,
}

impl Default for ViewBuilder {
    fn default() -> Self {
        Self::new()
    }
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
            player_gaits: HashMap::new(),
            vm_seq_cache: HashMap::new(),
            last_vm_frame: 0.0,
            vfs: None,
            collision: None,
            anim: None,
            last_weaponanim: -1,
            last_viewmodel: -1,
        }
    }

    /// Provide world collision and VFS so client view building can trace flashlights and read
    /// studio animation events.
    pub fn set_world_context(&mut self, collision: Option<Arc<WorldCollision>>, vfs: Option<Vfs>) {
        self.collision = collision;
        self.vfs = vfs;
        self.vm_seq_cache.clear();
    }

    /// The shared model names a [`MapFxWorld`] reads.
    pub fn names(&self) -> Rc<RefCell<BTreeMap<u16, String>>> {
        Rc::clone(&self.names)
    }

    /// Drops per-map scene state on map change (`changelevel`).
    pub fn reset_map(&mut self) {
        self.beams.clear();
        self.glows.clear();
        self.dlights.clear();
        self.particles.clear();
        self.beam_sys.clear();
        self.pending_decals.clear();
        self.lag_state.reset();
        self.player_gaits.clear();
        self.client_models.clear();
        self.anim = None;
        self.last_weaponanim = -1;
        self.last_viewmodel = -1;
        self.last_vm_frame = 0.0;
        self.models_key = 0;
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
        let slot = match self.client_models.iter().position(|p| p == path) {
            Some(slot) => slot,
            None => {
                if self.client_models.len() >= 256 {
                    return None;
                }
                self.client_models.push(path.to_string());
                self.client_models.len() - 1
            }
        };
        self.models
            .resolve(i32::from(CLIENT_MODEL_BASE) + slot as i32)
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
        let now = inputs.now;
        let registry_changed = self.ensure_registry(conn.precache());

        conn.frames_mut().advance(inputs.frametime.clamp(0.0, 0.25));
        // What the weapon events consult: the state bits of the weapon in hand.
        let weapon_state = {
            let p = conn.predictor().prediction();
            p.weapon
                .weapons
                .get(&(p.weapon.weapon_id.clamp(0, 255) as u8))
                .map_or(0, |w| w.m_i_weapon_state)
        };
        conn.set_fx_local(weapon_state, false, inputs.right_hand);
        let fired = conn.update_effects(now);

        // The client data of the newest update: the acked origin until the prediction is in.
        let cd = conn.client_data().cloned();
        let slot = conn.player_slot();
        let local_number = slot.map(|s| u16::from(s) + 1);
        // The predicted player (the server's origin while the map's collision is not set).
        let prediction = conn.predictor().prediction().clone();
        let (mut eye, mut punch, mut fov, alive, weaponanim, viewmodel_index) = match &cd {
            Some(cd) => (
                if prediction.predicted {
                    prediction.simorg + prediction.viewheight
                } else {
                    cd.origin + cd.view_ofs
                },
                if prediction.predicted {
                    prediction.punchangle
                } else {
                    cd.punchangle
                },
                cd.fov,
                cd.deadflag == 0 && cd.health > 0.0,
                cd.weaponanim,
                cd.viewmodel,
            ),
            None => (Vec3::ZERO, Vec3::ZERO, 0.0, false, 0, 0),
        };
        if !eye.is_finite() {
            eye = Vec3::ZERO;
        }
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

        let emitted = conn.frames_mut().emit(&EmitInputs {
            simorg: prediction
                .predicted
                .then_some(prediction.simorg)
                .or_else(|| cd.as_ref().map(|c| c.origin)),
            standing_on: prediction.standing_on(),
        });

        // A player is drawn with the model its userinfo names (`model`): `models/player.mdl` is
        // only the entity's placeholder.
        let player_models: BTreeMap<u8, String> = conn
            .userinfos()
            .iter()
            .filter_map(|(slot, raw)| {
                let info = cs_proto48::info::InfoString::parse(raw);
                let m = info.get("model")?;
                (!m.is_empty() && m.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
                    .then(|| (*slot, format!("models/player/{m}/{m}.mdl")))
            })
            .collect();

        let time = now as f32;
        let mut scene = SceneFrame {
            time,
            view: ViewBasis::from_angles(eye, view_angles + punch),
            entities: Vec::new(),
        };
        let mut stats = ViewStats::default();

        let dt = inputs.frametime.clamp(0.0, 0.25) as f32;
        self.dlights.step(now, dt);
        self.particles.step(now, dt);

        // Local player flashlight spotlight (`impulse 100` -> Flashlight HUD/prediction).
        let mut spotlights: Vec<SpotLight> = Vec::new();
        if alive
            && (conn.fx().hud.flashlight_on || (prediction.effects as u32 & ef::DIMLIGHT) != 0)
        {
            let (fwd, _, _) = crate::render::scene::angle_vectors(view_angles + punch);
            let (spot, bounce) = compute_flashlight_spot(
                eye,
                fwd,
                self.collision.as_deref(),
                now,
            );
            spotlights.push(spot);
            if let Some(b) = bounce {
                self.dlights.add_light(b);
            }
        }

        // Entity loop: build RenderEntities + collect entity EF_* lights & world positions.
        let mut entity_positions: HashMap<u16, Vec3> = HashMap::with_capacity(emitted.entities.len());
        for e in &emitted.entities {
            entity_positions.insert(e.number, e.origin);
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
                    spotlights.push(spot);
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
            if let Some(r) = self.render_entity(e, &player_models, dt, time) {
                match e.model {
                    CoreModelKind::Brush => stats.brush += 1,
                    CoreModelKind::Studio => stats.studio += 1,
                    CoreModelKind::Sprite => stats.sprites += 1,
                }
                scene.entities.push(r);
            }
        }
        stats.entities = scene.entities.len();

        // Temp entities: shells, sprites, gibs.
        let temps: Vec<TempEnt> = conn.fx().tempents.render_list().cloned().collect();
        for t in &temps {
            if let Some(r) = self.render_tempent(t, scene.entities.len() as u16) {
                stats.tempents += 1;
                scene.entities.push(r);
            }
        }

        // Effects that are not entries of the pool.
        let pos_lookup = |id: u16| entity_positions.get(&id).copied();
        for effect in conn.fx_mut().tempents.take_effects() {
            self.take_effect(effect, now, &pos_lookup);
        }
        let weather = conn.fx().hud.weather;
        self.particles.step_weather(now, dt, weather, eye);
        self.beam_sys.step(now, &pos_lookup);
        self.beams.retain(|b| b.die > now);
        self.glows.retain(|g| g.die > now);
        let overflow = self.beams.len().saturating_sub(MAX_ACTIVE_FX);
        self.beams.drain(..overflow);
        let overflow = self.glows.len().saturating_sub(MAX_ACTIVE_FX);
        self.glows.drain(..overflow);

        let prev_vm_seq = self.anim.map(|a| a.sequence).unwrap_or(-1);
        self.update_view_anim(conn.fx_mut(), &fired, weaponanim, viewmodel_index, time);
        let viewmodel = self.viewmodel(conn, alive, fov, viewmodel_index, inputs);
        let mut anim_sounds = Vec::new();
        if let Some(ref vm) = viewmodel {
            let events = self.sample_viewmodel_events(vm, prev_vm_seq, time);
            for (ev_id, opts) in events {
                match ev_id {
                    5001 | 5011 | 5021 | 5031 => {
                        let (fwd, _, _) = crate::render::scene::angle_vectors(view_angles + punch);
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
                    5004 if !opts.is_empty() => anim_sounds.push(opts),
                    _ => {}
                }
            }
        }

        let mut beams: Vec<FxBeam> = self
            .beams
            .iter()
            .map(|b| FxBeam {
                start: b.start,
                end: b.end,
                width: b.width,
                color: b.color,
                additive: true,
            })
            .collect();
        self.beam_sys.emit_fx(now, &mut beams);
        let mut points: Vec<FxPoint> = self
            .glows
            .iter()
            .map(|g| {
                let fade = ((g.die - now) / 0.25).clamp(0.0, 1.0) as f32;
                FxPoint {
                    origin: g.origin,
                    size: g.size,
                    color: [g.color[0], g.color[1], g.color[2], g.color[3] * fade],
                    additive: true,
                }
            })
            .collect();
        self.particles.emit_fx(now, &mut beams, &mut points);
        stats.beams = beams.len();

        let bob = calc_bob(
            time,
            prediction.simvel,
            prediction.onground >= 0,
            &BobParams {
                cl_bob: inputs.cl_bob,
                cl_bobcycle: inputs.cl_bobcycle,
                cl_bobup: inputs.cl_bobup,
            },
        );
        let roll_deg = calc_roll(
            view_angles.y,
            prediction.simvel,
            inputs.cl_rollangle,
            inputs.cl_rollspeed,
        );
        let (shake_angles, shake_z) =
            calc_screen_shake(conn.fx().hud.screen_shake.as_ref(), now);
        let viewmodel_lag =
            self.lag_state
                .update(scene.view.forward, dt, inputs.cl_viewmodel_lag);
        let (underwater_tint, underwater_fov_delta) = underwater_tint_and_fov(
            prediction.watertype,
            prediction.waterlevel.max(0) as u8,
            time,
        );

        let lightstyles = evaluate_lightstyles(&conn.fx().hud.lightstyles, time);
        let fog = conn.fx().hud.fog.map(|f| {
            (
                [
                    f32::from(f.color[0]) / 255.0,
                    f32::from(f.color[1]) / 255.0,
                    f32::from(f.color[2]) / 255.0,
                ],
                f.density,
            )
        });
        let lights = self.dlights.pack_uniform(eye, &spotlights);
        let elights = self.dlights.elights.clone();
        let decals = std::mem::take(&mut self.pending_decals);
        let custom_decals = conn.fx().hud.custom_decals.clone();

        (
            FrameView {
                scene,
                eye,
                punch,
                fov,
                bob,
                roll_deg,
                shake_angles,
                shake_z,
                viewmodel_lag,
                underwater_tint,
                underwater_fov_delta,
                viewmodel,
                beams,
                points,
                lights,
                elights,
                decals,
                lightstyles,
                fog,
                anim_sounds,
                custom_decals,
                alive,
                stats,
            },
            registry_changed.then(|| self.models.clone()),
        )
    }

    fn sample_viewmodel_events(
        &mut self,
        vm: &ViewmodelView,
        prev_seq: i32,
        now_sec: f32,
    ) -> Vec<(i32, String)> {
        let mut out = Vec::new();
        let key = vm.path.to_ascii_lowercase();
        if !self.vm_seq_cache.contains_key(&key) {
            let meta = self
                .vfs
                .as_ref()
                .and_then(|vfs| StudioModel::load(vfs, &key).ok())
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
            if let Some(list) = meta {
                self.vm_seq_cache.insert(key.clone(), list);
            } else {
                self.vm_seq_cache.insert(key.clone(), Vec::new());
            }
        }
        let seq_idx = vm.sequence.max(0) as usize;
        let Some(seq_meta) = self.vm_seq_cache.get(&key).and_then(|v| v.get(seq_idx)) else {
            return out;
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
        let prev_frame = if prev_seq != vm.sequence {
            self.last_vm_frame = -0.01;
            -0.01
        } else {
            self.last_vm_frame
        };
        self.last_vm_frame = cur_frame;
        if cur_frame > prev_frame {
            for ev in &seq_meta.events {
                let f = ev.frame as f32;
                if f > prev_frame && f <= cur_frame {
                    out.push((ev.event, ev.options.clone()));
                }
            }
        }
        out
    }

    /// `R_AddEntity`'s input for one entity of the frame.
    fn render_entity(
        &mut self,
        e: &VisibleEntity,
        player_models: &BTreeMap<u8, String>,
        dt: f32,
        time: f32,
    ) -> Option<RenderEntity> {
        let player_model = (e.kind == EntityKind::Player)
            .then(|| e.number.checked_sub(1))
            .flatten()
            .and_then(|slot| player_models.get(&(slot as u8)))
            .and_then(|path| self.client_handle(path));
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
                permanent,
                ..
            } => {
                if self.pending_decals.len() < 128 {
                    self.pending_decals.push(DecalSpawn {
                        origin: position,
                        decal_name: decal_name
                            .unwrap_or_else(|| format!("{{shot{}", (decal % 5) + 1)),
                        half_size: 16.0,
                        permanent,
                        player_spray: None,
                    });
                }
            }
            RenderEffect::PlayerDecal {
                player, position, ..
            } => {
                if self.pending_decals.len() < 128 {
                    self.pending_decals.push(DecalSpawn {
                        origin: position,
                        decal_name: "spray".to_string(),
                        half_size: 24.0,
                        permanent: true,
                        player_spray: Some(player),
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
            self.anim = Some(ViewAnimState {
                sequence,
                body,
                started: time,
            });
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

    fn viewmodel<T: PacketTransport>(
        &self,
        conn: &Connection<T>,
        alive: bool,
        fov: f32,
        viewmodel_index: i32,
        inputs: &ViewInputs,
    ) -> Option<ViewmodelView> {
        // Hide the viewmodel at sniper scope zoom tiers (<= 40 deg), keeping AUG/SG552 (55 deg).
        if !alive || !inputs.draw_viewmodel || viewmodel_index <= 0 || fov <= 40.0 {
            return None;
        }
        let path = conn.precache().model(u16::try_from(viewmodel_index).ok()?)?;
        if path.is_empty() {
            return None;
        }
        let anim = self.anim.unwrap_or(ViewAnimState {
            sequence: 0,
            body: 0,
            started: 0.0,
        });
        Some(ViewmodelView {
            path: path.to_string(),
            sequence: anim.sequence,
            body: anim.body,
            animtime: anim.started,
            right_hand: inputs.right_hand,
        })
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
}
