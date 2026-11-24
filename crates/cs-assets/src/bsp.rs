//! GoldSrc BSP v30 parser, WAD texture resolver, PVS decompressor, and lightmap atlas builder.

use std::collections::HashMap;

use cs_vfs::Vfs;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

use crate::wad::{DecodedTexture, WadArchive, decode_miptex, fallback_checkerboard, read_cstr};

pub const BSP_VERSION_GOLDSRC: i32 = 30;
pub const LUMP_COUNT: usize = 15;

pub const LUMP_ENTITIES: usize = 0;
pub const LUMP_PLANES: usize = 1;
pub const LUMP_TEXTURES: usize = 2;
pub const LUMP_VERTEXES: usize = 3;
pub const LUMP_VISIBILITY: usize = 4;
pub const LUMP_NODES: usize = 5;
pub const LUMP_TEXINFO: usize = 6;
pub const LUMP_FACES: usize = 7;
pub const LUMP_LIGHTING: usize = 8;
pub const LUMP_CLIPNODES: usize = 9;
pub const LUMP_LEAFS: usize = 10;
pub const LUMP_MARKSURFACES: usize = 11;
pub const LUMP_EDGES: usize = 12;
pub const LUMP_SURFEDGES: usize = 13;
pub const LUMP_MODELS: usize = 14;

pub const LIGHTMAP_ATLAS_SIZE: u32 = 2048;

#[derive(Debug, Error)]
pub enum BspError {
    #[error("unsupported BSP version {0} (expected GoldSrc version 30)")]
    UnsupportedVersion(i32),
    #[error("truncated BSP data: {0}")]
    Truncated(&'static str),
    #[error("invalid lump {lump} bounds (offset={offset}, len={len}, file_len={file_len})")]
    InvalidLump {
        lump: usize,
        offset: usize,
        len: usize,
        file_len: usize,
    },
    #[error("VFS error: {0}")]
    Vfs(#[from] cs_vfs::VfsError),
}

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub normal: Vec3,
    pub dist: f32,
    pub plane_type: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct Node {
    pub plane: u32,
    pub children: [i16; 2],
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_face: u16,
    pub num_faces: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct ClipNode {
    pub plane: i32,
    pub children: [i16; 2],
}

#[derive(Debug, Clone, Copy)]
pub struct Leaf {
    pub contents: i32,
    pub vis_offset: i32,
    pub mins: [i16; 3],
    pub maxs: [i16; 3],
    pub first_mark_surface: u16,
    pub num_mark_surfaces: u16,
    pub ambient_levels: [u8; 4],
}

#[derive(Debug, Clone, Copy)]
pub struct TexInfo {
    pub s_vec: Vec3,
    pub s_shift: f32,
    pub t_vec: Vec3,
    pub t_shift: f32,
    pub miptex: i32,
    pub flags: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct Face {
    pub plane: u16,
    pub plane_side: u16,
    pub first_edge: i32,
    pub num_edges: u16,
    pub texinfo: u16,
    pub styles: [u8; 4],
    pub lightmap_offset: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct BspModel {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub head_nodes: [i32; 4],
    pub vis_leafs: i32,
    pub first_face: i32,
    pub num_faces: i32,
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub properties: HashMap<String, String>,
}

impl Entity {
    pub fn classname(&self) -> &str {
        self.properties
            .get("classname")
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(String::as_str)
    }

    pub fn get_vec3(&self, key: &str) -> Option<Vec3> {
        let s = self.get(key)?;
        let mut parts = s.split_whitespace();
        let x: f32 = parts.next()?.parse().ok()?;
        let y: f32 = parts.next()?.parse().ok()?;
        let z: f32 = parts.next()?.parse().ok()?;
        Some(Vec3::new(x, y, z))
    }

    pub fn get_f32(&self, key: &str) -> Option<f32> {
        self.get(key)?.trim().parse().ok()
    }

    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.get(key)?.trim().parse().ok()
    }
}

#[derive(Debug, Clone)]
pub struct SpawnPoint {
    pub position: Vec3,
    pub yaw_deg: f32,
    pub team: SpawnTeam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnTeam {
    CounterTerrorist,
    Terrorist,
}

/// GoldSrc entity render mode (`rendermode` keyvalue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Normal,
    AlphaTest,
    Transparent,
    Additive,
}

/// Surface flags bitmask shared with `bsp.wgsl`.
pub const SURF_FLAG_ALPHATEST: u32 = 1 << 0;
pub const SURF_FLAG_WATER: u32 = 1 << 1;
pub const SURF_FLAG_NO_LIGHTMAP: u32 = 1 << 2;
pub const SURF_FLAG_SCROLL: u32 = 1 << 3;

#[derive(Debug, Clone, Copy)]
pub struct BspVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub lm_uv: [f32; 2],
    pub alpha: f32,
    pub flags: u32,
}

/// Contiguous indexed triangle batch sharing one texture, render mode, and optional dynamic brush model (`0` = static world).
#[derive(Debug, Clone)]
pub struct RenderBatch {
    pub texture_index: usize,
    pub render_mode: RenderMode,
    pub model_index: usize,
    pub first_index: u32,
    pub index_count: u32,
}

#[derive(Debug, Clone)]
pub struct LightmapAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub packed_faces: usize,
}

/// Packed atlas placement and lightstyle metadata for a single BSP face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceLightmapInfo {
    pub face_index: usize,
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub width: u32,
    pub height: u32,
    pub styles: [u8; 4],
    pub num_styles: u8,
    pub lightmap_offset: usize,
}

impl LightmapAtlas {
    /// Recomputes atlas pixels for every face whose active lightstyle multipliers changed between
    /// `prev_values` and `style_values`. Returns dirty `(x, y, width, height)` rectangles
    /// (including the 1-pixel clamp border) for partial GPU `write_texture` uploads.
    pub fn rebuild_for_styles(
        &mut self,
        lighting: &[u8],
        rects: &[FaceLightmapInfo],
        style_values: &[f32; 64],
        prev_values: &[f32; 64],
    ) -> Vec<(u32, u32, u32, u32)> {
        let mut dirty = Vec::new();
        for r in rects {
            let ns = usize::from(r.num_styles.min(4));
            let changed = r.styles[..ns].iter().any(|&s| {
                let idx = usize::from(s);
                idx < 64 && (style_values[idx] - prev_values[idx]).abs() > 1e-4
            });
            if !changed || r.width == 0 || r.height == 0 {
                continue;
            }
            let page_bytes = (r.width as usize) * (r.height as usize) * 3;
            let pw = r.width + 2;
            let ph = r.height + 2;
            let ox = r.atlas_x.saturating_sub(1);
            let oy = r.atlas_y.saturating_sub(1);
            for py in 0..ph {
                let sy = py.saturating_sub(1).min(r.height - 1);
                for px in 0..pw {
                    let sx = px.saturating_sub(1).min(r.width - 1);
                    let texel_off = ((sy * r.width + sx) * 3) as usize;
                    let mut sum = [0.0f32; 3];
                    for page in 0..ns {
                        let style_id = usize::from(r.styles[page]);
                        let mul = if style_id < 64 { style_values[style_id] } else { 1.0 };
                        let src = r.lightmap_offset + page * page_bytes + texel_off;
                        if src + 2 < lighting.len() {
                            sum[0] += f32::from(lighting[src]) * mul;
                            sum[1] += f32::from(lighting[src + 1]) * mul;
                            sum[2] += f32::from(lighting[src + 2]) * mul;
                        }
                    }
                    let dst = (((oy + py) * self.width + (ox + px)) * 4) as usize;
                    if dst + 3 < self.rgba.len() {
                        self.rgba[dst] = sum[0].round().clamp(0.0, 255.0) as u8;
                        self.rgba[dst + 1] = sum[1].round().clamp(0.0, 255.0) as u8;
                        self.rgba[dst + 2] = sum[2].round().clamp(0.0, 255.0) as u8;
                        self.rgba[dst + 3] = 255;
                    }
                }
            }
            dirty.push((ox, oy, pw, ph));
        }
        dirty
    }
}

/// Animated (`+0..+9`) and alternate/toggleable (`+A..+J`) BSP texture resolution table.
#[derive(Debug, Clone, Default)]
pub struct TextureAnimTable {
    entries: Vec<TextureAnimEntry>,
}

#[derive(Debug, Clone, Default)]
struct TextureAnimEntry {
    primary: Vec<usize>,
    alt: Vec<usize>,
}

impl TextureAnimTable {
    pub fn from_textures(textures: &[DecodedTexture]) -> Self {
        type AnimGroup = (Vec<(u8, usize)>, Vec<(u8, usize)>);
        let mut entries = vec![TextureAnimEntry::default(); textures.len()];
        let mut groups: HashMap<String, AnimGroup> = HashMap::new();
        for (idx, tex) in textures.iter().enumerate() {
            let lower = tex.name.to_ascii_lowercase();
            let bytes = lower.as_bytes();
            if bytes.len() >= 3 && bytes[0] == b'+' {
                let tag = bytes[1];
                let base = lower[2..].to_string();
                let g = groups.entry(base).or_default();
                if tag.is_ascii_digit() {
                    g.0.push((tag, idx));
                } else if (b'a'..=b'j').contains(&tag) {
                    g.1.push((tag, idx));
                }
            }
        }
        for (_, (mut primary_pairs, mut alt_pairs)) in groups {
            primary_pairs.sort_by_key(|&(tag, _)| tag);
            alt_pairs.sort_by_key(|&(tag, _)| tag);
            let primary: Vec<usize> = primary_pairs.into_iter().map(|(_, i)| i).collect();
            let alt: Vec<usize> = alt_pairs.into_iter().map(|(_, i)| i).collect();
            for &i in primary.iter().chain(alt.iter()) {
                entries[i] = TextureAnimEntry {
                    primary: primary.clone(),
                    alt: alt.clone(),
                };
            }
        }
        Self { entries }
    }

