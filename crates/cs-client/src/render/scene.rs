//! Renderer-facing scene API: entities keyed by precache model index, independent of the legacy
//! `WorldSnapshot`. Everything in this file is pure (no GPU), so the scene build is testable
//! headless.
//!
//! The client core hands the shell a list of [`RenderEntity`] (one per visible edict or tempent)
//! plus the precache model name list. [`ModelRegistry`] maps a `modelindex` to what has to be drawn
//! (`*N` brush submodel of the world BSP, `.mdl` studio model, `.spr` sprite), and
//! [`build_draw_lists`] turns the entity list into sorted draw lists for the three renderers.
//!
//! Behaviour sources (GPL-3-or-later, Xash3D FWGS, read 2026-11-23, files in
//! `scratchpad\p3src\xash_r`): `ref/gl/gl_rmain.c` (`R_AddEntity`, `R_DrawEntitiesOnList`,
//! `R_RotateForEntity`, `CL_FxBlend`), `ref/gl/gl_rsurf.c` (`R_SetRenderMode`,
//! `R_DrawBrushModel`), `ref/gl/gl_sprite.c` (`R_DrawSpriteModel`, `R_SpriteGlowBlend`).
//! The Valve HLSDK is read-for-behaviour only and nothing is ported from it.

// The shell (Task 8b) is the consumer of this API; until it lands only the tests use parts of it.
#![allow(dead_code)]

use cs_assets::spr::SpriteType;
use glam::{Mat4, Vec3, Vec4};

/// `EF_NODRAW` effect bit (assumption: value 128 taken from the GoldSrc `const.h` layout, which
/// the protocol-48 reader in `cs-proto48` also uses for `effects`).
pub const EF_NODRAW: u32 = 128;

/// `kRenderFx*` values (`const.h` of the Xash3D FWGS common headers, lines 698-720, read
/// 2026-11-23).
pub const RENDERFX_NONE: u8 = 0;
pub const RENDERFX_PULSE_SLOW: u8 = 1;
pub const RENDERFX_PULSE_FAST: u8 = 2;
pub const RENDERFX_PULSE_SLOW_WIDE: u8 = 3;
pub const RENDERFX_PULSE_FAST_WIDE: u8 = 4;
pub const RENDERFX_FADE_SLOW: u8 = 5;
pub const RENDERFX_FADE_FAST: u8 = 6;
pub const RENDERFX_SOLID_SLOW: u8 = 7;
pub const RENDERFX_SOLID_FAST: u8 = 8;
pub const RENDERFX_STROBE_SLOW: u8 = 9;
pub const RENDERFX_STROBE_FAST: u8 = 10;
pub const RENDERFX_STROBE_FASTER: u8 = 11;
pub const RENDERFX_FLICKER_SLOW: u8 = 12;
pub const RENDERFX_FLICKER_FAST: u8 = 13;
pub const RENDERFX_NO_DISSIPATION: u8 = 14;
pub const RENDERFX_DISTORT: u8 = 15;
pub const RENDERFX_HOLOGRAM: u8 = 16;
pub const RENDERFX_DEAD_PLAYER: u8 = 17;
pub const RENDERFX_EXPLODE: u8 = 18;
pub const RENDERFX_LIGHT_MULTIPLIER: u8 = 21;

/// GoldSrc `kRender*` entity render modes (`entity_state_t::rendermode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntRenderMode {
    Normal = 0,
    TransColor = 1,
    TransTexture = 2,
    Glow = 3,
    TransAlpha = 4,
    TransAdd = 5,
}

impl EntRenderMode {
    /// Unknown values behave like `kRenderNormal`'s fall-through in the engine's `default:` arms;
    /// here they map to `Normal`.
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::TransColor,
            2 => Self::TransTexture,
            3 => Self::Glow,
            4 => Self::TransAlpha,
            5 => Self::TransAdd,
            _ => Self::Normal,
        }
    }

    /// `R_ModelOpaque`: only `kRenderNormal` (`ref/gl/gl_local.h:76`, read 2026-11-23).
    pub fn is_opaque(self) -> bool {
        self == Self::Normal
    }
}

/// `R_OpaqueEntity` (`ref/gl/gl_rmain.c:63-77`): solid pass only for `kRenderNormal` with a
/// renderfx of none, dead player, light multiplier or explode.
pub fn is_opaque_entity(mode: EntRenderMode, renderfx: u8) -> bool {
    mode.is_opaque()
        && matches!(
            renderfx,
            RENDERFX_NONE | RENDERFX_DEAD_PLAYER | RENDERFX_LIGHT_MULTIPLIER | RENDERFX_EXPLODE
        )
}

/// `R_RankForRenderMode` (`ref/gl/gl_rmain.c:30-42`): tie-break of the translucent sort.
pub fn render_mode_rank(mode: EntRenderMode) -> u8 {
    match mode {
        EntRenderMode::TransTexture => 1,
        EntRenderMode::TransAdd => 2,
        EntRenderMode::Glow => 3,
        _ => 0,
    }
}

/// Handle of a model in the [`ModelRegistry`]: the precache index (`modelindex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelHandle(pub u16);

