//! GoldSrc sprite (`.spr`) renderer across all 5 orientation types and all render modes, with
//! `WorldCollision` line-of-sight occlusion for `kRenderGlow`, run-length draw-call batching
//! (45-50 smoke puffs in 1 draw call), `GL_EXP2` fog, `gamma`/`texgamma`/`r_software_mode`,
//! `TextureFilterMode`, and MSAA (`1 | 4`).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use cs_assets::spr::{GoldSrcSprite, SpriteType};
use cs_pm::collision::WorldCollision;
use cs_vfs::Vfs;
use glam::Mat4;
use tracing::warn;
use wgpu::util::DeviceExt;

use crate::render::lighting::{TextureFilterMode, create_sampler, upload_rgba_texture_mipmapped};
use crate::render::scene::{
    BrushCollider, EntRenderMode, ModelRegistry, SpriteDraw, ViewBasis, glow_blend, sprite_axes,
    sprite_quad, trace_glow_occlusion,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpriteVertex {
    position: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpriteUniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    fog_color_density: [f32; 4],
    gamma_params: [f32; 4],
}

/// Shared camera, fog, and gamma parameters for [`SpriteRenderer::prepare`].
#[derive(Debug, Clone, Copy)]
pub struct SpritePassParams {
    pub view_proj: Mat4,
    pub gamma: f32,
    pub texgamma: f32,
    pub brightness: f32,
    pub software_mode: bool,
    pub fog: Option<([f32; 3], f32)>,
}

/// How a sprite quad is blended (`R_DrawSpriteModel`'s render-mode switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteBlend {
    Opaque,
    Alpha,
    AlphaNoDepthWrite,
    Add,
    Glow,
}

pub fn sprite_blend_for(mode: EntRenderMode) -> SpriteBlend {
    match mode {
        EntRenderMode::Normal => SpriteBlend::Opaque,
        EntRenderMode::TransColor | EntRenderMode::TransTexture => SpriteBlend::Alpha,
        EntRenderMode::TransAlpha => SpriteBlend::AlphaNoDepthWrite,
        EntRenderMode::TransAdd => SpriteBlend::Add,
        EntRenderMode::Glow => SpriteBlend::Glow,
    }
}

pub fn clamp_frame(frame: f32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let f = frame as i64;
    Some(f.clamp(0, count as i64 - 1) as usize)
}

pub fn frame_extents(
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
) -> (f32, f32, f32, f32) {
    let left = origin_x as f32;
    let up = origin_y as f32;
    (left, left + width as f32, up, up - height as f32)
}