    /// Resolve active animated/toggleable texture index at `time_sec` (5 Hz) and `entity_frame`.
    pub fn resolve(&self, tex_idx: usize, time_sec: f32, entity_frame: f32) -> usize {
        let Some(entry) = self.entries.get(tex_idx) else {
            return tex_idx;
        };
        let chain = if entity_frame != 0.0 && !entry.alt.is_empty() {
            &entry.alt
        } else if !entry.primary.is_empty() {
            &entry.primary
        } else if !entry.alt.is_empty() {
            &entry.alt
        } else {
            return tex_idx;
        };
        let step = (time_sec.max(0.0) * 5.0).floor() as usize;
        chain[step % chain.len()]
    }
}

/// Per-face render span in the dedicated PVS-cullable world geometry buffer.
#[derive(Debug, Clone, Copy)]
pub struct WorldFaceSpan {
    pub face_index: usize,
    pub texture_index: usize,
    pub render_mode: RenderMode,
    pub first_index: u32,
    pub index_count: u32,
}

/// Triangulated / polygonal BSP face geometry returned by [`BspMap::query_faces_in_sphere`].
#[derive(Debug, Clone)]
pub struct DecalFacePoly {
    pub face_index: usize,
    pub model_index: usize,
    pub normal: Vec3,
    pub plane_dist: f32,
    pub verts: Vec<Vec3>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrushVolumeKind {
    Solid,
    Ladder,
    BuyZone { team: Option<SpawnTeam> },
    BombTarget,
    HostageRescue,
    Breakable { health: f32, material: i32 },
    Door {
        speed: f32,
        lip: f32,
        move_dir: Vec3,
        wait: f32,
        rotating: bool,
    },
}

/// Parsed brush entity (`*1..*N`) spatial metadata used by `cs-sim` for collision & game zones.
#[derive(Debug, Clone)]
pub struct BrushEntity {
    pub classname: String,
    pub model_index: usize,
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub kind: BrushVolumeKind,
}

/// Complete parsed BSP v30 map plus ready-to-upload render geometry, textures, and lightmap atlas.
#[derive(Debug, Clone)]
pub struct BspMap {
    pub name: String,
    pub skyname: String,
    pub entities: Vec<Entity>,
    pub spawns: Vec<SpawnPoint>,
    pub hostage_spawns: Vec<(Vec3, f32)>,
    pub brush_entities: Vec<BrushEntity>,
    pub planes: Vec<Plane>,
    pub vertices_raw: Vec<Vec3>,
    pub visibility: Vec<u8>,
    pub nodes: Vec<Node>,
    pub texinfos: Vec<TexInfo>,
    pub faces: Vec<Face>,
    pub lighting: Vec<u8>,
    pub clipnodes: Vec<ClipNode>,
    pub leafs: Vec<Leaf>,
    pub mark_surfaces: Vec<u16>,
    pub edges: Vec<[u16; 2]>,
    pub surf_edges: Vec<i32>,
    pub models: Vec<BspModel>,
    pub textures: Vec<DecodedTexture>,
    pub lightmap_atlas: LightmapAtlas,
    pub render_vertices: Vec<BspVertex>,
    pub render_indices: Vec<u32>,
    pub batches: Vec<RenderBatch>,
}

impl BspMap {
    /// Load and parse a map by name (e.g. `"de_dust2"`) from the VFS, resolving external WADs.
    pub fn load(vfs: &Vfs, map_name: &str) -> Result<Self, BspError> {
        let clean = map_name.strip_suffix(".bsp").unwrap_or(map_name);
        let rel_path = format!("maps/{clean}.bsp");
        let bytes = vfs.read_bytes(&rel_path)?;
        Self::parse(clean, &bytes, vfs)
    }

    pub fn parse(name: &str, data: &[u8], vfs: &Vfs) -> Result<Self, BspError> {
        if data.len() < 4 + LUMP_COUNT * 8 {
            return Err(BspError::Truncated("BSP header"));
        }
        let version = read_i32(data, 0)?;
        if version != BSP_VERSION_GOLDSRC {
            return Err(BspError::UnsupportedVersion(version));
        }

        let mut lumps = [(0usize, 0usize); LUMP_COUNT];
        for (i, slot) in lumps.iter_mut().enumerate() {
            let off = read_i32(data, 4 + i * 8)?;
            let len = read_i32(data, 4 + i * 8 + 4)?;
            if off < 0 || len < 0 || (off as usize).saturating_add(len as usize) > data.len() {
                return Err(BspError::InvalidLump {
                    lump: i,
                    offset: off.max(0) as usize,
                    len: len.max(0) as usize,
                    file_len: data.len(),
                });
            }
            *slot = (off as usize, len as usize);
        }

        let lump_slice = |idx: usize| -> &[u8] {
            let (off, len) = lumps[idx];
            &data[off..off + len]
        };

        let entities_text = String::from_utf8_lossy(lump_slice(LUMP_ENTITIES));
        let entities = parse_entities(&entities_text);

        let worldspawn = entities.iter().find(|e| e.classname() == "worldspawn");
        let skyname = worldspawn
            .and_then(|w| w.get("skyname"))
            .filter(|s| !s.is_empty())
            .unwrap_or("desert")
            .to_string();
        let wad_list = worldspawn
            .and_then(|w| w.get("wad"))
            .map(parse_wad_list)
            .unwrap_or_default();

        let planes = parse_planes(lump_slice(LUMP_PLANES))?;
        let vertices_raw = parse_vertices(lump_slice(LUMP_VERTEXES))?;
        let visibility = lump_slice(LUMP_VISIBILITY).to_vec();
        let nodes = parse_nodes(lump_slice(LUMP_NODES))?;
        let texinfos = parse_texinfos(lump_slice(LUMP_TEXINFO))?;
        let faces = parse_faces(lump_slice(LUMP_FACES))?;
        let lighting = lump_slice(LUMP_LIGHTING).to_vec();
        let clipnodes = parse_clipnodes(lump_slice(LUMP_CLIPNODES))?;
        let leafs = parse_leafs(lump_slice(LUMP_LEAFS))?;
        let mark_surfaces = parse_u16_vec(lump_slice(LUMP_MARKSURFACES))?;
        let edges = parse_edges(lump_slice(LUMP_EDGES))?;
        let surf_edges = parse_i32_vec(lump_slice(LUMP_SURFEDGES))?;
        let models = parse_models(lump_slice(LUMP_MODELS))?;

        let textures = parse_and_resolve_textures(lump_slice(LUMP_TEXTURES), &wad_list, vfs);
        let spawns = extract_spawns(&entities);
        let hostage_spawns = extract_hostage_spawns(&entities);
        let brush_entities = extract_brush_entities(&entities, &models);

        let (lightmap_atlas, render_vertices, render_indices, batches) = build_render_geometry(
            &entities,
            &planes,
            &vertices_raw,
            &texinfos,
            &faces,
            &lighting,
            &edges,
            &surf_edges,
            &models,
            &textures,
        );

        debug!(
            map = name,
            sky = %skyname,
            faces = faces.len(),
            textures = textures.len(),
            batches = batches.len(),
            render_tris = render_indices.len() / 3,
            spawns = spawns.len(),
            lm_faces = lightmap_atlas.packed_faces,
            "Parsed GoldSrc BSP v30"
        );

        Ok(Self {
            name: name.to_string(),
            skyname,
            entities,
            spawns,
            hostage_spawns,
            brush_entities,
            planes,
            vertices_raw,
            visibility,
            nodes,
            texinfos,
            faces,
            lighting,
            clipnodes,
            leafs,
            mark_surfaces,
            edges,
            surf_edges,
            models,
            textures,
            lightmap_atlas,
            render_vertices,
            render_indices,
            batches,
        })
    }

    /// Find the BSP leaf index containing `point` in world model 0.
    pub fn find_leaf(&self, point: Vec3) -> usize {
        if self.nodes.is_empty() || self.leafs.is_empty() {
            return 0;
        }
        let mut node_idx: i32 = self
            .models
            .first()
            .map(|m| m.head_nodes[0])
            .unwrap_or(0);
        // A valid tree is walked in fewer steps than it has nodes; a hostile map whose nodes
        // lead back to themselves ends in the solid leaf.
        let mut steps_left = self.nodes.len() + self.leafs.len();
        while node_idx >= 0 {
            if steps_left == 0 {
                return 0;
            }
            steps_left -= 1;
            let Some(node) = self.nodes.get(node_idx as usize) else {
                return 0;
            };
            let Some(plane) = self.planes.get(node.plane as usize) else {
                return 0;
            };
            let d = plane.normal.dot(point) - plane.dist;
            node_idx = if d >= 0.0 {
                node.children[0] as i32
            } else {
                node.children[1] as i32
            };
        }
        let leaf_idx = (-node_idx - 1) as usize;
        leaf_idx.min(self.leafs.len().saturating_sub(1))
    }