/// What a precached model name denotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// The world BSP (`maps/<name>.bsp`, precache index 1): drawn by the world pass, not as an entity.
    World,
    /// `*N` brush submodel of the world BSP.
    Brush(u32),
    /// `.mdl` studio model.
    Studio,
    /// `.spr` sprite.
    Sprite,
    /// Anything else (empty name, unknown extension).
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelEntry {
    pub name: String,
    pub kind: ModelKind,
    /// `(mins + maxs) / 2` of the model, used by the translucent sort (`R_TransEntityCompare`).
    /// Zero until [`ModelRegistry::set_brush_bounds`] fills brush submodels; studio models and
    /// sprites stay at zero (assumption: their bounds are centred on the origin).
    pub center: Vec3,
}

/// Parse a `*N` brush submodel name (`"*12"` -> `Some(12)`).
pub fn parse_brush_index(name: &str) -> Option<u32> {
    let digits = name.strip_prefix('*')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Classify a precache model name.
pub fn classify_model(name: &str) -> ModelKind {
    if let Some(n) = parse_brush_index(name) {
        return ModelKind::Brush(n);
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".mdl") {
        ModelKind::Studio
    } else if lower.ends_with(".spr") {
        ModelKind::Sprite
    } else if lower.ends_with(".bsp") {
        ModelKind::World
    } else {
        ModelKind::Unknown
    }
}

/// Model table keyed by precache index (index 0 is always empty, as in the engine).
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    entries: Vec<Option<ModelEntry>>,
}

impl ModelRegistry {
    /// Build from the precache list where element `i` is the model with `modelindex == i`
    /// (element 0 is the empty slot). Empty names stay unregistered.
    pub fn from_precache<S: AsRef<str>>(names: &[S]) -> Self {
        // modelindex is a u16 on our side; ignore anything past it.
        let entries = names
            .iter()
            .take(usize::from(u16::MAX) + 1)
            .enumerate()
            .map(|(i, n)| {
                let name = n.as_ref();
                if i == 0 || name.is_empty() {
                    None
                } else {
                    Some(ModelEntry {
                        name: name.to_string(),
                        kind: classify_model(name),
                        center: Vec3::ZERO,
                    })
                }
            })
            .collect();
        Self { entries }
    }

