//! Top-level wgpu renderer coordinating BSP world, Skybox, the entities of the scene API (brush
//! models, studio models, sprites), flat 3D FX (beams, glows), the depth-cleared first-person
//! view model, and the VGUI2/HUD overlay.

pub mod bsp_renderer;
pub mod decal_renderer;
pub mod fx_renderer;
pub mod gamma;
pub mod lighting;
pub mod mdl_renderer;
pub mod particles;
pub mod scene;
pub mod skybox;
pub mod sprite_renderer;

use std::collections::HashMap;
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_assets::scheme::SkyboxTextures;
use cs_clientcore::hud::CustomDecalInfo;
use cs_clientcore::tempents::DynamicLight;
use cs_pm::collision::WorldCollision;
use cs_vfs::Vfs;
use egui_wgpu::ScreenDescriptor;
use glam::Vec3;
use tracing::info;
#[cfg(not(target_arch = "wasm32"))]
use winit::window::Window;

use crate::camera::Camera;
use crate::render::bsp_renderer::{BspDrawParams, BspRenderStats, BspRenderer};
use crate::render::decal_renderer::{DecalRenderer, DecalSpawn};
use crate::render::fx_renderer::{FxBeam, FxDrawParams, FxPoint, FxRenderer};
use crate::render::lighting::{GpuLightsUniform, TextureFilterMode};
use crate::render::mdl_renderer::{MdlPassParams, MdlRenderer, StudioInstance};
use crate::render::scene::{
    BrushDraw, DrawLists, FxFadeTracker, ModelKind, ModelRegistry, SceneFrame, StudioDraw,
    build_draw_lists_tracked,
};
use crate::render::skybox::SkyboxRenderer;
use crate::render::sprite_renderer::{SpritePassParams, SpriteRenderer};

pub struct RenderSettings<'a> {
    pub gamma: f32,
    pub texgamma: f32,
    pub brightness: f32,
    pub fov_deg: f32,
    pub viewmodel_fov: f32,
    pub aspect_mode: &'a str,
    pub texture_mode: TextureFilterMode,
    pub anisotropy: u16,
    pub msaa_samples: u32,
    pub software_mode: bool,
    pub r_vis: bool,
    pub r_decals: usize,
    pub wave_height: f32,
    pub debug_mode: f32,
    pub time_sec: f32,
    pub fog: Option<([f32; 3], f32)>,
}

impl Default for RenderSettings<'static> {
    fn default() -> Self {
        Self {
            gamma: 2.5,
            texgamma: 2.0,
            brightness: 1.0,
            fov_deg: 90.0,
            viewmodel_fov: 90.0,
            aspect_mode: "widescreen",
            texture_mode: TextureFilterMode::LinearMipmapLinear,
            anisotropy: 1,
            msaa_samples: 1,
            software_mode: false,
            r_vis: true,
            r_decals: 1024,
            wave_height: 0.0,
            debug_mode: 0.0,
            time_sec: 0.0,
            fog: None,
        }
    }
}

/// What the shell hands the renderer next to the entity scene.
pub struct SceneDynamicData<'a> {
    pub vfs: Option<&'a Vfs>,
    /// The first-person view model, drawn in its own depth-cleared pass.
    pub viewmodel: Option<StudioInstance<'a>>,
    pub beams: &'a [FxBeam],
    pub points: &'a [FxPoint],
    pub decals: &'a [DecalSpawn],
    pub custom_decals: Option<&'a HashMap<u8, CustomDecalInfo>>,
    pub lightstyles: &'a [f32; 64],
    pub lights: GpuLightsUniform,
    pub elights: &'a [DynamicLight],
}

const DEFAULT_LIGHTSTYLES_EVAL: [f32; 64] = [1.0; 64];

impl<'a> Default for SceneDynamicData<'a> {
    fn default() -> Self {
        Self {
            vfs: None,
            viewmodel: None,
            beams: &[],
            points: &[],
            decals: &[],
            custom_decals: None,
            lightstyles: &DEFAULT_LIGHTSTYLES_EVAL,
            lights: GpuLightsUniform::default(),
            elights: &[],
        }
    }
}

pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    msaa_samples: u32,
    msaa_color_view: Option<wgpu::TextureView>,
    pub bsp: BspRenderer,
    pub skybox: SkyboxRenderer,
    pub decals: DecalRenderer,
    pub mdl_view: MdlRenderer,
    /// Studio entities of the scene API.
    mdl_scene: MdlRenderer,
    sprites: SpriteRenderer,
    /// Precache model table of the scene API.
    models: ModelRegistry,
    /// Entities of the scene API for the next frame; `None` draws the world alone.
    entity_scene: Option<SceneFrame>,
    fx_fade_tracker: FxFadeTracker,
    loaded_map: Option<BspMap>,
    world_collision: Option<WorldCollision>,
    pub fx: FxRenderer,
    pub egui_renderer: egui_wgpu::Renderer,
    pub adapter_name: String,
    pub backend_name: String,
}

/// Selects WebGL2 downlevel limits on GL backends so browser WebGL2 fallback stays within caps.
pub fn required_limits_for_adapter(adapter: &wgpu::Adapter) -> wgpu::Limits {
    if adapter.get_info().backend == wgpu::Backend::Gl {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    } else {
        wgpu::Limits::default().using_resolution(adapter.limits())
    }
}

impl Renderer {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window)
            .expect("create wgpu surface");
        pollster::block_on(Self::new_from_surface(
            &instance,
            surface,
            size.width,
            size.height,
        ))
        .expect("initialize native wgpu renderer")
    }

    /// Initializes the renderer asynchronously from any pre-created `wgpu::Surface` (desktop or canvas).
    pub async fn new_from_surface(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| format!("find compatible GPU adapter: {e}"))?;

        let adapter_info = adapter.get_info();
        let backend_name = format!("{:?}", adapter_info.backend);
        info!(
            gpu = %adapter_info.name,
            backend = %backend_name,
            "Initialized wgpu adapter"
        );

        let limits = required_limits_for_adapter(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("cs16-device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("create wgpu device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .ok_or_else(|| "surface reported no supported texture formats".to_string())?;

        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };

        let surface_usage = if caps.usages.contains(wgpu::TextureUsages::COPY_DST) {
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST
        } else {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        };

        let config = wgpu::SurfaceConfiguration {
            usage: surface_usage,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let msaa_samples = 1;
        let depth_view = create_depth_view(&device, config.width, config.height, msaa_samples);
        let bsp = BspRenderer::new(&device, format);
        let skybox = SkyboxRenderer::new(&device, format);
        let decals = DecalRenderer::new(&device, format);
        let mdl_view = MdlRenderer::new(&device, format);
        let mdl_scene = MdlRenderer::new(&device, format);
        let sprites = SpriteRenderer::new(&device, format);
        let fx = FxRenderer::new(&device, format);
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions {
                depth_stencil_format: None,
                msaa_samples: 1,
                ..Default::default()
            },
        );

        Ok(Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            msaa_samples,
            msaa_color_view: None,
            bsp,
            skybox,
            decals,
            mdl_view,
            mdl_scene,
            sprites,
            models: ModelRegistry::default(),
            entity_scene: None,
            fx_fade_tracker: FxFadeTracker::default(),
            loaded_map: None,
            world_collision: None,
            fx,
            egui_renderer,
            adapter_name: adapter_info.name,
            backend_name,
        })
    }

    fn ensure_msaa_targets(&mut self, samples: u32) {
        let target_samples = if samples >= 4 { 4 } else { 1 };
        if self.msaa_samples == target_samples && (target_samples == 1 || self.msaa_color_view.is_some())
        {
            return;
        }
        self.msaa_samples = target_samples;
        self.depth_view = create_depth_view(
            &self.device,
            self.config.width,
            self.config.height,
            target_samples,
        );
        self.msaa_color_view = if target_samples > 1 {
            Some(create_msaa_color_view(
                &self.device,
                self.config.format,
                self.config.width,
                self.config.height,
                target_samples,
            ))
        } else {
            None
        };
        self.bsp.rebuild_pipelines(&self.device, target_samples);
        self.skybox.rebuild_pipelines(&self.device, target_samples);
        self.decals.rebuild_pipelines(&self.device, target_samples);
        self.mdl_view.rebuild_pipelines(&self.device, target_samples);
        self.mdl_scene.rebuild_pipelines(&self.device, target_samples);
        self.sprites.rebuild_pipelines(&self.device, target_samples);
        self.fx.rebuild_pipelines(&self.device, target_samples);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, width, height, self.msaa_samples);
        if self.msaa_samples > 1 {
            self.msaa_color_view = Some(create_msaa_color_view(
                &self.device,
                self.config.format,
                width,
                height,
                self.msaa_samples,
            ));
        }
    }

    pub fn load_map(&mut self, map: &BspMap, sky: &SkyboxTextures) {
        self.bsp.upload_map(&self.device, &self.queue, map);
        self.skybox.upload_skybox(&self.device, &self.queue, sky);
        self.decals.clear();
        self.fx_fade_tracker.clear();
        self.loaded_map = Some(map.clone());
        self.world_collision = Some(WorldCollision::from_bsp(map));
    }

    pub fn unload_map(&mut self) {
        self.bsp.clear_map();
        self.decals.clear();
        self.fx_fade_tracker.clear();
        self.loaded_map = None;
        self.world_collision = None;
    }

    pub fn clear_model_caches(&mut self) {
        self.mdl_view.clear_cache();
        self.mdl_scene.clear_cache();
        self.sprites.clear_cache();
    }

    /// Install the precache model table (`modelindex` -> `*N` / `.mdl` / `.spr`) used to resolve
    /// scene entities. Call on every new map / precache list.
    pub fn set_model_registry(&mut self, models: ModelRegistry) {
        self.models = models;
    }

    /// Provide the entities of the next frame through the scene API. While a scene is set the
    /// world pass skips the BSP submodels (they are drawn as entities with their transform).
    /// `None` draws the world alone.
    pub fn set_entity_scene(&mut self, scene: Option<SceneFrame>) {
        self.entity_scene = scene;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_frame(
        &mut self,
        camera: &Camera,
        settings: &RenderSettings<'_>,
        scene: &SceneDynamicData<'_>,
        mut egui_textures: egui::TexturesDelta,
        egui_jobs: &[egui::ClippedPrimitive],
        pixels_per_point: f32,
        screenshot_path: Option<&Path>,
    ) -> Option<BspRenderStats> {
        for (id, deltas) in egui_textures.set.drain() {
            for delta in &deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, id, delta);
            }
        }
        let textures_to_free: Vec<_> = egui_textures.free.drain().collect();

        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(tex)
            | wgpu::CurrentSurfaceTexture::Suboptimal(tex) => tex,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                for id in &textures_to_free {
                    self.egui_renderer.free_texture(id);
                }
                return None;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => {
                for id in &textures_to_free {
                    self.egui_renderer.free_texture(id);
                }
                return None;
            }
        };

        let capture_tex = if cfg!(not(target_arch = "wasm32")) {
            screenshot_path.map(|_| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("cs16_screenshot_target"),
                    size: wgpu::Extent3d {
                        width: self.config.width.max(1),
                        height: self.config.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.config.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            })
        } else {
            let _ = screenshot_path;
            None
        };

        let view = if let Some(ref tex) = capture_tex {
            tex.create_view(&wgpu::TextureViewDescriptor::default())
        } else {
            output
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default())
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cs16_frame_encoder"),
            });

        let screen_desc = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let extra_cmds = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            egui_jobs,
            &screen_desc,
        );

        let (vx, vy, vw, vh) = compute_viewport(
            self.config.width as f32,
            self.config.height as f32,
            settings.aspect_mode,
        );

        // Prepare dynamic 3D world models, FX, decals, lighting, MSAA and viewmodel
        self.ensure_msaa_targets(settings.msaa_samples);

        let mut prepared_viewmodel = None;
        let mut prepared_solid_brushes = Vec::new();
        let mut prepared_trans_brushes = Vec::new();
        let mut prepared_scene_studio = None;
        let mut prepared_sprites = None;

        let draw_lists: Option<DrawLists> = self.entity_scene.as_ref().map(|frame| {
            build_draw_lists_tracked(frame, &self.models, Some(&mut self.fx_fade_tracker))
        });
        self.bsp.set_skip_submodels(draw_lists.is_some());

        let filter_mode = settings.texture_mode;
        let aniso = settings.anisotropy.clamp(1, 16);
        self.bsp.set_texture_filter(&self.device, filter_mode, aniso);
        self.skybox.set_texture_filter(&self.device, filter_mode);
        self.decals.set_texture_filter(&self.device, filter_mode);
        self.mdl_view.set_texture_filter(&self.device, filter_mode, aniso);
        self.mdl_scene.set_texture_filter(&self.device, filter_mode, aniso);
        self.sprites.set_texture_filter(&self.device, filter_mode);

        let cam_fwd = camera.forward();
        let cam_right = camera.right();
        let cam_up = cam_right.cross(cam_fwd).normalize_or_zero();
        let default_sprays = HashMap::new();
        let custom_sprays = scene.custom_decals.unwrap_or(&default_sprays);

        let sample_light_at = |pos: Vec3| -> Vec3 {
            let mut rgb = self
                .loaded_map
                .as_ref()
                .map(|m| m.sample_light_point_styled(pos, scene.lightstyles))
                .unwrap_or_else(|| Vec3::splat(0.75));
            for l in scene.elights {
                let r = l.radius.max(1.0);
                let dist = (l.origin - pos).length();
                if dist < r {
                    let atten = 1.0 - dist / r;
                    rgb += Vec3::new(
                        f32::from(l.color[0]) / 255.0,
                        f32::from(l.color[1]) / 255.0,
                        f32::from(l.color[2]) / 255.0,
                    ) * atten;
                }
            }
            rgb.clamp(Vec3::splat(0.05), Vec3::splat(2.0))
        };

        if self.bsp.has_map() {
            self.bsp.update_lightstyles(&self.queue, scene.lightstyles);
            self.bsp.prepare_world_pvs(
                &self.device,
                &self.queue,
                camera.position,
                settings.r_vis,
            );
            let proj = camera.proj_matrix(vw, vh, settings.fov_deg, settings.aspect_mode);
            let view_mat = camera.view_matrix();
            let view_proj = proj * view_mat;

            if let Some(vfs) = scene.vfs
                && !scene.decals.is_empty()
            {
                self.decals.spawn_decals(
                    &self.device,
                    &self.queue,
                    vfs,
                    self.loaded_map.as_ref(),
                    scene.decals,
                    settings.r_decals,
                    custom_sprays,
                );
            }
            self.decals.prepare(
                &self.device,
                &self.queue,
                view_proj,
                camera.position,
                settings.gamma,
                settings.texgamma,
                settings.brightness,
                settings.software_mode,
                settings.fog,
            );

            if let (Some(vfs), Some(vm)) = (scene.vfs, scene.viewmodel.as_ref()) {
                let vm_proj = camera.viewmodel_proj_matrix_fov(
                    vw,
                    vh,
                    settings.viewmodel_fov,
                    settings.aspect_mode,
                );
                let vm_vp = vm_proj * view_mat;
                let vm_inst = StudioInstance {
                    model_path: vm.model_path,
                    transform: vm.transform,
                    sequence: vm.sequence,
                    frame: vm.frame,
                    animtime: vm.animtime,
                    framerate: vm.framerate,
                    now: vm.now,
                    body: vm.body,
                    skin: vm.skin,
                    controllers: vm.controllers,
                    mouth: vm.mouth,
                    blending: vm.blending,
                    gait: vm.gait,
                    prev_sequence: vm.prev_sequence,
                    light_rgb: sample_light_at(camera.position),
                    rendermode: vm.rendermode,
                    renderfx: vm.renderfx,
                    rendercolor: vm.rendercolor,
                    renderamt: vm.renderamt,
                    blend: vm.blend,
                    flip_winding: vm.flip_winding,
                    attached_model: vm.attached_model,
                };
                let vm_params = MdlPassParams {
                    view_proj: vm_vp,
                    camera_pos: camera.position,
                    cam_right,
                    cam_up,
                    gamma: settings.gamma,
                    texgamma: settings.texgamma,
                    brightness: settings.brightness,
                    software_mode: settings.software_mode,
                    fog: settings.fog,
                    lights: scene.lights,
                };
                prepared_viewmodel = Some(self.mdl_view.prepare_studio(
                    &self.device,
                    &self.queue,
                    vfs,
                    std::slice::from_ref(&vm_inst),
                    &vm_params,
                ));
            }

            if let (Some(lists), Some(_)) = (&draw_lists, &self.entity_scene) {
                let brushes: Vec<&BrushDraw> = lists
                    .solid_brush
                    .iter()
                    .chain(lists.trans_brush.iter())
                    .collect();
                let (solid, trans) = self
                    .bsp
                    .prepare_entities(&self.device, &self.queue, &brushes, settings.time_sec)
                    .into_iter()
                    .partition(|p| p.is_opaque());
                prepared_solid_brushes = solid;
                prepared_trans_brushes = trans;
            }

            if let (Some(lists), Some(frame), Some(vfs)) =
                (&draw_lists, &self.entity_scene, scene.vfs)
            {
                let to_instance = |d: &StudioDraw| -> Option<StudioInstance<'_>> {
                    let entry = self.models.get(d.model)?;
                    (entry.kind == ModelKind::Studio).then(|| StudioInstance {
                        model_path: entry.name.as_str(),
                        transform: d.matrix,
                        sequence: d.sequence,
                        frame: d.frame,
                        animtime: d.animtime,
                        framerate: d.framerate,
                        now: frame.time,
                        body: d.body,
                        skin: d.skin,
                        controllers: d.controllers,
                        mouth: d.mouth,
                        blending: d.blending,
                        gait: d.gait,
                        prev_sequence: d.prev_sequence,
                        light_rgb: sample_light_at(d.origin),
                        rendermode: d.mode,
                        renderfx: d.renderfx,
                        rendercolor: d.rendercolor,
                        renderamt: d.renderamt,
                        blend: d.blend,
                        flip_winding: false,
                        attached_model: d
                            .attached
                            .and_then(|h| self.models.get(h))
                            .map(|e| e.name.as_str()),
                    })
                };
                let studio: Vec<StudioInstance<'_>> =
                    lists.studio.iter().filter_map(to_instance).collect();
                let scene_params = MdlPassParams {
                    view_proj,
                    camera_pos: camera.position,
                    cam_right,
                    cam_up,
                    gamma: settings.gamma,
                    texgamma: settings.texgamma,
                    brightness: settings.brightness,
                    software_mode: settings.software_mode,
                    fog: settings.fog,
                    lights: scene.lights,
                };
                if !studio.is_empty() {
                    prepared_scene_studio = Some(self.mdl_scene.prepare_studio(
                        &self.device,
                        &self.queue,
                        vfs,
                        &studio,
                        &scene_params,
                    ));
                }

                if !lists.sprites.is_empty() {
                    let paths: Vec<Option<&str>> = lists
                        .sprites
                        .iter()
                        .map(|d| self.models.get(d.model).map(|e| e.name.as_str()))
                        .collect();
                    let spr_params = SpritePassParams {
                        view_proj,
                        gamma: settings.gamma,
                        texgamma: settings.texgamma,
                        brightness: settings.brightness,
                        software_mode: settings.software_mode,
                        fog: settings.fog,
                    };
                    prepared_sprites = Some(self.sprites.prepare(
                        &self.device,
                        &self.queue,
                        vfs,
                        &lists.sprites,
                        &paths,
                        &frame.view,
                        &spr_params,
                        self.world_collision.as_ref(),
                    ));
                }
            }

            let fx_params = FxDrawParams {
                view_proj,
                cam_pos: camera.position,
                cam_right,
                cam_up,
                gamma: settings.gamma,
                brightness: settings.brightness,
                software_mode: settings.software_mode,
                fog: settings.fog,
            };
            self.fx.prepare(
                &self.device,
                &self.queue,
                &fx_params,
                scene.beams,
                scene.points,
            );
        }

        // 2. Main 3D world pass: BSP + Decals + Solid MDLs + Skybox + FX + Translucent Brushes/MDLs/Sprites
        let mut stats = BspRenderStats {
            draw_calls: 0,
            triangles: 0,
        };
        let (world_color_view, world_resolve) = match &self.msaa_color_view {
            Some(msaa) => (msaa, Some(&view)),
            None => (&view, None),
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cs16_3d_world_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: world_color_view,
                    depth_slice: None,
                    resolve_target: world_resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.06,
                            b: 0.05,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            if self.bsp.has_map() {
                pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
                pass.set_scissor_rect(vx as u32, vy as u32, vw as u32, vh as u32);

                let proj = camera.proj_matrix(vw, vh, settings.fov_deg, settings.aspect_mode);
                let view_proj = proj * camera.view_matrix();
                let sky_view_proj = proj * camera.sky_view_matrix();

                let bsp_params = BspDrawParams {
                    view_proj,
                    camera_pos: camera.position,
                    gamma: settings.gamma,
                    texgamma: settings.texgamma,
                    brightness: settings.brightness,
                    software_mode: settings.software_mode,
                    time_sec: settings.time_sec,
                    debug_mode: settings.debug_mode,
                    wave_height: settings.wave_height,
                    fog: settings.fog,
                    r_vis: settings.r_vis,
                    lights: scene.lights,
                };
                stats = self.bsp.draw(&self.queue, &mut pass, &bsp_params, &[]);
                self.decals.draw(&mut pass);

                self.bsp.draw_entities(
                    &mut pass,
                    &prepared_solid_brushes,
                    settings.time_sec,
                );
                if let Some(ref studio) = prepared_scene_studio {
                    self.mdl_scene.draw_opaque(&mut pass, studio);
                }

                self.skybox.draw(
                    &self.queue,
                    &mut pass,
                    sky_view_proj,
                    settings.gamma,
                    settings.texgamma,
                    settings.brightness,
                    settings.software_mode,
                    settings.fog,
                );

                self.fx.draw(&mut pass);

                self.bsp.draw_entities(
                    &mut pass,
                    &prepared_trans_brushes,
                    settings.time_sec,
                );
                if let Some(ref studio) = prepared_scene_studio {
                    self.mdl_scene.draw_translucent(&mut pass, studio);
                }
                if let Some(ref spr) = prepared_sprites {
                    self.sprites.draw(&mut pass, spr);
                }
            }
        }

        // 3. Dedicated depth-cleared Viewmodel pass (weapons never clip through walls!)
        if let Some(ref vm_pass_data) = prepared_viewmodel {
            let (vm_color_view, vm_resolve) = match &self.msaa_color_view {
                Some(msaa) => (msaa, Some(&view)),
                None => (&view, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cs16_viewmodel_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: vm_color_view,
                    depth_slice: None,
                    resolve_target: vm_resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.set_scissor_rect(vx as u32, vy as u32, vw as u32, vh as u32);
            self.mdl_view.draw(&mut pass, vm_pass_data);
        }

        // 4. VGUI2 / HUD / Console pass (egui)
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cs16_egui_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            let mut pass_static = pass.forget_lifetime();
            self.egui_renderer
                .render(&mut pass_static, egui_jobs, &screen_desc);
        }

        // Optional GPU readback for `screenshot` / `--screenshot` (native only)
        #[cfg(not(target_arch = "wasm32"))]
        let staging = if let (Some(tex), Some(_)) = (&capture_tex, screenshot_path) {
            let w = self.config.width.max(1);
            let h = self.config.height.max(1);
            if self.config.usage.contains(wgpu::TextureUsages::COPY_DST) {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &output.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
            let unpadded_bpr = w * 4;
            let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let padded_bpr = unpadded_bpr.div_ceil(align) * align;
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cs16_screenshot_staging"),
                size: (padded_bpr as u64) * (h as u64),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_bpr),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            Some((buf, w, h, padded_bpr))
        } else {
            None
        };
        #[cfg(target_arch = "wasm32")]
        let _ = capture_tex;

        self.queue
            .submit(extra_cmds.into_iter().chain(std::iter::once(encoder.finish())));

        #[cfg(not(target_arch = "wasm32"))]
        if let (Some((buf, w, h, padded_bpr)), Some(out_path)) = (staging, screenshot_path) {
            let slice = buf.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            if let Ok(Ok(())) = rx.recv()
                && let Ok(mapped) = slice.get_mapped_range()
            {
                let is_bgra = matches!(
                    self.config.format,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                );
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h {
                    let row_start = (y * padded_bpr) as usize;
                    let row = &mapped[row_start..row_start + (w * 4) as usize];
                    for px in row.as_chunks::<4>().0 {
                        if is_bgra {
                            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                        } else {
                            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                        }
                    }
                }
                drop(mapped);
                buf.unmap();
                if let Err(err) = image::save_buffer(out_path, &rgba, w, h, image::ColorType::Rgba8)
                {
                    tracing::error!(path = %out_path.display(), %err, "Failed to save screenshot PNG");
                } else {
                    info!(path = %out_path.display(), width = w, height = h, "Saved GPU screenshot");
                }
            }
        }

        self.queue.present(output);

        for id in &textures_to_free {
            self.egui_renderer.free_texture(id);
        }

        Some(stats)
    }
}