    /// Decompress RLE-encoded Potentially Visible Set (PVS) bitmask for a leaf.
    pub fn decompress_pvs(&self, leaf_idx: usize) -> Vec<bool> {
        // The count comes from the file: it never sizes the row beyond the leaves there are.
        let num_vis_leafs = self
            .models
            .first()
            .map(|m| m.vis_leafs.max(0) as usize)
            .unwrap_or(self.leafs.len().saturating_sub(1))
            .min(self.leafs.len());
        let mut visible = vec![false; num_vis_leafs];
        let Some(leaf) = self.leafs.get(leaf_idx) else {
            visible.fill(true);
            return visible;
        };
        if leaf.vis_offset < 0 || self.visibility.is_empty() {
            visible.fill(true);
            return visible;
        }

        let mut in_idx = leaf.vis_offset as usize;
        let mut out_leaf = 0usize;
        while out_leaf < num_vis_leafs && in_idx < self.visibility.len() {
            let byte = self.visibility[in_idx];
            in_idx += 1;
            if byte == 0 {
                let Some(&run) = self.visibility.get(in_idx) else {
                    break;
                };
                in_idx += 1;
                out_leaf += (run as usize) * 8;
            } else {
                for bit in 0..8 {
                    if out_leaf + bit < num_vis_leafs && (byte & (1 << bit)) != 0 {
                        visible[out_leaf + bit] = true;
                    }
                }
                out_leaf += 8;
            }
        }
        visible
    }

    /// Approximate GoldSrc `R_LightPoint(point)` by sampling the lightmap of the floor face
    /// below `point`, returning RGB in `[0.0, 2.0]` (matching GoldSrc overbright scale).
    pub fn sample_light_point(&self, point: Vec3) -> Vec3 {
        let leaf_idx = self.find_leaf(point - Vec3::new(0.0, 0.0, 32.0));
        let Some(leaf) = self.leafs.get(leaf_idx) else {
            return Vec3::splat(0.85);
        };
        let first = leaf.first_mark_surface as usize;
        let count = leaf.num_mark_surfaces as usize;
        let end = first.saturating_add(count).min(self.mark_surfaces.len());

        let mut best_rgb: Option<Vec3> = None;
        let mut best_up = -2.0f32;

        for &surf_idx in &self.mark_surfaces[first..end] {
            let Some(face) = self.faces.get(surf_idx as usize) else {
                continue;
            };
            if face.lightmap_offset < 0 || face.styles[0] == 255 {
                continue;
            }
            let Some(plane) = self.planes.get(face.plane as usize) else {
                continue;
            };
            let normal = if face.plane_side != 0 {
                -plane.normal
            } else {
                plane.normal
            };
            if normal.z <= best_up {
                continue;
            }
            let off = face.lightmap_offset as usize;
            if off + 2 < self.lighting.len() {
                let r = (self.lighting[off] as f32 / 255.0) * 2.0;
                let g = (self.lighting[off + 1] as f32 / 255.0) * 2.0;
                let b = (self.lighting[off + 2] as f32 / 255.0) * 2.0;
                best_rgb = Some(Vec3::new(r, g, b).clamp(Vec3::splat(0.18), Vec3::splat(1.6)));
                best_up = normal.z;
            }
        }

        best_rgb.unwrap_or(Vec3::splat(0.85))
    }

    /// Return the texture name of the floor surface in the leaf below `point` (for footstep materials).
    pub fn floor_texture_name(&self, point: Vec3) -> Option<&str> {
        let leaf_idx = self.find_leaf(point);
        let leaf = self.leafs.get(leaf_idx)?;
        let first = leaf.first_mark_surface as usize;
        let count = leaf.num_mark_surfaces as usize;
        let end = first.saturating_add(count).min(self.mark_surfaces.len());

        let mut best_tex: Option<&str> = None;
        let mut best_up = 0.5f32;
        for &surf_idx in &self.mark_surfaces[first..end] {
            let Some(face) = self.faces.get(surf_idx as usize) else {
                continue;
            };
            let Some(plane) = self.planes.get(face.plane as usize) else {
                continue;
            };
            let normal = if face.plane_side != 0 {
                -plane.normal
            } else {
                plane.normal
            };
            if normal.z > best_up
                && let Some(ti) = self.texinfos.get(face.texinfo as usize)
                && ti.miptex >= 0
                && let Some(tex) = self.textures.get(ti.miptex as usize)
            {
                best_up = normal.z;
                best_tex = Some(tex.name.as_str());
            }
        }
        best_tex
    }

    /// Reconstruct exact per-face lightmap atlas rectangles matching [`LightmapAtlas`] packing.
    pub fn build_face_lightmap_rects(&self) -> Vec<FaceLightmapInfo> {
        let draw_models = collect_brush_draw_models(&self.entities, &self.models);
        let mut packer = ShelfPacker::new(LIGHTMAP_ATLAS_SIZE, LIGHTMAP_ATLAS_SIZE);
        let _ = packer.alloc_rect(2, 2);
        let mut out = Vec::new();
        for brush in &draw_models {
            let model = &self.models[brush.model_index];
            let first = model.first_face.max(0) as usize;
            let count = model.num_faces.max(0) as usize;
            let end = first.saturating_add(count).min(self.faces.len());
            for face_index in first..end {
                let face = &self.faces[face_index];
                let Some(texinfo) = self.texinfos.get(face.texinfo as usize) else {
                    continue;
                };
                if texinfo.miptex < 0 || (texinfo.miptex as usize) >= self.textures.len() {
                    continue;
                }
                let tex = &self.textures[texinfo.miptex as usize];
                if is_sky_texture(&tex.name) || is_tool_texture(&tex.name) {
                    continue;
                }
                let Some((_poly, _st, _ems, _emt, lm_w, lm_h)) = face_extents_and_st(
                    face,
                    texinfo,
                    &self.vertices_raw,
                    &self.edges,
                    &self.surf_edges,
                ) else {
                    continue;
                };
                if self.planes.get(face.plane as usize).is_none() {
                    continue;
                }
                let is_water = is_water_texture(&tex.name);
                let has_lightmap = !is_water
                    && face.lightmap_offset >= 0
                    && face.styles[0] != 255
                    && (texinfo.flags & 1) == 0;
                if !has_lightmap {
                    continue;
                }
                let lm_off = face.lightmap_offset as usize;
                let page_bytes = (lm_w as usize) * (lm_h as usize) * 3;
                if lm_off.saturating_add(page_bytes) > self.lighting.len() {
                    continue;
                }
                let Some((ox, oy)) = packer.alloc_rect(lm_w, lm_h) else {
                    continue;
                };
                let mut num_styles = 0u8;
                for (page, &s) in face.styles.iter().enumerate() {
                    if s == 255 || lm_off + (page + 1) * page_bytes > self.lighting.len() {
                        break;
                    }
                    num_styles += 1;
                }
                out.push(FaceLightmapInfo {
                    face_index,
                    atlas_x: ox + 1,
                    atlas_y: oy + 1,
                    width: lm_w,
                    height: lm_h,
                    styles: face.styles,
                    num_styles: num_styles.max(1),
                    lightmap_offset: lm_off,
                });
            }
        }
        out
    }

    pub fn build_texture_anim_table(&self) -> TextureAnimTable {
        TextureAnimTable::from_textures(&self.textures)
    }

    /// Return a mask indexed by `face_index` (`0..self.faces.len()`) of world faces in any leaf
    /// potentially visible from `camera_pos`. Returns `None` if `visibility` is empty or the
    /// camera is outside valid PVS leafs.
    pub fn visible_world_face_mask(&self, camera_pos: Vec3) -> Option<Vec<bool>> {
        if self.visibility.is_empty() || self.leafs.is_empty() || self.faces.is_empty() {
            return None;
        }
        let cam_leaf = self.find_leaf(camera_pos);
        let pvs = self.decompress_pvs(cam_leaf);
        let world = self.models.first()?;
        let num_vis_leafs = world
            .vis_leafs
            .max(0)
            .min(self.leafs.len().saturating_sub(1) as i32) as usize;
        if cam_leaf == 0 {
            return None;
        }
        let mut mask = vec![false; self.faces.len()];
        // PVS bit `v` corresponds to BSP leaf `v + 1` (leaf 0 is solid).
        for (v, &vis) in pvs.iter().take(num_vis_leafs).enumerate() {
            if !vis {
                continue;
            }
            let Some(leaf) = self.leafs.get(v + 1) else {
                continue;
            };
            let first = leaf.first_mark_surface as usize;
            let count = leaf.num_mark_surfaces as usize;
            let end = first.saturating_add(count).min(self.mark_surfaces.len());
            for &face_idx in &self.mark_surfaces[first..end] {
                if let Some(slot) = mask.get_mut(face_idx as usize) {
                    *slot = true;
                }
            }
        }
        Some(mask)
    }