    /// Fill the sort centres of `*N` entries from the world BSP's submodel bounds
    /// (`bounds[n] = (mins, maxs)` of submodel `n`).
    pub fn set_brush_bounds(&mut self, bounds: &[(Vec3, Vec3)]) {
        for e in self.entries.iter_mut().flatten() {
            if let ModelKind::Brush(n) = e.kind
                && let Some((mins, maxs)) = bounds.get(n as usize)
            {
                e.center = (*mins + *maxs) * 0.5;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve a wire `modelindex` to a handle; `None` for 0, negative, or an index past the table.
    pub fn resolve(&self, modelindex: i32) -> Option<ModelHandle> {
        let idx = u16::try_from(modelindex).ok()?;
        self.entries
            .get(idx as usize)
            .and_then(|e| e.as_ref())
            .map(|_| ModelHandle(idx))
    }

    pub fn get(&self, handle: ModelHandle) -> Option<&ModelEntry> {
        self.entries.get(handle.0 as usize).and_then(|e| e.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = (ModelHandle, &ModelEntry)> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (ModelHandle(i as u16), e)))
    }
}

/// One renderable entity, resolved from entity state by the client core.
#[derive(Debug, Clone)]
pub struct RenderEntity {
    /// Entity number (used to de-sync the renderfx pulses, as the engine does with the index).
    pub index: u16,
    pub model: ModelHandle,
    pub origin: Vec3,
    /// GoldSrc angles, degrees: (pitch, yaw, roll).
    pub angles: Vec3,
    pub frame: f32,
    pub sequence: i32,
    pub animtime: f32,
    pub framerate: f32,
    pub skin: i32,
    pub body: i32,
    pub effects: u32,
    /// `cl_entity_t::player`: the entity is a player model, whose studio pitch is zeroed
    /// (`ref/gl/gl_studio.c:543`).
    pub player: bool,
    pub rendermode: EntRenderMode,
    pub renderamt: u8,
    pub rendercolor: [u8; 3],
    pub renderfx: u8,
    /// `entity_state_t::scale`; 0 means 1.
    pub scale: f32,
    /// A studio model merged onto this one's bones (`EF_BONEMERGE`: the weapon a player holds).
    pub attached: Option<ModelHandle>,
}

impl RenderEntity {
    /// An opaque, unrotated entity with the given model; the other fields are neutral.
    pub fn new(index: u16, model: ModelHandle, origin: Vec3) -> Self {
        Self {
            index,
            model,
            origin,
            angles: Vec3::ZERO,
            frame: 0.0,
            sequence: 0,
            animtime: 0.0,
            framerate: 1.0,
            skin: 0,
            body: 0,
            effects: 0,
            player: false,
            rendermode: EntRenderMode::Normal,
            renderamt: 255,
            rendercolor: [0, 0, 0],
            renderfx: 0,
            scale: 0.0,
            attached: None,
        }
    }
}

/// Camera basis used by the scene build (GoldSrc axes: x forward, y left, z up; `right` points right).
#[derive(Debug, Clone, Copy)]
pub struct ViewBasis {
    pub origin: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

impl ViewBasis {
    pub fn from_angles(origin: Vec3, angles: Vec3) -> Self {
        let (forward, right, up) = angle_vectors(angles);
        Self {
            origin,
            forward,
            right,
            up,
        }
    }
}

/// Entities of one frame plus the data the render-mode math needs.
#[derive(Debug, Clone)]
pub struct SceneFrame {
    /// Client time in seconds (renderfx pulses, studio frame advance).
    pub time: f32,
    pub view: ViewBasis,
    pub entities: Vec<RenderEntity>,
}

/// `AngleVectors` with GoldSrc conventions: angles are (pitch, yaw, roll) in degrees, positive
/// pitch looks down.
pub fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sp, cp) = angles.x.to_radians().sin_cos();
    let (sy, cy) = angles.y.to_radians().sin_cos();
    let (sr, cr) = angles.z.to_radians().sin_cos();
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
    (forward, right, up)
}

/// Entity-to-world matrix: columns `forward`, `-right`, `up`, translation `origin`, optionally
/// scaled. Port of `Matrix3x4_CreateFromEntity` as used by `R_RotateForEntity`
/// (`ref/gl/gl_rmain.c`, read 2026-11-23).
pub fn entity_matrix(origin: Vec3, angles: Vec3, scale: f32) -> Mat4 {
    let (forward, right, up) = angle_vectors(angles);
    Mat4::from_cols(
        (forward * scale).extend(0.0),
        (-right * scale).extend(0.0),
        (up * scale).extend(0.0),
        Vec4::new(origin.x, origin.y, origin.z, 1.0),
    )
}

/// Transform of a brush submodel entity: `R_DrawBrushModel` uses `R_RotateForEntity` when the
/// angles are non-zero and a pure translation otherwise, and `R_RotateForEntity` never scales
/// brush models (`ref/gl/gl_rsurf.c`, `gl_rmain.c`, read 2026-11-23).
pub fn brush_matrix(origin: Vec3, angles: Vec3) -> Mat4 {
    if angles == Vec3::ZERO {
        Mat4::from_translation(origin)
    } else {
        entity_matrix(origin, angles, 1.0)
    }
}

/// Transform of a studio model entity.
///
/// `R_StudioSetupModel`'s matrix (`ref/gl/gl_studio.c:535-545`, read 2026-11-23): the pitch is
/// negated ("stupid quake bug") and zeroed for players ("don't rotate clients, only aim").
/// The engine builds this matrix with scale 1; `scale` here is the entity scale applied to the
/// whole transform (assumption: the studio vertex path scales the same way).
pub fn studio_matrix(origin: Vec3, angles: Vec3, scale: f32, player: bool) -> Mat4 {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let pitch = if player { 0.0 } else { -angles.x };
    entity_matrix(origin, Vec3::new(pitch, angles.y, angles.z), s)
}

/// Port of `CL_FxBlend` (`ref/gl/gl_rmain.c`, read 2026-11-23), returning the 0..=255 blend.
///
/// `kRenderFx` ids follow `const.h` (distort 15, hologram 16). Deviations: the fade/solid fx mutate `renderamt` in the engine (state kept per entity); here
/// they return the current `renderamt` unchanged. Hologram/distort add `COM_RandomLong(-32, 31)`
/// in the engine; here the jitter is omitted. Both are assumptions to revisit with the shell.
pub fn fx_blend(e: &RenderEntity, time: f32, view: &ViewBasis) -> i32 {
    let amt = i32::from(e.renderamt);
    let offset = f32::from(e.index) * 363.0;
    let t = f64::from(time);
    let off = f64::from(offset);
    // C: `e->curstate.renderamt + 0x40 * sin(...)` is a double expression truncated to int.
    let pulse = |scale: f64, speed: f64| (f64::from(amt) + scale * (t * speed + off).sin()) as i32;
    let gate = |v: f64| if (v as i32) < 0 { 0 } else { amt };
    match e.renderfx {
        // gl_rmain.c:1228-1237: Wide pulses swing by 0x40, the others by 0x10.
        RENDERFX_PULSE_SLOW_WIDE => pulse(64.0, 2.0),
        RENDERFX_PULSE_FAST_WIDE => pulse(64.0, 8.0),
        RENDERFX_PULSE_SLOW => pulse(16.0, 2.0),
        RENDERFX_PULSE_FAST => pulse(16.0, 8.0),
        // 5 FadeSlow, 6 FadeFast, 7 SolidSlow, 8 SolidFast: see the doc comment.
        RENDERFX_STROBE_SLOW => gate(20.0 * (t * 4.0 + off).sin()),
        RENDERFX_STROBE_FAST => gate(20.0 * (t * 16.0 + off).sin()),
        RENDERFX_STROBE_FASTER => gate(20.0 * (t * 36.0 + off).sin()),
        RENDERFX_FLICKER_SLOW => gate(20.0 * ((t * 2.0).sin() + (t * 17.0 + off).sin())),
        RENDERFX_FLICKER_FAST => gate(20.0 * ((t * 16.0).sin() + (t * 23.0 + off).sin())),
        RENDERFX_DISTORT | RENDERFX_HOLOGRAM => {
            // Distort turns the distance fade off (dist = 1).
            let dist = if e.renderfx == RENDERFX_DISTORT {
                1.0
            } else {
                (e.origin - view.origin).dot(view.forward)
            };
            if dist <= 0.0 {
                0
            } else if dist <= 100.0 {
                180
            } else {
                ((1.0 - (dist - 100.0) * (1.0 / 400.0)) * 180.0) as i32
            }
        }
        _ => amt,
    }
}

/// Glow-sprite brightness and scale multiplier from the view distance: the distance part of
/// `R_SpriteGlowBlend` (`ref/gl/gl_sprite.c`, read 2026-11-23).
///
/// The engine also traces from the eye and hides the glow when the origin is occluded; that part
/// needs world traces and is not done here (assumption: glows show through walls for now).
pub fn glow_blend(dist: f32, renderfx: u8) -> (f32, f32) {
    if renderfx == RENDERFX_NO_DISSIPATION {
        return (1.0, 1.0);
    }
    const GLARE_FALLOFF: f32 = 19000.0;
    let brightness = (GLARE_FALLOFF / (dist * dist)).clamp(0.05, 1.0);
    (brightness, dist * (1.0 / 200.0))
}

/// World-space axes of a sprite quad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteAxes {
    pub origin: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

/// Port of the orientation switch of `R_DrawSpriteModel` (`ref/gl/gl_sprite.c`, read
/// 2026-11-23). Returns `None` when the sprite is invisible (a parallel-upright sprite seen from
/// within one degree of straight up or down).
pub fn sprite_axes(
    sprite_type: SpriteType,
    origin: Vec3,
    angles: Vec3,
    view: &ViewBasis,
) -> Option<SpriteAxes> {
    // Automatically roll parallel sprites when the entity has a roll angle.
    let sprite_type = if angles.z != 0.0 && sprite_type == SpriteType::Parallel {
        SpriteType::ParallelOriented
    } else {
        sprite_type
    };
    match sprite_type {
        SpriteType::Oriented => {
            let (forward, right, up) = angle_vectors(angles);
            // 0.01 back along forward "to avoid z-fighting".
            Some(SpriteAxes {
                origin: origin - forward * 0.01,
                right,
                up,
            })
        }
        SpriteType::FacingUpright => {
            let right = Vec3::new(origin.y - view.origin.y, -(origin.x - view.origin.x), 0.0)
                .normalize_or_zero();
            Some(SpriteAxes {
                origin,
                right,
                up: Vec3::Z,
            })
        }
        SpriteType::ParallelUpright => {
            let dot = view.forward.z;
            // cos(1 degree) = 0.999848
            if !(-0.999_848..=0.999_848).contains(&dot) {
                return None;
            }
            let right = Vec3::new(view.forward.y, -view.forward.x, 0.0).normalize_or_zero();
            Some(SpriteAxes {
                origin,
                right,
                up: Vec3::Z,
            })
        }
        SpriteType::ParallelOriented => {
            let (sr, cr) = angles.z.to_radians().sin_cos();
            Some(SpriteAxes {
                origin,
                right: view.right * cr + view.up * sr,
                up: view.right * -sr + view.up * cr,
            })
        }
        SpriteType::Parallel => Some(SpriteAxes {
            origin,
            right: view.right,
            up: view.up,
        }),
    }
}

/// The four corners of a sprite frame quad, in the engine's `R_DrawSpriteQuad` order
/// (bottom-left, top-left, top-right, bottom-right) with their texture coordinates.
///
/// `left`/`right`/`up`/`down` are the frame extents relative to the sprite origin: the on-disk
/// frame origin gives `left = origin_x`, `up = origin_y`, `right = left + width`,
/// `down = up - height`.
pub fn sprite_quad(
    axes: &SpriteAxes,
    left: f32,
    right: f32,
    up: f32,
    down: f32,
    scale: f32,
) -> [(Vec3, [f32; 2]); 4] {
    let p = |u: f32, r: f32| axes.origin + axes.up * (u * scale) + axes.right * (r * scale);
    [
        (p(down, left), [0.0, 1.0]),
        (p(up, left), [0.0, 0.0]),
        (p(up, right), [1.0, 0.0]),
        (p(down, right), [1.0, 1.0]),
    ]
}

/// A `*N` brush entity ready to draw.
#[derive(Debug, Clone)]
pub struct BrushDraw {
    pub submodel: u32,
    pub matrix: Mat4,
    pub mode: EntRenderMode,
    /// 0..=1 blend (`CL_FxBlend / 255` for non-opaque modes, 1 for opaque ones).
    pub blend: f32,
    /// Raw `renderamt / 255`: `kRenderTransColor` uses it instead of the fx blend
    /// (`gl_rsurf.c:1773`).
    pub amt: f32,
    pub color: [f32; 3],
    /// In the solid pass (`R_OpaqueEntity`) rather than the sorted translucent one.
    pub solid: bool,
    /// Translucent sort key (`R_TransEntityCompare`).
    pub distance_sq: f32,
    pub rank: u8,
}

/// A studio model entity ready to draw.
#[derive(Debug, Clone)]
pub struct StudioDraw {
    pub model: ModelHandle,
    pub matrix: Mat4,
    pub sequence: i32,
    pub frame: f32,
    pub animtime: f32,
    pub framerate: f32,
    pub body: i32,
    pub skin: i32,
    pub mode: EntRenderMode,
    pub blend: f32,
    pub attached: Option<ModelHandle>,
}

/// A sprite entity ready to draw.
#[derive(Debug, Clone)]
pub struct SpriteDraw {
    pub model: ModelHandle,
    pub origin: Vec3,
    pub angles: Vec3,
    pub frame: f32,
    pub scale: f32,
    pub mode: EntRenderMode,
    pub blend: f32,
    pub renderfx: u8,
    pub color: [f32; 3],
    pub distance_sq: f32,
    pub rank: u8,
}

/// Draw lists of one frame. `solid_*` are in submission order; translucent and sprite lists are
/// sorted far to near.
#[derive(Debug, Clone, Default)]
pub struct DrawLists {
    pub solid_brush: Vec<BrushDraw>,
    pub trans_brush: Vec<BrushDraw>,
    pub studio: Vec<StudioDraw>,
    pub sprites: Vec<SpriteDraw>,
}

/// `rendercolor` as 0..1; the engine ignores an all-zero color ("a stupid Valve Hammer Editor
/// bug", `R_DrawSpriteModel`) and uses white.
pub fn render_color(c: [u8; 3]) -> [f32; 3] {
    if c == [0, 0, 0] {
        [1.0; 3]
    } else {
        [
            f32::from(c[0]) / 255.0,
            f32::from(c[1]) / 255.0,
            f32::from(c[2]) / 255.0,
        ]
    }
}

/// Split the frame's entities into draw lists (`R_AddEntity` + `R_DrawEntitiesOnList`,
/// `ref/gl/gl_rmain.c:236-290, 819-935`, read 2026-11-23): drop `EF_NODRAW` and non-`Normal`
/// entities whose fx blend is zero, put `R_OpaqueEntity` ones in the solid pass and the rest in
/// the translucent pass, sorted by `R_TransEntityCompare`.
pub fn build_draw_lists(scene: &SceneFrame, registry: &ModelRegistry) -> DrawLists {
    let mut lists = DrawLists::default();
    for e in &scene.entities {
        if e.effects & EF_NODRAW != 0 {
            continue;
        }
        let Some(entry) = registry.get(e.model) else {
            continue;
        };
        // R_AddEntity: `!R_ModelOpaque(rendermode) && CL_FxBlend(e) <= 0` is invisible. The
        // translucent pass then uses `CL_FxBlend / 255` for every non-Normal mode and 1 for
        // Normal ("draw as solid but sorted by distance").
        let (blend, amt) = if e.rendermode.is_opaque() {
            (1.0, 1.0)
        } else {
            let b = fx_blend(e, scene.time, &scene.view);
            if b <= 0 {
                continue;
            }
            (b.clamp(0, 255) as f32 / 255.0, f32::from(e.renderamt) / 255.0)
        };
        let solid = is_opaque_entity(e.rendermode, e.renderfx);
        let rank = render_mode_rank(e.rendermode);
        // R_TransEntityCompare: distance to origin + model centre; TransAlpha brushes sort at
        // 1e9 so they draw first among the translucent ones.
        let distance_sq = if matches!(entry.kind, ModelKind::Brush(_))
            && e.rendermode == EntRenderMode::TransAlpha
        {
            1_000_000_000.0
        } else {
            (scene.view.origin - (e.origin + entry.center)).length_squared()
        };
        match entry.kind {
            ModelKind::Brush(n) => {
                let draw = BrushDraw {
                    submodel: n,
                    matrix: brush_matrix(e.origin, e.angles),
                    mode: e.rendermode,
                    blend,
                    amt,
                    color: render_color(e.rendercolor),
                    solid,
                    distance_sq,
                    rank,
                };
                if solid {
                    lists.solid_brush.push(draw);
                } else {
                    lists.trans_brush.push(draw);
                }
            }
            ModelKind::Studio => lists.studio.push(StudioDraw {
                model: e.model,
                matrix: studio_matrix(e.origin, e.angles, e.scale, e.player),
                sequence: e.sequence,
                frame: e.frame,
                animtime: e.animtime,
                framerate: e.framerate,
                body: e.body,
                skin: e.skin,
                mode: e.rendermode,
                blend,
                attached: e.attached,
            }),
            ModelKind::Sprite => lists.sprites.push(SpriteDraw {
                model: e.model,
                origin: e.origin,
                angles: e.angles,
                frame: e.frame,
                scale: if e.scale > 0.0 { e.scale } else { 1.0 },
                mode: e.rendermode,
                blend,
                renderfx: e.renderfx,
                color: render_color(e.rendercolor),
                distance_sq,
                rank,
            }),
            ModelKind::World | ModelKind::Unknown => {}
        }
    }
    // Far first, then by render mode rank.
    lists.trans_brush.sort_by(|a, b| {
        b.distance_sq
            .total_cmp(&a.distance_sq)
            .then(a.rank.cmp(&b.rank))
    });
    lists.sprites.sort_by(|a, b| {
        b.distance_sq
            .total_cmp(&a.distance_sq)
            .then(a.rank.cmp(&b.rank))
    });
    lists
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ModelRegistry {
        ModelRegistry::from_precache(&[
            "",
            "maps/de_dust2.bsp",
            "*1",
            "*12",
            "models/w_ak47.mdl",
            "sprites/smokepuff.spr",
            "weird.txt",
            "",
            "*x",
        ])
    }

    fn view() -> ViewBasis {
        ViewBasis::from_angles(Vec3::ZERO, Vec3::ZERO)
    }

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-4
    }

