//! GoldSrc StudioModel v10 (`.mdl`) renderer for first-person weapon viewmodels,
//! third-person player characters, dropped weapons, grenades, and planted C4.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use cs_assets::mdl::{STUDIO_NF_MASKED, StudioModel};
use cs_vfs::Vfs;
use glam::{Mat4, Quat, Vec3};
use tracing::warn;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MdlGpuVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MdlUniforms {
    view_proj: [[f32; 4]; 4],
    light_color: [f32; 4],
    params: [f32; 4],
}

/// A studio entity of the scene API.
pub struct StudioInstance<'a> {
    pub model_path: &'a str,
    pub transform: Mat4,
    pub sequence: i32,
    /// `entity_state_t::frame`.
    pub frame: f32,
    pub animtime: f32,
    pub framerate: f32,
    /// Client time in seconds.
    pub now: f32,
    pub body: i32,
    pub light_rgb: Vec3,
    /// Mirror the model (the left-handed view model, `cl_righthand 0`).
    pub flip_winding: bool,
    /// A studio model merged onto this one's bones (`EF_BONEMERGE`).
    pub attached_model: Option<&'a str>,
}

/// `STUDIO_LOOPING` sequence flag.
pub const STUDIO_LOOPING: i32 = 1;

/// Animation frame of a studio entity: port of `R_StudioEstimateFrame` (Xash3D FWGS
/// `ref/gl/gl_studio.c:565-590`, read 2026-11-23, interpolation on).
///
/// `frame` is the 0..255 entity frame byte scaled to `numframes - 1` (0 when the sequence has at
/// most one frame); the time since `animtime` advances it at `framerate * fps`. Looping
/// sequences wrap modulo `numframes - 1`, others clamp to `numframes - 1.001`.
pub fn studio_frame(
    frame: f32,
    animtime: f32,
    framerate: f32,
    now: f32,
    fps: f32,
    numframes: usize,
    looping: bool,
) -> f32 {
    // C computes `dfdt` in double and the frame in float.
    let dfdt = if now < animtime {
        0.0
    } else {
        f64::from(now - animtime) * f64::from(framerate) * f64::from(fps)
    };
    let span = numframes.saturating_sub(1) as f64;
    let mut f = if numframes <= 1 {
        0.0
    } else {
        f64::from(frame * span as f32 / 256.0)
    };
    f += dfdt;
    if looping {
        if numframes > 1 {
            f -= ((f / span) as i64) as f64 * span;
        }
        if f < 0.0 {
            f += span;
        }
    } else {
        if f >= span - 1.001 {
            f = span - 1.001;
        }
        if f < 0.0 {
            f = 0.0;
        }
    }
    f as f32
}

/// Submodel of body part `index` selected by the entity's `body` value:
/// `(body / base) % submodel_count`, where `base` is the product of the submodel counts of the
/// earlier body parts (assumption: the studio compiler's layout, first part varies fastest).
pub fn bodypart_submodel(
    parts: &[cs_assets::mdl::MdlBodyPart],
    index: usize,
    body: i32,
) -> Option<&cs_assets::mdl::MdlSubModel> {
    let part = parts.get(index)?;
    let count = part.submodels.len() as i64;
    if count == 0 {
        return None;
    }
    let base: i64 = parts[..index]
        .iter()
        .try_fold(1i64, |acc, p| acc.checked_mul((p.submodels.len() as i64).max(1)))?;
    let pick = (i64::from(body.max(0)) / base) % count;
    part.submodels.get(pick as usize)
}

struct CachedGpuModel {
    mdl: StudioModel,
    tex_bgs: Vec<(wgpu::BindGroup, bool)>, // (bind_group, alpha_test)
}

pub struct MdlRenderer {
    pipeline: wgpu::RenderPipeline,
    pipeline_flipped: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    cache: HashMap<String, Option<CachedGpuModel>>,
    // Dynamic per-frame buffers (re-allocated if exceeded)
    vertex_buf: wgpu::Buffer,
    vertex_cap: usize,
    uniform_buf: wgpu::Buffer,
    uniform_cap: usize,
    uniform_stride: u64,
    uniform_bg: wgpu::BindGroup,
}

