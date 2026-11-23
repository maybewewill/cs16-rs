//! Top-level wgpu renderer coordinating BSP world, Skybox, the entities of the scene API (brush
//! models, studio models, sprites), flat 3D FX (beams, glows), the depth-cleared first-person
//! view model, and the VGUI2/HUD overlay.

pub mod bsp_renderer;
pub mod fx_renderer;
pub mod mdl_renderer;
pub mod scene;
pub mod skybox;
pub mod sprite_renderer;

use std::path::Path;
use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_assets::scheme::SkyboxTextures;
use cs_vfs::Vfs;
use egui_wgpu::ScreenDescriptor;
use glam::Vec3;
use tracing::{error, info};
use winit::window::Window;

use crate::camera::Camera;
use crate::render::bsp_renderer::{BspRenderStats, BspRenderer};
use crate::render::fx_renderer::{FxBeam, FxPoint, FxRenderer};
use crate::render::mdl_renderer::{MdlRenderer, StudioInstance};
use crate::render::scene::{
    BrushDraw, DrawLists, ModelKind, ModelRegistry, SceneFrame, build_draw_lists,
};
use crate::render::skybox::SkyboxRenderer;
use crate::render::sprite_renderer::SpriteRenderer;

pub struct RenderSettings<'a> {
    pub gamma: f32,
    pub brightness: f32,
    pub fov_deg: f32,
    pub aspect_mode: &'a str,
    pub use_nearest_filter: bool,
    pub debug_mode: f32,
    pub time_sec: f32,
}

/// What the shell hands the renderer next to the entity scene.
pub struct SceneDynamicData<'a> {
    pub vfs: Option<&'a Vfs>,
    /// The first-person view model, drawn in its own depth-cleared pass.
    pub viewmodel: Option<StudioInstance<'a>>,
    pub beams: &'a [FxBeam],
    pub points: &'a [FxPoint],
}

pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    pub bsp: BspRenderer,
    pub skybox: SkyboxRenderer,
    pub mdl_view: MdlRenderer,
    /// Studio entities of the scene API.
    mdl_scene: MdlRenderer,
    sprites: SpriteRenderer,
    /// Precache model table of the scene API.
    models: ModelRegistry,
    /// Entities of the scene API for the next frame; `None` draws the world alone.
    entity_scene: Option<SceneFrame>,
    pub fx: FxRenderer,
    pub egui_renderer: egui_wgpu::Renderer,
    pub adapter_name: String,
}