    #[test]
    fn brush_index_parsing() {
        assert_eq!(parse_brush_index("*1"), Some(1));
        assert_eq!(parse_brush_index("*123"), Some(123));
        assert_eq!(parse_brush_index("*"), None);
        assert_eq!(parse_brush_index("*x"), None);
        assert_eq!(parse_brush_index("*-1"), None);
        assert_eq!(parse_brush_index("1"), None);
        assert_eq!(parse_brush_index("models/*1.mdl"), None);
    }

    #[test]
    fn precache_list_maps_to_kinds() {
        let r = registry();
        let kind = |i: i32| r.resolve(i).and_then(|h| r.get(h)).map(|e| e.kind);
        assert_eq!(kind(0), None);
        assert_eq!(kind(1), Some(ModelKind::World));
        assert_eq!(kind(2), Some(ModelKind::Brush(1)));
        assert_eq!(kind(3), Some(ModelKind::Brush(12)));
        assert_eq!(kind(4), Some(ModelKind::Studio));
        assert_eq!(kind(5), Some(ModelKind::Sprite));
        assert_eq!(kind(6), Some(ModelKind::Unknown));
        assert_eq!(kind(7), None, "empty name is not registered");
        assert_eq!(kind(8), Some(ModelKind::Unknown), "bad *N is not a brush");
        assert_eq!(kind(9), None, "past the table");
        assert_eq!(kind(-1), None);
        assert_eq!(kind(70000), None);
        assert_eq!(classify_model("Models/W_AK47.MDL"), ModelKind::Studio);
    }