struct GpuFrame {
    left: f32,
    right: f32,
    up: f32,
    down: f32,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

struct GpuSprite {
    sprite_type: SpriteType,
    frames: Vec<GpuFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreparedSpriteBatch {
    sprite_idx: u32,
    frame: u32,
    first_vertex: u32,
    vertex_count: u32,
    blend: SpriteBlend,
}

fn push_sprite_batch(
    batches: &mut Vec<PreparedSpriteBatch>,
    sprite_idx: u32,
    frame: u32,
    first_vertex: u32,
    vertex_count: u32,
    blend: SpriteBlend,
) {
    if let Some(last) = batches.last_mut()
        && last.blend == blend
        && last.sprite_idx == sprite_idx
        && last.frame == frame
        && last.first_vertex + last.vertex_count == first_vertex
    {
        last.vertex_count += vertex_count;
    } else {
        batches.push(PreparedSpriteBatch {
            sprite_idx,
            frame,
            first_vertex,
            vertex_count,
            blend,
        });
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PreparedSprites {
    batch_count: usize,
    sprite_count: usize,
}

impl PreparedSprites {
    /// Number of coalesced GPU draw calls in this prepared sprite pass.
    pub fn batch_count(&self) -> usize {
        self.batch_count
    }

    /// Number of sprite quads emitted across all batches in this pass.
    pub fn sprite_count(&self) -> usize {
        self.sprite_count
    }
}

pub struct SpriteRenderer {
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: [wgpu::RenderPipeline; 5],
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    filter_mode: TextureFilterMode,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    vertex_cap: usize,
    sprites: Vec<Option<GpuSprite>>,
    exact_path_to_id: HashMap<String, u32>,
    lower_path_to_id: HashMap<String, u32>,
    scratch_verts: Vec<SpriteVertex>,
    prepared_batches: Vec<PreparedSpriteBatch>,
    frame_upload_bytes: u64,
}

fn pipeline_slot(b: SpriteBlend) -> usize {
    match b {
        SpriteBlend::Opaque => 0,
        SpriteBlend::Alpha => 1,
        SpriteBlend::AlphaNoDepthWrite => 2,
        SpriteBlend::Add => 3,
        SpriteBlend::Glow => 4,
    }
}

fn create_sprite_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) -> [wgpu::RenderPipeline; 5] {
    let alpha_blend = Some(wgpu::BlendState::ALPHA_BLENDING);
    let add_blend = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    });
    let make = |label: &str,
                blend: Option<wgpu::BlendState>,
                depth_write: bool,
                depth_compare: wgpu::CompareFunction| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SpriteVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x2,
                        2 => Float32x4,
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(depth_compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count.max(1),
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    use wgpu::CompareFunction::{Always, LessEqual};
    [
        make("sprite_opaque", None, true, LessEqual),
        // GoldSrc `kRenderTransTexture` / `kRenderTransColor` sprites (bloodspray, blood drops,
        // smokepuff) blend without writing depth so overlapping puffs never cut opaque squares.
        make("sprite_alpha", alpha_blend, false, LessEqual),
        make("sprite_alpha_nodepth", alpha_blend, false, LessEqual),
        make("sprite_add", add_blend, false, LessEqual),
        make("sprite_glow", add_blend, false, Always),
    ]
}

impl SpriteRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sprite.wgsl").into()),
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite_uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite_tex_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });

        let msaa_samples = 1;
        let pipelines = create_sprite_pipelines(
            device,
            &shader,
            &pipeline_layout,
            color_format,
            msaa_samples,
        );

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sprite_uniform_buf"),
            contents: bytemuck::bytes_of(&SpriteUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0, 0.0, 0.0, 1.0],
                fog_color_density: [0.0, 0.0, 0.0, 0.0],
                gamma_params: [2.5, 2.0, 1.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite_uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let filter_mode = TextureFilterMode::LinearMipmapLinear;
        let sampler = create_sampler(device, "sprite_sampler", filter_mode, 1, true);
        let vertex_cap = 6 * 256;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite_dynamic_vbuf"),
            size: (vertex_cap * std::mem::size_of::<SpriteVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            color_format,
            msaa_samples,
            shader,
            pipeline_layout,
            pipelines,
            tex_bgl,
            sampler,
            filter_mode,
            uniform_buf,
            uniform_bg,
            vertex_buf,
            vertex_cap,
            sprites: Vec::new(),
            exact_path_to_id: HashMap::new(),
            lower_path_to_id: HashMap::new(),
            scratch_verts: Vec::new(),
            prepared_batches: Vec::new(),
            frame_upload_bytes: 0,
        }
    }

    pub(crate) fn take_upload_bytes(&mut self) -> u64 {
        std::mem::take(&mut self.frame_upload_bytes)
    }

    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device, msaa_samples: u32) {
        let samples = msaa_samples.max(1);
        if self.msaa_samples == samples {
            return;
        }
        self.msaa_samples = samples;
        self.pipelines = create_sprite_pipelines(
            device,
            &self.shader,
            &self.pipeline_layout,
            self.color_format,
            samples,
        );
    }

    pub fn set_texture_filter(&mut self, device: &wgpu::Device, mode: TextureFilterMode) {
        if self.filter_mode == mode {
            return;
        }
        self.filter_mode = mode;
        self.sampler = create_sampler(device, "sprite_sampler", mode, 1, true);
        for (idx, spr) in self.sprites.iter_mut().flatten().enumerate() {
            for (fi, f) in spr.frames.iter_mut().enumerate() {
                f.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("sprite_bg_{idx}_{fi}")),
                    layout: &self.tex_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&f.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
            }
        }
    }

    pub fn clear_cache(&mut self) {
        self.sprites.clear();
        self.exact_path_to_id.clear();
        self.lower_path_to_id.clear();
        self.prepared_batches.clear();
    }

    fn ensure_loaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        rel_path: &str,
    ) -> Option<u32> {
        if let Some(&id) = self.exact_path_to_id.get(rel_path) {
            return self
                .sprites
                .get(id as usize)
                .and_then(|o| o.as_ref())
                .map(|_| id);
        }

        let lower = rel_path.to_ascii_lowercase();
        if let Some(&id) = self.lower_path_to_id.get(&lower) {
            self.exact_path_to_id.insert(rel_path.to_string(), id);
            return self
                .sprites
                .get(id as usize)
                .and_then(|o| o.as_ref())
                .map(|_| id);
        }

        let loaded = match GoldSrcSprite::load(vfs, &lower) {
            Ok(spr) if !spr.frames.is_empty() => {
                let frames = spr
                    .frames
                    .iter()
                    .map(|f| {
                        let view = upload_rgba_texture_mipmapped(
                            device,
                            queue,
                            &lower,
                            f.width.max(1),
                            f.height.max(1),
                            &f.rgba,
                        );
                        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some(&lower),
                            layout: &self.tex_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(&view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                                },
                            ],
                        });
                        let (left, right, up, down) =
                            frame_extents(f.origin_x, f.origin_y, f.width, f.height);
                        GpuFrame {
                            left,
                            right,
                            up,
                            down,
                            view,
                            bind_group,
                        }
                    })
                    .collect();
                Some(GpuSprite {
                    sprite_type: spr.sprite_type,
                    frames,
                })
            }
            Ok(_) => {
                warn!(sprite = %lower, "Sprite has no frames");
                None
            }
            Err(err) => {
                warn!(sprite = %lower, %err, "Could not load sprite");
                None
            }
        };
        let is_some = loaded.is_some();
        let id = self.sprites.len() as u32;
        self.sprites.push(loaded);
        self.lower_path_to_id.insert(lower, id);
        self.exact_path_to_id.insert(rel_path.to_string(), id);
        is_some.then_some(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        draws: &[SpriteDraw],
        paths: &[Option<&str>],
        view: &ViewBasis,
        params: &SpritePassParams,
        collision: Option<&WorldCollision>,
        brush_colliders: &[BrushCollider],
    ) -> PreparedSprites {
        self.prepare_iter(
            device,
            queue,
            vfs,
            draws.iter().zip(paths.iter().copied()),
            view,
            params,
            collision,
            brush_colliders,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_from_registry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        draws: &[SpriteDraw],
        models: &ModelRegistry,
        view: &ViewBasis,
        params: &SpritePassParams,
        collision: Option<&WorldCollision>,
        brush_colliders: &[BrushCollider],
    ) -> PreparedSprites {
        self.prepare_iter(
            device,
            queue,
            vfs,
            draws
                .iter()
                .map(|d| (d, models.get(d.model).map(|e| e.name.as_str()))),
            view,
            params,
            collision,
            brush_colliders,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_iter<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        items: impl Iterator<Item = (&'a SpriteDraw, Option<&'a str>)>,
        view: &ViewBasis,
        params: &SpritePassParams,
        collision: Option<&WorldCollision>,
        brush_colliders: &[BrushCollider],
    ) -> PreparedSprites {
        self.scratch_verts.clear();
        self.prepared_batches.clear();
        let mut sprite_count = 0usize;

        for (d, path) in items {
            let Some(path) = path else { continue };
            if d.mode == EntRenderMode::Glow
                && let Some(world) = collision
                && trace_glow_occlusion(world, brush_colliders, view.origin, d.origin)
            {
                continue;
            }
            let Some(sprite_idx) = self.ensure_loaded(device, queue, vfs, path) else {
                continue;
            };
            let Some(Some(spr)) = self.sprites.get(sprite_idx as usize) else {
                continue;
            };
            let Some(frame_idx) = clamp_frame(d.frame, spr.frames.len()) else {
                continue;
            };
            let frame = &spr.frames[frame_idx];
            let Some(axes) = sprite_axes(spr.sprite_type, d.origin, d.angles, view) else {
                continue;
            };
            let mut scale = d.scale;
            let mut alpha = d.blend;
            if d.mode == EntRenderMode::Glow {
                let (brightness_mul, scale_mul) =
                    glow_blend((d.origin - view.origin).length(), d.renderfx);
                alpha *= brightness_mul;
                scale *= scale_mul;
                if alpha <= 0.01 {
                    continue;
                }
            }
            let color = [d.color[0], d.color[1], d.color[2], alpha];
            let quad = sprite_quad(&axes, frame.left, frame.right, frame.up, frame.down, scale);
            let first_vertex = self.scratch_verts.len() as u32;
            for i in [0usize, 1, 2, 0, 2, 3] {
                self.scratch_verts.push(SpriteVertex {
                    position: quad[i].0.to_array(),
                    uv: quad[i].1,
                    color,
                });
            }
            push_sprite_batch(
                &mut self.prepared_batches,
                sprite_idx,
                frame_idx as u32,
                first_vertex,
                6,
                sprite_blend_for(d.mode),
            );
            sprite_count += 1;
        }

        if self.scratch_verts.len() > self.vertex_cap {
            self.vertex_cap = self.scratch_verts.len().next_power_of_two();
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sprite_dynamic_vbuf"),
                size: (self.vertex_cap * std::mem::size_of::<SpriteVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.scratch_verts.is_empty() {
            queue.write_buffer(
                &self.vertex_buf,
                0,
                bytemuck::cast_slice(&self.scratch_verts),
            );
            self.frame_upload_bytes +=
                (self.scratch_verts.len() * std::mem::size_of::<SpriteVertex>()) as u64;
        }
        let (fog_rgb, fog_density) = params.fog.unwrap_or(([0.0, 0.0, 0.0], 0.0));
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&SpriteUniforms {
                view_proj: params.view_proj.to_cols_array_2d(),
                camera_pos: [view.origin.x, view.origin.y, view.origin.z, 1.0],
                fog_color_density: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_density],
                gamma_params: [
                    params.gamma,
                    params.texgamma,
                    params.brightness,
                    if params.software_mode { 1.0 } else { 0.0 },
                ],
            }),
        );
        self.frame_upload_bytes += std::mem::size_of::<SpriteUniforms>() as u64;
        PreparedSprites {
            batch_count: self.prepared_batches.len(),
            sprite_count,
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedSprites) {
        let _ = self.draw_counted(pass, prepared);
    }

    pub(crate) fn draw_counted<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedSprites,
    ) -> usize {
        if prepared.batch_count == 0 || self.prepared_batches.is_empty() {
            return 0;
        }
        let mut draw_calls = 0usize;
        let mut last_blend: Option<SpriteBlend> = None;
        let mut last_tex: Option<(u32, u32)> = None;
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        for b in &self.prepared_batches {
            let Some(Some(spr)) = self.sprites.get(b.sprite_idx as usize) else {
                continue;
            };
            let Some(frame) = spr.frames.get(b.frame as usize) else {
                continue;
            };
            if last_blend != Some(b.blend) {
                pass.set_pipeline(&self.pipelines[pipeline_slot(b.blend)]);
                last_blend = Some(b.blend);
            }
            let tex_key = (b.sprite_idx, b.frame);
            if last_tex != Some(tex_key) {
                pass.set_bind_group(1, &frame.bind_group, &[]);
                last_tex = Some(tex_key);
            }
            pass.draw(b.first_vertex..b.first_vertex + b.vertex_count, 0..1);
            draw_calls += 1;
        }
        draw_calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::scene::ModelHandle;
    use glam::Vec3;

    #[test]
    fn blend_per_render_mode() {
        assert_eq!(sprite_blend_for(EntRenderMode::Normal), SpriteBlend::Opaque);
        assert_eq!(
            sprite_blend_for(EntRenderMode::TransAlpha),
            SpriteBlend::AlphaNoDepthWrite
        );
        assert_eq!(
            sprite_blend_for(EntRenderMode::TransColor),
            SpriteBlend::Alpha
        );
        assert_eq!(
            sprite_blend_for(EntRenderMode::TransTexture),
            SpriteBlend::Alpha
        );
        assert_eq!(sprite_blend_for(EntRenderMode::TransAdd), SpriteBlend::Add);
        assert_eq!(sprite_blend_for(EntRenderMode::Glow), SpriteBlend::Glow);
    }

    #[test]
    fn frame_index_clamps() {
        assert_eq!(clamp_frame(0.0, 0), None);
        assert_eq!(clamp_frame(2.9, 5), Some(2));
        assert_eq!(clamp_frame(-3.0, 5), Some(0));
        assert_eq!(clamp_frame(99.0, 5), Some(4));
        assert_eq!(clamp_frame(f32::NAN, 5), Some(0));
    }

    #[test]
    fn frame_extents_from_disk_origin() {
        assert_eq!(frame_extents(-8, 4, 16, 8), (-8.0, 8.0, 4.0, -4.0));
    }

    #[test]
    fn coalesces_consecutive_smoke_puffs_into_one_batch_and_preserves_order() {
        let mut batches = Vec::new();
        for i in 0..50u32 {
            push_sprite_batch(&mut batches, 0, 0, i * 6, 6, SpriteBlend::AlphaNoDepthWrite);
        }
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].first_vertex, 0);
        assert_eq!(batches[0].vertex_count, 300);

        // Interleaving a different blend/sprite splits cleanly without reordering.
        batches.clear();
        push_sprite_batch(&mut batches, 0, 0, 0, 6, SpriteBlend::AlphaNoDepthWrite);
        push_sprite_batch(&mut batches, 0, 0, 6, 6, SpriteBlend::AlphaNoDepthWrite);
        push_sprite_batch(&mut batches, 1, 0, 12, 6, SpriteBlend::Add);
        push_sprite_batch(&mut batches, 0, 0, 18, 6, SpriteBlend::AlphaNoDepthWrite);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].vertex_count, 12);
        assert_eq!(batches[1].vertex_count, 6);
        assert_eq!(batches[2].vertex_count, 6);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn smoke_cloud_45_puffs_draws_in_one_draw_call_if_installed() {
        let Ok(gamedir) = std::env::var("CS16_GAMEDIR") else {
            return;
        };
        let Ok(vfs) = Vfs::open(std::path::Path::new(&gamedir), cs_vfs::VfsConfig::default())
        else {
            return;
        };
        if !vfs.exists("sprites/gas_puff_01.spr") {
            return;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            }))
        else {
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("sprite-batch-test"),
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            }))
        else {
            return;
        };

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = SpriteRenderer::new(&device, format);
        let draws: Vec<SpriteDraw> = (0..45)
            .map(|i| SpriteDraw {
                model: ModelHandle(1),
                origin: Vec3::new(150.0 + (i as f32) * 2.0, ((i % 7) as f32 - 3.0) * 12.0, 0.0),
                angles: Vec3::ZERO,
                frame: 0.0,
                scale: 2.9,
                mode: EntRenderMode::TransAlpha,
                blend: 1.0,
                renderfx: 0,
                color: [0.5, 0.5, 0.5],
                distance_sq: 10_000.0 - (i as f32),
                rank: 0,
            })
            .collect();
        let paths: Vec<Option<&str>> = vec![Some("sprites/gas_puff_01.spr"); 45];
        let view = ViewBasis::from_angles(Vec3::ZERO, Vec3::ZERO);
        let params = SpritePassParams {
            view_proj: Mat4::IDENTITY,
            gamma: 2.5,
            texgamma: 2.0,
            brightness: 0.0,
            software_mode: false,
            fog: None,
        };
        let prep = renderer.prepare(
            &device,
            &queue,
            &vfs,
            &draws,
            &paths,
            &view,
            &params,
            None,
            &[],
        );
        assert_eq!(prep.sprite_count(), 45);
        assert_eq!(
            prep.batch_count(),
            1,
            "45 gas_puff_01.spr puffs must coalesce into 1 batch"
        );

        let color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("spr_test_color"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("spr_test_depth"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let calls = {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("spr_test_pass"),
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
            renderer.draw_counted(&mut pass, &prep)
        };
        assert_eq!(calls, 1, "45 smoke puffs must render in 1 GPU draw call");
        queue.submit(Some(enc.finish()));
    }
}