struct PreparedDraw {
    model_key: String,
    tex_idx: usize,
    first_vertex: u32,
    vertex_count: u32,
    uniform_slot: u32,
    flipped: bool,
}

pub struct PreparedMdlPass {
    draws: Vec<PreparedDraw>,
}

impl MdlRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mdl.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mdl.wgsl").into()),
        });

        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let raw_size = std::mem::size_of::<MdlUniforms>() as u64;
        let uniform_stride = raw_size.div_ceil(align) * align;

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mdl_uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(raw_size),
                },
                count: None,
            }],
        });

        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mdl_tex_bgl"),
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
            label: Some("mdl_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });

        let make_pipe = |label: &str, cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<MdlGpuVertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3,
                            1 => Float32x3,
                            2 => Float32x2,
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        // Disable backface culling for MDL so both left/right-handed viewmodels and thin sleeves render solid
        let pipeline = make_pipe("mdl_pipeline", None);
        let pipeline_flipped = make_pipe("mdl_pipeline_flipped", None);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mdl_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let vertex_cap = 16384;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_dynamic_vbuf"),
            size: (vertex_cap * std::mem::size_of::<MdlGpuVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_cap = 256;
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_dynamic_ubuf"),
            size: (uniform_cap as u64) * uniform_stride,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mdl_uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(raw_size),
                }),
            }],
        });

        Self {
            pipeline,
            pipeline_flipped,
            uniform_bgl,
            tex_bgl,
            sampler,
            cache: HashMap::new(),
            vertex_buf,
            vertex_cap,
            uniform_buf,
            uniform_cap,
            uniform_stride,
            uniform_bg,
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
            let loaded = match StudioModel::load(vfs, &key) {
                Ok(mdl) => {
                    let mut tex_bgs = Vec::with_capacity(mdl.textures.len());
                    for tex in &mdl.textures {
                        let size = wgpu::Extent3d {
                            width: tex.width.max(1),
                            height: tex.height.max(1),
                            depth_or_array_layers: 1,
                        };
                        let gpu_tex = device.create_texture(&wgpu::TextureDescriptor {
                            label: Some(&tex.name),
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        });
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &gpu_tex,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            &tex.rgba,
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(4 * tex.width.max(1)),
                                rows_per_image: Some(tex.height.max(1)),
                            },
                            size,
                        );
                        let view = gpu_tex.create_view(&wgpu::TextureViewDescriptor::default());
                        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some(&tex.name),
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
                        let alpha_test = (tex.flags & STUDIO_NF_MASKED) != 0;
                        tex_bgs.push((bg, alpha_test));
                    }
                    Some(CachedGpuModel { mdl, tex_bgs })
                }
                Err(err) => {
                    warn!(model = %key, %err, "Could not load StudioModel");
                    None
                }
            };
            self.cache.insert(key.clone(), loaded);
        }

        if self.cache.get(&key).and_then(|o| o.as_ref()).is_some() {
            Some(key)
        } else {
            None
        }
    }

    /// Skin and upload studio model entities of the scene API (sequence index, entity frame,
    /// body group), for one render pass.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_studio(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        instances: &[StudioInstance<'_>],
        view_proj: Mat4,
        gamma: f32,
        brightness: f32,
    ) -> PreparedMdlPass {
        let mut all_verts: Vec<MdlGpuVertex> = Vec::new();
        let mut draws: Vec<PreparedDraw> = Vec::new();
        let mut uniform_bytes: Vec<u8> = Vec::new();
        let mut uniform_slot: u32 = 0;

        for inst in instances {
            let Some(key) = self.ensure_loaded(device, queue, vfs, inst.model_path) else {
                continue;
            };
            let attached_key = inst
                .attached_model
                .and_then(|p| self.ensure_loaded(device, queue, vfs, p));
            let Some(Some(cached)) = self.cache.get(&key) else {
                continue;
            };

            let seq_idx = if (inst.sequence.max(0) as usize) < cached.mdl.sequences.len() {
                inst.sequence.max(0) as usize
            } else {
                0
            };
            let fps = cached
                .mdl
                .sequences
                .get(seq_idx)
                .map(|s| s.fps)
                .unwrap_or(30.0);
            let (numframes, looping) = cached
                .mdl
                .sequences
                .get(seq_idx)
                .map(|s| (s.num_frames, s.flags & STUDIO_LOOPING != 0))
                .unwrap_or((1, false));
            // The estimate already wrapped or clamped the frame.
            let frame = studio_frame(
                inst.frame,
                inst.animtime,
                inst.framerate,
                inst.now,
                fps,
                numframes,
                looping,
            );
            let loop_anim = false;
            let bones = cached.mdl.evaluate_bones_ex(seq_idx, frame, loop_anim);

            let mut push_model = |m_key: &str, cached_m: &CachedGpuModel, m_bones: &[Mat4]| {
                for bi in 0..cached_m.mdl.bodyparts.len() {
                    let Some(sub) = bodypart_submodel(&cached_m.mdl.bodyparts, bi, inst.body)
                    else {
                        continue;
                    };
                    for mesh in &sub.meshes {
                        if mesh.vertices.is_empty() {
                            continue;
                        }
                        let tex_idx = mesh.texture_index.min(cached_m.tex_bgs.len().saturating_sub(1));
                        let alpha_test = cached_m
                            .tex_bgs
                            .get(tex_idx)
                            .map(|(_, at)| *at)
                            .unwrap_or(false);

                        let first_vertex = all_verts.len() as u32;
                        for v in &mesh.vertices {
                            let bm = m_bones
                                .get(v.bone as usize)
                                .copied()
                                .unwrap_or(Mat4::IDENTITY);
                            let m = inst.transform * bm;
                            let pos = m.transform_point3(v.pos);
                            let norm = m.transform_vector3(v.normal).normalize_or_zero();
                            all_verts.push(MdlGpuVertex {
                                position: pos.to_array(),
                                normal: norm.to_array(),
                                uv: v.uv,
                            });
                        }
                        let vertex_count = (all_verts.len() as u32) - first_vertex;

                        let u = MdlUniforms {
                            view_proj: view_proj.to_cols_array_2d(),
                            light_color: [inst.light_rgb.x, inst.light_rgb.y, inst.light_rgb.z, gamma],
                            params: [brightness, if alpha_test { 1.0 } else { 0.0 }, 0.0, 0.0],
                        };
                        let start = uniform_bytes.len();
                        uniform_bytes.resize(start + self.uniform_stride as usize, 0);
                        let raw = bytemuck::bytes_of(&u);
                        uniform_bytes[start..start + raw.len()].copy_from_slice(raw);

                        draws.push(PreparedDraw {
                            model_key: m_key.to_string(),
                            tex_idx,
                            first_vertex,
                            vertex_count,
                            uniform_slot,
                            flipped: inst.flip_winding,
                        });
                        uniform_slot += 1;
                    }
                }
            };

            push_model(&key, cached, &bones);

            // True GoldSrc `EF_BONEMERGE` attachment (`p_ak47.mdl` merged onto `Bip01 R Hand`)!
            if let Some(ref att_key) = attached_key
                && let Some(Some(att_cached)) = self.cache.get(att_key)
            {
                let merged_bones = att_cached.mdl.evaluate_bones_merged(&cached.mdl, &bones);
                push_model(att_key, att_cached, &merged_bones);
            }
        }

        if all_verts.len() > self.vertex_cap {
            self.vertex_cap = all_verts.len().next_power_of_two();
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mdl_dynamic_vbuf"),
                size: (self.vertex_cap * std::mem::size_of::<MdlGpuVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !all_verts.is_empty() {
            queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&all_verts));
        }

        if (uniform_slot as usize) > self.uniform_cap {
            self.uniform_cap = (uniform_slot as usize).next_power_of_two();
            self.uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mdl_dynamic_ubuf"),
                size: (self.uniform_cap as u64) * self.uniform_stride,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let raw_size = std::mem::size_of::<MdlUniforms>() as u64;
            self.uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mdl_uniform_bg"),
                layout: &self.uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniform_buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(raw_size),
                    }),
                }],
            });
        }
        if !uniform_bytes.is_empty() {
            queue.write_buffer(&self.uniform_buf, 0, &uniform_bytes);
        }

        PreparedMdlPass { draws }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedMdlPass) {
        if prepared.draws.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        for d in &prepared.draws {
            let Some(Some(cached)) = self.cache.get(&d.model_key) else {
                continue;
            };
            let Some((tex_bg, _)) = cached.tex_bgs.get(d.tex_idx) else {
                continue;
            };
            pass.set_pipeline(if d.flipped {
                &self.pipeline_flipped
            } else {
                &self.pipeline
            });
            let offset = d.uniform_slot * (self.uniform_stride as u32);
            pass.set_bind_group(0, &self.uniform_bg, &[offset]);
            pass.set_bind_group(1, tex_bg, &[]);
            pass.draw(d.first_vertex..d.first_vertex + d.vertex_count, 0..1);
        }
    }
}