fn compute_viewport(win_w: f32, win_h: f32, aspect_mode: &str) -> (f32, f32, f32, f32) {
    if aspect_mode.eq_ignore_ascii_case("4:3_bars") {
        let target_w = (win_h * (4.0 / 3.0)).min(win_w);
        let target_h = (target_w * (3.0 / 4.0)).min(win_h);
        let x = ((win_w - target_w) * 0.5).floor();
        let y = ((win_h - target_h) * 0.5).floor();
        (x, y, target_w.max(1.0), target_h.max(1.0))
    } else {
        (0.0, 0.0, win_w.max(1.0), win_h.max(1.0))
    }
}

fn create_depth_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    samples: u32,
) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cs16_depth_buffer"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples.max(1),
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn create_msaa_color_view(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    samples: u32,
) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cs16_msaa_color"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples.max(1),
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Offscreen headless render of a [`BspMap`] + optional decals/FX for automated visual tests.
#[cfg(not(target_arch = "wasm32"))]
pub fn render_offscreen_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    map: &BspMap,
    sky: &SkyboxTextures,
    camera: &Camera,
    width: u32,
    height: u32,
    params: &BspDrawParams,
) -> BspRenderStats {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let w = width.max(1);
    let h = height.max(1);
    let color_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen_color"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = create_depth_view(device, w, h, 1);

    let mut bsp = BspRenderer::new(device, format);
    bsp.upload_map(device, queue, map);
    bsp.prepare_world_pvs(device, queue, camera.position, params.r_vis);
    let mut skybox = SkyboxRenderer::new(device, format);
    skybox.upload_skybox(device, queue, sky);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("offscreen_encoder"),
    });
    let stats = {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offscreen_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        let s = bsp.draw(queue, &mut pass, params, &[]);
        skybox.draw(
            queue,
            &mut pass,
            camera.proj_matrix(w as f32, h as f32, 90.0, "widescreen") * camera.sky_view_matrix(),
            params.gamma,
            params.texgamma,
            params.brightness,
            params.software_mode,
            params.fog,
        );
        s
    };
    queue.submit(Some(encoder.finish()));
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webgl2_limits_fit_lightmap_atlas() {
        let gl_limits = wgpu::Limits::downlevel_webgl2_defaults();
        assert!(
            gl_limits.max_texture_dimension_2d >= cs_assets::bsp::LIGHTMAP_ATLAS_SIZE,
            "WebGL2 texture dimension {} smaller than lightmap atlas {}",
            gl_limits.max_texture_dimension_2d,
            cs_assets::bsp::LIGHTMAP_ATLAS_SIZE
        );
    }

    /// Headless device for pipeline/shader validation; `None` when the machine has no adapter.
    /// The GPU test below is a no-op in that case (it returns without assertions, like the other
    /// environment-dependent tests), so a green run does not prove the WGSL compiles.
    #[cfg(not(target_arch = "wasm32"))]
    fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cs16-test-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .ok()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn scene_renderers_build_on_a_headless_device() {
        let Some((device, queue)) = headless_device() else {
            return;
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut bsp = BspRenderer::new(&device, format);
        let _sprites = SpriteRenderer::new(&device, format);
        let _mdl = MdlRenderer::new(&device, format);
        // No map uploaded: entity preparation is a no-op, not a panic.
        let draws: Vec<&BrushDraw> = Vec::new();
        assert!(bsp.prepare_entities(&device, &queue, &draws, 0.0).is_empty());
    }
}