    /// Build per-face triangulated geometry for world model `0` so the renderer can cull world
    /// faces dynamically against PVS visibility while using the same [`LightmapAtlas`] UVs.
    pub fn build_world_face_meshes(&self) -> (Vec<BspVertex>, Vec<u32>, Vec<Option<WorldFaceSpan>>) {
        let mut spans = vec![None; self.faces.len()];
        let Some(world) = self.models.first() else {
            return (Vec::new(), Vec::new(), spans);
        };
        let rects = self.build_face_lightmap_rects();
        let mut rect_map: HashMap<usize, FaceLightmapInfo> =
            HashMap::with_capacity(rects.len());
        for r in rects {
            rect_map.insert(r.face_index, r);
        }
        let atlas_f = LIGHTMAP_ATLAS_SIZE as f32;
        let neutral_uv = [2.0 / atlas_f, 2.0 / atlas_f];
        let mut verts = Vec::new();
        let mut indices = Vec::new();

        let first = world.first_face.max(0) as usize;
        let count = world.num_faces.max(0) as usize;
        let end = first.saturating_add(count).min(self.faces.len());
        for face_index in first..end {
            let face = &self.faces[face_index];
            let Some(texinfo) = self.texinfos.get(face.texinfo as usize) else {
                continue;
            };
            if texinfo.miptex < 0 || (texinfo.miptex as usize) >= self.textures.len() {
                continue;
            }
            let tex_idx = texinfo.miptex as usize;
            let tex = &self.textures[tex_idx];
            if is_sky_texture(&tex.name) || is_tool_texture(&tex.name) {
                continue;
            }
            let Some((poly_verts, st_coords, ext_min_s, ext_min_t, _lm_w, _lm_h)) =
                face_extents_and_st(face, texinfo, &self.vertices_raw, &self.edges, &self.surf_edges)
            else {
                continue;
            };
            let Some(plane) = self.planes.get(face.plane as usize) else {
                continue;
            };
            let normal = if face.plane_side != 0 {
                -plane.normal
            } else {
                plane.normal
            };
            let is_water = is_water_texture(&tex.name);
            let is_scroll = tex.name.to_ascii_lowercase().starts_with("scroll");
            let rect = rect_map.get(&face_index).copied();

            let mut surf_flags = 0u32;
            if tex.alpha_test {
                surf_flags |= SURF_FLAG_ALPHATEST;
            }
            if is_water {
                surf_flags |= SURF_FLAG_WATER;
            }
            if is_scroll {
                surf_flags |= SURF_FLAG_SCROLL;
            }
            if rect.is_none() {
                surf_flags |= SURF_FLAG_NO_LIGHTMAP;
            }
            let render_mode = if tex.alpha_test {
                RenderMode::AlphaTest
            } else if is_water {
                RenderMode::Transparent
            } else {
                RenderMode::Normal
            };
            let alpha = if is_water { 0.78 } else { 1.0 };
            let tex_w = tex.width.max(1) as f32;
            let tex_h = tex.height.max(1) as f32;
            let base_v = verts.len() as u32;
            let first_index = indices.len() as u32;

            for (&p, &(s, t)) in poly_verts.iter().zip(st_coords.iter()) {
                let world_pos = p + world.origin;
                let lm_uv = if let Some(r) = rect {
                    [
                        (r.atlas_x as f32 + (s / 16.0 - ext_min_s) + 0.5) / atlas_f,
                        (r.atlas_y as f32 + (t / 16.0 - ext_min_t) + 0.5) / atlas_f,
                    ]
                } else {
                    neutral_uv
                };
                verts.push(BspVertex {
                    position: world_pos.to_array(),
                    normal: normal.to_array(),
                    uv: [s / tex_w, t / tex_h],
                    lm_uv,
                    alpha,
                    flags: surf_flags,
                });
            }

            let mut area_vec = Vec3::ZERO;
            for i in 1..(poly_verts.len() - 1) {
                area_vec += (poly_verts[i] - poly_verts[0]).cross(poly_verts[i + 1] - poly_verts[0]);
            }
            let flip = area_vec.dot(normal) < 0.0;
            for i in 1..(poly_verts.len() as u32 - 1) {
                if flip {
                    indices.extend_from_slice(&[base_v, base_v + i + 1, base_v + i]);
                } else {
                    indices.extend_from_slice(&[base_v, base_v + i, base_v + i + 1]);
                }
            }
            spans[face_index] = Some(WorldFaceSpan {
                face_index,
                texture_index: tex_idx,
                render_mode,
                first_index,
                index_count: (indices.len() as u32) - first_index,
            });
        }
        (verts, indices, spans)
    }

    /// GoldSrc `R_LightPoint(point)` with vertical node raycast and 4-style lightstyle summation.
    pub fn sample_light_point_styled(&self, point: Vec3, style_values: &[f32; 64]) -> Vec3 {
        let Some(world) = self.models.first() else {
            return Vec3::splat(0.85);
        };
        if let Some(rgb) = self.recursive_light_point(
            world.head_nodes[0],
            point,
            point - Vec3::new(0.0, 0.0, 2048.0),
            style_values,
            0,
        ) {
            return rgb;
        }
        self.sample_light_point(point)
    }

    fn recursive_light_point(
        &self,
        node_idx: i32,
        start: Vec3,
        end: Vec3,
        style_values: &[f32; 64],
        depth: usize,
    ) -> Option<Vec3> {
        if node_idx < 0 || depth > 256 {
            return None;
        }
        let node = self.nodes.get(node_idx as usize)?;
        let plane = self.planes.get(node.plane as usize)?;
        let front = plane.normal.dot(start) - plane.dist;
        let back = plane.normal.dot(end) - plane.dist;
        let side = usize::from(front < 0.0);
        if (back < 0.0) == (front < 0.0) {
            return self.recursive_light_point(
                i32::from(node.children[side]),
                start,
                end,
                style_values,
                depth + 1,
            );
        }
        let frac = (front / (front - back)).clamp(0.0, 1.0);
        let mid = start + (end - start) * frac;
        if let Some(hit) = self.recursive_light_point(
            i32::from(node.children[side]),
            start,
            mid,
            style_values,
            depth + 1,
        ) {
            return Some(hit);
        }

        // Check faces belonging to this split node at intersection `mid`.
        let first = usize::from(node.first_face);
        let count = usize::from(node.num_faces);
        let end_face = first.saturating_add(count).min(self.faces.len());
        for face in &self.faces[first..end_face] {
            if face.lightmap_offset < 0 || face.styles[0] == 255 {
                continue;
            }
            let Some(texinfo) = self.texinfos.get(face.texinfo as usize) else {
                continue;
            };
            if texinfo.flags & 1 != 0 {
                continue;
            }
            let Some((poly, _st, ext_min_s, ext_min_t, lm_w, lm_h)) = face_extents_and_st(
                face,
                texinfo,
                &self.vertices_raw,
                &self.edges,
                &self.surf_edges,
            ) else {
                continue;
            };
            let s = mid.dot(texinfo.s_vec) + texinfo.s_shift;
            let t = mid.dot(texinfo.t_vec) + texinfo.t_shift;
            let ds = s / 16.0 - ext_min_s;
            let dt = t / 16.0 - ext_min_t;
            if ds < -0.5 || dt < -0.5 || ds > lm_w as f32 + 0.5 || dt > lm_h as f32 + 0.5 {
                continue;
            }
            let Some(fplane) = self.planes.get(face.plane as usize) else {
                continue;
            };
            let fnormal = if face.plane_side != 0 {
                -fplane.normal
            } else {
                fplane.normal
            };
            if !point_in_bsp_face(mid, &poly, fnormal) {
                continue;
            }
            let tx = (ds.round() as i32).clamp(0, lm_w as i32 - 1) as usize;
            let ty = (dt.round() as i32).clamp(0, lm_h as i32 - 1) as usize;
            let lm_off = face.lightmap_offset as usize;
            let page_bytes = (lm_w as usize) * (lm_h as usize) * 3;
            let texel = (ty * lm_w as usize + tx) * 3;
            let mut rgb = Vec3::ZERO;
            for (page, &style) in face.styles.iter().enumerate() {
                if style == 255 {
                    break;
                }
                let src = lm_off + page * page_bytes + texel;
                if src + 2 >= self.lighting.len() {
                    break;
                }
                let mul = style_values.get(usize::from(style)).copied().unwrap_or(1.0);
                rgb += Vec3::new(
                    f32::from(self.lighting[src]) / 255.0 * 2.0,
                    f32::from(self.lighting[src + 1]) / 255.0 * 2.0,
                    f32::from(self.lighting[src + 2]) / 255.0 * 2.0,
                ) * mul;
            }
            return Some(rgb.clamp(Vec3::splat(0.12), Vec3::splat(1.8)));
        }

        self.recursive_light_point(
            i32::from(node.children[side ^ 1]),
            mid,
            end,
            style_values,
            depth + 1,
        )
    }