/// Build the authentic GoldSrc first-person viewmodel matrix anchored to the camera eye.
pub fn viewmodel_transform(
    eye: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    bob_offset: Vec3,
    right_handed: bool,
) -> Mat4 {
    let rot = Quat::from_rotation_z(yaw_deg.to_radians())
        * Quat::from_rotation_y(pitch_deg.to_radians());
    // GoldSrc viewmodels are authored facing +X in right-handed Z-up, with left-hand default mirrored by cl_righthand!
    let scale = if right_handed {
        Vec3::new(1.0, -1.0, 1.0)
    } else {
        Vec3::ONE
    };
    let pos = eye + rot * bob_offset;
    Mat4::from_scale_rotation_translation(scale, rot, pos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_assets::mdl::{MdlBodyPart, MdlSubModel};

    fn part(n: usize) -> MdlBodyPart {
        MdlBodyPart {
            name: String::new(),
            submodels: (0..n)
                .map(|i| MdlSubModel {
                    name: format!("s{i}"),
                    meshes: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn body_selects_submodels_per_part() {
        let parts = [part(3), part(2)];
        let name = |i, body| bodypart_submodel(&parts, i, body).map(|s| s.name.clone());
        assert_eq!(name(0, 0).as_deref(), Some("s0"));
        assert_eq!(name(0, 4).as_deref(), Some("s1"));
        assert_eq!(name(1, 0).as_deref(), Some("s0"));
        // base of part 1 is 3: body 3..5 -> second submodel.
        assert_eq!(name(1, 3).as_deref(), Some("s1"));
        assert_eq!(name(1, 5).as_deref(), Some("s1"));
        assert_eq!(name(0, -7).as_deref(), Some("s0"));
        assert!(bodypart_submodel(&parts, 2, 0).is_none());
        assert!(bodypart_submodel(&[part(0)], 0, 0).is_none());
    }

    #[test]
    fn studio_frame_follows_estimate_frame() {
        // frame byte 128 of a 41-frame sequence: 128 * 40 / 256 = 20.
        assert_eq!(studio_frame(128.0, 10.0, 1.0, 10.0, 30.0, 41, false), 20.0);
        // one second at fps 30 advances 30 frames: 20 + 30 = 50 wraps modulo 40 -> 10.
        assert_eq!(studio_frame(128.0, 10.0, 1.0, 11.0, 30.0, 41, true), 10.0);
        // non-looping clamps to numframes - 1.001.
        assert_eq!(
            studio_frame(128.0, 10.0, 1.0, 11.0, 30.0, 41, false),
            (40.0f64 - 1.001) as f32
        );
        // framerate 0 freezes the animation at the scaled frame byte.
        assert_eq!(studio_frame(64.0, 10.0, 0.0, 15.0, 30.0, 41, true), 10.0);
        // animtime in the future: no negative advance.
        assert_eq!(studio_frame(0.0, 12.0, 1.0, 11.0, 30.0, 41, true), 0.0);
        // single-frame sequences: the frame term is 0; the engine still adds the time term when
        // looping (bone evaluation ignores it for one-frame sequences) and clamps otherwise.
        assert_eq!(studio_frame(200.0, 0.0, 1.0, 5.0, 30.0, 1, true), 150.0);
        assert_eq!(studio_frame(200.0, 0.0, 1.0, 5.0, 30.0, 1, false), 0.0);
    }
}
