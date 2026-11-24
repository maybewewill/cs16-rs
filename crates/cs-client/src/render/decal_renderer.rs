//! GoldSrc decal projection renderer: loads `decals.wad` and custom player sprays
//! (`tempdecal.wad` / `pldecal.wad`), clips polygons cleanly onto BSP world/brush faces via
//! Sutherland-Hodgman clipping, and maintains a configurable `r_decals` ring buffer.

use std::collections::{HashMap, VecDeque};

use bytemuck::{Pod, Zeroable};
use cs_assets::bsp::{BspMap, DecalFacePoly};
use cs_assets::wad::WadArchive;
use cs_clientcore::hud::CustomDecalInfo;
use cs_vfs::Vfs;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::render::lighting::{
    TextureFilterMode, create_sampler, upload_rgba_texture_mipmapped,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DecalGpuVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DecalUniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    fog_color_density: [f32; 4],
    gamma_params: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct DecalSpawn {
    pub origin: Vec3,
    pub decal_name: String,
    pub half_size: f32,
    pub permanent: bool,
    pub player_spray: Option<u8>,
}

/// Clip a `(pos, uv)` polygon to the half-space `(p - plane_origin).dot(plane_normal) >= 0`.
pub fn clip_polygon_to_plane(
    poly: &[(Vec3, [f32; 2])],
    plane_origin: Vec3,
    plane_normal: Vec3,
) -> Vec<(Vec3, [f32; 2])> {
    if poly.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(poly.len() + 2);
    let mut prev = poly[poly.len() - 1];
    let mut prev_dist = (prev.0 - plane_origin).dot(plane_normal);

    for &curr in poly {
        let curr_dist = (curr.0 - plane_origin).dot(plane_normal);
        if curr_dist >= 0.0 {
            if prev_dist < 0.0 {
                let denom = (prev_dist - curr_dist).abs().max(1e-6);
                let t = (-prev_dist / denom).clamp(0.0, 1.0);
                let p = prev.0.lerp(curr.0, t);
                let uv = [
                    prev.1[0] + (curr.1[0] - prev.1[0]) * t,
                    prev.1[1] + (curr.1[1] - prev.1[1]) * t,
                ];
                out.push((p, uv));
            }
            out.push(curr);
        } else if prev_dist >= 0.0 {
            let denom = (prev_dist - curr_dist).abs().max(1e-6);
            let t = (prev_dist / denom).clamp(0.0, 1.0);
            let p = prev.0.lerp(curr.0, t);
            let uv = [
                prev.1[0] + (curr.1[0] - prev.1[0]) * t,
                prev.1[1] + (curr.1[1] - prev.1[1]) * t,
            ];
            out.push((p, uv));
        }
        prev = curr;
        prev_dist = curr_dist;
    }
    out
}

/// Project a square decal at `origin` with extent `[-half_size, +half_size]` onto candidate BSP
/// faces, returning triangulated vertices shifted slightly along each face normal to prevent z-fighting.
pub fn project_decal_onto_faces(
    faces: &[DecalFacePoly],
    origin: Vec3,
    half_size: f32,
    normal_offset: f32,
    tint: [f32; 4],
) -> Vec<DecalGpuVertex> {
    let mut tris = Vec::new();
    let hs = half_size.max(1.0);
    for face in faces {
        if face.verts.len() < 3 {
            continue;
        }
        // Reject faces far from the decal plane along the face normal.
        let plane_dist = (origin - face.verts[0]).dot(face.normal).abs();
        if plane_dist > hs * 1.25 {
            continue;
        }
        let ref_up = if face.normal.z.abs() < 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let right = face.normal.cross(ref_up).normalize_or_zero();
        let up = right.cross(face.normal).normalize_or_zero();
        if right.length_squared() < 1e-4 || up.length_squared() < 1e-4 {
            continue;
        }

        let inv_span = 1.0 / (2.0 * hs);
        let mut poly: Vec<(Vec3, [f32; 2])> = face
            .verts
            .iter()
            .map(|&p| {
                let d = p - origin;
                let u = 0.5 + d.dot(right) * inv_span;
                let v = 0.5 - d.dot(up) * inv_span;
                (p, [u, v])
            })
            .collect();

        poly = clip_polygon_to_plane(&poly, origin - right * hs, right);
        poly = clip_polygon_to_plane(&poly, origin + right * hs, -right);
        poly = clip_polygon_to_plane(&poly, origin - up * hs, up);
        poly = clip_polygon_to_plane(&poly, origin + up * hs, -up);
        if poly.len() < 3 {
            continue;
        }

        let offset_vec = face.normal * normal_offset;
        for i in 1..(poly.len() - 1) {
            for &(pos, uv) in &[poly[0], poly[i], poly[i + 1]] {
                tris.push(DecalGpuVertex {
                    position: (pos + offset_vec).to_array(),
                    uv,
                    color: tint,
                });
            }
        }
    }
    tris
}

struct CachedDecalTex {
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
    tint: [f32; 4],
}

struct PlacedDecal {
    tex_key: String,
    permanent: bool,
    verts: Vec<DecalGpuVertex>,
}

pub struct DecalRenderer {
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    filter_mode: TextureFilterMode,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    vertex_cap: usize,
    decals_wad: Option<Option<WadArchive>>,
    tempdecal_wad: Option<Option<WadArchive>>,
    pldecal_wad: Option<Option<WadArchive>>,
    textures: HashMap<String, CachedDecalTex>,
    placed: VecDeque<PlacedDecal>,
    draw_batches: Vec<(String, u32, u32)>,
    dirty_vbuf: bool,
}

fn create_decal_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("decal_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<DecalGpuVertex>() as wgpu::BufferAddress,
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: -2,
                slope_scale: -2.0,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count.max(1),
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn procedural_decal_rgba(name: &str) -> (u32, u32, Vec<u8>) {
    let lower = name.to_ascii_lowercase();
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let is_blood = lower.contains("blood");
    let is_scorch = lower.contains("scorch") || lower.contains("smscorch");
    let is_spray = lower.contains("spray") || lower.contains("lambda");
    for y in 0..size {
        for x in 0..size {
            let nx = (x as f32 + 0.5) / size as f32 * 2.0 - 1.0;
            let ny = (y as f32 + 0.5) / size as f32 * 2.0 - 1.0;
            let r = (nx * nx + ny * ny).sqrt();
            let falloff = (1.0 - r).clamp(0.0, 1.0);
            let a = (falloff * falloff * 235.0) as u8;
            if is_blood {
                rgba.extend_from_slice(&[125, 14, 14, a]);
            } else if is_scorch {
                rgba.extend_from_slice(&[24, 22, 20, a]);
            } else if is_spray {
                rgba.extend_from_slice(&[230, 140, 24, a]);
            } else {
                rgba.extend_from_slice(&[22, 22, 24, a]);
            }
        }
    }
    (size, size, rgba)
}

impl DecalRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("decal.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/decal.wgsl").into()),
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("decal_uniform_bgl"),
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
            label: Some("decal_tex_bgl"),
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
            label: Some("decal_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let msaa_samples = 1;
        let pipeline =
            create_decal_pipeline(device, &shader, &pipeline_layout, color_format, msaa_samples);

        let filter_mode = TextureFilterMode::LinearMipmapLinear;
        let sampler = create_sampler(device, "decal_sampler", filter_mode, 1, true);
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("decal_uniform_buf"),
            contents: bytemuck::bytes_of(&DecalUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0, 0.0, 0.0, 1.0],
                fog_color_density: [0.0, 0.0, 0.0, 0.0],
                gamma_params: [2.5, 2.0, 1.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decal_uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let vertex_cap = 6144;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("decal_vbuf"),
            size: (vertex_cap * std::mem::size_of::<DecalGpuVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            color_format,
            msaa_samples,
            shader,
            pipeline_layout,
            pipeline,
            tex_bgl,
            sampler,
            filter_mode,
            uniform_buf,
            uniform_bg,
            vertex_buf,
            vertex_cap,
            decals_wad: None,
            tempdecal_wad: None,
            pldecal_wad: None,
            textures: HashMap::new(),
            placed: VecDeque::new(),
            draw_batches: Vec::new(),
            dirty_vbuf: false,
        }
    }

    pub fn clear(&mut self) {
        self.placed.clear();
        self.draw_batches.clear();
        self.dirty_vbuf = true;
    }

    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device, msaa_samples: u32) {
        let samples = msaa_samples.max(1);
        if self.msaa_samples == samples {
            return;
        }
        self.msaa_samples = samples;
        self.pipeline = create_decal_pipeline(
            device,
            &self.shader,
            &self.pipeline_layout,
            self.color_format,
            samples,
        );
    }

    pub fn set_texture_filter(&mut self, device: &wgpu::Device, mode: TextureFilterMode) {
        let clamped_mode = match mode {
            TextureFilterMode::Nearest
            | TextureFilterMode::NearestMipmapNearest
            | TextureFilterMode::NearestMipmapLinear => TextureFilterMode::Nearest,
            _ => TextureFilterMode::Linear,
        };
        if self.filter_mode == clamped_mode {
            return;
        }
        self.filter_mode = clamped_mode;
        self.sampler = create_sampler(device, "decal_sampler", clamped_mode, 1, true);
        for (idx, tex) in self.textures.values_mut().enumerate() {
            tex.bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("decal_tex_bg_{idx}")),
                layout: &self.tex_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&tex.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        }
    }

    fn load_wad(slot: &mut Option<Option<WadArchive>>, vfs: &Vfs, name: &str) {
        if slot.is_none() {
            let parsed = vfs
                .read_bytes(name)
                .ok()
                .and_then(|bytes| WadArchive::parse(bytes).ok());
            *slot = Some(parsed);
        }
    }

    fn ensure_decal_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        name: &str,
        spray_player: Option<u8>,
        custom_sprays: &HashMap<u8, CustomDecalInfo>,
    ) -> (String, [f32; 4]) {
        let key = if let Some(p) = spray_player {
            format!("spray:{p}:{}", name.to_ascii_lowercase())
        } else {
            name.to_ascii_lowercase()
        };
        if let Some(existing) = self.textures.get(&key) {
            return (key, existing.tint);
        }

        Self::load_wad(&mut self.decals_wad, vfs, "decals.wad");
        Self::load_wad(&mut self.tempdecal_wad, vfs, "tempdecal.wad");
        Self::load_wad(&mut self.pldecal_wad, vfs, "pldecal.wad");

        let mut decoded = None;
        if spray_player.is_some() {
            for wad in [&self.tempdecal_wad, &self.pldecal_wad].into_iter().flatten().flatten() {
                if let Some(&first) = wad.entry_names().first()
                    && let Ok(Some(d)) = wad.decode_decal(first)
                {
                    decoded = Some(d);
                    break;
                }
            }
            if decoded.is_none()
                && let Some(info) = spray_player.and_then(|p| custom_sprays.get(&p))
                && let Ok(bytes) = vfs.read_bytes(&info.filename)
                && let Ok(wad) = WadArchive::parse(bytes)
                && let Some(&first) = wad.entry_names().first()
                && let Ok(Some(d)) = wad.decode_decal(first)
            {
                decoded = Some(d);
            }
        }
        if decoded.is_none()
            && let Some(Some(wad)) = &self.decals_wad
        {
            if let Ok(Some(d)) = wad.decode_decal(name) {
                decoded = Some(d);
            } else {
                let candidate = format!("{{{name}");
                if let Ok(Some(d)) = wad.decode_decal(&candidate) {
                    decoded = Some(d);
                }
            }
        }

        let (w, h, rgba) = match decoded {
            Some(d) => (d.width.max(1), d.height.max(1), d.rgba),
            None => procedural_decal_rgba(name),
        };
        let view = upload_rgba_texture_mipmapped(device, queue, &key, w, h, &rgba);
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
        let tint = [1.0, 1.0, 1.0, 1.0];
        self.textures.insert(key.clone(), CachedDecalTex { view, bg, tint });
        (key, tint)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_decals(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        map: Option<&BspMap>,
        spawns: &[DecalSpawn],
        r_decals: usize,
        custom_sprays: &HashMap<u8, CustomDecalInfo>,
    ) {
        let Some(map) = map else {
            return;
        };
        let max_transient = r_decals.min(4096);
        for s in spawns {
            let (tex_key, tint) = self.ensure_decal_texture(
                device,
                queue,
                vfs,
                &s.decal_name,
                s.player_spray,
                custom_sprays,
            );
            let half_size = s.half_size.max(2.0);
            let faces = map.query_faces_in_sphere(s.origin, half_size * 1.5);
            let verts = project_decal_onto_faces(&faces, s.origin, half_size, 0.35, tint);
            if verts.is_empty() {
                continue;
            }
            self.placed.push_back(PlacedDecal {
                tex_key,
                permanent: s.permanent,
                verts,
            });
            self.dirty_vbuf = true;
        }

        // Enforce r_decals limit on transient (non-permanent) decals.
        let mut transient_count = self.placed.iter().filter(|d| !d.permanent).count();
        while transient_count > max_transient {
            if let Some(pos) = self.placed.iter().position(|d| !d.permanent) {
                self.placed.remove(pos);
                transient_count -= 1;
                self.dirty_vbuf = true;
            } else {
                break;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        camera_pos: Vec3,
        gamma: f32,
        texgamma: f32,
        brightness: f32,
        software_mode: bool,
        fog: Option<([f32; 3], f32)>,
    ) {
        let (fog_rgb, fog_density) = fog.unwrap_or(([0.0, 0.0, 0.0], 0.0));
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&DecalUniforms {
                view_proj: view_proj.to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 1.0],
                fog_color_density: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_density],
                gamma_params: [
                    gamma,
                    texgamma,
                    brightness,
                    if software_mode { 1.0 } else { 0.0 },
                ],
            }),
        );

        if !self.dirty_vbuf {
            return;
        }
        self.dirty_vbuf = false;
        self.draw_batches.clear();
        let mut all_verts: Vec<DecalGpuVertex> = Vec::new();
        for d in &self.placed {
            let first = all_verts.len() as u32;
            all_verts.extend_from_slice(&d.verts);
            let count = d.verts.len() as u32;
            if let Some(last) = self.draw_batches.last_mut()
                && last.0 == d.tex_key
                && last.1 + last.2 == first
            {
                last.2 += count;
            } else {
                self.draw_batches.push((d.tex_key.clone(), first, count));
            }
        }
        if all_verts.len() > self.vertex_cap {
            self.vertex_cap = all_verts.len().next_power_of_two();
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("decal_vbuf"),
                size: (self.vertex_cap * std::mem::size_of::<DecalGpuVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !all_verts.is_empty() {
            queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&all_verts));
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.draw_batches.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        for (tex_key, first, count) in &self.draw_batches {
            if let Some(tex) = self.textures.get(tex_key) {
                pass.set_bind_group(1, &tex.bg, &[]);
                pass.draw(*first..(*first + *count), 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_clip_bounds_uvs_to_unit_square() {
        let face = DecalFacePoly {
            face_index: 0,
            model_index: 0,
            normal: Vec3::Z,
            plane_dist: 0.0,
            verts: vec![
                Vec3::new(-100.0, -100.0, 0.0),
                Vec3::new(100.0, -100.0, 0.0),
                Vec3::new(100.0, 100.0, 0.0),
                Vec3::new(-100.0, 100.0, 0.0),
            ],
        };
        let tris = project_decal_onto_faces(&[face], Vec3::ZERO, 16.0, 0.5, [1.0; 4]);
        assert_eq!(tris.len(), 6);
        for v in &tris {
            assert!((v.position[2] - 0.5).abs() < 1e-4);
            assert!(v.uv[0] >= -1e-4 && v.uv[0] <= 1.0 + 1e-4);
            assert!(v.uv[1] >= -1e-4 && v.uv[1] <= 1.0 + 1e-4);
        }
    }
}