    /// Return BSP face polygons intersecting a sphere around `center` (for decal projection/clipping).
    pub fn query_faces_in_sphere(&self, center: Vec3, radius: f32) -> Vec<DecalFacePoly> {
        let mut out = Vec::new();
        let draw_models = collect_brush_draw_models(&self.entities, &self.models);
        for brush in &draw_models {
            let model = &self.models[brush.model_index];
            let offset = brush.origin + model.origin;
            let min = model.mins + offset - Vec3::splat(radius);
            let max = model.maxs + offset + Vec3::splat(radius);
            if center.x < min.x
                || center.x > max.x
                || center.y < min.y
                || center.y > max.y
                || center.z < min.z
                || center.z > max.z
            {
                continue;
            }
            let first = model.first_face.max(0) as usize;
            let count = model.num_faces.max(0) as usize;
            let end = first.saturating_add(count).min(self.faces.len());
            for face_index in first..end {
                let face = &self.faces[face_index];
                let Some(texinfo) = self.texinfos.get(face.texinfo as usize) else {
                    continue;
                };
                if let Some(tex) = usize::try_from(texinfo.miptex)
                    .ok()
                    .and_then(|i| self.textures.get(i))
                    && (is_sky_texture(&tex.name)
                        || is_tool_texture(&tex.name)
                        || is_water_texture(&tex.name))
                {
                    continue;
                }
                let Some(plane) = self.planes.get(face.plane as usize) else {
                    continue;
                };
                let normal = if face.plane_side != 0 {
                    -plane.normal
                } else {
                    plane.normal
                };
                let dist_signed = if face.plane_side != 0 {
                    -plane.dist
                } else {
                    plane.dist
                } + normal.dot(offset);
                if (normal.dot(center) - dist_signed).abs() > radius {
                    continue;
                }
                let Some((mut poly, _, _, _, _, _)) = face_extents_and_st(
                    face,
                    texinfo,
                    &self.vertices_raw,
                    &self.edges,
                    &self.surf_edges,
                ) else {
                    continue;
                };
                let mut fmin = Vec3::splat(f32::INFINITY);
                let mut fmax = Vec3::splat(f32::NEG_INFINITY);
                for v in &mut poly {
                    *v += offset;
                    fmin = fmin.min(*v);
                    fmax = fmax.max(*v);
                }
                if center.x < fmin.x - radius
                    || center.x > fmax.x + radius
                    || center.y < fmin.y - radius
                    || center.y > fmax.y + radius
                    || center.z < fmin.z - radius
                    || center.z > fmax.z + radius
                {
                    continue;
                }
                out.push(DecalFacePoly {
                    face_index,
                    model_index: brush.model_index,
                    normal,
                    plane_dist: dist_signed,
                    verts: poly,
                });
            }
        }
        out
    }
}

fn point_in_bsp_face(p: Vec3, poly: &[Vec3], normal: Vec3) -> bool {
    let mut area_vec = Vec3::ZERO;
    for i in 1..poly.len().saturating_sub(1) {
        area_vec += (poly[i] - poly[0]).cross(poly[i + 1] - poly[0]);
    }
    let sign = if area_vec.dot(normal) < 0.0 { -1.0 } else { 1.0 };
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let edge_n = normal.cross(b - a) * sign;
        if (p - a).dot(edge_n) < -1.0 {
            return false;
        }
    }
    true
}

fn parse_wad_list(raw: &str) -> Vec<String> {
    raw.split(';')
        .filter_map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return None;
            }
            let file = trimmed
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(trimmed)
                .trim();
            if file.is_empty() {
                None
            } else {
                Some(file.to_ascii_lowercase())
            }
        })
        .collect()
}

fn parse_and_resolve_textures(
    tex_lump: &[u8],
    wad_names: &[String],
    vfs: &Vfs,
) -> Vec<DecodedTexture> {
    if tex_lump.len() < 4 {
        return Vec::new();
    }
    let Ok(count) = read_u32(tex_lump, 0) else {
        return Vec::new();
    };
    let count = count as usize;

    // Load referenced WADs first, plus common CS/HL fallback WADs.
    let mut loaded_wads: Vec<WadArchive> = Vec::new();
    let mut seen_wads: HashMap<String, ()> = HashMap::new();
    let mut load_wad = |wad_name: &str| {
        let lower = wad_name.to_ascii_lowercase();
        if seen_wads.contains_key(&lower) {
            return;
        }
        seen_wads.insert(lower.clone(), ());
        if let Ok(bytes) = vfs.read_bytes(&lower) {
            match WadArchive::parse(bytes) {
                Ok(archive) => loaded_wads.push(archive),
                Err(err) => warn!(wad = %lower, %err, "Failed to parse WAD3 archive"),
            }
        }
    };

    for w in wad_names {
        load_wad(w);
    }
    for fallback in ["cstrike.wad", "halflife.wad", "liquids.wad", "cs_dust.wad"] {
        load_wad(fallback);
    }

    let mut textures = Vec::with_capacity(count);
    for i in 0..count {
        let off_pos = 4 + i * 4;
        let Ok(offset) = read_i32(tex_lump, off_pos) else {
            textures.push(fallback_checkerboard("missing"));
            continue;
        };
        if offset < 0 || (offset as usize) + 40 > tex_lump.len() {
            textures.push(fallback_checkerboard("missing"));
            continue;
        }
        let start = offset as usize;
        let slice = &tex_lump[start..];
        let name = read_cstr(&slice[0..16]);
        let mip0_off = read_u32(slice, 24).unwrap_or(0);

        if mip0_off != 0 {
            // Embedded texture inside BSP
            match decode_miptex(slice) {
                Ok(tex) => {
                    textures.push(tex);
                    continue;
                }
                Err(err) => {
                    warn!(texture = %name, %err, "Failed to decode embedded BSP texture");
                }
            }
        }

        // External texture: look up in loaded WAD archives
        let mut resolved = None;
        for wad in &loaded_wads {
            if let Ok(Some(tex)) = wad.decode_texture(&name) {
                resolved = Some(tex);
                break;
            }
        }

        // Last resort: scan any other WADs in VFS root if not found yet
        if resolved.is_none() && !is_tool_texture(&name) {
            for wad_file in vfs.list_dir("", Some("wad")) {
                if seen_wads.contains_key(&wad_file) {
                    continue;
                }
                seen_wads.insert(wad_file.clone(), ());
                if let Ok(bytes) = vfs.read_bytes(&wad_file)
                    && let Ok(archive) = WadArchive::parse(bytes)
                {
                    if let Ok(Some(tex)) = archive.decode_texture(&name) {
                        resolved = Some(tex);
                    }
                    loaded_wads.push(archive);
                    if resolved.is_some() {
                        break;
                    }
                }
            }
        }

        textures.push(resolved.unwrap_or_else(|| fallback_checkerboard(&name)));
    }

    textures
}

fn is_tool_texture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "sky"
            | "aaatrigger"
            | "clip"
            | "origin"
            | "hint"
            | "skip"
            | "null"
            | "bevel"
            | "black"
    )
}

fn is_sky_texture(name: &str) -> bool {
    name.eq_ignore_ascii_case("sky")
}

fn is_water_texture(name: &str) -> bool {
    name.starts_with('!') || name.starts_with('*')
}

struct ShelfPacker {
    width: u32,
    height: u32,
    cursor_x: u32,
    cursor_y: u32,
    shelf_h: u32,
    rgba: Vec<u8>,
}

impl ShelfPacker {
    fn new(width: u32, height: u32) -> Self {
        // Initialize lightmap atlas with neutral mid-bright (128,128,128 -> x2 overbright = 1.0)
        let mut rgba = vec![128u8; (width * height * 4) as usize];
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Self {
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            shelf_h: 0,
            rgba,
        }
    }

    fn alloc_rect(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let pw = w + 2;
        let ph = h + 2;
        if pw > self.width || ph > self.height {
            return None;
        }
        if self.cursor_x + pw > self.width {
            self.cursor_x = 0;
            self.cursor_y += self.shelf_h;
            self.shelf_h = 0;
        }
        if self.cursor_y + ph > self.height {
            return None;
        }
        let ox = self.cursor_x;
        let oy = self.cursor_y;
        self.cursor_x += pw;
        self.shelf_h = self.shelf_h.max(ph);
        Some((ox, oy))
    }

    /// Allocate a `(w + 2) x (h + 2)` padded slot and copy RGB luxels with 1px clamped border.
    /// Returns top-left `(inner_x, inner_y)` of the `w x h` content inside the atlas.
    fn pack_rgb(&mut self, w: u32, h: u32, rgb: &[u8]) -> Option<(u32, u32)> {
        let (ox, oy) = self.alloc_rect(w, h)?;
        let pw = w + 2;
        let ph = h + 2;

        // Fill padded rectangle by clamping coordinates into [0..w-1, 0..h-1]
        for py in 0..ph {
            let sy = py.saturating_sub(1).min(h - 1);
            for px in 0..pw {
                let sx = px.saturating_sub(1).min(w - 1);
                let src = ((sy * w + sx) * 3) as usize;
                let dst = (((oy + py) * self.width + (ox + px)) * 4) as usize;
                if src + 2 < rgb.len() && dst + 3 < self.rgba.len() {
                    self.rgba[dst] = rgb[src];
                    self.rgba[dst + 1] = rgb[src + 1];
                    self.rgba[dst + 2] = rgb[src + 2];
                    self.rgba[dst + 3] = 255;
                }
            }
        }

        Some((ox + 1, oy + 1))
    }
}

