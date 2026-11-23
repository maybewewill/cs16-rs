//! GoldSrc sprite (`.spr`) renderer: camera-facing / upright / oriented quads with the engine's
//! render-mode blending, used for `env_sprite` entities and (later) client tempents.
//!
//! Behaviour: Xash3D FWGS `ref/gl/gl_sprite.c` `R_DrawSpriteModel` (GPL-3-or-later, read
//! 2026-11-23). Not done yet (assumptions to revisit): sprite frame interpolation between frames
//! (`R_GetSpriteFrameInterpolant`), the "sprite lightmap" pass for lit alpha-test sprites,
//! occlusion of glow sprites by world geometry, and angled frame groups.

// Consumed by the shell in Task 8b.
#![allow(dead_code)]

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use cs_assets::spr::{GoldSrcSprite, SpriteType};
use cs_vfs::Vfs;
use glam::Mat4;
use tracing::warn;
use wgpu::util::DeviceExt;

use crate::render::scene::{
    EntRenderMode, SpriteDraw, ViewBasis, glow_blend, sprite_axes, sprite_quad,
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
    params: [f32; 4],
}

/// How a sprite quad is blended (`R_DrawSpriteModel`'s render-mode switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteBlend {
    /// `kRenderNormal` and unknown modes: no blending, alpha test only.
    Opaque,
    /// `kRenderTransColor` / `kRenderTransTexture`: alpha blend, depth write kept.
    Alpha,
    /// `kRenderTransAlpha`: alpha blend, no depth write.
    AlphaNoDepthWrite,
    /// `kRenderTransAdd`: `GL_SRC_ALPHA, GL_ONE`, no depth write.
    Add,
    /// `kRenderGlow`: additive, no depth test and no depth write.
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

/// Frame index of an entity `frame` value: `(int)frame`, clamped into `0..count`
/// (`R_GetSpriteFrameInterpolant`, `gl_sprite.c`). `None` for a sprite without frames.
pub fn clamp_frame(frame: f32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let f = frame as i64; // truncates toward zero, NaN -> 0
    Some(f.clamp(0, count as i64 - 1) as usize)
}

/// Frame extents `(left, right, up, down)` relative to the sprite origin, as the engine's sprite
/// loader derives them from the on-disk frame origin and size (`left = origin_x`,
/// `up = origin_y`, `right = origin_x + width`, `down = origin_y - height`).
pub fn frame_extents(origin_x: i32, origin_y: i32, width: u32, height: u32) -> (f32, f32, f32, f32) {
    let left = origin_x as f32;
    let up = origin_y as f32;
    (left, left + width as f32, up, up - height as f32)
}

struct GpuFrame {
    left: f32,
    right: f32,
    up: f32,
    down: f32,
    bind_group: wgpu::BindGroup,
}

struct GpuSprite {
    sprite_type: SpriteType,
    frames: Vec<GpuFrame>,
}

struct PreparedSprite {
    key: String,
    frame: usize,
    first_vertex: u32,
    blend: SpriteBlend,
}

pub struct PreparedSprites {
    draws: Vec<PreparedSprite>,
}