impl Renderer {
    pub fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window)
            .expect("create wgpu surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("find compatible GPU adapter");

        let adapter_info = adapter.get_info();
        info!(
            gpu = %adapter_info.name,
            backend = ?adapter_info.backend,
            "Initialized wgpu adapter"
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cs16-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .expect("create wgpu device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

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
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let depth_view = create_depth_view(&device, config.width, config.height);
        let bsp = BspRenderer::new(&device, format);
        let skybox = SkyboxRenderer::new(&device, format);
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

        Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            bsp,
            skybox,
            mdl_view,
            mdl_scene,
            sprites,
            models: ModelRegistry::default(),
            entity_scene: None,
            fx,
            egui_renderer,
            adapter_name: adapter_info.name,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, width, height);
    }

    pub fn load_map(&mut self, map: &BspMap, sky: &SkyboxTextures) {
        self.bsp.upload_map(&self.device, &self.queue, map);
        self.skybox.upload_skybox(&self.device, &self.queue, sky);
    }

    pub fn unload_map(&mut self) {
        self.bsp.clear_map();
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

        let capture_tex = screenshot_path.map(|_| {
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
        });

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

        // Prepare dynamic 3D world models, FX, and viewmodel before opening render passes
        let mut prepared_viewmodel = None;
        let mut prepared_solid_brushes = Vec::new();
        let mut prepared_trans_brushes = Vec::new();
        let mut prepared_scene_studio = None;
        let mut prepared_sprites = None;

        let draw_lists: Option<DrawLists> = self
            .entity_scene
            .as_ref()
            .map(|scene| build_draw_lists(scene, &self.models));
        self.bsp.set_skip_submodels(draw_lists.is_some());

        if self.bsp.has_map() {
            let proj = camera.proj_matrix(vw, vh, settings.fov_deg, settings.aspect_mode);
            let view_mat = camera.view_matrix();
            let view_proj = proj * view_mat;

            if let (Some(vfs), Some(vm)) = (scene.vfs, scene.viewmodel.as_ref()) {
                let vm_proj = camera.viewmodel_proj_matrix(vw, vh, settings.aspect_mode);
                let vm_vp = vm_proj * view_mat;
                prepared_viewmodel = Some(self.mdl_view.prepare_studio(
                    &self.device,
                    &self.queue,
                    vfs,
                    std::slice::from_ref(vm),
                    vm_vp,
                    settings.gamma,
                    settings.brightness,
                ));
            }

            if let (Some(lists), Some(_)) = (&draw_lists, &self.entity_scene) {
                // Brush entities need no assets from the VFS: prepare them unconditionally.
                let brushes: Vec<&BrushDraw> = lists
                    .solid_brush
                    .iter()
                    .chain(lists.trans_brush.iter())
                    .collect();
                let (solid, trans) = self
                    .bsp
                    .prepare_entities(&self.device, &self.queue, &brushes)
                    .into_iter()
                    .partition(|p| p.is_opaque());
                prepared_solid_brushes = solid;
                prepared_trans_brushes = trans;
            }

            if let (Some(lists), Some(frame), Some(vfs)) =
                (&draw_lists, &self.entity_scene, scene.vfs)
            {
                let studio: Vec<StudioInstance<'_>> = lists
                    .studio
                    .iter()
                    .filter_map(|d| {
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
                            // Assumption: flat ambient until the shell samples the BSP lightgrid.
                            light_rgb: Vec3::splat(0.8),
                            flip_winding: false,
                            attached_model: d
                                .attached
                                .and_then(|h| self.models.get(h))
                                .map(|e| e.name.as_str()),
                        })
                    })
                    .collect();
                if !studio.is_empty() {
                    prepared_scene_studio = Some(self.mdl_scene.prepare_studio(
                        &self.device,
                        &self.queue,
                        vfs,
                        &studio,
                        view_proj,
                        settings.gamma,
                        settings.brightness,
                    ));
                }

                if !lists.sprites.is_empty() {
                    let paths: Vec<Option<&str>> = lists
                        .sprites
                        .iter()
                        .map(|d| self.models.get(d.model).map(|e| e.name.as_str()))
                        .collect();
                    prepared_sprites = Some(self.sprites.prepare(
                        &self.device,
                        &self.queue,
                        vfs,
                        &lists.sprites,
                        &paths,
                        &frame.view,
                        view_proj,
                        settings.gamma,
                        settings.brightness,
                    ));
                }
            }

            let cam_fwd = camera.forward();
            let cam_right = camera.right();
            let cam_up = cam_right.cross(cam_fwd).normalize_or_zero();
            self.fx.prepare(
                &self.device,
                &self.queue,
                view_proj,
                camera.position,
                cam_right,
                cam_up,
                scene.beams,
                scene.points,
            );
        }

        // 2. Main 3D world pass: BSP + 3D player/prop MDLs + Skybox + 3D FX
        let mut stats = BspRenderStats {
            draw_calls: 0,
            triangles: 0,
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cs16_3d_world_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
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

                stats = self.bsp.draw(
                    &self.queue,
                    &mut pass,
                    view_proj,
                    settings.gamma,
                    settings.brightness,
                    settings.time_sec,
                    settings.debug_mode,
                    settings.use_nearest_filter,
                    &[],
                );

                self.bsp.draw_entities(
                    &mut pass,
                    &prepared_solid_brushes,
                    settings.use_nearest_filter,
                );
                if let Some(ref studio) = prepared_scene_studio {
                    self.mdl_scene.draw(&mut pass, studio);
                }

                self.skybox.draw(
                    &self.queue,
                    &mut pass,
                    sky_view_proj,
                    settings.gamma,
                    settings.brightness,
                );

                self.fx.draw(&mut pass);

                // Translucent brush entities (far to near) and sprites come last, as in
                // `R_DrawEntitiesOnList`.
                self.bsp.draw_entities(
                    &mut pass,
                    &prepared_trans_brushes,
                    settings.use_nearest_filter,
                );
                if let Some(ref spr) = prepared_sprites {
                    self.sprites.draw(&mut pass, spr);
                }
            }
        }

        // 3. Dedicated depth-cleared Viewmodel pass (weapons never clip through walls!)
        if let Some(ref vm_pass_data) = prepared_viewmodel {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cs16_viewmodel_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
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

        // Optional GPU readback for `screenshot` / `--screenshot`
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

        self.queue
            .submit(extra_cmds.into_iter().chain(std::iter::once(encoder.finish())));

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
                    error!(path = %out_path.display(), %err, "Failed to save screenshot PNG");
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

fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cs16_depth_buffer"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Headless device for pipeline/shader validation; `None` when the machine has no adapter.
    /// The GPU test below is a no-op in that case (it returns without assertions, like the other
    /// environment-dependent tests), so a green run does not prove the WGSL compiles.
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
        assert!(bsp.prepare_entities(&device, &queue, &draws).is_empty());
    }
}