struct BrushDrawInfo {
    model_index: usize,
    dynamic_tag: usize,
    origin: Vec3,
    render_mode: RenderMode,
    alpha: f32,
}

fn collect_brush_draw_models(entities: &[Entity], models: &[BspModel]) -> Vec<BrushDrawInfo> {
    let mut draw_models = Vec::new();
    if !models.is_empty() {
        draw_models.push(BrushDrawInfo {
            model_index: 0,
            dynamic_tag: 0,
            origin: Vec3::ZERO,
            render_mode: RenderMode::Normal,
            alpha: 1.0,
        });
    }

    for ent in entities {
        let Some(model_str) = ent.get("model") else {
            continue;
        };
        let Some(idx_str) = model_str.strip_prefix('*') else {
            continue;
        };
        let Ok(model_index) = idx_str.parse::<usize>() else {
            continue;
        };
        if model_index == 0 || model_index >= models.len() {
            continue;
        }

        // Skip invisible trigger/zone/clip brush entities
        let cls = ent.classname();
        if cls.starts_with("trigger_")
            || cls == "func_buyzone"
            || cls == "func_bomb_target"
            || cls == "func_hostage_rescue"
            || cls == "func_escapezone"
            || cls == "func_vip_safetyzone"
            || cls == "func_ladder"
            || cls == "env_sound"
        {
            continue;
        }

        let raw_mode = ent.get_i32("rendermode").unwrap_or(0);
        let raw_amt = ent.get_f32("renderamt").unwrap_or(255.0).clamp(0.0, 255.0);
        // Invisible brush entity (e.g. func_wall with rendermode 2 & renderamt 0)
        if matches!(raw_mode, 1 | 2 | 5) && raw_amt <= 0.5 {
            continue;
        }

        let (render_mode, alpha) = match raw_mode {
            2 | 1 => (RenderMode::Transparent, raw_amt / 255.0),
            4 => (RenderMode::AlphaTest, 1.0),
            5 => (RenderMode::Additive, raw_amt / 255.0),
            _ => (RenderMode::Normal, 1.0),
        };

        let dynamic_model = matches!(
            cls,
            "func_breakable" | "func_door" | "func_door_rotating"
        );
        let origin = ent.get_vec3("origin").unwrap_or(Vec3::ZERO);
        draw_models.push(BrushDrawInfo {
            model_index,
            dynamic_tag: if dynamic_model { model_index } else { 0 },
            origin,
            render_mode,
            alpha,
        });
    }
    draw_models
}

type FaceExtents = (Vec<Vec3>, Vec<(f32, f32)>, f32, f32, u32, u32);

fn face_extents_and_st(
    face: &Face,
    texinfo: &TexInfo,
    vertices_raw: &[Vec3],
    edges: &[[u16; 2]],
    surf_edges: &[i32],
) -> Option<FaceExtents> {
    let num_edges = face.num_edges as usize;
    if num_edges < 3 || face.first_edge < 0 {
        return None;
    }
    let fe = face.first_edge as usize;
    if fe.saturating_add(num_edges) > surf_edges.len() {
        return None;
    }

    let mut poly_verts = Vec::with_capacity(num_edges);
    for &se in &surf_edges[fe..fe + num_edges] {
        let v_idx = if se >= 0 {
            edges.get(se as usize).map(|e| e[0] as usize)
        } else {
            edges.get((-se) as usize).map(|e| e[1] as usize)
        };
        let pos = v_idx.and_then(|idx| vertices_raw.get(idx).copied())?;
        poly_verts.push(pos);
    }
    if poly_verts.len() < 3 {
        return None;
    }

    let mut st_coords = Vec::with_capacity(poly_verts.len());
    let mut min_s = f32::INFINITY;
    let mut max_s = f32::NEG_INFINITY;
    let mut min_t = f32::INFINITY;
    let mut max_t = f32::NEG_INFINITY;
    for &p in &poly_verts {
        let s = p.dot(texinfo.s_vec) + texinfo.s_shift;
        let t = p.dot(texinfo.t_vec) + texinfo.t_shift;
        min_s = min_s.min(s);
        max_s = max_s.max(s);
        min_t = min_t.min(t);
        max_t = max_t.max(t);
        st_coords.push((s, t));
    }

    let ext_min_s = (min_s / 16.0).floor();
    let ext_max_s = (max_s / 16.0).ceil();
    let ext_min_t = (min_t / 16.0).floor();
    let ext_max_t = (max_t / 16.0).ceil();
    let lm_w = ((ext_max_s - ext_min_s) as i32 + 1).clamp(1, 256) as u32;
    let lm_h = ((ext_max_t - ext_min_t) as i32 + 1).clamp(1, 256) as u32;
    Some((poly_verts, st_coords, ext_min_s, ext_min_t, lm_w, lm_h))
}

#[allow(clippy::too_many_arguments)]
fn build_render_geometry(
    entities: &[Entity],
    planes: &[Plane],
    vertices_raw: &[Vec3],
    texinfos: &[TexInfo],
    faces: &[Face],
    lighting: &[u8],
    edges: &[[u16; 2]],
    surf_edges: &[i32],
    models: &[BspModel],
    textures: &[DecodedTexture],
) -> (LightmapAtlas, Vec<BspVertex>, Vec<u32>, Vec<RenderBatch>) {
    let draw_models = collect_brush_draw_models(entities, models);

    let mut packer = ShelfPacker::new(LIGHTMAP_ATLAS_SIZE, LIGHTMAP_ATLAS_SIZE);
    let mut packed_faces = 0usize;

    // Reserve a tiny 2x2 white-neutral lightmap at (0,0) for unlit faces (water/fullbright)
    let neutral_rgb = [128u8; 2 * 2 * 3];
    let neutral_slot = packer.pack_rgb(2, 2, &neutral_rgb).unwrap_or((1, 1));
    let neutral_uv = [
        (neutral_slot.0 as f32 + 1.0) / LIGHTMAP_ATLAS_SIZE as f32,
        (neutral_slot.1 as f32 + 1.0) / LIGHTMAP_ATLAS_SIZE as f32,
    ];

    // Group triangles by (render_mode_rank, dynamic_tag, texture_index) for minimal draw calls
    // while keeping breakables & doors independently togglable/movable!
    type BucketKey = (u8, usize, usize);
    let mut buckets: HashMap<BucketKey, (RenderMode, Vec<BspVertex>, Vec<u32>)> = HashMap::new();

    for brush in &draw_models {
        let model = &models[brush.model_index];
        let first = model.first_face.max(0) as usize;
        let count = model.num_faces.max(0) as usize;
        let end = first.saturating_add(count).min(faces.len());

        for face in &faces[first..end] {
            let Some(texinfo) = texinfos.get(face.texinfo as usize) else {
                continue;
            };
            if texinfo.miptex < 0 || (texinfo.miptex as usize) >= textures.len() {
                continue;
            }
            let tex_idx = texinfo.miptex as usize;
            let tex = &textures[tex_idx];
            if is_sky_texture(&tex.name) || is_tool_texture(&tex.name) {
                continue;
            }

            let Some((poly_verts, st_coords, ext_min_s, ext_min_t, lm_w, lm_h)) =
                face_extents_and_st(face, texinfo, vertices_raw, edges, surf_edges)
            else {
                continue;
            };

            let Some(plane) = planes.get(face.plane as usize) else {
                continue;
            };
            let normal = if face.plane_side != 0 {
                -plane.normal
            } else {
                plane.normal
            };

            let is_water = is_water_texture(&tex.name);
            let is_scroll = tex.name.to_ascii_lowercase().starts_with("scroll");
            let alpha_test = tex.alpha_test || brush.render_mode == RenderMode::AlphaTest;

            let has_lightmap =
                !is_water && face.lightmap_offset >= 0 && face.styles[0] != 255 && (texinfo.flags & 1) == 0;

            let packed_slot = if has_lightmap {
                let lm_off = face.lightmap_offset as usize;
                let byte_len = (lm_w as usize) * (lm_h as usize) * 3;
                if lm_off.saturating_add(byte_len) <= lighting.len() {
                    let slot = packer.pack_rgb(lm_w, lm_h, &lighting[lm_off..lm_off + byte_len]);
                    if slot.is_some() {
                        packed_faces += 1;
                    }
                    slot
                } else {
                    None
                }
            } else {
                None
            };

            let mut surf_flags = 0u32;
            if alpha_test {
                surf_flags |= SURF_FLAG_ALPHATEST;
            }
            if is_water {
                surf_flags |= SURF_FLAG_WATER;
            }
            if packed_slot.is_none() {
                surf_flags |= SURF_FLAG_NO_LIGHTMAP;
            }
            if is_scroll {
                surf_flags |= SURF_FLAG_SCROLL;
            }

            let effective_mode = match brush.render_mode {
                RenderMode::Additive => RenderMode::Additive,
                RenderMode::Transparent => RenderMode::Transparent,
                RenderMode::AlphaTest => RenderMode::AlphaTest,
                RenderMode::Normal => {
                    if alpha_test {
                        RenderMode::AlphaTest
                    } else if is_water {
                        RenderMode::Transparent
                    } else {
                        RenderMode::Normal
                    }
                }
            };
            let mode_rank = match effective_mode {
                RenderMode::Normal => 0,
                RenderMode::AlphaTest => 1,
                RenderMode::Transparent => 2,
                RenderMode::Additive => 3,
            };

            let alpha = if is_water && brush.render_mode == RenderMode::Normal {
                0.78
            } else {
                brush.alpha
            };

            let entry = buckets
                .entry((mode_rank, brush.dynamic_tag, tex_idx))
                .or_insert_with(|| (effective_mode, Vec::new(), Vec::new()));

            let base_v = entry.1.len() as u32;
            let tex_w = tex.width.max(1) as f32;
            let tex_h = tex.height.max(1) as f32;
            let atlas_f = LIGHTMAP_ATLAS_SIZE as f32;

            for (&p, &(s, t)) in poly_verts.iter().zip(st_coords.iter()) {
                let world_pos = p + brush.origin + model.origin;
                let lm_uv = if let Some((ax, ay)) = packed_slot {
                    let lu = (ax as f32 + (s / 16.0 - ext_min_s) + 0.5) / atlas_f;
                    let lv = (ay as f32 + (t / 16.0 - ext_min_t) + 0.5) / atlas_f;
                    [lu, lv]
                } else {
                    neutral_uv
                };
                entry.1.push(BspVertex {
                    position: world_pos.to_array(),
                    normal: normal.to_array(),
                    uv: [s / tex_w, t / tex_h],
                    lm_uv,
                    alpha,
                    flags: surf_flags,
                });
            }

            // Ensure CCW winding relative to the BSP face normal!
            // Sum cross products across the entire fan so collinear T-junction vertices
            // inserted by hlbsp/zhlt along an edge never produce a zero cross product.
            let mut area_vec = Vec3::ZERO;
            for i in 1..(poly_verts.len() - 1) {
                let e1 = poly_verts[i] - poly_verts[0];
                let e2 = poly_verts[i + 1] - poly_verts[0];
                area_vec += e1.cross(e2);
            }
            let flip = area_vec.dot(normal) < 0.0;

            for i in 1..(poly_verts.len() as u32 - 1) {
                if flip {
                    entry.2.push(base_v);
                    entry.2.push(base_v + i + 1);
                    entry.2.push(base_v + i);
                } else {
                    entry.2.push(base_v);
                    entry.2.push(base_v + i);
                    entry.2.push(base_v + i + 1);
                }
            }
        }
    }

    // Flatten buckets sorted by (render_mode_rank, texture_index) so opaque draws before transparent
    let mut keys: Vec<BucketKey> = buckets.keys().copied().collect();
    keys.sort_unstable();

    let mut all_vertices = Vec::new();
    let mut all_indices = Vec::new();
    let mut batches = Vec::with_capacity(keys.len());

    for key in keys {
        let Some((render_mode, verts, inds)) = buckets.remove(&key) else {
            continue;
        };
        if inds.is_empty() {
            continue;
        }
        let vertex_offset = all_vertices.len() as u32;
        let first_index = all_indices.len() as u32;
        all_vertices.extend(verts);
        all_indices.extend(inds.into_iter().map(|idx| idx + vertex_offset));
        batches.push(RenderBatch {
            texture_index: key.2,
            render_mode,
            model_index: key.1,
            first_index,
            index_count: (all_indices.len() as u32) - first_index,
        });
    }

    let atlas = LightmapAtlas {
        width: LIGHTMAP_ATLAS_SIZE,
        height: LIGHTMAP_ATLAS_SIZE,
        rgba: packer.rgba,
        packed_faces,
    };

    (atlas, all_vertices, all_indices, batches)
}