    #[test]
    fn angle_vectors_axes() {
        let (f, r, u) = angle_vectors(Vec3::new(0.0, 90.0, 0.0));
        assert!(approx(f, Vec3::Y));
        assert!(approx(r, Vec3::X));
        assert!(approx(u, Vec3::Z));
        let (f, _, u) = angle_vectors(Vec3::new(90.0, 0.0, 0.0));
        assert!(approx(f, -Vec3::Z), "positive pitch looks down");
        assert!(approx(u, Vec3::X));
    }

    #[test]
    fn brush_transform_translation_and_yaw() {
        let m = brush_matrix(Vec3::new(10.0, 20.0, 30.0), Vec3::ZERO);
        assert!(approx(
            m.transform_point3(Vec3::new(1.0, 2.0, 3.0)),
            Vec3::new(11.0, 22.0, 33.0)
        ));
        // yaw 90: model +X maps to world +Y, model +Y to world -X.
        let m = brush_matrix(Vec3::new(100.0, 0.0, 0.0), Vec3::new(0.0, 90.0, 0.0));
        assert!(approx(
            m.transform_point3(Vec3::X),
            Vec3::new(100.0, 1.0, 0.0)
        ));
        assert!(approx(
            m.transform_point3(Vec3::Y),
            Vec3::new(99.0, 0.0, 0.0)
        ));
        // Rotation keeps lengths and handedness.
        assert!((m.determinant() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn studio_matrix_scale_and_pitch_sign() {
        let m = studio_matrix(Vec3::ZERO, Vec3::ZERO, 2.0, false);
        assert!(approx(m.transform_vector3(Vec3::X), Vec3::new(2.0, 0.0, 0.0)));
        let m0 = studio_matrix(Vec3::ZERO, Vec3::ZERO, 0.0, false);
        assert!(approx(m0.transform_vector3(Vec3::X), Vec3::X), "scale 0 means 1");
        let m = studio_matrix(Vec3::ZERO, Vec3::new(30.0, 0.0, 0.0), 1.0, false);
        assert!(m.transform_vector3(Vec3::X).z > 0.0, "negated pitch lifts the nose");
        // gl_studio.c:543: players only aim, the model is not pitched.
        let m = studio_matrix(Vec3::ZERO, Vec3::new(30.0, 0.0, 0.0), 1.0, true);
        assert!(approx(m.transform_vector3(Vec3::X), Vec3::X));
    }

    #[test]
    fn fx_blend_pulse_constants_per_id() {
        // const.h:699-703 and gl_rmain.c:1228-1237: Wide = 0x40, plain = 0x10; Slow = x2, Fast = x8.
        let r = registry();
        let mut e = RenderEntity::new(1, r.resolve(5).unwrap(), Vec3::ZERO);
        e.renderamt = 100;
        let t = 1.5f64;
        let off = 363.0f64;
        for (id, swing, speed) in [
            (RENDERFX_PULSE_SLOW, 16.0, 2.0),
            (RENDERFX_PULSE_FAST, 16.0, 8.0),
            (RENDERFX_PULSE_SLOW_WIDE, 64.0, 2.0),
            (RENDERFX_PULSE_FAST_WIDE, 64.0, 8.0),
        ] {
            e.renderfx = id;
            let want = (100.0 + swing * (t * speed + off).sin()) as i32;
            assert_eq!(fx_blend(&e, t as f32, &view()), want, "renderfx {id}");
        }
        e.renderfx = RENDERFX_NONE;
        assert_eq!(fx_blend(&e, 3.0, &view()), 100);
    }

    #[test]
    fn fx_blend_distort_and_hologram() {
        let r = registry();
        let mut e = RenderEntity::new(1, r.resolve(5).unwrap(), Vec3::new(300.0, 0.0, 0.0));
        e.renderamt = 100;
        // const.h:714-715: 15 = Distort (no distance fade), 16 = Hologram (distance fade).
        e.renderfx = RENDERFX_DISTORT;
        assert_eq!(fx_blend(&e, 0.0, &view()), 180);
        e.renderfx = RENDERFX_HOLOGRAM;
        assert_eq!(fx_blend(&e, 0.0, &view()), 90);
        e.origin = Vec3::new(50.0, 0.0, 0.0);
        assert_eq!(fx_blend(&e, 0.0, &view()), 180);
        e.origin = Vec3::new(-10.0, 0.0, 0.0);
        assert_eq!(fx_blend(&e, 0.0, &view()), 0);
        e.renderfx = RENDERFX_DISTORT;
        assert_eq!(fx_blend(&e, 0.0, &view()), 180, "distort ignores the view side");
    }

    #[test]
    fn opaque_entity_rules() {
        use EntRenderMode::*;
        assert!(is_opaque_entity(Normal, RENDERFX_NONE));
        assert!(is_opaque_entity(Normal, RENDERFX_DEAD_PLAYER));
        assert!(is_opaque_entity(Normal, RENDERFX_EXPLODE));
        assert!(is_opaque_entity(Normal, RENDERFX_LIGHT_MULTIPLIER));
        assert!(!is_opaque_entity(Normal, RENDERFX_PULSE_SLOW));
        assert!(!is_opaque_entity(Normal, RENDERFX_HOLOGRAM));
        assert!(!is_opaque_entity(TransAlpha, RENDERFX_NONE), "gl_local.h:76");
        assert!(!is_opaque_entity(TransAdd, RENDERFX_NONE));
    }

    #[test]
    fn precache_list_is_capped_at_u16() {
        let names: Vec<String> = (0..70_000).map(|i| format!("models/m{i}.mdl")).collect();
        let r = ModelRegistry::from_precache(&names);
        assert_eq!(r.len(), 65_536);
        assert!(r.resolve(65_535).is_some());
        assert!(r.resolve(65_536).is_none());
    }

    #[test]
    fn glow_falloff() {
        assert_eq!(glow_blend(500.0, RENDERFX_NO_DISSIPATION), (1.0, 1.0));
        let (b, s) = glow_blend(100.0, 0);
        assert_eq!(b, 1.0);
        assert!((s - 0.5).abs() < 1e-6);
        let (b, _) = glow_blend(10_000.0, 0);
        assert_eq!(b, 0.05);
        let (b, _) = glow_blend(200.0, 0);
        assert!((b - 0.475).abs() < 1e-6);
    }

    #[test]
    fn sprite_parallel_uses_view_axes() {
        let v = ViewBasis::from_angles(Vec3::ZERO, Vec3::new(0.0, 90.0, 0.0));
        let a = sprite_axes(SpriteType::Parallel, Vec3::new(5.0, 5.0, 5.0), Vec3::ZERO, &v).unwrap();
        assert!(approx(a.right, v.right) && approx(a.up, v.up));
        // A roll angle turns a parallel sprite into a rolled one.
        let a = sprite_axes(
            SpriteType::Parallel,
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 90.0),
            &v,
        )
        .unwrap();
        assert!(approx(a.right, v.up));
        assert!(approx(a.up, -v.right));
    }

    #[test]
    fn sprite_facing_upright_turns_to_the_viewer() {
        let v = view();
        let a = sprite_axes(
            SpriteType::FacingUpright,
            Vec3::new(100.0, 0.0, 10.0),
            Vec3::ZERO,
            &v,
        )
        .unwrap();
        assert!(approx(a.up, Vec3::Z));
        assert!(approx(a.right, Vec3::new(0.0, -1.0, 0.0)));
        assert!(a.right.dot(a.up).abs() < 1e-6);
        // Directly above the viewer: degenerate, no NaN.
        let a = sprite_axes(SpriteType::FacingUpright, Vec3::new(0.0, 0.0, 50.0), Vec3::ZERO, &v)
            .unwrap();
        assert_eq!(a.right, Vec3::ZERO);
    }

    #[test]
    fn sprite_parallel_upright_hides_looking_straight_up() {
        let flat = view();
        let a = sprite_axes(SpriteType::ParallelUpright, Vec3::ZERO, Vec3::ZERO, &flat).unwrap();
        assert!(approx(a.right, Vec3::new(0.0, -1.0, 0.0)));
        assert!(approx(a.up, Vec3::Z));
        let down = ViewBasis::from_angles(Vec3::ZERO, Vec3::new(89.5, 0.0, 0.0));
        assert!(sprite_axes(SpriteType::ParallelUpright, Vec3::ZERO, Vec3::ZERO, &down).is_none());
        let tilted = ViewBasis::from_angles(Vec3::ZERO, Vec3::new(60.0, 0.0, 0.0));
        assert!(sprite_axes(SpriteType::ParallelUpright, Vec3::ZERO, Vec3::ZERO, &tilted).is_some());
    }

    #[test]
    fn sprite_oriented_uses_entity_angles_and_backs_off() {
        let a = sprite_axes(
            SpriteType::Oriented,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 90.0, 0.0),
            &view(),
        )
        .unwrap();
        assert!(approx(a.right, Vec3::X));
        assert!(approx(a.up, Vec3::Z));
        assert!(approx(a.origin, Vec3::new(10.0, -0.01, 0.0)));
    }

    #[test]
    fn sprite_quad_corner_order_and_scale() {
        let axes = SpriteAxes {
            origin: Vec3::new(0.0, 0.0, 100.0),
            right: Vec3::X,
            up: Vec3::Z,
        };
        // 16x8 frame centred: left -8, right 8, up 4, down -4.
        let q = sprite_quad(&axes, -8.0, 8.0, 4.0, -4.0, 2.0);
        assert!(approx(q[0].0, Vec3::new(-16.0, 0.0, 92.0)));
        assert!(approx(q[1].0, Vec3::new(-16.0, 0.0, 108.0)));
        assert!(approx(q[2].0, Vec3::new(16.0, 0.0, 108.0)));
        assert!(approx(q[3].0, Vec3::new(16.0, 0.0, 92.0)));
        assert_eq!(q[0].1, [0.0, 1.0]);
        assert_eq!(q[2].1, [1.0, 0.0]);
    }

    #[test]
    fn headless_scene_build_sorts_and_filters() {
        let r = registry();
        let h = |i| r.resolve(i).unwrap();
        let mut door = RenderEntity::new(1, h(2), Vec3::new(10.0, 0.0, 0.0));
        door.angles = Vec3::new(0.0, 90.0, 0.0);
        let mut glass_far = RenderEntity::new(2, h(3), Vec3::new(500.0, 0.0, 0.0));
        glass_far.rendermode = EntRenderMode::TransTexture;
        glass_far.renderamt = 128;
        let mut glass_near = glass_far.clone();
        glass_near.index = 3;
        glass_near.origin = Vec3::new(100.0, 0.0, 0.0);
        let mut hidden = glass_far.clone();
        hidden.index = 4;
        hidden.renderamt = 0;
        let mut nodraw = door.clone();
        nodraw.index = 5;
        nodraw.effects = EF_NODRAW;
        let gun = RenderEntity::new(6, h(4), Vec3::new(1.0, 2.0, 3.0));
        let mut puff_far = RenderEntity::new(7, h(5), Vec3::new(400.0, 0.0, 0.0));
        puff_far.rendermode = EntRenderMode::TransAdd;
        puff_far.renderamt = 200;
        let mut puff_near = puff_far.clone();
        puff_near.index = 8;
        puff_near.origin = Vec3::new(50.0, 0.0, 0.0);
        puff_near.scale = 0.5;
        let mut alpha_grate = RenderEntity::new(12, h(2), Vec3::new(1.0, 0.0, 0.0));
        alpha_grate.rendermode = EntRenderMode::TransAlpha;
        alpha_grate.renderamt = 255;
        let mut pulsing = RenderEntity::new(13, h(4), Vec3::new(2.0, 0.0, 0.0));
        pulsing.renderfx = RENDERFX_PULSE_SLOW;
        let world = RenderEntity::new(9, h(1), Vec3::ZERO);
        let unknown = RenderEntity::new(10, h(6), Vec3::ZERO);
        let missing = RenderEntity::new(11, ModelHandle(40), Vec3::ZERO);

        let scene = SceneFrame {
            time: 1.0,
            view: view(),
            entities: vec![
                glass_far, door, glass_near, hidden, nodraw, gun, puff_far, puff_near, world,
                unknown, missing, alpha_grate, pulsing,
            ],
        };
        let l = build_draw_lists(&scene, &r);
        assert_eq!(l.solid_brush.len(), 1);
        assert_eq!(l.solid_brush[0].submodel, 1);
        assert_eq!(l.solid_brush[0].blend, 1.0);
        assert_eq!(l.trans_brush.len(), 3, "renderamt 0 and nodraw are dropped");
        // TransAlpha brushes sort at 1e9: first among the translucent ones (gl_rmain.c:96-104).
        assert_eq!(l.trans_brush[0].mode, EntRenderMode::TransAlpha);
        assert!(!l.trans_brush[0].solid);
        assert_eq!(l.trans_brush[0].distance_sq, 1_000_000_000.0);
        assert!(l.trans_brush[1].distance_sq > l.trans_brush[2].distance_sq);
        assert_eq!(l.trans_brush[1].submodel, 12);
        assert!((l.trans_brush[1].blend - 128.0 / 255.0).abs() < 1e-6);
        assert!((l.trans_brush[1].amt - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(l.studio.len(), 2, "a Normal entity with a pulse fx is still drawn");
        assert_eq!(l.sprites.len(), 2);
        assert!(l.sprites[0].distance_sq > l.sprites[1].distance_sq);
        assert_eq!(l.sprites[0].scale, 1.0, "scale 0 means 1");
        assert_eq!(l.sprites[1].scale, 0.5);
        assert_eq!(l.sprites[0].color, [1.0; 3], "all-zero rendercolor is white");
        assert!(l.solid_brush[0].solid);
        // The door's yaw rotated its matrix.
        assert!(approx(
            l.solid_brush[0].matrix.transform_vector3(Vec3::X),
            Vec3::Y
        ));
    }

    #[test]
    fn translucent_sort_uses_the_model_centre() {
        let mut r = registry();
        // *1 is centred 1000 units along +X, *12 sits on the origin.
        let mut bounds = vec![(Vec3::ZERO, Vec3::ZERO); 13];
        bounds[1] = (Vec3::new(900.0, -10.0, -10.0), Vec3::new(1100.0, 10.0, 10.0));
        r.set_brush_bounds(&bounds);
        let h = |i| r.resolve(i).unwrap();
        let mut a = RenderEntity::new(1, h(2), Vec3::ZERO); // centre at x=1000
        a.rendermode = EntRenderMode::TransTexture;
        a.renderamt = 100;
        let mut b = RenderEntity::new(2, h(3), Vec3::new(500.0, 0.0, 0.0)); // centre at x=500
        b.rendermode = EntRenderMode::TransTexture;
        b.renderamt = 100;
        let scene = SceneFrame {
            time: 0.0,
            view: view(),
            entities: vec![b, a],
        };
        let l = build_draw_lists(&scene, &r);
        assert_eq!(l.trans_brush[0].submodel, 1, "far centre first");
        assert_eq!(l.trans_brush[0].distance_sq, 1_000_000.0);
    }
}