pub struct SpriteRenderer {
    pipelines: [wgpu::RenderPipeline; 5],
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    vertex_cap: usize,
    cache: HashMap<String, Option<GpuSprite>>,
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });

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
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
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
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // Sprites are double sided (`SPR_CULL_NONE` is the common case).
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
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction::{Always, LessEqual};
        let pipelines = [
            make("sprite_opaque", None, true, LessEqual),
            make("sprite_alpha", alpha_blend, true, LessEqual),
            make("sprite_alpha_nodepth", alpha_blend, false, LessEqual),
            make("sprite_add", add_blend, false, LessEqual),
            make("sprite_glow", add_blend, false, Always),
        ];

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sprite_uniform_buf"),
            contents: bytemuck::bytes_of(&SpriteUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                params: [2.5, 1.0, 0.0, 0.0],
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let vertex_cap = 6 * 256;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite_dynamic_vbuf"),
            size: (vertex_cap * std::mem::size_of::<SpriteVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipelines,
            tex_bgl,
            sampler,
            uniform_buf,
            uniform_bg,
            vertex_buf,
            vertex_cap,
            cache: HashMap::new(),
        }
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn ensure_loaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        rel_path: &str,
    ) -> Option<String> {
        let key = rel_path.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let loaded = match GoldSrcSprite::load(vfs, &key) {
                Ok(spr) if !spr.frames.is_empty() => {
                    let frames = spr
                        .frames
                        .iter()
                        .map(|f| {
                            let size = wgpu::Extent3d {
                                width: f.width.max(1),
                                height: f.height.max(1),
                                depth_or_array_layers: 1,
                            };
                            let tex = device.create_texture(&wgpu::TextureDescriptor {
                                label: Some(&key),
                                size,
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: wgpu::TextureDimension::D2,
                                format: wgpu::TextureFormat::Rgba8Unorm,
                                usage: wgpu::TextureUsages::TEXTURE_BINDING
                                    | wgpu::TextureUsages::COPY_DST,
                                view_formats: &[],
                            });
                            queue.write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture: &tex,
                                    mip_level: 0,
                                    origin: wgpu::Origin3d::ZERO,
                                    aspect: wgpu::TextureAspect::All,
                                },
                                &f.rgba,
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(4 * size.width),
                                    rows_per_image: Some(size.height),
                                },
                                size,
                            );
                            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some(&key),
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
                    warn!(sprite = %key, "Sprite has no frames");
                    None
                }
                Err(err) => {
                    warn!(sprite = %key, %err, "Could not load sprite");
                    None
                }
            };
            self.cache.insert(key.clone(), loaded);
        }
        self.cache
            .get(&key)
            .and_then(|o| o.as_ref())
            .map(|_| key)
    }

    /// Build the quads of this frame's sprites. `paths` gives the `.spr` path of each draw's
    /// model (same length and order as `draws`; `None` skips the draw).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        draws: &[SpriteDraw],
        paths: &[Option<&str>],
        view: &ViewBasis,
        view_proj: Mat4,
        gamma: f32,
        brightness: f32,
    ) -> PreparedSprites {
        let mut verts: Vec<SpriteVertex> = Vec::new();
        let mut prepared = Vec::new();
        for (d, path) in draws.iter().zip(paths) {
            let Some(path) = path else { continue };
            let Some(key) = self.ensure_loaded(device, queue, vfs, path) else {
                continue;
            };
            let Some(Some(spr)) = self.cache.get(&key) else {
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
            let first_vertex = verts.len() as u32;
            for i in [0usize, 1, 2, 0, 2, 3] {
                verts.push(SpriteVertex {
                    position: quad[i].0.to_array(),
                    uv: quad[i].1,
                    color,
                });
            }
            prepared.push(PreparedSprite {
                key,
                frame: frame_idx,
                first_vertex,
                blend: sprite_blend_for(d.mode),
            });
        }

        if verts.len() > self.vertex_cap {
            self.vertex_cap = verts.len().next_power_of_two();
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sprite_dynamic_vbuf"),
                size: (self.vertex_cap * std::mem::size_of::<SpriteVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&verts));
        }
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&SpriteUniforms {
                view_proj: view_proj.to_cols_array_2d(),
                params: [gamma, brightness, 0.0, 0.0],
            }),
        );
        PreparedSprites { draws: prepared }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedSprites) {
        if prepared.draws.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        for d in &prepared.draws {
            let Some(Some(spr)) = self.cache.get(&d.key) else {
                continue;
            };
            let Some(frame) = spr.frames.get(d.frame) else {
                continue;
            };
            pass.set_pipeline(&self.pipelines[pipeline_slot(d.blend)]);
            pass.set_bind_group(1, &frame.bind_group, &[]);
            pass.draw(d.first_vertex..d.first_vertex + 6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_per_render_mode() {
        assert_eq!(sprite_blend_for(EntRenderMode::Normal), SpriteBlend::Opaque);
        assert_eq!(sprite_blend_for(EntRenderMode::TransAlpha), SpriteBlend::AlphaNoDepthWrite);
        assert_eq!(sprite_blend_for(EntRenderMode::TransColor), SpriteBlend::Alpha);
        assert_eq!(sprite_blend_for(EntRenderMode::TransTexture), SpriteBlend::Alpha);
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
        // Typical centred frame: origin (-8, 4), 16x8.
        assert_eq!(frame_extents(-8, 4, 16, 8), (-8.0, 8.0, 4.0, -4.0));
    }
}