fn extract_spawns(entities: &[Entity]) -> Vec<SpawnPoint> {
    let mut spawns = Vec::new();
    for ent in entities {
        let team = match ent.classname() {
            "info_player_start" => SpawnTeam::CounterTerrorist,
            "info_player_deathmatch" => SpawnTeam::Terrorist,
            _ => continue,
        };
        let Some(position) = ent.get_vec3("origin") else {
            continue;
        };
        let yaw_deg = ent
            .get_vec3("angles")
            .map(|a| a.y)
            .or_else(|| ent.get_f32("angle"))
            .unwrap_or(0.0);
        spawns.push(SpawnPoint {
            position,
            yaw_deg,
            team,
        });
    }
    spawns
}

fn extract_hostage_spawns(entities: &[Entity]) -> Vec<(Vec3, f32)> {
    let mut out = Vec::new();
    for ent in entities {
        if ent.classname() != "hostage_entity" {
            continue;
        }
        let Some(pos) = ent.get_vec3("origin") else {
            continue;
        };
        let yaw = ent
            .get_vec3("angles")
            .map(|a| a.y)
            .or_else(|| ent.get_f32("angle"))
            .unwrap_or(0.0);
        out.push((pos, yaw));
    }
    out
}

fn extract_brush_entities(entities: &[Entity], models: &[BspModel]) -> Vec<BrushEntity> {
    let mut out = Vec::new();
    for ent in entities {
        let Some(model_str) = ent.get("model") else {
            continue;
        };
        let Some(idx_str) = model_str.strip_prefix('*') else {
            continue;
        };
        let Ok(model_index) = idx_str.parse::<usize>() else {
            continue;
        };
        if model_index == 0 || model_index >= models.len() {
            continue;
        }
        let m = &models[model_index];
        let origin = ent.get_vec3("origin").unwrap_or(Vec3::ZERO) + m.origin;
        let mins = m.mins + origin;
        let maxs = m.maxs + origin;
        let cls = ent.classname();

        let kind = match cls {
            "func_ladder" => BrushVolumeKind::Ladder,
            "func_buyzone" => {
                let team = match ent.get_i32("team") {
                    Some(1) => Some(SpawnTeam::Terrorist),
                    Some(2) => Some(SpawnTeam::CounterTerrorist),
                    _ => None,
                };
                BrushVolumeKind::BuyZone { team }
            }
            "func_bomb_target" => BrushVolumeKind::BombTarget,
            "func_hostage_rescue" => BrushVolumeKind::HostageRescue,
            "func_breakable" => {
                let health = ent.get_f32("health").unwrap_or(50.0).max(1.0);
                let material = ent.get_i32("material").unwrap_or(0);
                BrushVolumeKind::Breakable { health, material }
            }
            "func_door" | "func_door_rotating" => {
                let speed = ent.get_f32("speed").unwrap_or(100.0).max(20.0);
                let lip = ent.get_f32("lip").unwrap_or(8.0);
                let wait = ent.get_f32("wait").unwrap_or(4.0);
                let yaw = ent
                    .get_vec3("angles")
                    .map(|a| a.y)
                    .or_else(|| ent.get_f32("angle"))
                    .unwrap_or(0.0);
                let move_dir = if (yaw - (-1.0)).abs() < 0.1 {
                    Vec3::Z
                } else if (yaw - (-2.0)).abs() < 0.1 {
                    -Vec3::Z
                } else {
                    let r = yaw.to_radians();
                    Vec3::new(r.cos(), r.sin(), 0.0)
                };
                BrushVolumeKind::Door {
                    speed,
                    lip,
                    move_dir,
                    wait,
                    rotating: cls == "func_door_rotating",
                }
            }
            "func_wall" | "func_pushable" | "func_button" | "func_plat" | "func_train" => {
                BrushVolumeKind::Solid
            }
            _ => continue,
        };

        out.push(BrushEntity {
            classname: cls.to_string(),
            model_index,
            origin,
            mins,
            maxs,
            kind,
        });
    }
    out
}

/// Parse BSP `LUMP_ENTITIES` blocks (`{ "key" "value" ... }`).
pub fn parse_entities(input: &str) -> Vec<Entity> {
    let mut entities = Vec::new();
    let mut current: Option<HashMap<String, String>> = None;
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            '{' => {
                chars.next();
                current = Some(HashMap::new());
            }
            '}' => {
                chars.next();
                if let Some(map) = current.take() {
                    entities.push(Entity { properties: map });
                }
            }
            '"' => {
                let key = read_quoted(&mut chars);
                skip_ws(&mut chars);
                if chars.peek() == Some(&'"') {
                    let val = read_quoted(&mut chars);
                    if let Some(ref mut map) = current {
                        map.insert(key.to_ascii_lowercase(), val);
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }

    entities
}

fn skip_ws(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
}

fn read_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut out = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
    }
    for ch in chars.by_ref() {
        if ch == '"' {
            break;
        }
        out.push(ch);
    }
    out
}

fn parse_planes(data: &[u8]) -> Result<Vec<Plane>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 20);
    for chunk in data.as_chunks::<20>().0 {
        out.push(Plane {
            normal: Vec3::new(read_f32(chunk, 0)?, read_f32(chunk, 4)?, read_f32(chunk, 8)?),
            dist: read_f32(chunk, 12)?,
            plane_type: read_i32(chunk, 16)?,
        });
    }
    Ok(out)
}

fn parse_vertices(data: &[u8]) -> Result<Vec<Vec3>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 12);
    for chunk in data.as_chunks::<12>().0 {
        out.push(Vec3::new(
            read_f32(chunk, 0)?,
            read_f32(chunk, 4)?,
            read_f32(chunk, 8)?,
        ));
    }
    Ok(out)
}

fn parse_nodes(data: &[u8]) -> Result<Vec<Node>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 24);
    for chunk in data.as_chunks::<24>().0 {
        out.push(Node {
            plane: read_u32(chunk, 0)?,
            children: [read_i16(chunk, 4)?, read_i16(chunk, 6)?],
            mins: [read_i16(chunk, 8)?, read_i16(chunk, 10)?, read_i16(chunk, 12)?],
            maxs: [read_i16(chunk, 14)?, read_i16(chunk, 16)?, read_i16(chunk, 18)?],
            first_face: read_u16(chunk, 20)?,
            num_faces: read_u16(chunk, 22)?,
        });
    }
    Ok(out)
}

fn parse_texinfos(data: &[u8]) -> Result<Vec<TexInfo>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 40);
    for chunk in data.as_chunks::<40>().0 {
        out.push(TexInfo {
            s_vec: Vec3::new(read_f32(chunk, 0)?, read_f32(chunk, 4)?, read_f32(chunk, 8)?),
            s_shift: read_f32(chunk, 12)?,
            t_vec: Vec3::new(read_f32(chunk, 16)?, read_f32(chunk, 20)?, read_f32(chunk, 24)?),
            t_shift: read_f32(chunk, 28)?,
            miptex: read_i32(chunk, 32)?,
            flags: read_i32(chunk, 36)?,
        });
    }
    Ok(out)
}

fn parse_faces(data: &[u8]) -> Result<Vec<Face>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 20);
    for chunk in data.as_chunks::<20>().0 {
        out.push(Face {
            plane: read_u16(chunk, 0)?,
            plane_side: read_u16(chunk, 2)?,
            first_edge: read_i32(chunk, 4)?,
            num_edges: read_u16(chunk, 8)?,
            texinfo: read_u16(chunk, 10)?,
            styles: [chunk[12], chunk[13], chunk[14], chunk[15]],
            lightmap_offset: read_i32(chunk, 16)?,
        });
    }
    Ok(out)
}

fn parse_clipnodes(data: &[u8]) -> Result<Vec<ClipNode>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 8);
    for chunk in data.as_chunks::<8>().0 {
        out.push(ClipNode {
            plane: read_i32(chunk, 0)?,
            children: [read_i16(chunk, 4)?, read_i16(chunk, 6)?],
        });
    }
    Ok(out)
}

fn parse_leafs(data: &[u8]) -> Result<Vec<Leaf>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 28);
    for chunk in data.as_chunks::<28>().0 {
        out.push(Leaf {
            contents: read_i32(chunk, 0)?,
            vis_offset: read_i32(chunk, 4)?,
            mins: [read_i16(chunk, 8)?, read_i16(chunk, 10)?, read_i16(chunk, 12)?],
            maxs: [read_i16(chunk, 14)?, read_i16(chunk, 16)?, read_i16(chunk, 18)?],
            first_mark_surface: read_u16(chunk, 20)?,
            num_mark_surfaces: read_u16(chunk, 22)?,
            ambient_levels: [chunk[24], chunk[25], chunk[26], chunk[27]],
        });
    }
    Ok(out)
}

fn parse_u16_vec(data: &[u8]) -> Result<Vec<u16>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 2);
    for chunk in data.as_chunks::<2>().0 {
        out.push(read_u16(chunk, 0)?);
    }
    Ok(out)
}

fn parse_edges(data: &[u8]) -> Result<Vec<[u16; 2]>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 4);
    for chunk in data.as_chunks::<4>().0 {
        out.push([read_u16(chunk, 0)?, read_u16(chunk, 2)?]);
    }
    Ok(out)
}

fn parse_i32_vec(data: &[u8]) -> Result<Vec<i32>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 4);
    for chunk in data.as_chunks::<4>().0 {
        out.push(read_i32(chunk, 0)?);
    }
    Ok(out)
}

fn parse_models(data: &[u8]) -> Result<Vec<BspModel>, BspError> {
    let mut out = Vec::with_capacity(data.len() / 64);
    for chunk in data.as_chunks::<64>().0 {
        out.push(BspModel {
            mins: Vec3::new(read_f32(chunk, 0)?, read_f32(chunk, 4)?, read_f32(chunk, 8)?),
            maxs: Vec3::new(read_f32(chunk, 12)?, read_f32(chunk, 16)?, read_f32(chunk, 20)?),
            origin: Vec3::new(read_f32(chunk, 24)?, read_f32(chunk, 28)?, read_f32(chunk, 32)?),
            head_nodes: [
                read_i32(chunk, 36)?,
                read_i32(chunk, 40)?,
                read_i32(chunk, 44)?,
                read_i32(chunk, 48)?,
            ],
            vis_leafs: read_i32(chunk, 52)?,
            first_face: read_i32(chunk, 56)?,
            num_faces: read_i32(chunk, 60)?,
        });
    }
    Ok(out)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, BspError> {
    let arr: [u8; 2] = bytes
        .get(offset..offset + 2)
        .ok_or(BspError::Truncated("u16"))?
        .try_into()
        .map_err(|_| BspError::Truncated("u16"))?;
    Ok(u16::from_le_bytes(arr))
}

fn read_i16(bytes: &[u8], offset: usize) -> Result<i16, BspError> {
    let arr: [u8; 2] = bytes
        .get(offset..offset + 2)
        .ok_or(BspError::Truncated("i16"))?
        .try_into()
        .map_err(|_| BspError::Truncated("i16"))?;
    Ok(i16::from_le_bytes(arr))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, BspError> {
    let arr: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(BspError::Truncated("u32"))?
        .try_into()
        .map_err(|_| BspError::Truncated("u32"))?;
    Ok(u32::from_le_bytes(arr))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, BspError> {
    let arr: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(BspError::Truncated("i32"))?
        .try_into()
        .map_err(|_| BspError::Truncated("i32"))?;
    Ok(i32::from_le_bytes(arr))
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, BspError> {
    let arr: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(BspError::Truncated("f32"))?
        .try_into()
        .map_err(|_| BspError::Truncated("f32"))?;
    Ok(f32::from_le_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_anim_table_resolves_primary_and_alt_chains() {
        let tex = |name: &str| DecodedTexture {
            name: name.to_string(),
            width: 16,
            height: 16,
            rgba: vec![0; 16 * 16 * 4],
            alpha_test: false,
        };
        let table = TextureAnimTable::from_textures(&[
            tex("+0button"),
            tex("+1button"),
            tex("+abutton"),
            tex("+bbutton"),
            tex("wall"),
        ]);
        // At 5 Hz, 0.0s -> frame 0 (+0button), 0.2s -> frame 1 (+1button)
        assert_eq!(table.resolve(0, 0.0, 0.0), 0);
        assert_eq!(table.resolve(0, 0.25, 0.0), 1);
        // Non-zero entity frame switches to +A/+B chain
        assert_eq!(table.resolve(0, 0.0, 1.0), 2);
        assert_eq!(table.resolve(0, 0.25, 1.0), 3);
        // Plain texture unchanged
        assert_eq!(table.resolve(4, 0.5, 1.0), 4);
    }

    #[test]
    fn rebuild_for_styles_updates_only_dirty_rects_and_weights_pages() {
        let mut packer = ShelfPacker::new(64, 64);
        let (ox, oy) = packer.alloc_rect(2, 2).unwrap();
        let mut atlas = LightmapAtlas {
            width: 64,
            height: 64,
            rgba: packer.rgba,
            packed_faces: 1,
        };
        let lighting = vec![
            100, 50, 25, 100, 50, 25, 100, 50, 25, 100, 50, 25, // style 0
            20, 40, 60, 20, 40, 60, 20, 40, 60, 20, 40, 60,     // style 1
        ];
        let rects = vec![FaceLightmapInfo {
            face_index: 0,
            atlas_x: ox + 1,
            atlas_y: oy + 1,
            width: 2,
            height: 2,
            styles: [0, 1, 255, 255],
            num_styles: 2,
            lightmap_offset: 0,
        }];
        let mut prev = [1.0f32; 64];
        let curr_same = [1.0f32; 64];
        assert!(atlas.rebuild_for_styles(&lighting, &rects, &curr_same, &prev).is_empty());

        let mut curr = [1.0f32; 64];
        curr[1] = 2.0; // boost style 1 -> RGB = 100+40=140, 50+80=130, 25+120=145
        let dirty = atlas.rebuild_for_styles(&lighting, &rects, &curr, &prev);
        assert_eq!(dirty, vec![(ox, oy, 4, 4)]);
        let px = (((oy + 1) * 64 + (ox + 1)) * 4) as usize;
        assert_eq!(&atlas.rgba[px..px + 4], &[140, 130, 145, 255]);
        prev[1] = 2.0;
        assert!(atlas.rebuild_for_styles(&lighting, &rects, &curr, &prev).is_empty());
    }
}
