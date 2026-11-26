//! Bit-exact GoldSrc Delta Compression engine (`ReHLDS/engine/delta.cpp` + `cstrike/delta.lst`).
//!
//! Supports:
//! - Parsing real Valve `cstrike/delta.lst` / `valve/delta.lst` schemas.
//! - Encoding/decoding `svc_deltadescription` (opcode 14) via `g_MetaDelta`.
//! - Bit-exact `write_delta` / `read_delta` for all 8 `DT_*` field types (`DT_BYTE`,
//!   `DT_SHORT`, `DT_FLOAT`, `DT_INTEGER`, `DT_ANGLE`, `DT_TIMEWINDOW_8`,
//!   `DT_TIMEWINDOW_BIG`, `DT_STRING`, plus `DT_SIGNED`).
//! - Typed struct conversions for `clientdata_t`, `entity_state_t`, `entity_state_player_t`,
//!   `custom_entity_state_t`, `usercmd_t`, `weapon_data_t`, and `event_t`.

use std::collections::HashMap;
use std::sync::OnceLock;

use glam::Vec3;

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::delta_offsets::field_layout;

pub const DELTA_MAX_FIELDS: usize = 56;

pub const DT_BYTE: u32 = 1 << 0;
pub const DT_SHORT: u32 = 1 << 1;
pub const DT_FLOAT: u32 = 1 << 2;
pub const DT_INTEGER: u32 = 1 << 3;
pub const DT_ANGLE: u32 = 1 << 4;
pub const DT_TIMEWINDOW_8: u32 = 1 << 5;
pub const DT_TIMEWINDOW_BIG: u32 = 1 << 6;
pub const DT_STRING: u32 = 1 << 7;
pub const DT_SIGNED: u32 = 1 << 31;

/// Single field entry inside a GoldSrc `delta_t` schema (`delta_description_t`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaFieldDesc {
    pub field_type: u32,
    pub name: String,
    pub field_offset: u16,
    pub field_size: u8,
    pub significant_bits: u8,
    pub premultiply: f32,
    pub postmultiply: f32,
}

impl DeltaFieldDesc {
    #[inline]
    pub fn base_type(&self) -> u32 {
        self.field_type & !DT_SIGNED
    }

    #[inline]
    pub fn is_signed(&self) -> bool {
        (self.field_type & DT_SIGNED) != 0
    }
}

/// Complete delta schema for one GoldSrc wire struct (e.g. `entity_state_t`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaSchema {
    pub name: String,
    pub encoder: Option<String>,
    pub fields: Vec<DeltaFieldDesc>,
    /// Name -> index of the first field of that name, built on the first lookup. It is a cache
    /// of `fields` (equality and `Clone` ignore it), so `fields` must not change after a lookup.
    index: FieldIndexCache,
    /// Lazily compiled per-slot field accessor plans for direct struct-to-bitstream codecs.
    plans: CompiledPlanCache,
}

/// The lazily built name -> field index map of a [`DeltaSchema`]: a lookup was a linear scan of
/// string compares, run about 55 times per entity state decoded.
#[derive(Debug, Default)]
struct FieldIndexCache(OnceLock<HashMap<String, usize>>);

impl Clone for FieldIndexCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for FieldIndexCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// Precompiled per-slot metadata for one schema field inside a [`CompiledPlan`].
#[derive(Debug, Clone, Copy)]
struct CompiledSlot<F: Copy> {
    field_id: F,
    base_type: u32,
    signed: bool,
    bits: u32,
    pre: f64,
    post: f64,
    pre_is_unit: bool,
    post_is_unit: bool,
}

/// Precompiled slot-to-field accessor plan for a [`DeltaSchema`] paired with struct type `T`.
///
/// Built at most once per [`DeltaSchema`] via [`OnceLock`], mapping schema slot `0..len`
/// (`<= 64`) directly to a compact `#[repr(u8)] Copy` field identifier so struct-to-bitstream
/// delta encoding and decoding perform zero `HashMap` lookups and zero heap allocations.
#[derive(Debug, Clone)]
pub struct CompiledPlan<F: Copy> {
    slots: [CompiledSlot<F>; 64],
    len: usize,
    valid_mask: u64,
    origin_mask: u64,
    angles_mask: u64,
    skin_seq_mask: u64,
    animtime_mask: u64,
}

impl<F: Copy + Eq> CompiledPlan<F> {
    fn compile(
        fields: &[DeltaFieldDesc],
        unknown: F,
        map_name: impl Fn(&str) -> F,
        tag_masks: impl Fn(F, u64, &mut u64, &mut u64, &mut u64, &mut u64),
    ) -> Self {
        let dummy = CompiledSlot {
            field_id: unknown,
            base_type: 0,
            signed: false,
            bits: 0,
            pre: 1.0,
            post: 1.0,
            pre_is_unit: true,
            post_is_unit: true,
        };
        let mut slots = [dummy; 64];
        let len = fields.len().min(64);
        let valid_mask = if len >= 64 {
            u64::MAX
        } else {
            (1u64 << len) - 1
        };
        let mut origin_mask = 0u64;
        let mut angles_mask = 0u64;
        let mut skin_seq_mask = 0u64;
        let mut animtime_mask = 0u64;

        for (i, field) in fields.iter().enumerate().take(len) {
            // Preserve `DeltaSchema::field_index` first-occurrence semantics on duplicate names.
            let is_dup = fields[..i].iter().any(|prev| prev.name == field.name);
            let field_id = if is_dup {
                unknown
            } else {
                map_name(&field.name)
            };
            let pre = field.premultiply as f64;
            let post = field.postmultiply as f64;
            slots[i] = CompiledSlot {
                field_id,
                base_type: field.base_type(),
                signed: field.is_signed(),
                bits: field.significant_bits as u32,
                pre,
                post,
                pre_is_unit: (0.9999..=1.0001).contains(&pre),
                post_is_unit: (0.9999..=1.0001).contains(&post),
            };
            if field_id != unknown {
                tag_masks(
                    field_id,
                    1u64 << i,
                    &mut origin_mask,
                    &mut angles_mask,
                    &mut skin_seq_mask,
                    &mut animtime_mask,
                );
            }
        }

        Self {
            slots,
            len,
            valid_mask,
            origin_mask,
            angles_mask,
            skin_seq_mask,
            animtime_mask,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn valid_mask(&self) -> u64 {
        self.valid_mask
    }
}

#[derive(Debug, Default)]
struct CompiledPlanCache {
    entity: OnceLock<Box<CompiledPlan<EntityFieldId>>>,
    clientdata: OnceLock<Box<CompiledPlan<ClientDataFieldId>>>,
    weapondata: OnceLock<Box<CompiledPlan<WeaponDataFieldId>>>,
    usercmd: OnceLock<Box<CompiledPlan<UserCmdFieldId>>>,
}

impl Clone for CompiledPlanCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for CompiledPlanCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl DeltaSchema {
    /// A schema of `fields`.
    pub fn new(name: String, encoder: Option<String>, fields: Vec<DeltaFieldDesc>) -> Self {
        Self {
            name,
            encoder,
            fields,
            index: FieldIndexCache::default(),
            plans: CompiledPlanCache::default(),
        }
    }

    /// The index of the first field called `name`.
    pub fn field_index(&self, name: &str) -> Option<usize> {
        let map = self.index.0.get_or_init(|| {
            let mut map = HashMap::with_capacity(self.fields.len());
            for (i, f) in self.fields.iter().enumerate() {
                map.entry(f.name.clone()).or_insert(i);
            }
            map
        });
        map.get(name).copied()
    }

    /// Precompiled field accessor plan for [`GoldSrcEntityState`] (`entity_state_t`,
    /// `entity_state_player_t`, `custom_entity_state_t`).
    pub fn entity_plan(&self) -> &CompiledPlan<EntityFieldId> {
        self.plans.entity.get_or_init(|| {
            Box::new(CompiledPlan::compile(
                &self.fields,
                EntityFieldId::Unknown,
                EntityFieldId::from_name,
                |id, bit, origin, angles, skin_seq, animtime| match id {
                    EntityFieldId::Origin0 | EntityFieldId::Origin1 | EntityFieldId::Origin2 => {
                        *origin |= bit;
                    }
                    EntityFieldId::Angles0 | EntityFieldId::Angles1 | EntityFieldId::Angles2 => {
                        *angles |= bit;
                    }
                    EntityFieldId::Skin | EntityFieldId::Sequence => {
                        *skin_seq |= bit;
                    }
                    EntityFieldId::AnimTime => {
                        *animtime |= bit;
                    }
                    _ => {}
                },
            ))
        })
    }

    /// Precompiled field accessor plan for [`GoldSrcClientData`] (`clientdata_t`).
    pub fn clientdata_plan(&self) -> &CompiledPlan<ClientDataFieldId> {
        self.plans.clientdata.get_or_init(|| {
            Box::new(CompiledPlan::compile(
                &self.fields,
                ClientDataFieldId::Unknown,
                ClientDataFieldId::from_name,
                |_, _, _, _, _, _| {},
            ))
        })
    }

    /// Precompiled field accessor plan for [`GoldSrcWeaponData`] (`weapon_data_t`).
    pub fn weapondata_plan(&self) -> &CompiledPlan<WeaponDataFieldId> {
        self.plans.weapondata.get_or_init(|| {
            Box::new(CompiledPlan::compile(
                &self.fields,
                WeaponDataFieldId::Unknown,
                WeaponDataFieldId::from_name,
                |_, _, _, _, _, _| {},
            ))
        })
    }

    /// Precompiled field accessor plan for [`GoldSrcUserCmd`] (`usercmd_t`).
    pub fn usercmd_plan(&self) -> &CompiledPlan<UserCmdFieldId> {
        self.plans.usercmd.get_or_init(|| {
            Box::new(CompiledPlan::compile(
                &self.fields,
                UserCmdFieldId::Unknown,
                UserCmdFieldId::from_name,
                |_, _, _, _, _, _| {},
            ))
        })
    }

    pub fn zero_record(&self) -> DeltaRecord {
        let values = self
            .fields
            .iter()
            .map(|f| match f.base_type() {
                DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG => DeltaValue::Float(0.0),
                DT_STRING => DeltaValue::Str(String::new()),
                _ if f.is_signed() => DeltaValue::Int(0),
                _ => DeltaValue::Uint(0),
            })
            .collect();
        DeltaRecord { values }
    }
}

/// Compact field identifier for [`GoldSrcEntityState`] schemas (`entity_state_t`,
/// `entity_state_player_t`, `custom_entity_state_t`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityFieldId {
    Unknown = 0,
    AnimTime,
    Frame,
    Origin0,
    Origin1,
    Origin2,
    Angles0,
    Angles1,
    Angles2,
    Sequence,
    GaitSequence,
    ModelIndex,
    MoveType,
    Solid,
    Mins0,
    Mins1,
    Mins2,
    Maxs0,
    Maxs1,
    Maxs2,
    WeaponModel,
    Owner,
    Effects,
    ColorMap,
    FrameRate,
    Skin,
    Body,
    RenderMode,
    RenderAmt,
    RenderFx,
    Scale,
    Aiment,
    UseHull,
    Spectator,
    EFlags,
    RenderColorR,
    RenderColorG,
    RenderColorB,
    Controller0,
    Controller1,
    Controller2,
    Controller3,
    Blending0,
    Blending1,
    Friction,
    Gravity,
    BaseVelocity0,
    BaseVelocity1,
    BaseVelocity2,
    StartPos0,
    StartPos1,
    StartPos2,
    EndPos0,
    EndPos1,
    EndPos2,
    ImpactTime,
    StartTime,
    IUser4,
}

impl EntityFieldId {
    pub fn from_name(name: &str) -> Self {
        match name {
            "animtime" => Self::AnimTime,
            "frame" => Self::Frame,
            "origin[0]" => Self::Origin0,
            "origin[1]" => Self::Origin1,
            "origin[2]" => Self::Origin2,
            "angles[0]" => Self::Angles0,
            "angles[1]" => Self::Angles1,
            "angles[2]" => Self::Angles2,
            "sequence" => Self::Sequence,
            "gaitsequence" => Self::GaitSequence,
            "modelindex" => Self::ModelIndex,
            "movetype" => Self::MoveType,
            "solid" => Self::Solid,
            "mins[0]" => Self::Mins0,
            "mins[1]" => Self::Mins1,
            "mins[2]" => Self::Mins2,
            "maxs[0]" => Self::Maxs0,
            "maxs[1]" => Self::Maxs1,
            "maxs[2]" => Self::Maxs2,
            "weaponmodel" => Self::WeaponModel,
            "owner" => Self::Owner,
            "effects" => Self::Effects,
            "colormap" => Self::ColorMap,
            "framerate" => Self::FrameRate,
            "skin" => Self::Skin,
            "body" => Self::Body,
            "rendermode" => Self::RenderMode,
            "renderamt" => Self::RenderAmt,
            "renderfx" => Self::RenderFx,
            "scale" => Self::Scale,
            "aiment" => Self::Aiment,
            "usehull" => Self::UseHull,
            "spectator" => Self::Spectator,
            "eflags" => Self::EFlags,
            "rendercolor.r" => Self::RenderColorR,
            "rendercolor.g" => Self::RenderColorG,
            "rendercolor.b" => Self::RenderColorB,
            "controller[0]" => Self::Controller0,
            "controller[1]" => Self::Controller1,
            "controller[2]" => Self::Controller2,
            "controller[3]" => Self::Controller3,
            "blending[0]" => Self::Blending0,
            "blending[1]" => Self::Blending1,
            "friction" => Self::Friction,
            "gravity" => Self::Gravity,
            "basevelocity[0]" => Self::BaseVelocity0,
            "basevelocity[1]" => Self::BaseVelocity1,
            "basevelocity[2]" => Self::BaseVelocity2,
            "startpos[0]" => Self::StartPos0,
            "startpos[1]" => Self::StartPos1,
            "startpos[2]" => Self::StartPos2,
            "endpos[0]" => Self::EndPos0,
            "endpos[1]" => Self::EndPos1,
            "endpos[2]" => Self::EndPos2,
            "impacttime" => Self::ImpactTime,
            "starttime" => Self::StartTime,
            "iuser4" => Self::IUser4,
            _ => Self::Unknown,
        }
    }
}

/// Compact field identifier for [`GoldSrcClientData`] (`clientdata_t`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientDataFieldId {
    Unknown = 0,
    Origin0,
    Origin1,
    Origin2,
    Velocity0,
    Velocity1,
    Velocity2,
    ViewModel,
    PunchAngle0,
    PunchAngle1,
    PunchAngle2,
    Flags,
    WaterLevel,
    ViewOfs2,
    Health,
    BInDuck,
    Weapons,
    AmmoNails,
    AmmoShells,
    AmmoCells,
    AmmoRockets,
    FlTimeStepSound,
    FlDuckTime,
    MaxSpeed,
    MId,
    MFlNextAttack,
    Fov,
    WeaponAnim,
    DeadFlag,
    IUser1,
    IUser2,
    IUser3,
    IUser4,
    PhysInfo,
    FlSwimTime,
    WaterJumpTime,
    PushMsec,
    FUser1,
    FUser2,
    FUser3,
    VUser2_0,
    VUser2_1,
    VUser2_2,
    VUser3_0,
    VUser3_1,
    VUser3_2,
    VUser4_0,
    VUser4_1,
}

impl ClientDataFieldId {
    pub fn from_name(name: &str) -> Self {
        match name {
            "origin[0]" => Self::Origin0,
            "origin[1]" => Self::Origin1,
            "origin[2]" => Self::Origin2,
            "velocity[0]" => Self::Velocity0,
            "velocity[1]" => Self::Velocity1,
            "velocity[2]" => Self::Velocity2,
            "viewmodel" => Self::ViewModel,
            "punchangle[0]" => Self::PunchAngle0,
            "punchangle[1]" => Self::PunchAngle1,
            "punchangle[2]" => Self::PunchAngle2,
            "flags" => Self::Flags,
            "waterlevel" => Self::WaterLevel,
            "view_ofs[2]" => Self::ViewOfs2,
            "health" => Self::Health,
            "bInDuck" => Self::BInDuck,
            "weapons" => Self::Weapons,
            "ammo_nails" => Self::AmmoNails,
            "ammo_shells" => Self::AmmoShells,
            "ammo_cells" => Self::AmmoCells,
            "ammo_rockets" => Self::AmmoRockets,
            "flTimeStepSound" => Self::FlTimeStepSound,
            "flDuckTime" => Self::FlDuckTime,
            "maxspeed" => Self::MaxSpeed,
            "m_iId" => Self::MId,
            "m_flNextAttack" => Self::MFlNextAttack,
            "fov" => Self::Fov,
            "weaponanim" => Self::WeaponAnim,
            "deadflag" => Self::DeadFlag,
            "iuser1" => Self::IUser1,
            "iuser2" => Self::IUser2,
            "iuser3" => Self::IUser3,
            "iuser4" => Self::IUser4,
            "physinfo" => Self::PhysInfo,
            "flSwimTime" => Self::FlSwimTime,
            "waterjumptime" => Self::WaterJumpTime,
            "pushmsec" => Self::PushMsec,
            "fuser1" => Self::FUser1,
            "fuser2" => Self::FUser2,
            "fuser3" => Self::FUser3,
            "vuser2[0]" => Self::VUser2_0,
            "vuser2[1]" => Self::VUser2_1,
            "vuser2[2]" => Self::VUser2_2,
            "vuser3[0]" => Self::VUser3_0,
            "vuser3[1]" => Self::VUser3_1,
            "vuser3[2]" => Self::VUser3_2,
            "vuser4[0]" => Self::VUser4_0,
            "vuser4[1]" => Self::VUser4_1,
            _ => Self::Unknown,
        }
    }
}

/// Compact field identifier for [`GoldSrcWeaponData`] (`weapon_data_t`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponDataFieldId {
    Unknown = 0,
    MId,
    MClip,
    MFlNextPrimaryAttack,
    MFlNextSecondaryAttack,
    MFlTimeWeaponIdle,
    MFInReload,
    MFInSpecialReload,
    MFlNextReload,
    MFlPumpTime,
    MFReloadTime,
    MFAimedDamage,
    MFNextAimBonus,
    MFInZoom,
    MIWeaponState,
    IUser1,
    IUser2,
    IUser3,
    IUser4,
    FUser1,
    FUser2,
    FUser3,
    FUser4,
}

impl WeaponDataFieldId {
    pub fn from_name(name: &str) -> Self {
        match name {
            "m_iId" => Self::MId,
            "m_iClip" => Self::MClip,
            "m_flNextPrimaryAttack" => Self::MFlNextPrimaryAttack,
            "m_flNextSecondaryAttack" => Self::MFlNextSecondaryAttack,
            "m_flTimeWeaponIdle" => Self::MFlTimeWeaponIdle,
            "m_fInReload" => Self::MFInReload,
            "m_fInSpecialReload" => Self::MFInSpecialReload,
            "m_flNextReload" => Self::MFlNextReload,
            "m_flPumpTime" => Self::MFlPumpTime,
            "m_fReloadTime" => Self::MFReloadTime,
            "m_fAimedDamage" => Self::MFAimedDamage,
            "m_fNextAimBonus" => Self::MFNextAimBonus,
            "m_fInZoom" => Self::MFInZoom,
            "m_iWeaponState" => Self::MIWeaponState,
            "iuser1" => Self::IUser1,
            "iuser2" => Self::IUser2,
            "iuser3" => Self::IUser3,
            "iuser4" => Self::IUser4,
            "fuser1" => Self::FUser1,
            "fuser2" => Self::FUser2,
            "fuser3" => Self::FUser3,
            "fuser4" => Self::FUser4,
            _ => Self::Unknown,
        }
    }
}

/// Compact field identifier for [`GoldSrcUserCmd`] (`usercmd_t`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCmdFieldId {
    Unknown = 0,
    LerpMsec,
    Msec,
    ViewAngles0,
    ViewAngles1,
    ViewAngles2,
    ForwardMove,
    SideMove,
    UpMove,
    LightLevel,
    Buttons,
    Impulse,
    WeaponSelect,
    ImpactIndex,
    ImpactPosition0,
    ImpactPosition1,
    ImpactPosition2,
}

impl UserCmdFieldId {
    pub fn from_name(name: &str) -> Self {
        match name {
            "lerp_msec" => Self::LerpMsec,
            "msec" => Self::Msec,
            "viewangles[0]" => Self::ViewAngles0,
            "viewangles[1]" => Self::ViewAngles1,
            "viewangles[2]" => Self::ViewAngles2,
            "forwardmove" => Self::ForwardMove,
            "sidemove" => Self::SideMove,
            "upmove" => Self::UpMove,
            "lightlevel" => Self::LightLevel,
            "buttons" => Self::Buttons,
            "impulse" => Self::Impulse,
            "weaponselect" => Self::WeaponSelect,
            "impact_index" => Self::ImpactIndex,
            "impact_position[0]" => Self::ImpactPosition0,
            "impact_position[1]" => Self::ImpactPosition1,
            "impact_position[2]" => Self::ImpactPosition2,
            _ => Self::Unknown,
        }
    }
}

/// Dynamic field value stored in a `DeltaRecord`.
#[derive(Debug, Clone, PartialEq)]
pub enum DeltaValue {
    Int(i32),
    Uint(u32),
    Float(f32),
    Str(String),
}

impl DeltaValue {
    pub fn as_f32(&self) -> f32 {
        match *self {
            Self::Float(v) => v,
            Self::Int(v) => v as f32,
            Self::Uint(v) => v as f32,
            Self::Str(_) => 0.0,
        }
    }

    pub fn as_i32(&self) -> i32 {
        match *self {
            Self::Int(v) => v,
            Self::Uint(v) => v as i32,
            Self::Float(v) => v as i32,
            Self::Str(_) => 0,
        }
    }

    pub fn as_u32(&self) -> u32 {
        match *self {
            Self::Uint(v) => v,
            Self::Int(v) => v as u32,
            Self::Float(v) => v as u32,
            Self::Str(_) => 0,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Str(s) => s.as_str(),
            _ => "",
        }
    }

    /// GoldSrc `DELTA_MarkSendFields` equality semantics (`delta.cpp:526-568`).
    fn equals_wire(&self, other: &Self, field: &DeltaFieldDesc) -> bool {
        match field.base_type() {
            DT_BYTE => (self.as_u32() as u8) == (other.as_u32() as u8),
            DT_SHORT => (self.as_u32() as u16) == (other.as_u32() as u16),
            DT_INTEGER => self.as_u32() == other.as_u32(),
            DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG => {
                self.as_f32().to_bits() == other.as_f32().to_bits()
            }
            DT_STRING => self.as_str().eq_ignore_ascii_case(other.as_str()),
            _ => true,
        }
    }
}

/// Ordered field values matching a `DeltaSchema`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DeltaRecord {
    pub values: Vec<DeltaValue>,
}

impl DeltaRecord {
    pub fn set_u32(&mut self, schema: &DeltaSchema, name: &str, v: u32) {
        if let Some(idx) = schema.field_index(name) {
            if schema.fields[idx].is_signed() {
                self.values[idx] = DeltaValue::Int(v as i32);
            } else {
                self.values[idx] = DeltaValue::Uint(v);
            }
        }
    }

    pub fn set_i32(&mut self, schema: &DeltaSchema, name: &str, v: i32) {
        if let Some(idx) = schema.field_index(name) {
            if schema.fields[idx].is_signed() {
                self.values[idx] = DeltaValue::Int(v);
            } else {
                self.values[idx] = DeltaValue::Uint(v as u32);
            }
        }
    }

    pub fn set_f32(&mut self, schema: &DeltaSchema, name: &str, v: f32) {
        if let Some(idx) = schema.field_index(name) {
            self.values[idx] = DeltaValue::Float(v);
        }
    }

    pub fn set_str(&mut self, schema: &DeltaSchema, name: &str, v: impl Into<String>) {
        if let Some(idx) = schema.field_index(name) {
            self.values[idx] = DeltaValue::Str(v.into());
        }
    }

    pub fn get_f32(&self, schema: &DeltaSchema, name: &str) -> f32 {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or(0.0, DeltaValue::as_f32)
    }

    pub fn get_i32(&self, schema: &DeltaSchema, name: &str) -> i32 {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or(0, DeltaValue::as_i32)
    }

    pub fn get_u32(&self, schema: &DeltaSchema, name: &str) -> u32 {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or(0, DeltaValue::as_u32)
    }

    pub fn get_str<'a>(&'a self, schema: &DeltaSchema, name: &str) -> &'a str {
        schema
            .field_index(name)
            .and_then(|i| self.values.get(i))
            .map_or("", DeltaValue::as_str)
    }
}

/// Built-in `g_MetaDelta` (`delta.cpp:83-92`) used to serialize and deserialize
/// `svc_deltadescription` schemas over the wire.
pub fn meta_delta_schema() -> DeltaSchema {
    DeltaSchema::new(
        "delta_description_t".to_string(),
        None,
        vec![
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "fieldType".to_string(),
                field_offset: 0,
                field_size: 1,
                significant_bits: 32,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_STRING,
                name: "fieldName".to_string(),
                field_offset: 4,
                field_size: 1,
                significant_bits: 1,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "fieldOffset".to_string(),
                field_offset: 36,
                field_size: 1,
                significant_bits: 16,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "fieldSize".to_string(),
                field_offset: 40,
                field_size: 1,
                significant_bits: 8,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_INTEGER,
                name: "significant_bits".to_string(),
                field_offset: 44,
                field_size: 1,
                significant_bits: 8,
                premultiply: 1.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_FLOAT,
                name: "premultiply".to_string(),
                field_offset: 48,
                field_size: 1,
                significant_bits: 32,
                premultiply: 4000.0,
                postmultiply: 1.0,
            },
            DeltaFieldDesc {
                field_type: DT_FLOAT,
                name: "postmultiply".to_string(),
                field_offset: 52,
                field_size: 1,
                significant_bits: 32,
                premultiply: 4000.0,
                postmultiply: 1.0,
            },
        ],
    )
}

/// The changed-field marks (`FDT_MARK`) of one delta, as a game's conditional encoder sees them
/// (`DELTA_SETBYINDEX`, `DELTA_UNSETBYINDEX`, `DELTA_FINDFIELD` of `rehlds/engine/delta.cpp`).
/// A field whose mark is set goes on the wire.
pub struct DeltaMarks<'a> {
    schema: &'a DeltaSchema,
    mask: u64,
}

impl<'a> DeltaMarks<'a> {
    /// The marks of `schema`'s fields in `mask` (bit `n` for field `n`): what the writer hands
    /// an encoder, and a test hands one.
    pub fn new(schema: &'a DeltaSchema, mask: u64) -> Self {
        Self { schema, mask }
    }

    /// The marks as a bit mask, bit `n` for field `n`.
    pub fn mask(&self) -> u64 {
        self.mask
    }

    /// `DELTA_FINDFIELD`: the index of field `name`, `None` when the schema has no such field.
    pub fn find_field(&self, name: &str) -> Option<usize> {
        self.schema.field_index(name)
    }

    /// `DELTA_SETBYINDEX`: marks field `index` as changed. An index out of range is ignored.
    pub fn set(&mut self, index: usize) {
        if index < self.schema.fields.len().min(64) {
            self.mask |= 1u64 << index;
        }
    }

    /// `DELTA_UNSETBYINDEX`: clears the mark of field `index`. An index out of range is ignored.
    pub fn unset(&mut self, index: usize) {
        if index < 64 {
            self.mask &= !(1u64 << index);
        }
    }

    /// Whether field `index` is marked.
    pub fn is_set(&self, index: usize) -> bool {
        index < 64 && self.mask & (1u64 << index) != 0
    }

    /// Sets every valid field bit in `bits` (`DELTA_SETBYINDEX` in bulk).
    #[inline]
    pub fn set_mask(&mut self, bits: u64) {
        let len = self.schema.fields.len().min(64);
        let valid = if len >= 64 { u64::MAX } else { (1u64 << len) - 1 };
        self.mask |= bits & valid;
    }

    /// Clears every field bit in `bits` (`DELTA_UNSETBYINDEX` in bulk).
    #[inline]
    pub fn unset_mask(&mut self, bits: u64) {
        self.mask &= !bits;
    }

    /// Precomputed bitmask of `origin[0..=2]` in this schema (zero `HashMap` lookups).
    #[inline]
    pub fn entity_origin_mask(&self) -> u64 {
        self.schema.entity_plan().origin_mask
    }

    /// Precomputed bitmask of `angles[0..=2]` in this schema (zero `HashMap` lookups).
    #[inline]
    pub fn entity_angles_mask(&self) -> u64 {
        self.schema.entity_plan().angles_mask
    }

    /// Precomputed bitmask of `skin` and `sequence` in this schema (zero `HashMap` lookups).
    #[inline]
    pub fn entity_skin_sequence_mask(&self) -> u64 {
        self.schema.entity_plan().skin_seq_mask
    }

    /// Precomputed bitmask of `animtime` in this schema (zero `HashMap` lookups).
    #[inline]
    pub fn entity_animtime_mask(&self) -> u64 {
        self.schema.entity_plan().animtime_mask
    }
}

/// A conditional encoder (`delta_t::conditionalencode`): called with the record the delta starts
/// from, the record it goes to and the marks of the fields that differ, it may set and clear
/// marks. The schema is the one `marks` carries.
pub type DeltaEncodeFn<'e> = &'e mut dyn FnMut(&DeltaRecord, &DeltaRecord, &mut DeltaMarks<'_>);

/// The marks of the fields that differ between `from` and `to` (`DELTA_MarkDelta`), edited by the
/// conditional encoder when there is one (`DELTA_CheckDelta`, `rehlds/engine/delta.cpp`: the
/// encoder runs after the marking and before the fields are counted).
pub fn delta_mask(
    schema: &DeltaSchema,
    from: &DeltaRecord,
    to: &DeltaRecord,
    encode: Option<DeltaEncodeFn<'_>>,
) -> u64 {
    let mut marks = DeltaMarks { schema, mask: 0 };
    for (i, field) in schema.fields.iter().enumerate().take(64) {
        if !from.values[i].equals_wire(&to.values[i], field) {
            marks.mask |= 1u64 << i;
        }
    }
    if let Some(encode) = encode {
        encode(from, to, &mut marks);
    }
    marks.mask
}

/// Encode a `DeltaRecord` diff (`DELTA_WriteDelta`, `delta.cpp:622-865`) into an active
/// bit-writing `MsgWriter`. Returns `true` if any fields differed (or `force` was set).
pub fn write_delta(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    from: &DeltaRecord,
    to: &DeltaRecord,
    force: bool,
    time_base: f64,
) -> bool {
    write_delta_encoded(w, schema, from, to, force, time_base, None)
}

/// [`write_delta`] with the schema's conditional encoder (see [`DeltaEncodeFn`]): the fields it
/// leaves marked are the ones written. Returns `true` if any field was marked.
pub fn write_delta_encoded(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    from: &DeltaRecord,
    to: &DeltaRecord,
    force: bool,
    time_base: f64,
    encode: Option<DeltaEncodeFn<'_>>,
) -> bool {
    let mask = delta_mask(schema, from, to, encode);
    write_delta_masked(w, schema, to, mask, force, time_base)
}

/// [`write_delta_encoded`] for a `mask` [`delta_mask`] already made: the conditional encoder is
/// not run again. Returns `true` if any field was marked.
pub fn write_delta_masked(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    to: &DeltaRecord,
    mask: u64,
    force: bool,
    time_base: f64,
) -> bool {
    let last_bit = (mask != 0).then(|| 63 - mask.leading_zeros() as usize);

    if last_bit.is_none() && !force {
        return false;
    }

    let byte_count = match last_bit {
        Some(lb) => (lb >> 3) + 1,
        None => 0,
    };

    w.write_bits(byte_count as u32, 3);
    let mask_bytes = mask.to_le_bytes();
    for &b in &mask_bytes[..byte_count] {
        w.write_bits(b as u32, 8);
    }

    let n = schema.fields.len().min(64);
    let valid_mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
    let mut rem = mask & valid_mask;
    while rem != 0 {
        let i = rem.trailing_zeros() as usize;
        rem &= rem - 1;
        let field = &schema.fields[i];
        let val = DeltaFieldRef::from_value(&to.values[i]);
        write_wire_field(
            w,
            val,
            field.base_type(),
            field.is_signed(),
            field.significant_bits as u32,
            field.premultiply as f64,
            (0.9999..=1.0001).contains(&(field.premultiply as f64)),
            time_base,
        );
    }

    last_bit.is_some()
}

/// Borrowed stack-only representation of a single delta field value during zero-alloc encoding.
#[derive(Debug, Clone, Copy)]
pub enum DeltaFieldRef<'a> {
    Int(i32),
    Uint(u32),
    Float(f32),
    Str(&'a str),
}

impl<'a> DeltaFieldRef<'a> {
    #[inline]
    fn from_value(v: &'a DeltaValue) -> Self {
        match v {
            DeltaValue::Int(x) => Self::Int(*x),
            DeltaValue::Uint(x) => Self::Uint(*x),
            DeltaValue::Float(x) => Self::Float(*x),
            DeltaValue::Str(s) => Self::Str(s.as_str()),
        }
    }

    #[inline]
    fn zero_for(base_type: u32, signed: bool) -> Self {
        match base_type {
            DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG => Self::Float(0.0),
            DT_STRING => Self::Str(""),
            _ if signed => Self::Int(0),
            _ => Self::Uint(0),
        }
    }

    #[inline]
    fn int_for(v: i32, signed: bool) -> Self {
        if signed {
            Self::Int(v)
        } else {
            Self::Uint(v as u32)
        }
    }

    #[inline]
    fn uint_for(v: u32, signed: bool) -> Self {
        if signed {
            Self::Int(v as i32)
        } else {
            Self::Uint(v)
        }
    }

    #[inline]
    fn to_delta_value(self) -> DeltaValue {
        match self {
            Self::Int(v) => DeltaValue::Int(v),
            Self::Uint(v) => DeltaValue::Uint(v),
            Self::Float(v) => DeltaValue::Float(v),
            Self::Str(s) => DeltaValue::Str(s.to_string()),
        }
    }

    #[inline]
    pub fn as_f32(self) -> f32 {
        match self {
            Self::Float(v) => v,
            Self::Int(v) => v as f32,
            Self::Uint(v) => v as f32,
            Self::Str(_) => 0.0,
        }
    }

    #[inline]
    pub fn as_i32(self) -> i32 {
        match self {
            Self::Int(v) => v,
            Self::Uint(v) => v as i32,
            Self::Float(v) => v as i32,
            Self::Str(_) => 0,
        }
    }

    #[inline]
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Uint(v) => v,
            Self::Int(v) => v as u32,
            Self::Float(v) => v as u32,
            Self::Str(_) => 0,
        }
    }

    #[inline]
    pub fn as_str(self) -> &'a str {
        match self {
            Self::Str(s) => s,
            _ => "",
        }
    }

    #[inline]
    fn equals_wire(self, other: Self, base_type: u32) -> bool {
        match base_type {
            DT_BYTE => (self.as_u32() as u8) == (other.as_u32() as u8),
            DT_SHORT => (self.as_u32() as u16) == (other.as_u32() as u16),
            DT_INTEGER => self.as_u32() == other.as_u32(),
            DT_FLOAT | DT_ANGLE | DT_TIMEWINDOW_8 | DT_TIMEWINDOW_BIG => {
                self.as_f32().to_bits() == other.as_f32().to_bits()
            }
            DT_STRING => self.as_str().eq_ignore_ascii_case(other.as_str()),
            _ => true,
        }
    }
}

/// Decoded single field value passed into [`DeltaStruct::write_field`] without `Vec` allocation.
pub enum DecodedField {
    Int(i32),
    Uint(u32),
    Float(f32),
    Str(String),
}

impl DecodedField {
    #[inline]
    pub fn as_f32(&self) -> f32 {
        match *self {
            Self::Float(v) => v,
            Self::Int(v) => v as f32,
            Self::Uint(v) => v as f32,
            Self::Str(_) => 0.0,
        }
    }

    #[inline]
    pub fn as_i32(&self) -> i32 {
        match *self {
            Self::Int(v) => v,
            Self::Uint(v) => v as i32,
            Self::Float(v) => v as i32,
            Self::Str(_) => 0,
        }
    }

    #[inline]
    pub fn as_u32(&self) -> u32 {
        match *self {
            Self::Uint(v) => v,
            Self::Int(v) => v as u32,
            Self::Float(v) => v as u32,
            Self::Str(_) => 0,
        }
    }

    #[inline]
    pub fn into_string(self) -> String {
        match self {
            Self::Str(s) => s,
            _ => String::new(),
        }
    }
}

/// GoldSrc wire struct supporting zero-allocation precompiled delta encoding and decoding.
pub trait DeltaStruct: Sized {
    type FieldId: Copy + Eq + 'static;

    fn compiled_plan(schema: &DeltaSchema) -> &CompiledPlan<Self::FieldId>;
    fn unknown_field_id() -> Self::FieldId;
    fn read_field(&self, id: Self::FieldId, base_type: u32, signed: bool) -> DeltaFieldRef<'_>;
    fn write_field(&mut self, id: Self::FieldId, val: DecodedField);
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn write_wire_field(
    w: &mut MsgWriter,
    val: DeltaFieldRef<'_>,
    base_type: u32,
    signed: bool,
    bits: u32,
    pre: f64,
    pre_is_unit: bool,
    time_base: f64,
) {
    match base_type {
        DT_BYTE => {
            if signed {
                let v = ((val.as_i32() as i8) as f64 * pre) as i8;
                w.write_sbits(v as i32, bits);
            } else {
                let v = ((val.as_u32() as u8) as f64 * pre) as u8;
                w.write_bits(v as u32, bits);
            }
        }
        DT_SHORT => {
            if signed {
                let v = ((val.as_i32() as i16) as f64 * pre) as i16;
                w.write_sbits(v as i32, bits);
            } else {
                let v = ((val.as_u32() as u16) as f64 * pre) as u16;
                w.write_bits(v as u32, bits);
            }
        }
        DT_FLOAT => {
            let scaled = (val.as_f32() as f64) * pre;
            if signed {
                w.write_sbits(scaled as i32, bits);
            } else {
                w.write_bits(scaled as u32, bits);
            }
        }
        DT_INTEGER => {
            if signed {
                let mut v = val.as_i32();
                if !pre_is_unit {
                    v = (v as f64 * pre) as i32;
                }
                w.write_sbits(v, bits);
            } else {
                let mut v = val.as_u32();
                if !pre_is_unit {
                    v = (v as f64 * pre) as u32;
                }
                w.write_bits(v, bits);
            }
        }
        DT_ANGLE => {
            w.write_bit_angle(val.as_f32(), bits);
        }
        DT_TIMEWINDOW_8 => {
            let f2 = val.as_f32() as f64;
            let tw = (time_base * 100.0) as i32 - (f2 * 100.0) as i32;
            w.write_sbits(tw, 8);
        }
        DT_TIMEWINDOW_BIG => {
            let f2 = val.as_f32() as f64;
            let tw = (time_base * pre) as i32 - (f2 * pre) as i32;
            w.write_sbits(tw, bits);
        }
        DT_STRING => {
            w.write_bit_string(val.as_str());
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn read_wire_field(
    r: &mut MsgReader<'_>,
    base_type: u32,
    signed: bool,
    bits: u32,
    pre: f64,
    post: f64,
    pre_is_unit: bool,
    post_is_unit: bool,
    time_base: f64,
) -> Option<DecodedField> {
    let scale = |mut d: f64| -> f64 {
        if !pre_is_unit && pre != 0.0 {
            d /= pre;
        }
        if !post_is_unit {
            d *= post;
        }
        d
    };

    match base_type {
        DT_BYTE => {
            if signed {
                let d = scale(r.read_sbits(bits) as f64);
                Some(DecodedField::Int((d as i8) as i32))
            } else {
                let d = scale(r.read_bits(bits) as f64);
                Some(DecodedField::Uint((d as u8) as u32))
            }
        }
        DT_SHORT => {
            if signed {
                let d = scale(r.read_sbits(bits) as f64);
                Some(DecodedField::Int((d as i16) as i32))
            } else {
                let d = scale(r.read_bits(bits) as f64);
                Some(DecodedField::Uint((d as u16) as u32))
            }
        }
        DT_FLOAT => {
            let raw = if signed {
                r.read_sbits(bits) as f64
            } else {
                r.read_bits(bits) as f64
            };
            Some(DecodedField::Float(scale(raw) as f32))
        }
        DT_INTEGER => {
            if signed {
                let d = scale(r.read_sbits(bits) as f64);
                Some(DecodedField::Int(d as i32))
            } else {
                let d = scale(r.read_bits(bits) as f64);
                Some(DecodedField::Uint(d as u32))
            }
        }
        DT_ANGLE => Some(DecodedField::Float(r.read_bit_angle(bits))),
        DT_TIMEWINDOW_8 => {
            let addt = r.read_sbits(8) as f64;
            let t = (time_base * 100.0 - addt) / 100.0;
            Some(DecodedField::Float(t as f32))
        }
        DT_TIMEWINDOW_BIG => {
            let addt = r.read_sbits(bits) as f64;
            let t = if !pre_is_unit && pre != 0.0 {
                (time_base * pre - addt) / pre
            } else {
                time_base - addt
            };
            Some(DecodedField::Float(t as f32))
        }
        DT_STRING => Some(DecodedField::Str(r.read_bit_string())),
        _ => None,
    }
}

/// Conditional encoder callback for direct struct-to-bitstream encoding (zero [`DeltaRecord`]
/// allocations).
pub type StructDeltaEncodeFn<'e, T> = &'e mut dyn FnMut(&T, &T, &mut DeltaMarks<'_>);

/// Computes the 64-bit changed-field bitmask between `from` and `to` directly via `schema`'s
/// precompiled [`CompiledPlan`] (zero heap allocations).
pub fn struct_delta_mask<T: DeltaStruct>(schema: &DeltaSchema, from: &T, to: &T) -> u64 {
    let plan = T::compiled_plan(schema);
    let mut mask = 0u64;
    for (i, slot) in plan.slots[..plan.len].iter().enumerate() {
        let a = from.read_field(slot.field_id, slot.base_type, slot.signed);
        let b = to.read_field(slot.field_id, slot.base_type, slot.signed);
        if !a.equals_wire(b, slot.base_type) {
            mask |= 1u64 << i;
        }
    }
    mask
}

/// [`struct_delta_mask`] with an optional conditional encoder running on stack [`DeltaMarks`].
pub fn struct_delta_mask_encoded<T: DeltaStruct>(
    schema: &DeltaSchema,
    from: &T,
    to: &T,
    encode: Option<StructDeltaEncodeFn<'_, T>>,
) -> u64 {
    let mask = struct_delta_mask(schema, from, to);
    match encode {
        Some(encode) => {
            let mut marks = DeltaMarks { schema, mask };
            encode(from, to, &mut marks);
            marks.mask
        }
        None => mask,
    }
}

/// Computes the 64-bit changed-field bitmask comparing an existing [`DeltaRecord`] `from`
/// against a typed struct `to` without allocating a [`DeltaRecord`] for `to`.
pub fn struct_vs_record_delta_mask<T: DeltaStruct>(
    schema: &DeltaSchema,
    from: &DeltaRecord,
    to: &T,
) -> u64 {
    let plan = T::compiled_plan(schema);
    let mut mask = 0u64;
    for (i, slot) in plan.slots[..plan.len].iter().enumerate() {
        let a = from
            .values
            .get(i)
            .map(DeltaFieldRef::from_value)
            .unwrap_or_else(|| DeltaFieldRef::zero_for(slot.base_type, slot.signed));
        let b = to.read_field(slot.field_id, slot.base_type, slot.signed);
        if !a.equals_wire(b, slot.base_type) {
            mask |= 1u64 << i;
        }
    }
    mask
}

/// Returns `true` if any wire field in `schema` differs between `from` and `to` (zero allocs).
pub fn has_struct_delta_changes<T: DeltaStruct>(schema: &DeltaSchema, from: &T, to: &T) -> bool {
    struct_delta_mask(schema, from, to) != 0
}

/// Writes the fields of `to` selected by `mask` directly into `w` via `schema`'s precompiled
/// [`CompiledPlan`] (zero heap allocations). Returns `true` if `mask != 0`.
pub fn write_struct_delta_masked<T: DeltaStruct>(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    to: &T,
    mask: u64,
    force: bool,
    time_base: f64,
) -> bool {
    let last_bit = (mask != 0).then(|| 63 - mask.leading_zeros() as usize);
    if last_bit.is_none() && !force {
        return false;
    }

    let byte_count = match last_bit {
        Some(lb) => (lb >> 3) + 1,
        None => 0,
    };
    w.write_bits(byte_count as u32, 3);
    let mask_bytes = mask.to_le_bytes();
    for &b in &mask_bytes[..byte_count] {
        w.write_bits(b as u32, 8);
    }

    let plan = T::compiled_plan(schema);
    let mut rem = mask & plan.valid_mask;
    while rem != 0 {
        let i = rem.trailing_zeros() as usize;
        rem &= rem - 1;
        let slot = &plan.slots[i];
        let val = to.read_field(slot.field_id, slot.base_type, slot.signed);
        write_wire_field(
            w,
            val,
            slot.base_type,
            slot.signed,
            slot.bits,
            slot.pre,
            slot.pre_is_unit,
            time_base,
        );
    }

    last_bit.is_some()
}

/// Zero-allocation direct struct-to-bitstream delta encoder. Produces bit-for-bit identical wire
/// output to `to_delta() + write_delta()`.
pub fn write_struct_delta<T: DeltaStruct>(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    from: &T,
    to: &T,
    force: bool,
    time_base: f64,
) -> bool {
    let mask = struct_delta_mask(schema, from, to);
    write_struct_delta_masked(w, schema, to, mask, force, time_base)
}

/// [`write_struct_delta`] with an optional conditional encoder running on stack [`DeltaMarks`].
pub fn write_struct_delta_encoded<T: DeltaStruct>(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    from: &T,
    to: &T,
    force: bool,
    time_base: f64,
    encode: Option<StructDeltaEncodeFn<'_, T>>,
) -> bool {
    let mask = struct_delta_mask_encoded(schema, from, to, encode);
    write_struct_delta_masked(w, schema, to, mask, force, time_base)
}

/// Direct struct-to-bitstream delta encoder against a [`DeltaRecord`] baseline (zero allocations).
pub fn write_struct_delta_from_record<T: DeltaStruct>(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    from: &DeltaRecord,
    to: &T,
    force: bool,
    time_base: f64,
) -> bool {
    let mask = struct_vs_record_delta_mask(schema, from, to);
    write_struct_delta_masked(w, schema, to, mask, force, time_base)
}

/// Zero-allocation direct bitstream-to-struct delta decoder writing changed fields in place
/// into `out` (which starts from the previous/base state).
pub fn read_struct_delta_into<T: DeltaStruct>(
    r: &mut MsgReader<'_>,
    schema: &DeltaSchema,
    out: &mut T,
    time_base: f64,
) -> Result<(), ProtoError> {
    let nbytes = r.read_bits(3) as usize;
    if r.is_bad_read() {
        return Err(ProtoError::UnexpectedEof);
    }
    let mut mask_bytes = [0u8; 8];
    for slot in mask_bytes.iter_mut().take(nbytes.min(8)) {
        *slot = r.read_bits(8) as u8;
    }
    if r.is_bad_read() {
        return Err(ProtoError::UnexpectedEof);
    }
    let mask = u64::from_le_bytes(mask_bytes);

    let plan = T::compiled_plan(schema);
    let unknown = T::unknown_field_id();
    let mut rem = mask & plan.valid_mask;
    while rem != 0 {
        let i = rem.trailing_zeros() as usize;
        rem &= rem - 1;
        let slot = &plan.slots[i];
        if let Some(val) = read_wire_field(
            r,
            slot.base_type,
            slot.signed,
            slot.bits,
            slot.pre,
            slot.post,
            slot.pre_is_unit,
            slot.post_is_unit,
            time_base,
        ) && slot.field_id != unknown
        {
            out.write_field(slot.field_id, val);
        }
        if r.is_bad_read() {
            return Err(ProtoError::UnexpectedEof);
        }
    }

    Ok(())
}

/// Direct bitstream-to-struct delta decoder returning a new `T` cloned from `from` with all
/// changed wire fields applied in place (zero [`DeltaRecord`] allocations).
pub fn read_struct_delta<T: DeltaStruct + Clone>(
    r: &mut MsgReader<'_>,
    schema: &DeltaSchema,
    from: &T,
    time_base: f64,
) -> Result<T, ProtoError> {
    let mut out = from.clone();
    read_struct_delta_into(r, schema, &mut out, time_base)?;
    Ok(out)
}

/// Check whether two records differ without writing anything (`DELTA_CheckDelta`).
pub fn has_delta_changes(schema: &DeltaSchema, from: &DeltaRecord, to: &DeltaRecord) -> bool {
    has_delta_changes_encoded(schema, from, to, None)
}

/// [`has_delta_changes`] after the schema's conditional encoder (see [`DeltaEncodeFn`]).
pub fn has_delta_changes_encoded(
    schema: &DeltaSchema,
    from: &DeltaRecord,
    to: &DeltaRecord,
    encode: Option<DeltaEncodeFn<'_>>,
) -> bool {
    delta_mask(schema, from, to, encode) != 0
}

/// Decode a `DeltaRecord` diff (`DELTA_ParseDelta`, `delta.cpp:867-1069`) from an active
/// bit-reading `MsgReader`.
pub fn read_delta(
    r: &mut MsgReader<'_>,
    schema: &DeltaSchema,
    from: &DeltaRecord,
    time_base: f64,
) -> Result<DeltaRecord, ProtoError> {
    let mut to = from.clone();
    if to.values.len() < schema.fields.len() {
        to = schema.zero_record();
    }

    let nbytes = r.read_bits(3) as usize;
    if r.is_bad_read() {
        return Err(ProtoError::UnexpectedEof);
    }
    let mut mask_bytes = [0u8; 8];
    for slot in mask_bytes.iter_mut().take(nbytes.min(8)) {
        *slot = r.read_bits(8) as u8;
    }
    if r.is_bad_read() {
        return Err(ProtoError::UnexpectedEof);
    }
    let mask = u64::from_le_bytes(mask_bytes);

    let n = schema.fields.len().min(64);
    let valid_mask = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };
    let mut rem = mask & valid_mask;
    while rem != 0 {
        let i = rem.trailing_zeros() as usize;
        rem &= rem - 1;
        let field = &schema.fields[i];
        let pre = field.premultiply as f64;
        let post = field.postmultiply as f64;
        if let Some(decoded) = read_wire_field(
            r,
            field.base_type(),
            field.is_signed(),
            field.significant_bits as u32,
            pre,
            post,
            (0.9999..=1.0001).contains(&pre),
            (0.9999..=1.0001).contains(&post),
            time_base,
        ) {
            to.values[i] = match decoded {
                DecodedField::Int(v) => DeltaValue::Int(v),
                DecodedField::Uint(v) => DeltaValue::Uint(v),
                DecodedField::Float(v) => DeltaValue::Float(v),
                DecodedField::Str(s) => DeltaValue::Str(s),
            };
        }

        if r.is_bad_read() {
            return Err(ProtoError::UnexpectedEof);
        }
    }

    Ok(to)
}

/// Write a `svc_deltadescription` body (after the opcode byte) using `g_MetaDelta`
/// (`SV_WriteDeltaDescriptionsToClient`, `sv_main.cpp:1054-1067`).
pub fn write_delta_description(w: &mut MsgWriter, schema: &DeltaSchema) {
    let meta = meta_delta_schema();
    let null_rec = meta.zero_record();
    w.write_str(&schema.name);
    w.start_bit_writing();
    w.write_bits(schema.fields.len() as u32, 16);
    for field in &schema.fields {
        let mut rec = meta.zero_record();
        rec.set_u32(&meta, "fieldType", field.field_type);
        rec.set_str(&meta, "fieldName", &field.name);
        rec.set_u32(&meta, "fieldOffset", field.field_offset as u32);
        rec.set_u32(&meta, "fieldSize", field.field_size as u32);
        rec.set_u32(&meta, "significant_bits", field.significant_bits as u32);
        rec.set_f32(&meta, "premultiply", field.premultiply);
        rec.set_f32(&meta, "postmultiply", field.postmultiply);
        write_delta(w, &meta, &null_rec, &rec, true, 0.0);
    }
    w.end_bit_writing();
}

/// Read a `svc_deltadescription` body (after the opcode byte) using `g_MetaDelta`.
pub fn read_delta_description(r: &mut MsgReader<'_>) -> Result<DeltaSchema, ProtoError> {
    let meta = meta_delta_schema();
    let null_rec = meta.zero_record();
    let name = r.read_str()?;
    r.start_bit_reading();
    let count = r.read_bits(16) as usize;
    if count > 256 {
        return Err(ProtoError::Delta(format!(
            "implausible delta field count {count} for {name}"
        )));
    }
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let rec = read_delta(r, &meta, &null_rec, 0.0)?;
        fields.push(DeltaFieldDesc {
            field_type: rec.get_u32(&meta, "fieldType"),
            name: rec.get_str(&meta, "fieldName").to_string(),
            field_offset: rec.get_u32(&meta, "fieldOffset") as u16,
            field_size: rec.get_u32(&meta, "fieldSize") as u8,
            significant_bits: rec.get_u32(&meta, "significant_bits") as u8,
            premultiply: rec.get_f32(&meta, "premultiply"),
            postmultiply: rec.get_f32(&meta, "postmultiply"),
        });
    }
    r.end_bit_reading()?;
    Ok(DeltaSchema::new(name, None, fields))
}

/// Parse Valve's `cstrike/delta.lst` or `valve/delta.lst` file into `DeltaSchema`s.
pub fn parse_delta_lst(source: &str) -> Result<Vec<DeltaSchema>, ProtoError> {
    // Strip `//` line comments first.
    let mut cleaned = String::with_capacity(source.len());
    for line in source.lines() {
        let stripped = match line.find("//") {
            Some(pos) => &line[..pos],
            None => line,
        };
        cleaned.push_str(stripped);
        cleaned.push('\n');
    }

    let mut schemas = Vec::new();
    let mut rest = cleaned.as_str();

    while let Some(open_brace) = rest.find('{') {
        let header = rest[..open_brace].trim();
        let after_open = &rest[open_brace + 1..];
        let close_brace = after_open
            .find('}')
            .ok_or_else(|| ProtoError::Delta("unclosed '{' in delta.lst".to_string()))?;
        let body = &after_open[..close_brace];
        rest = &after_open[close_brace + 1..];

        let mut header_tokens = header.split_whitespace();
        let Some(struct_name) = header_tokens.next() else {
            continue;
        };
        let encoder = match header_tokens.next() {
            Some("none") | None => None,
            Some("gamedll" | "clientdll") => header_tokens.next().map(str::to_string),
            Some(other) => Some(other.to_string()),
        };

        let mut fields = Vec::new();
        let mut cursor = body;
        while let Some(pos) = cursor.find("DEFINE_DELTA") {
            let after_macro = &cursor[pos..];
            let is_post = after_macro.starts_with("DEFINE_DELTA_POST");
            let Some(lparen) = after_macro.find('(') else {
                break;
            };
            let Some(rparen) = after_macro[lparen + 1..].find(')') else {
                break;
            };
            let args_str = &after_macro[lparen + 1..lparen + 1 + rparen];
            cursor = &after_macro[lparen + 1 + rparen + 1..];

            let args: Vec<&str> = args_str.split(',').map(str::trim).collect();
            if args.len() < 4 {
                continue;
            }
            let field_name = args[0].to_string();
            let field_type = parse_dt_flags(args[1])?;
            let significant_bits: u8 = args[2]
                .parse()
                .map_err(|_| ProtoError::Delta(format!("bad bits '{}' in {field_name}", args[2])))?;
            let premultiply: f32 = args[3].parse().map_err(|_| {
                ProtoError::Delta(format!("bad premultiply '{}' in {field_name}", args[3]))
            })?;
            let postmultiply: f32 = if is_post && args.len() >= 5 {
                args[4].parse().map_err(|_| {
                    ProtoError::Delta(format!("bad postmultiply '{}' in {field_name}", args[4]))
                })?
            } else {
                1.0
            };

            // The real `offsetof` of the member in the original client's C struct: the client
            // stores each decoded value there. An unknown struct or member is a hard error,
            // like ReHLDS `DELTA_FindOffset`.
            let layout = field_layout(struct_name, &field_name).ok_or_else(|| {
                ProtoError::Delta(format!(
                    "no C struct offset for field '{field_name}' of delta struct '{struct_name}'"
                ))
            })?;

            fields.push(DeltaFieldDesc {
                field_type,
                name: field_name,
                field_offset: layout.offset,
                // ReHLDS `DELTA_ParseField` sets `fieldSize = 1` for every parsed field.
                field_size: 1,
                significant_bits,
                premultiply,
                postmultiply,
            });
        }

        schemas.push(DeltaSchema::new(struct_name.to_string(), encoder, fields));
    }

    Ok(schemas)
}

fn parse_dt_flags(expr: &str) -> Result<u32, ProtoError> {
    let mut flags = 0u32;
    for part in expr.split('|').map(str::trim) {
        match part {
            "DT_BYTE" => flags |= DT_BYTE,
            "DT_SHORT" => flags |= DT_SHORT,
            "DT_FLOAT" => flags |= DT_FLOAT,
            "DT_INTEGER" => flags |= DT_INTEGER,
            "DT_ANGLE" => flags |= DT_ANGLE,
            "DT_TIMEWINDOW_8" => flags |= DT_TIMEWINDOW_8,
            "DT_TIMEWINDOW_BIG" => flags |= DT_TIMEWINDOW_BIG,
            "DT_STRING" => flags |= DT_STRING,
            "DT_SIGNED" => flags |= DT_SIGNED,
            "" => {}
            other => {
                return Err(ProtoError::Delta(format!("unknown delta flag '{other}'")));
            }
        }
    }
    Ok(flags)
}

/// Registry holding all active GoldSrc delta schemas for a session (loaded either from
/// `cstrike/delta.lst` on the server or from `svc_deltadescription` packets on the client).
#[derive(Debug, Default, Clone)]
pub struct DeltaRegistry {
    schemas: HashMap<String, DeltaSchema>,
    order: Vec<String>,
}

impl DeltaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry from Valve's `delta.lst` text.
    pub fn from_delta_lst(source: &str) -> Result<Self, ProtoError> {
        let mut reg = Self::new();
        for schema in parse_delta_lst(source)? {
            reg.insert(schema);
        }
        Ok(reg)
    }

    /// Built-in CS 1.6 default delta registry (matches Valve `cstrike/delta.lst`).
    pub fn cs16_default() -> Self {
        Self::from_delta_lst(CS16_DEFAULT_DELTA_LST).expect("valid built-in CS 1.6 delta.lst")
    }

    pub fn insert(&mut self, schema: DeltaSchema) {
        let name = schema.name.clone();
        if !self.schemas.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.schemas.insert(name, schema);
    }

    pub fn get(&self, name: &str) -> Option<&DeltaSchema> {
        self.schemas.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &DeltaSchema> {
        self.order.iter().filter_map(|k| self.schemas.get(k))
    }
}

/// High-level GoldSrc `usercmd_t` (`common/usercmd.h`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GoldSrcUserCmd {
    pub lerp_msec: i16,
    pub msec: u8,
    pub viewangles: Vec3,
    pub forwardmove: f32,
    pub sidemove: f32,
    pub upmove: f32,
    pub lightlevel: u8,
    pub buttons: u16,
    pub impulse: u8,
    pub weaponselect: u8,
    pub impact_index: i32,
    pub impact_position: Vec3,
}

fn to_delta_via_plan<T: DeltaStruct>(val: &T, schema: &DeltaSchema) -> DeltaRecord {
    let plan = T::compiled_plan(schema);
    let mut values = Vec::with_capacity(schema.fields.len());
    for slot in &plan.slots[..plan.len] {
        values.push(
            val.read_field(slot.field_id, slot.base_type, slot.signed)
                .to_delta_value(),
        );
    }
    for field in schema.fields.iter().skip(plan.len) {
        values.push(
            DeltaFieldRef::zero_for(field.base_type(), field.is_signed()).to_delta_value(),
        );
    }
    DeltaRecord { values }
}

fn from_delta_via_plan<T: DeltaStruct>(mut out: T, rec: &DeltaRecord, schema: &DeltaSchema) -> T {
    let plan = T::compiled_plan(schema);
    let unknown = T::unknown_field_id();
    let count = plan.len.min(rec.values.len());
    for (slot, val) in plan.slots[..count].iter().zip(&rec.values[..count]) {
        if slot.field_id == unknown {
            continue;
        }
        let decoded = match val {
            DeltaValue::Int(v) => DecodedField::Int(*v),
            DeltaValue::Uint(v) => DecodedField::Uint(*v),
            DeltaValue::Float(v) => DecodedField::Float(*v),
            DeltaValue::Str(s) => DecodedField::Str(s.clone()),
        };
        out.write_field(slot.field_id, decoded);
    }
    out
}

impl DeltaStruct for GoldSrcUserCmd {
    type FieldId = UserCmdFieldId;

    #[inline]
    fn compiled_plan(schema: &DeltaSchema) -> &CompiledPlan<Self::FieldId> {
        schema.usercmd_plan()
    }

    #[inline]
    fn unknown_field_id() -> Self::FieldId {
        UserCmdFieldId::Unknown
    }

    #[inline]
    fn read_field(&self, id: Self::FieldId, base_type: u32, signed: bool) -> DeltaFieldRef<'_> {
        match id {
            UserCmdFieldId::LerpMsec => DeltaFieldRef::int_for(self.lerp_msec as i32, signed),
            UserCmdFieldId::Msec => DeltaFieldRef::uint_for(self.msec as u32, signed),
            UserCmdFieldId::ViewAngles0 => DeltaFieldRef::Float(self.viewangles.x),
            UserCmdFieldId::ViewAngles1 => DeltaFieldRef::Float(self.viewangles.y),
            UserCmdFieldId::ViewAngles2 => DeltaFieldRef::Float(self.viewangles.z),
            UserCmdFieldId::ForwardMove => DeltaFieldRef::Float(self.forwardmove),
            UserCmdFieldId::SideMove => DeltaFieldRef::Float(self.sidemove),
            UserCmdFieldId::UpMove => DeltaFieldRef::Float(self.upmove),
            UserCmdFieldId::LightLevel => DeltaFieldRef::uint_for(self.lightlevel as u32, signed),
            UserCmdFieldId::Buttons => DeltaFieldRef::uint_for(self.buttons as u32, signed),
            UserCmdFieldId::Impulse => DeltaFieldRef::uint_for(self.impulse as u32, signed),
            UserCmdFieldId::WeaponSelect => {
                DeltaFieldRef::uint_for(self.weaponselect as u32, signed)
            }
            UserCmdFieldId::ImpactIndex => DeltaFieldRef::int_for(self.impact_index, signed),
            UserCmdFieldId::ImpactPosition0 => DeltaFieldRef::Float(self.impact_position.x),
            UserCmdFieldId::ImpactPosition1 => DeltaFieldRef::Float(self.impact_position.y),
            UserCmdFieldId::ImpactPosition2 => DeltaFieldRef::Float(self.impact_position.z),
            UserCmdFieldId::Unknown => DeltaFieldRef::zero_for(base_type, signed),
        }
    }

    #[inline]
    fn write_field(&mut self, id: Self::FieldId, val: DecodedField) {
        match id {
            UserCmdFieldId::LerpMsec => self.lerp_msec = val.as_i32() as i16,
            UserCmdFieldId::Msec => self.msec = val.as_u32() as u8,
            UserCmdFieldId::ViewAngles0 => self.viewangles.x = val.as_f32(),
            UserCmdFieldId::ViewAngles1 => self.viewangles.y = val.as_f32(),
            UserCmdFieldId::ViewAngles2 => self.viewangles.z = val.as_f32(),
            UserCmdFieldId::ForwardMove => self.forwardmove = val.as_f32(),
            UserCmdFieldId::SideMove => self.sidemove = val.as_f32(),
            UserCmdFieldId::UpMove => self.upmove = val.as_f32(),
            UserCmdFieldId::LightLevel => self.lightlevel = val.as_u32() as u8,
            UserCmdFieldId::Buttons => self.buttons = val.as_u32() as u16,
            UserCmdFieldId::Impulse => self.impulse = val.as_u32() as u8,
            UserCmdFieldId::WeaponSelect => self.weaponselect = val.as_u32() as u8,
            UserCmdFieldId::ImpactIndex => self.impact_index = val.as_i32(),
            UserCmdFieldId::ImpactPosition0 => self.impact_position.x = val.as_f32(),
            UserCmdFieldId::ImpactPosition1 => self.impact_position.y = val.as_f32(),
            UserCmdFieldId::ImpactPosition2 => self.impact_position.z = val.as_f32(),
            UserCmdFieldId::Unknown => {}
        }
    }
}

impl GoldSrcUserCmd {
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        to_delta_via_plan(self, schema)
    }

    pub fn from_delta(rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        from_delta_via_plan(Self::default(), rec, schema)
    }
}

/// High-level GoldSrc `clientdata_t` (`common/entity_state.h`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GoldSrcClientData {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub viewmodel: i32,
    pub punchangle: Vec3,
    pub flags: u32,
    pub waterlevel: i32,
    pub view_ofs: Vec3,
    pub health: f32,
    pub b_in_duck: i32,
    pub weapons: u32,
    /// `ammo_nails`, `ammo_shells`, `ammo_cells`, `ammo_rockets`: 10 signed bits each on the
    /// wire (the HL ammo counters the stock client reads; CS sends its ammo in `AmmoX`).
    pub ammo_nails: i32,
    pub ammo_shells: i32,
    pub ammo_cells: i32,
    pub ammo_rockets: i32,
    pub fl_time_step_sound: i32,
    pub fl_duck_time: i32,
    pub maxspeed: f32,
    pub m_i_id: i32,
    pub m_fl_next_attack: f32,
    pub fov: f32,
    pub weaponanim: i32,
    pub deadflag: i32,
    pub iuser1: i32,
    pub iuser2: i32,
    pub iuser3: i32,
    pub iuser4: i32,
    pub physinfo: String,
    /// `flSwimTime` (10 bits on the wire).
    pub fl_swim_time: i32,
    /// `waterjumptime` (15 bits).
    pub waterjumptime: i32,
    /// `pushmsec` (11 bits).
    pub pushmsec: i32,
    pub fuser1: f32,
    pub fuser2: f32,
    pub fuser3: f32,
    pub vuser2: Vec3,
    pub vuser3: Vec3,
    /// Only axes 0 and 1 are in the schema; `from_delta` gives axis 2 as 0.
    pub vuser4: Vec3,
}

impl DeltaStruct for GoldSrcClientData {
    type FieldId = ClientDataFieldId;

    #[inline]
    fn compiled_plan(schema: &DeltaSchema) -> &CompiledPlan<Self::FieldId> {
        schema.clientdata_plan()
    }

    #[inline]
    fn unknown_field_id() -> Self::FieldId {
        ClientDataFieldId::Unknown
    }

    #[inline]
    fn read_field(&self, id: Self::FieldId, base_type: u32, signed: bool) -> DeltaFieldRef<'_> {
        match id {
            ClientDataFieldId::Origin0 => DeltaFieldRef::Float(self.origin.x),
            ClientDataFieldId::Origin1 => DeltaFieldRef::Float(self.origin.y),
            ClientDataFieldId::Origin2 => DeltaFieldRef::Float(self.origin.z),
            ClientDataFieldId::Velocity0 => DeltaFieldRef::Float(self.velocity.x),
            ClientDataFieldId::Velocity1 => DeltaFieldRef::Float(self.velocity.y),
            ClientDataFieldId::Velocity2 => DeltaFieldRef::Float(self.velocity.z),
            ClientDataFieldId::ViewModel => DeltaFieldRef::int_for(self.viewmodel, signed),
            ClientDataFieldId::PunchAngle0 => DeltaFieldRef::Float(self.punchangle.x),
            ClientDataFieldId::PunchAngle1 => DeltaFieldRef::Float(self.punchangle.y),
            ClientDataFieldId::PunchAngle2 => DeltaFieldRef::Float(self.punchangle.z),
            ClientDataFieldId::Flags => DeltaFieldRef::uint_for(self.flags, signed),
            ClientDataFieldId::WaterLevel => DeltaFieldRef::int_for(self.waterlevel, signed),
            ClientDataFieldId::ViewOfs2 => DeltaFieldRef::Float(self.view_ofs.z),
            ClientDataFieldId::Health => DeltaFieldRef::Float(self.health),
            ClientDataFieldId::BInDuck => DeltaFieldRef::int_for(self.b_in_duck, signed),
            ClientDataFieldId::Weapons => DeltaFieldRef::uint_for(self.weapons, signed),
            ClientDataFieldId::AmmoNails => DeltaFieldRef::int_for(self.ammo_nails, signed),
            ClientDataFieldId::AmmoShells => DeltaFieldRef::int_for(self.ammo_shells, signed),
            ClientDataFieldId::AmmoCells => DeltaFieldRef::int_for(self.ammo_cells, signed),
            ClientDataFieldId::AmmoRockets => DeltaFieldRef::int_for(self.ammo_rockets, signed),
            ClientDataFieldId::FlTimeStepSound => {
                DeltaFieldRef::int_for(self.fl_time_step_sound, signed)
            }
            ClientDataFieldId::FlDuckTime => DeltaFieldRef::int_for(self.fl_duck_time, signed),
            ClientDataFieldId::MaxSpeed => DeltaFieldRef::Float(self.maxspeed),
            ClientDataFieldId::MId => DeltaFieldRef::int_for(self.m_i_id, signed),
            ClientDataFieldId::MFlNextAttack => DeltaFieldRef::Float(self.m_fl_next_attack),
            ClientDataFieldId::Fov => DeltaFieldRef::Float(self.fov),
            ClientDataFieldId::WeaponAnim => DeltaFieldRef::int_for(self.weaponanim, signed),
            ClientDataFieldId::DeadFlag => DeltaFieldRef::int_for(self.deadflag, signed),
            ClientDataFieldId::IUser1 => DeltaFieldRef::int_for(self.iuser1, signed),
            ClientDataFieldId::IUser2 => DeltaFieldRef::int_for(self.iuser2, signed),
            ClientDataFieldId::IUser3 => DeltaFieldRef::int_for(self.iuser3, signed),
            ClientDataFieldId::IUser4 => DeltaFieldRef::int_for(self.iuser4, signed),
            ClientDataFieldId::PhysInfo => DeltaFieldRef::Str(self.physinfo.as_str()),
            ClientDataFieldId::FlSwimTime => DeltaFieldRef::int_for(self.fl_swim_time, signed),
            ClientDataFieldId::WaterJumpTime => {
                DeltaFieldRef::int_for(self.waterjumptime, signed)
            }
            ClientDataFieldId::PushMsec => DeltaFieldRef::int_for(self.pushmsec, signed),
            ClientDataFieldId::FUser1 => DeltaFieldRef::Float(self.fuser1),
            ClientDataFieldId::FUser2 => DeltaFieldRef::Float(self.fuser2),
            ClientDataFieldId::FUser3 => DeltaFieldRef::Float(self.fuser3),
            ClientDataFieldId::VUser2_0 => DeltaFieldRef::Float(self.vuser2.x),
            ClientDataFieldId::VUser2_1 => DeltaFieldRef::Float(self.vuser2.y),
            ClientDataFieldId::VUser2_2 => DeltaFieldRef::Float(self.vuser2.z),
            ClientDataFieldId::VUser3_0 => DeltaFieldRef::Float(self.vuser3.x),
            ClientDataFieldId::VUser3_1 => DeltaFieldRef::Float(self.vuser3.y),
            ClientDataFieldId::VUser3_2 => DeltaFieldRef::Float(self.vuser3.z),
            ClientDataFieldId::VUser4_0 => DeltaFieldRef::Float(self.vuser4.x),
            ClientDataFieldId::VUser4_1 => DeltaFieldRef::Float(self.vuser4.y),
            ClientDataFieldId::Unknown => DeltaFieldRef::zero_for(base_type, signed),
        }
    }

    #[inline]
    fn write_field(&mut self, id: Self::FieldId, val: DecodedField) {
        match id {
            ClientDataFieldId::Origin0 => self.origin.x = val.as_f32(),
            ClientDataFieldId::Origin1 => self.origin.y = val.as_f32(),
            ClientDataFieldId::Origin2 => self.origin.z = val.as_f32(),
            ClientDataFieldId::Velocity0 => self.velocity.x = val.as_f32(),
            ClientDataFieldId::Velocity1 => self.velocity.y = val.as_f32(),
            ClientDataFieldId::Velocity2 => self.velocity.z = val.as_f32(),
            ClientDataFieldId::ViewModel => self.viewmodel = val.as_i32(),
            ClientDataFieldId::PunchAngle0 => self.punchangle.x = val.as_f32(),
            ClientDataFieldId::PunchAngle1 => self.punchangle.y = val.as_f32(),
            ClientDataFieldId::PunchAngle2 => self.punchangle.z = val.as_f32(),
            ClientDataFieldId::Flags => self.flags = val.as_u32(),
            ClientDataFieldId::WaterLevel => self.waterlevel = val.as_i32(),
            ClientDataFieldId::ViewOfs2 => self.view_ofs.z = val.as_f32(),
            ClientDataFieldId::Health => self.health = val.as_f32(),
            ClientDataFieldId::BInDuck => self.b_in_duck = val.as_i32(),
            ClientDataFieldId::Weapons => self.weapons = val.as_u32(),
            ClientDataFieldId::AmmoNails => self.ammo_nails = val.as_i32(),
            ClientDataFieldId::AmmoShells => self.ammo_shells = val.as_i32(),
            ClientDataFieldId::AmmoCells => self.ammo_cells = val.as_i32(),
            ClientDataFieldId::AmmoRockets => self.ammo_rockets = val.as_i32(),
            ClientDataFieldId::FlTimeStepSound => self.fl_time_step_sound = val.as_i32(),
            ClientDataFieldId::FlDuckTime => self.fl_duck_time = val.as_i32(),
            ClientDataFieldId::MaxSpeed => self.maxspeed = val.as_f32(),
            ClientDataFieldId::MId => self.m_i_id = val.as_i32(),
            ClientDataFieldId::MFlNextAttack => self.m_fl_next_attack = val.as_f32(),
            ClientDataFieldId::Fov => self.fov = val.as_f32(),
            ClientDataFieldId::WeaponAnim => self.weaponanim = val.as_i32(),
            ClientDataFieldId::DeadFlag => self.deadflag = val.as_i32(),
            ClientDataFieldId::IUser1 => self.iuser1 = val.as_i32(),
            ClientDataFieldId::IUser2 => self.iuser2 = val.as_i32(),
            ClientDataFieldId::IUser3 => self.iuser3 = val.as_i32(),
            ClientDataFieldId::IUser4 => self.iuser4 = val.as_i32(),
            ClientDataFieldId::PhysInfo => self.physinfo = val.into_string(),
            ClientDataFieldId::FlSwimTime => self.fl_swim_time = val.as_i32(),
            ClientDataFieldId::WaterJumpTime => self.waterjumptime = val.as_i32(),
            ClientDataFieldId::PushMsec => self.pushmsec = val.as_i32(),
            ClientDataFieldId::FUser1 => self.fuser1 = val.as_f32(),
            ClientDataFieldId::FUser2 => self.fuser2 = val.as_f32(),
            ClientDataFieldId::FUser3 => self.fuser3 = val.as_f32(),
            ClientDataFieldId::VUser2_0 => self.vuser2.x = val.as_f32(),
            ClientDataFieldId::VUser2_1 => self.vuser2.y = val.as_f32(),
            ClientDataFieldId::VUser2_2 => self.vuser2.z = val.as_f32(),
            ClientDataFieldId::VUser3_0 => self.vuser3.x = val.as_f32(),
            ClientDataFieldId::VUser3_1 => self.vuser3.y = val.as_f32(),
            ClientDataFieldId::VUser3_2 => self.vuser3.z = val.as_f32(),
            ClientDataFieldId::VUser4_0 => self.vuser4.x = val.as_f32(),
            ClientDataFieldId::VUser4_1 => self.vuser4.y = val.as_f32(),
            ClientDataFieldId::Unknown => {}
        }
    }
}

impl GoldSrcClientData {
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        to_delta_via_plan(self, schema)
    }

    pub fn from_delta(rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        from_delta_via_plan(Self::default(), rec, schema)
    }
}

/// `weapon_data_t` (`common/weaponinfo.h`): the per-weapon state the server sends in
/// `svc_clientdata` and the client predicts with. Field names are the C names in snake_case.
/// The `weapon_data_t` delta schema carries 18 of the 22 fields (checked against the installed
/// `cstrike/delta.lst`, 2026-11-22): `iuser1` is the only `iuser`, and `iuser2..4` and `fuser4`
/// are not on the wire (`to_delta` leaves them out, `from_delta` gives them as 0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GoldSrcWeaponData {
    pub m_i_id: i32,
    pub m_i_clip: i32,
    pub m_fl_next_primary_attack: f32,
    pub m_fl_next_secondary_attack: f32,
    pub m_fl_time_weapon_idle: f32,
    pub m_f_in_reload: i32,
    pub m_f_in_special_reload: i32,
    pub m_fl_next_reload: f32,
    pub m_fl_pump_time: f32,
    pub m_f_reload_time: f32,
    pub m_f_aimed_damage: f32,
    pub m_f_next_aim_bonus: f32,
    pub m_f_in_zoom: i32,
    pub m_i_weapon_state: i32,
    pub iuser1: i32,
    pub iuser2: i32,
    pub iuser3: i32,
    pub iuser4: i32,
    pub fuser1: f32,
    pub fuser2: f32,
    pub fuser3: f32,
    pub fuser4: f32,
}

impl DeltaStruct for GoldSrcWeaponData {
    type FieldId = WeaponDataFieldId;

    #[inline]
    fn compiled_plan(schema: &DeltaSchema) -> &CompiledPlan<Self::FieldId> {
        schema.weapondata_plan()
    }

    #[inline]
    fn unknown_field_id() -> Self::FieldId {
        WeaponDataFieldId::Unknown
    }

    #[inline]
    fn read_field(&self, id: Self::FieldId, base_type: u32, signed: bool) -> DeltaFieldRef<'_> {
        match id {
            WeaponDataFieldId::MId => DeltaFieldRef::int_for(self.m_i_id, signed),
            WeaponDataFieldId::MClip => DeltaFieldRef::int_for(self.m_i_clip, signed),
            WeaponDataFieldId::MFlNextPrimaryAttack => {
                DeltaFieldRef::Float(self.m_fl_next_primary_attack)
            }
            WeaponDataFieldId::MFlNextSecondaryAttack => {
                DeltaFieldRef::Float(self.m_fl_next_secondary_attack)
            }
            WeaponDataFieldId::MFlTimeWeaponIdle => {
                DeltaFieldRef::Float(self.m_fl_time_weapon_idle)
            }
            WeaponDataFieldId::MFInReload => DeltaFieldRef::int_for(self.m_f_in_reload, signed),
            WeaponDataFieldId::MFInSpecialReload => {
                DeltaFieldRef::int_for(self.m_f_in_special_reload, signed)
            }
            WeaponDataFieldId::MFlNextReload => DeltaFieldRef::Float(self.m_fl_next_reload),
            WeaponDataFieldId::MFlPumpTime => DeltaFieldRef::Float(self.m_fl_pump_time),
            WeaponDataFieldId::MFReloadTime => DeltaFieldRef::Float(self.m_f_reload_time),
            WeaponDataFieldId::MFAimedDamage => DeltaFieldRef::Float(self.m_f_aimed_damage),
            WeaponDataFieldId::MFNextAimBonus => DeltaFieldRef::Float(self.m_f_next_aim_bonus),
            WeaponDataFieldId::MFInZoom => DeltaFieldRef::int_for(self.m_f_in_zoom, signed),
            WeaponDataFieldId::MIWeaponState => {
                DeltaFieldRef::int_for(self.m_i_weapon_state, signed)
            }
            WeaponDataFieldId::IUser1 => DeltaFieldRef::int_for(self.iuser1, signed),
            WeaponDataFieldId::IUser2 => DeltaFieldRef::int_for(self.iuser2, signed),
            WeaponDataFieldId::IUser3 => DeltaFieldRef::int_for(self.iuser3, signed),
            WeaponDataFieldId::IUser4 => DeltaFieldRef::int_for(self.iuser4, signed),
            WeaponDataFieldId::FUser1 => DeltaFieldRef::Float(self.fuser1),
            WeaponDataFieldId::FUser2 => DeltaFieldRef::Float(self.fuser2),
            WeaponDataFieldId::FUser3 => DeltaFieldRef::Float(self.fuser3),
            WeaponDataFieldId::FUser4 => DeltaFieldRef::Float(self.fuser4),
            WeaponDataFieldId::Unknown => DeltaFieldRef::zero_for(base_type, signed),
        }
    }

    #[inline]
    fn write_field(&mut self, id: Self::FieldId, val: DecodedField) {
        match id {
            WeaponDataFieldId::MId => self.m_i_id = val.as_i32(),
            WeaponDataFieldId::MClip => self.m_i_clip = val.as_i32(),
            WeaponDataFieldId::MFlNextPrimaryAttack => {
                self.m_fl_next_primary_attack = val.as_f32()
            }
            WeaponDataFieldId::MFlNextSecondaryAttack => {
                self.m_fl_next_secondary_attack = val.as_f32()
            }
            WeaponDataFieldId::MFlTimeWeaponIdle => self.m_fl_time_weapon_idle = val.as_f32(),
            WeaponDataFieldId::MFInReload => self.m_f_in_reload = val.as_i32(),
            WeaponDataFieldId::MFInSpecialReload => self.m_f_in_special_reload = val.as_i32(),
            WeaponDataFieldId::MFlNextReload => self.m_fl_next_reload = val.as_f32(),
            WeaponDataFieldId::MFlPumpTime => self.m_fl_pump_time = val.as_f32(),
            WeaponDataFieldId::MFReloadTime => self.m_f_reload_time = val.as_f32(),
            WeaponDataFieldId::MFAimedDamage => self.m_f_aimed_damage = val.as_f32(),
            WeaponDataFieldId::MFNextAimBonus => self.m_f_next_aim_bonus = val.as_f32(),
            WeaponDataFieldId::MFInZoom => self.m_f_in_zoom = val.as_i32(),
            WeaponDataFieldId::MIWeaponState => self.m_i_weapon_state = val.as_i32(),
            WeaponDataFieldId::IUser1 => self.iuser1 = val.as_i32(),
            WeaponDataFieldId::IUser2 => self.iuser2 = val.as_i32(),
            WeaponDataFieldId::IUser3 => self.iuser3 = val.as_i32(),
            WeaponDataFieldId::IUser4 => self.iuser4 = val.as_i32(),
            WeaponDataFieldId::FUser1 => self.fuser1 = val.as_f32(),
            WeaponDataFieldId::FUser2 => self.fuser2 = val.as_f32(),
            WeaponDataFieldId::FUser3 => self.fuser3 = val.as_f32(),
            WeaponDataFieldId::FUser4 => self.fuser4 = val.as_f32(),
            WeaponDataFieldId::Unknown => {}
        }
    }
}

impl GoldSrcWeaponData {
    /// The record of the fields the schema has.
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        to_delta_via_plan(self, schema)
    }

    pub fn from_delta(rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        from_delta_via_plan(Self::default(), rec, schema)
    }
}

/// High-level GoldSrc `entity_state_t` (`common/entity_state.h`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GoldSrcEntityState {
    pub number: u16,
    pub entity_type: u8,
    pub origin: Vec3,
    pub angles: Vec3,
    pub modelindex: u16,
    pub sequence: u16,
    pub gaitsequence: u16,
    pub frame: f32,
    pub animtime: f32,
    pub framerate: f32,
    pub weaponmodel: u16,
    pub movetype: u8,
    pub solid: u8,
    pub effects: u32,
    pub rendermode: u8,
    pub renderamt: u8,
    pub renderfx: u8,
    pub skin: i16,
    pub body: u8,
    pub scale: f32,
    pub owner: u16,
    pub aiment: u16,
    pub usehull: u8,
    pub spectator: bool,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub colormap: u16,
    /// `eflags` (`EFLAG_SLERP`: studio interpolation); 2 bits on the wire.
    pub eflags: u8,
    /// `rendercolor` (`color24`): red, green, blue.
    pub rendercolor: [u8; 3],
    /// The four bone controller positions a studio model is posed with.
    pub controller: [u8; 4],
    /// The two sequence blend weights; the client pitches a player model with `blending[0]`.
    /// (`entity_state_t::blending` has four bytes, the delta description names two.)
    pub blending: [u8; 2],
    /// Friction for prediction (`entity_state_player_t` only).
    pub friction: f32,
    /// Gravity multiplier for prediction (`entity_state_player_t` only).
    pub gravity: f32,
    /// Conveyor velocity the entity stands on.
    pub basevelocity: Vec3,
    /// Parametric movement overrides (`startpos`, `endpos`, `starttime`, `impacttime`).
    pub startpos: Vec3,
    pub endpos: Vec3,
    pub impacttime: f32,
    pub starttime: f32,
    pub iuser4: i32,
}

impl DeltaStruct for GoldSrcEntityState {
    type FieldId = EntityFieldId;

    #[inline]
    fn compiled_plan(schema: &DeltaSchema) -> &CompiledPlan<Self::FieldId> {
        schema.entity_plan()
    }

    #[inline]
    fn unknown_field_id() -> Self::FieldId {
        EntityFieldId::Unknown
    }

    #[inline]
    fn read_field(&self, id: Self::FieldId, base_type: u32, signed: bool) -> DeltaFieldRef<'_> {
        match id {
            EntityFieldId::AnimTime => DeltaFieldRef::Float(self.animtime),
            EntityFieldId::Frame => DeltaFieldRef::Float(self.frame),
            EntityFieldId::Origin0 => DeltaFieldRef::Float(self.origin.x),
            EntityFieldId::Origin1 => DeltaFieldRef::Float(self.origin.y),
            EntityFieldId::Origin2 => DeltaFieldRef::Float(self.origin.z),
            EntityFieldId::Angles0 => DeltaFieldRef::Float(self.angles.x),
            EntityFieldId::Angles1 => DeltaFieldRef::Float(self.angles.y),
            EntityFieldId::Angles2 => DeltaFieldRef::Float(self.angles.z),
            EntityFieldId::Sequence => DeltaFieldRef::uint_for(self.sequence as u32, signed),
            EntityFieldId::GaitSequence => {
                DeltaFieldRef::uint_for(self.gaitsequence as u32, signed)
            }
            EntityFieldId::ModelIndex => DeltaFieldRef::uint_for(self.modelindex as u32, signed),
            EntityFieldId::MoveType => DeltaFieldRef::uint_for(self.movetype as u32, signed),
            EntityFieldId::Solid => DeltaFieldRef::uint_for(self.solid as u32, signed),
            EntityFieldId::Mins0 => DeltaFieldRef::Float(self.mins.x),
            EntityFieldId::Mins1 => DeltaFieldRef::Float(self.mins.y),
            EntityFieldId::Mins2 => DeltaFieldRef::Float(self.mins.z),
            EntityFieldId::Maxs0 => DeltaFieldRef::Float(self.maxs.x),
            EntityFieldId::Maxs1 => DeltaFieldRef::Float(self.maxs.y),
            EntityFieldId::Maxs2 => DeltaFieldRef::Float(self.maxs.z),
            EntityFieldId::WeaponModel => DeltaFieldRef::uint_for(self.weaponmodel as u32, signed),
            EntityFieldId::Owner => DeltaFieldRef::uint_for(self.owner as u32, signed),
            EntityFieldId::Effects => DeltaFieldRef::uint_for(self.effects, signed),
            EntityFieldId::ColorMap => DeltaFieldRef::uint_for(self.colormap as u32, signed),
            EntityFieldId::FrameRate => DeltaFieldRef::Float(self.framerate),
            EntityFieldId::Skin => DeltaFieldRef::int_for(self.skin as i32, signed),
            EntityFieldId::Body => DeltaFieldRef::uint_for(self.body as u32, signed),
            EntityFieldId::RenderMode => DeltaFieldRef::uint_for(self.rendermode as u32, signed),
            EntityFieldId::RenderAmt => DeltaFieldRef::uint_for(self.renderamt as u32, signed),
            EntityFieldId::RenderFx => DeltaFieldRef::uint_for(self.renderfx as u32, signed),
            EntityFieldId::Scale => DeltaFieldRef::Float(self.scale),
            EntityFieldId::Aiment => DeltaFieldRef::uint_for(self.aiment as u32, signed),
            EntityFieldId::UseHull => DeltaFieldRef::uint_for(self.usehull as u32, signed),
            EntityFieldId::Spectator => DeltaFieldRef::uint_for(u32::from(self.spectator), signed),
            EntityFieldId::EFlags => DeltaFieldRef::uint_for(u32::from(self.eflags), signed),
            EntityFieldId::RenderColorR => {
                DeltaFieldRef::uint_for(u32::from(self.rendercolor[0]), signed)
            }
            EntityFieldId::RenderColorG => {
                DeltaFieldRef::uint_for(u32::from(self.rendercolor[1]), signed)
            }
            EntityFieldId::RenderColorB => {
                DeltaFieldRef::uint_for(u32::from(self.rendercolor[2]), signed)
            }
            EntityFieldId::Controller0 => {
                DeltaFieldRef::uint_for(u32::from(self.controller[0]), signed)
            }
            EntityFieldId::Controller1 => {
                DeltaFieldRef::uint_for(u32::from(self.controller[1]), signed)
            }
            EntityFieldId::Controller2 => {
                DeltaFieldRef::uint_for(u32::from(self.controller[2]), signed)
            }
            EntityFieldId::Controller3 => {
                DeltaFieldRef::uint_for(u32::from(self.controller[3]), signed)
            }
            EntityFieldId::Blending0 => {
                DeltaFieldRef::uint_for(u32::from(self.blending[0]), signed)
            }
            EntityFieldId::Blending1 => {
                DeltaFieldRef::uint_for(u32::from(self.blending[1]), signed)
            }
            EntityFieldId::Friction => DeltaFieldRef::Float(self.friction),
            EntityFieldId::Gravity => DeltaFieldRef::Float(self.gravity),
            EntityFieldId::BaseVelocity0 => DeltaFieldRef::Float(self.basevelocity.x),
            EntityFieldId::BaseVelocity1 => DeltaFieldRef::Float(self.basevelocity.y),
            EntityFieldId::BaseVelocity2 => DeltaFieldRef::Float(self.basevelocity.z),
            EntityFieldId::StartPos0 => DeltaFieldRef::Float(self.startpos.x),
            EntityFieldId::StartPos1 => DeltaFieldRef::Float(self.startpos.y),
            EntityFieldId::StartPos2 => DeltaFieldRef::Float(self.startpos.z),
            EntityFieldId::EndPos0 => DeltaFieldRef::Float(self.endpos.x),
            EntityFieldId::EndPos1 => DeltaFieldRef::Float(self.endpos.y),
            EntityFieldId::EndPos2 => DeltaFieldRef::Float(self.endpos.z),
            EntityFieldId::ImpactTime => DeltaFieldRef::Float(self.impacttime),
            EntityFieldId::StartTime => DeltaFieldRef::Float(self.starttime),
            EntityFieldId::IUser4 => DeltaFieldRef::int_for(self.iuser4, signed),
            EntityFieldId::Unknown => DeltaFieldRef::zero_for(base_type, signed),
        }
    }

    #[inline]
    fn write_field(&mut self, id: Self::FieldId, val: DecodedField) {
        match id {
            EntityFieldId::AnimTime => self.animtime = val.as_f32(),
            EntityFieldId::Frame => self.frame = val.as_f32(),
            EntityFieldId::Origin0 => self.origin.x = val.as_f32(),
            EntityFieldId::Origin1 => self.origin.y = val.as_f32(),
            EntityFieldId::Origin2 => self.origin.z = val.as_f32(),
            EntityFieldId::Angles0 => self.angles.x = val.as_f32(),
            EntityFieldId::Angles1 => self.angles.y = val.as_f32(),
            EntityFieldId::Angles2 => self.angles.z = val.as_f32(),
            EntityFieldId::Sequence => self.sequence = val.as_u32() as u16,
            EntityFieldId::GaitSequence => self.gaitsequence = val.as_u32() as u16,
            EntityFieldId::ModelIndex => self.modelindex = val.as_u32() as u16,
            EntityFieldId::MoveType => self.movetype = val.as_u32() as u8,
            EntityFieldId::Solid => self.solid = val.as_u32() as u8,
            EntityFieldId::Mins0 => self.mins.x = val.as_f32(),
            EntityFieldId::Mins1 => self.mins.y = val.as_f32(),
            EntityFieldId::Mins2 => self.mins.z = val.as_f32(),
            EntityFieldId::Maxs0 => self.maxs.x = val.as_f32(),
            EntityFieldId::Maxs1 => self.maxs.y = val.as_f32(),
            EntityFieldId::Maxs2 => self.maxs.z = val.as_f32(),
            EntityFieldId::WeaponModel => self.weaponmodel = val.as_u32() as u16,
            EntityFieldId::Owner => self.owner = val.as_u32() as u16,
            EntityFieldId::Effects => self.effects = val.as_u32(),
            EntityFieldId::ColorMap => self.colormap = val.as_u32() as u16,
            EntityFieldId::FrameRate => self.framerate = val.as_f32(),
            EntityFieldId::Skin => self.skin = val.as_i32() as i16,
            EntityFieldId::Body => self.body = val.as_u32() as u8,
            EntityFieldId::RenderMode => self.rendermode = val.as_u32() as u8,
            EntityFieldId::RenderAmt => self.renderamt = val.as_u32() as u8,
            EntityFieldId::RenderFx => self.renderfx = val.as_u32() as u8,
            EntityFieldId::Scale => self.scale = val.as_f32(),
            EntityFieldId::Aiment => self.aiment = val.as_u32() as u16,
            EntityFieldId::UseHull => self.usehull = val.as_u32() as u8,
            EntityFieldId::Spectator => self.spectator = val.as_u32() != 0,
            EntityFieldId::EFlags => self.eflags = val.as_u32() as u8,
            EntityFieldId::RenderColorR => self.rendercolor[0] = val.as_u32() as u8,
            EntityFieldId::RenderColorG => self.rendercolor[1] = val.as_u32() as u8,
            EntityFieldId::RenderColorB => self.rendercolor[2] = val.as_u32() as u8,
            EntityFieldId::Controller0 => self.controller[0] = val.as_u32() as u8,
            EntityFieldId::Controller1 => self.controller[1] = val.as_u32() as u8,
            EntityFieldId::Controller2 => self.controller[2] = val.as_u32() as u8,
            EntityFieldId::Controller3 => self.controller[3] = val.as_u32() as u8,
            EntityFieldId::Blending0 => self.blending[0] = val.as_u32() as u8,
            EntityFieldId::Blending1 => self.blending[1] = val.as_u32() as u8,
            EntityFieldId::Friction => self.friction = val.as_f32(),
            EntityFieldId::Gravity => self.gravity = val.as_f32(),
            EntityFieldId::BaseVelocity0 => self.basevelocity.x = val.as_f32(),
            EntityFieldId::BaseVelocity1 => self.basevelocity.y = val.as_f32(),
            EntityFieldId::BaseVelocity2 => self.basevelocity.z = val.as_f32(),
            EntityFieldId::StartPos0 => self.startpos.x = val.as_f32(),
            EntityFieldId::StartPos1 => self.startpos.y = val.as_f32(),
            EntityFieldId::StartPos2 => self.startpos.z = val.as_f32(),
            EntityFieldId::EndPos0 => self.endpos.x = val.as_f32(),
            EntityFieldId::EndPos1 => self.endpos.y = val.as_f32(),
            EntityFieldId::EndPos2 => self.endpos.z = val.as_f32(),
            EntityFieldId::ImpactTime => self.impacttime = val.as_f32(),
            EntityFieldId::StartTime => self.starttime = val.as_f32(),
            EntityFieldId::IUser4 => self.iuser4 = val.as_i32(),
            EntityFieldId::Unknown => {}
        }
    }
}

impl GoldSrcEntityState {
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        to_delta_via_plan(self, schema)
    }

    pub fn from_delta(number: u16, entity_type: u8, rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        from_delta_via_plan(
            Self {
                number,
                entity_type,
                ..Self::default()
            },
            rec,
            schema,
        )
    }
}

/// Built-in CS 1.6 `delta.lst` schema matching Valve's `cstrike/delta.lst`.
const CS16_DEFAULT_DELTA_LST: &str = r#"
clientdata_t none
{
	DEFINE_DELTA( flTimeStepSound, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 21, 128.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 21, 128.0 ),
	DEFINE_DELTA( velocity[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( velocity[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( m_flNextAttack, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 21, 128.0 ),
	DEFINE_DELTA( velocity[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( ammo_nails, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( ammo_shells, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( ammo_cells, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( ammo_rockets, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( m_iId, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( punchangle[2], DT_SIGNED | DT_FLOAT, 21, 8.0 ),
	DEFINE_DELTA( flags, DT_INTEGER, 32, 1.0 ),
	DEFINE_DELTA( weaponanim, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( health, DT_SIGNED | DT_FLOAT, 10, 1.0 ),
	DEFINE_DELTA( maxspeed, DT_FLOAT, 16, 10.0 ),
	DEFINE_DELTA( flDuckTime, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( view_ofs[2], DT_SIGNED | DT_FLOAT, 10, 4.0 ),
	DEFINE_DELTA( punchangle[0], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( punchangle[1], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( viewmodel, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( weapons, DT_INTEGER, 32, 1.0 ),
	DEFINE_DELTA( pushmsec, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( deadflag, DT_INTEGER, 3, 1.0 ),
	DEFINE_DELTA( fov, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA( physinfo, DT_STRING, 1, 1.0 ),
	DEFINE_DELTA( bInDuck, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( flSwimTime, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( waterjumptime, DT_INTEGER, 15, 1.0 ),
	DEFINE_DELTA( waterlevel, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( iuser1, DT_INTEGER, 3, 1.0 ),
	DEFINE_DELTA( iuser2, DT_INTEGER, 6, 1.0 ),
	DEFINE_DELTA( iuser3, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( iuser4, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( vuser2[0], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser2[1], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser2[2], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser3[0], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser3[1], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser3[2], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser4[0], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( vuser4[1], DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( fuser1, DT_FLOAT, 9, 1.0 ),
	DEFINE_DELTA( fuser2, DT_FLOAT, 14, 1.0 ),
	DEFINE_DELTA( fuser3, DT_FLOAT, 10, 1.0 )
}

entity_state_t gamedll Entity_Encode
{
	DEFINE_DELTA( animtime, DT_TIMEWINDOW_8, 8, 1.0 ),
	DEFINE_DELTA( frame, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( angles[0], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( angles[1], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( sequence, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( modelindex, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( movetype, DT_INTEGER, 4, 1.0 ),
	DEFINE_DELTA( solid, DT_SHORT, 3, 1.0 ),
	DEFINE_DELTA( mins[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( endpos[0], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( endpos[1], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( endpos[2], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( startpos[0], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( startpos[1], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( startpos[2], DT_SIGNED | DT_FLOAT, 13, 1.0 ),
	DEFINE_DELTA( impacttime, DT_TIMEWINDOW_BIG, 13, 100.0 ),
	DEFINE_DELTA( starttime, DT_TIMEWINDOW_BIG, 13, 100.0 ),
	DEFINE_DELTA( weaponmodel, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( owner, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( effects, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( eflags, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( angles[2], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( colormap, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( framerate, DT_SIGNED | DT_FLOAT, 8, 16.0 ),
	DEFINE_DELTA( skin, DT_SHORT | DT_SIGNED, 9, 1.0 ),
	DEFINE_DELTA( controller[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[2], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[3], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( body, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( rendermode, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderamt, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderfx, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( scale, DT_FLOAT, 16, 256.0 ),
	DEFINE_DELTA( rendercolor.r, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.g, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.b, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( aiment, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( basevelocity[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( iuser4, DT_INTEGER, 2, 1.0 )
}

entity_state_player_t gamedll Player_Encode
{
	DEFINE_DELTA( animtime, DT_TIMEWINDOW_8, 8, 1.0 ),
	DEFINE_DELTA( frame, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 18, 32.0 ),
	DEFINE_DELTA( angles[0], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( angles[1], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 18, 32.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 18, 32.0 ),
	DEFINE_DELTA( gaitsequence, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( sequence, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( modelindex, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( movetype, DT_INTEGER, 4, 1.0 ),
	DEFINE_DELTA( solid, DT_SHORT, 3, 1.0 ),
	DEFINE_DELTA( mins[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( mins[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( maxs[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( weaponmodel, DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( owner, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( effects, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( angles[2], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( colormap, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( framerate, DT_SIGNED | DT_FLOAT, 8, 16.0 ),
	DEFINE_DELTA( skin, DT_SHORT | DT_SIGNED, 9, 1.0 ),
	DEFINE_DELTA( controller[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[2], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( controller[3], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[0], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( blending[1], DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( body, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( rendermode, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderamt, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderfx, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( scale, DT_FLOAT, 16, 256.0 ),
	DEFINE_DELTA( rendercolor.r, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.g, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.b, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( friction, DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( usehull, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( gravity, DT_SIGNED | DT_FLOAT, 16, 32.0 ),
	DEFINE_DELTA( aiment, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( basevelocity[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( basevelocity[2], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( spectator, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( iuser4, DT_INTEGER, 2, 1.0 )
}

custom_entity_state_t gamedll Custom_Encode
{
	DEFINE_DELTA( rendermode, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( angles[0], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( angles[1], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( angles[2], DT_SIGNED | DT_FLOAT, 17, 8.0 ),
	DEFINE_DELTA( sequence, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( skin, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA( modelindex, DT_INTEGER, 16, 1.0 ),
	DEFINE_DELTA_POST( scale, DT_FLOAT, 8, 1.0, 0.1 ),
	DEFINE_DELTA( body, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.r, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.g, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( rendercolor.b, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( renderfx, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( renderamt, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( frame, DT_FLOAT, 8, 1.0 ),
	DEFINE_DELTA_POST( animtime, DT_FLOAT, 8, 1.0, 0.1 )
}

usercmd_t none
{
	DEFINE_DELTA( lerp_msec, DT_SHORT, 9, 1.0 ),
	DEFINE_DELTA( msec, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( viewangles[1], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( viewangles[0], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( buttons, DT_SHORT, 16, 1.0 ),
	DEFINE_DELTA( forwardmove, DT_SIGNED | DT_FLOAT, 12, 1.0 ),
	DEFINE_DELTA( lightlevel, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( sidemove, DT_SIGNED | DT_FLOAT, 12, 1.0 ),
	DEFINE_DELTA( upmove, DT_SIGNED | DT_FLOAT, 12, 1.0 ),
	DEFINE_DELTA( impulse, DT_BYTE, 8, 1.0 ),
	DEFINE_DELTA( viewangles[2], DT_ANGLE, 16, 1.0 ),
	DEFINE_DELTA( impact_index, DT_INTEGER, 6, 1.0 ),
	DEFINE_DELTA( impact_position[0], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( impact_position[1], DT_SIGNED | DT_FLOAT, 16, 8.0 ),
	DEFINE_DELTA( impact_position[2], DT_SIGNED | DT_FLOAT, 16, 8.0 )
}

weapon_data_t none
{
	DEFINE_DELTA( m_flTimeWeaponIdle, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_flNextPrimaryAttack, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_flNextReload, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_fNextAimBonus, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_flNextSecondaryAttack, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_iClip, DT_SIGNED | DT_INTEGER, 10, 1.0 ),
	DEFINE_DELTA( m_flPumpTime, DT_FLOAT | DT_SIGNED, 22, 1000.0 ),
	DEFINE_DELTA( m_fInSpecialReload, DT_INTEGER, 2, 1.0 ),
	DEFINE_DELTA( m_fReloadTime, DT_FLOAT, 16, 100.0 ),
	DEFINE_DELTA( m_fInReload, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( m_fAimedDamage, DT_FLOAT, 22, 1000.0 ),
	DEFINE_DELTA( m_fInZoom, DT_INTEGER, 8, 1.0 ),
	DEFINE_DELTA( m_iWeaponState, DT_INTEGER, 7, 1.0 ),
	DEFINE_DELTA( m_iId, DT_INTEGER, 5, 1.0 ),
	DEFINE_DELTA( fuser1, DT_SIGNED | DT_FLOAT, 22, 1000.0 ),
	DEFINE_DELTA( fuser2, DT_SIGNED | DT_FLOAT, 22, 128.0 ),
	DEFINE_DELTA( fuser3, DT_SIGNED | DT_FLOAT, 22, 128.0 ),
	DEFINE_DELTA( iuser1, DT_SIGNED | DT_INTEGER, 16, 128.0 )
}

event_t none
{
	DEFINE_DELTA( entindex, DT_INTEGER, 11, 1.0 ),
	DEFINE_DELTA( bparam1, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( bparam2, DT_INTEGER, 1, 1.0 ),
	DEFINE_DELTA( origin[0], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( origin[1], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( origin[2], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( fparam1, DT_FLOAT | DT_SIGNED, 20, 100.0 ),
	DEFINE_DELTA( fparam2, DT_FLOAT | DT_SIGNED, 20, 100.0 ),
	DEFINE_DELTA( iparam1, DT_INTEGER | DT_SIGNED, 18, 1.0 ),
	DEFINE_DELTA( iparam2, DT_INTEGER | DT_SIGNED, 18, 1.0 ),
	DEFINE_DELTA( angles[0], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( angles[1], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( angles[2], DT_SIGNED | DT_FLOAT, 26, 8192.0 ),
	DEFINE_DELTA( ducking, DT_INTEGER, 1, 1.0 )
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clientdata_fields_round_trip() {
        use crate::messages::{
            SvcMessage, SvcParseContext, parse_svc_stream, write_svc_clientdata,
            write_svc_deltadescriptions,
        };

        let reg = DeltaRegistry::cs16_default();
        let schema = reg.get("clientdata_t").unwrap();
        // Values inside the widths the schema gives (flSwimTime 10 bits, waterjumptime 15,
        // pushmsec 11, fuser1 9, fuser2 14, fuser3 10, the vuser axes 9); all of them unsigned.
        let data = GoldSrcClientData {
            fl_swim_time: 321,
            waterjumptime: 12_345,
            pushmsec: 1_500,
            fuser1: 77.0,
            fuser2: 9_000.0,
            fuser3: 500.0,
            vuser2: Vec3::new(1.0, 2.0, 3.0),
            vuser3: Vec3::new(4.0, 5.0, 255.0),
            // The schema has no third axis for `vuser4`.
            vuser4: Vec3::new(6.0, 7.0, 0.0),
            ..Default::default()
        };
        // In memory: `to_delta` then `from_delta`.
        assert_eq!(GoldSrcClientData::from_delta(&data.to_delta(schema), schema), data);
        // A zero record leaves them all zero.
        let zero = GoldSrcClientData::from_delta(&schema.zero_record(), schema);
        assert_eq!(zero, GoldSrcClientData::default());

        // Over the wire: `svc_clientdata` as the server writes it and the client parses it.
        let mut w = MsgWriter::new();
        write_svc_deltadescriptions(&mut w, &reg);
        write_svc_clientdata(&mut w, schema, None, &data, 1.0);
        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        let got = parsed
            .iter()
            .find_map(|m| match m {
                SvcMessage::ClientData { parsed, .. } => Some(parsed.clone()),
                _ => None,
            })
            .expect("the clientdata message");
        assert_eq!(got, data);
    }

    #[test]
    fn parses_real_cs16_delta_lst_and_roundtrips_meta_wire_and_records() {
        let reg = DeltaRegistry::cs16_default();
        assert_eq!(reg.iter().count(), 7);
        let player_schema = reg.get("entity_state_player_t").unwrap();
        assert_eq!(player_schema.fields.len(), 48);

        // If Steam CS 1.6 is installed locally, also verify parsing Valve's real cstrike/delta.lst.
        let steam_delta = r"D:\SteamLibrary\steamapps\common\Half-Life\cstrike\delta.lst";
        if let Ok(raw) = std::fs::read_to_string(steam_delta) {
            let parsed = DeltaRegistry::from_delta_lst(&raw).unwrap();
            assert_eq!(parsed.iter().count(), 7);
            assert_eq!(
                parsed.get("entity_state_player_t").unwrap().fields.len(),
                48
            );
            assert_eq!(parsed.get("usercmd_t").unwrap().fields.len(), 15);
            assert_eq!(parsed.get("weapon_data_t").unwrap().fields.len(), 18);
        }

        // Round-trip a full schema through `svc_deltadescription` (`g_MetaDelta`).
        let mut w = MsgWriter::new();
        write_delta_description(&mut w, player_schema);
        let mut r = MsgReader::new(w.as_slice());
        let decoded_schema = read_delta_description(&mut r).unwrap();
        assert_eq!(decoded_schema.name, player_schema.name);
        assert_eq!(decoded_schema.fields.len(), player_schema.fields.len());
        for (a, b) in player_schema.fields.iter().zip(decoded_schema.fields.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.field_type, b.field_type);
            assert_eq!(a.significant_bits, b.significant_bits);
            assert!((a.premultiply - b.premultiply).abs() < 0.001);
            assert!((a.postmultiply - b.postmultiply).abs() < 0.001);
        }

        // Round-trip a `GoldSrcEntityState` delta diff.
        let from_ent = GoldSrcEntityState::default();
        let to_ent = GoldSrcEntityState {
            number: 1,
            entity_type: 1,
            origin: Vec3::new(-456.25, 1024.5, 64.0),
            angles: Vec3::new(12.5, 180.0, 0.0),
            modelindex: 42,
            sequence: 7,
            gaitsequence: 4,
            weaponmodel: 19,
            animtime: 10.25,
            ..Default::default()
        };
        let from_rec = from_ent.to_delta(&decoded_schema);
        let to_rec = to_ent.to_delta(&decoded_schema);

        let mut dw = MsgWriter::new();
        dw.start_bit_writing();
        assert!(write_delta(
            &mut dw,
            &decoded_schema,
            &from_rec,
            &to_rec,
            false,
            10.30
        ));
        dw.end_bit_writing();

        let mut dr = MsgReader::new(dw.as_slice());
        dr.start_bit_reading();
        let out_rec = read_delta(&mut dr, &decoded_schema, &from_rec, 10.30).unwrap();
        dr.end_bit_reading().unwrap();

        let decoded_ent = GoldSrcEntityState::from_delta(1, 1, &out_rec, &decoded_schema);
        assert!((decoded_ent.origin - to_ent.origin).length() < 0.05);
        assert!((decoded_ent.angles.y - 180.0).abs() < 0.02);
        assert_eq!(decoded_ent.modelindex, 42);
        assert_eq!(decoded_ent.sequence, 7);
        assert_eq!(decoded_ent.weaponmodel, 19);
        assert!((decoded_ent.animtime - 10.25).abs() < 0.02);
    }

    /// Every field the `entity_state_t` and `entity_state_player_t` delta descriptions name goes
    /// through the wire and back: the blending, controllers, colormap, friction, gravity,
    /// render colour, base velocity, the parametric movement and `iuser4`.
    #[test]
    fn entity_state_wire_round_trips_every_field() {
        let reg = DeltaRegistry::cs16_default();
        let full = GoldSrcEntityState {
            number: 1,
            entity_type: 1,
            colormap: 1,
            eflags: 1,
            rendercolor: [11, 22, 33],
            controller: [127, 128, 129, 130],
            blending: [128, 127],
            friction: 1.0,
            gravity: 1.0,
            basevelocity: Vec3::new(8.0, -16.0, 24.5),
            startpos: Vec3::new(100.0, -200.0, 300.0),
            endpos: Vec3::new(-100.0, 200.0, -300.0),
            impacttime: 12.5,
            starttime: 11.25,
            iuser4: 3,
            usehull: 1,
            spectator: true,
            gaitsequence: 9,
            ..Default::default()
        };
        for (name, player) in [("entity_state_player_t", true), ("entity_state_t", false)] {
            let schema = reg.get(name).unwrap();
            let zero = GoldSrcEntityState::default().to_delta(schema);
            let to = full.to_delta(schema);
            let mut w = MsgWriter::new();
            w.start_bit_writing();
            assert!(write_delta(&mut w, schema, &zero, &to, true, 20.0));
            w.end_bit_writing();
            let mut r = MsgReader::new(w.as_slice());
            r.start_bit_reading();
            let out = read_delta(&mut r, schema, &zero, 20.0).unwrap();
            r.end_bit_reading().unwrap();
            let got = GoldSrcEntityState::from_delta(1, 1, &out, schema);
            assert_eq!(got.colormap, 1, "{name}");
            assert_eq!(got.rendercolor, [11, 22, 33], "{name}");
            assert_eq!(got.controller, [127, 128, 129, 130], "{name}");
            assert_eq!(got.blending, [128, 127], "{name}");
            assert_eq!(got.basevelocity, full.basevelocity, "{name}");
            assert_eq!(got.iuser4, 3, "{name}");
            if player {
                assert_eq!((got.friction, got.gravity), (1.0, 1.0));
                // `eflags` and the parametric movement are not in `entity_state_player_t`.
                assert_eq!(got.eflags, 0);
                assert_eq!((got.usehull, got.spectator, got.gaitsequence), (1, true, 9));
            } else {
                // Only `entity_state_player_t` names them.
                assert_eq!((got.friction, got.gravity), (0.0, 0.0));
                assert_eq!(got.eflags, 1);
                assert_eq!(got.startpos, full.startpos);
                assert_eq!(got.endpos, full.endpos);
                assert!((got.impacttime - 12.5).abs() < 0.02 && (got.starttime - 11.25).abs() < 0.02);
            }
        }
    }

    /// A delta of the new fields against a previous state that is not zero: only the fields that
    /// changed are on the wire, the rest are the previous record's.
    #[test]
    fn entity_state_delta_against_a_non_zero_previous_state() {
        let reg = DeltaRegistry::cs16_default();
        let schema = reg.get("entity_state_player_t").unwrap();
        let prev = GoldSrcEntityState {
            number: 1,
            entity_type: 1,
            colormap: 1,
            controller: [1, 2, 3, 4],
            blending: [128, 127],
            friction: 1.0,
            gravity: 1.0,
            basevelocity: Vec3::new(8.0, 0.0, 0.0),
            rendercolor: [5, 6, 7],
            iuser4: 1,
            ..Default::default()
        };
        let next = GoldSrcEntityState {
            blending: [200, 127],
            controller: [1, 2, 9, 4],
            friction: 0.5,
            gravity: 1.0,
            basevelocity: Vec3::new(8.0, 16.0, 0.0),
            rendercolor: [5, 60, 7],
            iuser4: 2,
            ..prev.clone()
        };
        let (from, to) = (prev.to_delta(schema), next.to_delta(schema));
        let mut w = MsgWriter::new();
        w.start_bit_writing();
        assert!(write_delta(&mut w, schema, &from, &to, false, 20.0));
        w.end_bit_writing();
        let mut r = MsgReader::new(w.as_slice());
        r.start_bit_reading();
        let out = read_delta(&mut r, schema, &from, 20.0).unwrap();
        r.end_bit_reading().unwrap();
        let got = GoldSrcEntityState::from_delta(1, 1, &out, schema);
        assert_eq!(got.blending, [200, 127]);
        assert_eq!(got.controller, [1, 2, 9, 4]);
        assert_eq!((got.friction, got.gravity), (0.5, 1.0));
        assert_eq!(got.basevelocity, next.basevelocity);
        assert_eq!(got.rendercolor, [5, 60, 7]);
        assert_eq!((got.iuser4, got.colormap), (2, 1));
        // Nothing changed: nothing to write.
        let mut w = MsgWriter::new();
        w.start_bit_writing();
        assert!(!write_delta(&mut w, schema, &to, &to, false, 20.0));
    }

    /// `svc_deltadescription` for `clientdata_t` carries the real C offsets of the 32-bit
    /// struct (`fieldOffset` of the meta delta), and the ReHLDS `fieldSize` of 1.
    #[test]
    fn clientdata_description_wire_carries_real_c_offsets() {
        let reg = DeltaRegistry::cs16_default();
        let schema = reg.get("clientdata_t").unwrap();
        let mut w = MsgWriter::new();
        write_delta_description(&mut w, schema);
        let mut r = MsgReader::new(w.as_slice());
        let on_wire = read_delta_description(&mut r).unwrap();
        assert_eq!(on_wire.name, "clientdata_t");
        let offset = |name: &str| {
            on_wire
                .fields
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("{name} not on the wire"))
                .field_offset
        };
        // The values IDA shows for the original client's clientdata_t.
        assert_eq!(offset("viewmodel"), 24);
        assert_eq!(offset("origin[0]"), 0);
        assert_eq!(offset("origin[2]"), 8);
        assert_eq!(offset("velocity[0]"), 12);
        assert_eq!(offset("punchangle[2]"), 36);
        assert_eq!(offset("view_ofs[2]"), 60);
        assert_eq!(offset("m_flNextAttack"), 124);
        assert_eq!(offset("physinfo"), 140);
        assert_eq!(offset("fuser3"), 420);
        assert_eq!(offset("vuser4[1]"), 468);
        assert!(on_wire.fields.iter().all(|f| f.field_size == 1));
        let entity = reg.get("entity_state_player_t").unwrap();
        assert_eq!(
            entity.fields.iter().find(|f| f.name == "modelindex").unwrap().field_offset,
            40
        );
    }

    /// Every described field of every built-in struct has a real, distinct-per-member offset.
    #[test]
    fn built_in_schemas_have_no_fake_offsets() {
        let reg = DeltaRegistry::cs16_default();
        for schema in reg.iter() {
            for f in &schema.fields {
                let layout = field_layout(&schema.name, &f.name)
                    .unwrap_or_else(|| panic!("{}.{} has no layout", schema.name, f.name));
                assert_eq!(f.field_offset, layout.offset, "{}.{}", schema.name, f.name);
            }
            let mut offsets: Vec<u16> = schema.fields.iter().map(|f| f.field_offset).collect();
            offsets.sort_unstable();
            offsets.dedup();
            assert_eq!(offsets.len(), schema.fields.len(), "{} reuses an offset", schema.name);
        }
    }

    /// The hard-coded meta delta agrees with the `delta_description_s` layout.
    #[test]
    fn meta_delta_offsets_match_the_c_struct() {
        for f in &meta_delta_schema().fields {
            assert_eq!(
                f.field_offset,
                field_layout("delta_description_t", &f.name).unwrap().offset,
                "{}",
                f.name
            );
        }
    }

    #[test]
    fn unknown_field_or_struct_is_a_parse_error() {
        let bad_field = "clientdata_t none
{
	DEFINE_DELTA( not_a_member, DT_INTEGER, 8, 1.0 )
}
";
        let err = parse_delta_lst(bad_field).unwrap_err().to_string();
        assert!(err.contains("not_a_member"), "{err}");
        let bad_struct = "mystery_t none
{
	DEFINE_DELTA( flags, DT_INTEGER, 8, 1.0 )
}
";
        assert!(parse_delta_lst(bad_struct).is_err());
    }

    struct Lcg(u64);
    impl Lcg {
        fn next_u32(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 32) as u32
        }
        fn f32_range(&mut self, lo: f32, hi: f32) -> f32 {
            let t = (self.next_u32() as f32) / (u32::MAX as f32);
            lo + (hi - lo) * t
        }
        fn i32_range(&mut self, lo: i32, hi: i32) -> i32 {
            let span = (hi - lo + 1) as u32;
            lo + (self.next_u32() % span) as i32
        }
    }

    fn random_entity(rng: &mut Lcg, number: u16, entity_type: u8) -> GoldSrcEntityState {
        GoldSrcEntityState {
            number,
            entity_type,
            origin: Vec3::new(
                rng.f32_range(-2000.0, 2000.0),
                rng.f32_range(-2000.0, 2000.0),
                rng.f32_range(-512.0, 512.0),
            ),
            angles: Vec3::new(
                rng.f32_range(-89.0, 89.0),
                rng.f32_range(0.0, 360.0),
                rng.f32_range(-45.0, 45.0),
            ),
            modelindex: (rng.next_u32() % 512) as u16,
            sequence: (rng.next_u32() % 180) as u16,
            gaitsequence: (rng.next_u32() % 64) as u16,
            frame: rng.f32_range(0.0, 255.0),
            animtime: rng.f32_range(10.0, 12.0),
            framerate: rng.f32_range(-4.0, 4.0),
            weaponmodel: (rng.next_u32() % 512) as u16,
            movetype: (rng.next_u32() % 14) as u8,
            solid: (rng.next_u32() % 6) as u8,
            effects: rng.next_u32() & 0xFF,
            rendermode: (rng.next_u32() % 64) as u8,
            renderamt: (rng.next_u32() & 0xFF) as u8,
            renderfx: (rng.next_u32() % 32) as u8,
            skin: rng.i32_range(-16, 16) as i16,
            body: (rng.next_u32() & 0xFF) as u8,
            scale: rng.f32_range(0.0, 10.0),
            owner: (rng.next_u32() % 32) as u16,
            aiment: (rng.next_u32() % 1024) as u16,
            usehull: (rng.next_u32() & 1) as u8,
            spectator: (rng.next_u32() & 1) != 0,
            mins: Vec3::new(-16.0, -16.0, -36.0),
            maxs: Vec3::new(16.0, 16.0, 36.0),
            colormap: (rng.next_u32() & 0xFFFF) as u16,
            eflags: (rng.next_u32() & 3) as u8,
            rendercolor: [
                rng.next_u32() as u8,
                rng.next_u32() as u8,
                rng.next_u32() as u8,
            ],
            controller: [
                rng.next_u32() as u8,
                rng.next_u32() as u8,
                rng.next_u32() as u8,
                rng.next_u32() as u8,
            ],
            blending: [rng.next_u32() as u8, rng.next_u32() as u8],
            friction: rng.f32_range(0.0, 4.0),
            gravity: rng.f32_range(-2.0, 2.0),
            basevelocity: Vec3::new(
                rng.f32_range(-128.0, 128.0),
                rng.f32_range(-128.0, 128.0),
                rng.f32_range(-64.0, 64.0),
            ),
            startpos: Vec3::new(
                rng.f32_range(-512.0, 512.0),
                rng.f32_range(-512.0, 512.0),
                rng.f32_range(-512.0, 512.0),
            ),
            endpos: Vec3::new(
                rng.f32_range(-512.0, 512.0),
                rng.f32_range(-512.0, 512.0),
                rng.f32_range(-512.0, 512.0),
            ),
            impacttime: rng.f32_range(10.0, 14.0),
            starttime: rng.f32_range(9.0, 13.0),
            iuser4: rng.i32_range(0, 3),
        }
    }

    fn random_clientdata(rng: &mut Lcg) -> GoldSrcClientData {
        let phys = match rng.next_u32() % 3 {
            0 => String::new(),
            1 => "\\slj\\1".to_string(),
            _ => "\\s\\0\\v\\250".to_string(),
        };
        GoldSrcClientData {
            origin: Vec3::new(
                rng.f32_range(-4000.0, 4000.0),
                rng.f32_range(-4000.0, 4000.0),
                rng.f32_range(-512.0, 512.0),
            ),
            velocity: Vec3::new(
                rng.f32_range(-500.0, 500.0),
                rng.f32_range(-500.0, 500.0),
                rng.f32_range(-300.0, 300.0),
            ),
            viewmodel: rng.i32_range(0, 511),
            punchangle: Vec3::new(
                rng.f32_range(-4.0, 4.0),
                rng.f32_range(-4.0, 4.0),
                rng.f32_range(-4.0, 4.0),
            ),
            flags: rng.next_u32(),
            waterlevel: rng.i32_range(0, 3),
            view_ofs: Vec3::new(0.0, 0.0, rng.f32_range(12.0, 28.0)),
            health: rng.f32_range(-10.0, 100.0),
            b_in_duck: rng.i32_range(0, 1),
            weapons: rng.next_u32(),
            ammo_nails: rng.i32_range(-50, 200),
            ammo_shells: rng.i32_range(-50, 200),
            ammo_cells: rng.i32_range(-50, 200),
            ammo_rockets: rng.i32_range(-50, 200),
            fl_time_step_sound: rng.i32_range(0, 900),
            fl_duck_time: rng.i32_range(0, 1000),
            maxspeed: rng.f32_range(100.0, 320.0),
            m_i_id: rng.i32_range(0, 31),
            m_fl_next_attack: rng.f32_range(-1.0, 2.0),
            fov: rng.f32_range(10.0, 120.0),
            weaponanim: rng.i32_range(0, 255),
            deadflag: rng.i32_range(0, 4),
            iuser1: rng.i32_range(0, 6),
            iuser2: rng.i32_range(0, 32),
            iuser3: rng.i32_range(0, 255),
            iuser4: rng.i32_range(0, 3),
            physinfo: phys,
            fl_swim_time: rng.i32_range(0, 1000),
            waterjumptime: rng.i32_range(0, 20000),
            pushmsec: rng.i32_range(0, 1500),
            fuser1: rng.f32_range(0.0, 255.0),
            fuser2: rng.f32_range(0.0, 4000.0),
            fuser3: rng.f32_range(0.0, 500.0),
            vuser2: Vec3::new(
                rng.f32_range(0.0, 200.0),
                rng.f32_range(0.0, 200.0),
                rng.f32_range(0.0, 200.0),
            ),
            vuser3: Vec3::new(
                rng.f32_range(0.0, 200.0),
                rng.f32_range(0.0, 200.0),
                rng.f32_range(0.0, 200.0),
            ),
            vuser4: Vec3::new(rng.f32_range(0.0, 200.0), rng.f32_range(0.0, 200.0), 0.0),
        }
    }

    fn random_weapondata(rng: &mut Lcg) -> GoldSrcWeaponData {
        GoldSrcWeaponData {
            m_i_id: rng.i32_range(0, 31),
            m_i_clip: rng.i32_range(-1, 100),
            m_fl_next_primary_attack: rng.f32_range(-1.0, 2.0),
            m_fl_next_secondary_attack: rng.f32_range(-1.0, 2.0),
            m_fl_time_weapon_idle: rng.f32_range(-1.0, 5.0),
            m_f_in_reload: rng.i32_range(0, 1),
            m_f_in_special_reload: rng.i32_range(0, 2),
            m_fl_next_reload: rng.f32_range(-1.0, 3.0),
            m_fl_pump_time: rng.f32_range(-1.0, 2.0),
            m_f_reload_time: rng.f32_range(0.0, 4.0),
            m_f_aimed_damage: rng.f32_range(0.0, 100.0),
            m_f_next_aim_bonus: rng.f32_range(-1.0, 2.0),
            m_f_in_zoom: rng.i32_range(0, 90),
            m_i_weapon_state: rng.i32_range(0, 64),
            iuser1: rng.i32_range(-100, 100),
            iuser2: 0,
            iuser3: 0,
            iuser4: 0,
            fuser1: rng.f32_range(-10.0, 10.0),
            fuser2: rng.f32_range(-100.0, 100.0),
            fuser3: rng.f32_range(-100.0, 100.0),
            fuser4: 0.0,
        }
    }

    fn random_usercmd(rng: &mut Lcg) -> GoldSrcUserCmd {
        GoldSrcUserCmd {
            lerp_msec: rng.i32_range(0, 100) as i16,
            msec: (rng.next_u32() % 50) as u8,
            viewangles: Vec3::new(
                rng.f32_range(-89.0, 89.0),
                rng.f32_range(0.0, 360.0),
                rng.f32_range(-20.0, 20.0),
            ),
            forwardmove: rng.f32_range(-400.0, 400.0),
            sidemove: rng.f32_range(-400.0, 400.0),
            upmove: rng.f32_range(-400.0, 400.0),
            lightlevel: (rng.next_u32() & 0xFF) as u8,
            buttons: (rng.next_u32() & 0xFFFF) as u16,
            impulse: (rng.next_u32() & 0xFF) as u8,
            weaponselect: (rng.next_u32() % 32) as u8,
            impact_index: rng.i32_range(0, 63),
            impact_position: Vec3::new(
                rng.f32_range(-1024.0, 1024.0),
                rng.f32_range(-1024.0, 1024.0),
                rng.f32_range(-512.0, 512.0),
            ),
        }
    }

    /// Proves 100% bit-for-bit wire equivalence between zero-alloc `write_struct_delta` /
    /// `read_struct_delta` and legacy `to_delta() + write_delta()` / `read_delta() + from_delta()`
    /// across all 6 CS 1.6 schemas (`entity_state_t`, `entity_state_player_t`,
    /// `custom_entity_state_t`, `clientdata_t`, `weapon_data_t`, `usercmd_t`).
    #[test]
    fn compiled_plan_struct_delta_is_bit_identical_across_all_six_schemas() {
        let reg = DeltaRegistry::cs16_default();
        let mut rng = Lcg(0xC516_CAFE_BABE_1021);
        let time_base = 12.34f64;

        // 1-3: Entity schemas (`entity_state_t`, `entity_state_player_t`, `custom_entity_state_t`).
        for (schema_name, ent_type) in [
            ("entity_state_t", 1u8),
            ("entity_state_player_t", 1u8),
            ("custom_entity_state_t", 2u8),
        ] {
            let schema = reg.get(schema_name).unwrap();
            let mut prev_state = GoldSrcEntityState {
                number: 7,
                entity_type: ent_type,
                ..Default::default()
            };
            let mut prev_rec = prev_state.to_delta(schema);

            for iter in 0..32usize {
                let to_state = if iter % 5 == 0 {
                    prev_state.clone()
                } else {
                    random_entity(&mut rng, 7, ent_type)
                };
                let to_rec = to_state.to_delta(schema);
                let force = iter % 2 == 0;

                // Unencoded bit-for-bit check.
                let mut w_legacy = MsgWriter::new();
                w_legacy.start_bit_writing();
                let changed_legacy =
                    write_delta(&mut w_legacy, schema, &prev_rec, &to_rec, force, time_base);
                w_legacy.end_bit_writing();

                let mut w_fast = MsgWriter::new();
                w_fast.start_bit_writing();
                let changed_fast = write_struct_delta(
                    &mut w_fast,
                    schema,
                    &prev_state,
                    &to_state,
                    force,
                    time_base,
                );
                w_fast.end_bit_writing();

                assert_eq!(changed_fast, changed_legacy, "{schema_name} iter {iter}");
                assert_eq!(
                    w_fast.as_slice(),
                    w_legacy.as_slice(),
                    "{schema_name} wire mismatch iter {iter}"
                );

                // Hybrid struct-vs-record bit-for-bit check.
                let mut w_hybrid = MsgWriter::new();
                w_hybrid.start_bit_writing();
                let changed_hybrid = write_struct_delta_from_record(
                    &mut w_hybrid,
                    schema,
                    &prev_rec,
                    &to_state,
                    force,
                    time_base,
                );
                w_hybrid.end_bit_writing();
                assert_eq!(changed_hybrid, changed_legacy, "{schema_name} hybrid iter {iter}");
                assert_eq!(w_hybrid.as_slice(), w_legacy.as_slice(), "{schema_name} hybrid wire");

                // Conditional encoder check using stack DeltaMarks bitmask helpers.
                let mut w_enc_legacy = MsgWriter::new();
                w_enc_legacy.start_bit_writing();
                let mut enc_l = |_: &DeltaRecord, _: &DeltaRecord, marks: &mut DeltaMarks<'_>| {
                    marks.unset_mask(marks.entity_origin_mask());
                    if iter % 3 == 0 {
                        marks.set_mask(marks.entity_angles_mask());
                    }
                };
                let c_el = write_delta_encoded(
                    &mut w_enc_legacy,
                    schema,
                    &prev_rec,
                    &to_rec,
                    force,
                    time_base,
                    Some(&mut enc_l),
                );
                w_enc_legacy.end_bit_writing();

                let mut w_enc_fast = MsgWriter::new();
                w_enc_fast.start_bit_writing();
                let mut enc_f = |_: &GoldSrcEntityState,
                                 _: &GoldSrcEntityState,
                                 marks: &mut DeltaMarks<'_>| {
                    marks.unset_mask(marks.entity_origin_mask());
                    if iter % 3 == 0 {
                        marks.set_mask(marks.entity_angles_mask());
                    }
                };
                let c_ef = write_struct_delta_encoded(
                    &mut w_enc_fast,
                    schema,
                    &prev_state,
                    &to_state,
                    force,
                    time_base,
                    Some(&mut enc_f),
                );
                w_enc_fast.end_bit_writing();
                assert_eq!(c_ef, c_el, "{schema_name} encoded changed iter {iter}");
                assert_eq!(w_enc_fast.as_slice(), w_enc_legacy.as_slice());

                // Decode equivalence when bytes were emitted.
                if changed_legacy || force {
                    let mut r_legacy = MsgReader::new(w_legacy.as_slice());
                    r_legacy.start_bit_reading();
                    let dec_rec = read_delta(&mut r_legacy, schema, &prev_rec, time_base).unwrap();
                    r_legacy.end_bit_reading().unwrap();
                    let dec_legacy =
                        GoldSrcEntityState::from_delta(7, ent_type, &dec_rec, schema);

                    let mut r_fast = MsgReader::new(w_fast.as_slice());
                    r_fast.start_bit_reading();
                    let dec_fast =
                        read_struct_delta(&mut r_fast, schema, &prev_state, time_base).unwrap();
                    r_fast.end_bit_reading().unwrap();

                    assert_eq!(dec_fast, dec_legacy, "{schema_name} decode mismatch iter {iter}");
                    // Also verify hybrid `write_struct_delta_from_record` against raw decoded
                    // `DeltaRecord` (`dec_rec`) directly.
                    let mut w_rec_l = MsgWriter::new();
                    w_rec_l.start_bit_writing();
                    let cl_dec = write_delta(&mut w_rec_l, schema, &dec_rec, &to_rec, force, time_base);
                    w_rec_l.end_bit_writing();
                    let mut w_rec_f = MsgWriter::new();
                    w_rec_f.start_bit_writing();
                    let cf_dec = write_struct_delta_from_record(
                        &mut w_rec_f,
                        schema,
                        &dec_rec,
                        &to_state,
                        force,
                        time_base,
                    );
                    w_rec_f.end_bit_writing();
                    assert_eq!(cf_dec, cl_dec, "{schema_name} dec_rec hybrid iter {iter}");
                    assert_eq!(w_rec_f.as_slice(), w_rec_l.as_slice());

                    prev_state = dec_fast;
                    prev_rec = prev_state.to_delta(schema);
                }
            }
        }

        // 4: `clientdata_t`.
        {
            let schema = reg.get("clientdata_t").unwrap();
            let mut prev_state = GoldSrcClientData::default();
            let mut prev_rec = prev_state.to_delta(schema);
            for iter in 0..32usize {
                let to_state = if iter % 5 == 0 {
                    prev_state.clone()
                } else {
                    random_clientdata(&mut rng)
                };
                let to_rec = to_state.to_delta(schema);
                let force = iter % 2 == 0;

                let mut w_legacy = MsgWriter::new();
                w_legacy.start_bit_writing();
                let c_l = write_delta(&mut w_legacy, schema, &prev_rec, &to_rec, force, time_base);
                w_legacy.end_bit_writing();

                let mut w_fast = MsgWriter::new();
                w_fast.start_bit_writing();
                let c_f =
                    write_struct_delta(&mut w_fast, schema, &prev_state, &to_state, force, time_base);
                w_fast.end_bit_writing();

                assert_eq!(c_f, c_l, "clientdata_t iter {iter}");
                assert_eq!(w_fast.as_slice(), w_legacy.as_slice(), "clientdata_t wire iter {iter}");

                if c_l || force {
                    let mut r_l = MsgReader::new(w_legacy.as_slice());
                    r_l.start_bit_reading();
                    let dec_rec = read_delta(&mut r_l, schema, &prev_rec, time_base).unwrap();
                    r_l.end_bit_reading().unwrap();
                    let dec_l = GoldSrcClientData::from_delta(&dec_rec, schema);

                    let mut r_f = MsgReader::new(w_fast.as_slice());
                    r_f.start_bit_reading();
                    let dec_f = read_struct_delta(&mut r_f, schema, &prev_state, time_base).unwrap();
                    r_f.end_bit_reading().unwrap();

                    assert_eq!(dec_f, dec_l, "clientdata_t decode iter {iter}");
                    prev_state = dec_f;
                    prev_rec = dec_rec;
                }
            }
        }

        // 5: `weapon_data_t`.
        {
            let schema = reg.get("weapon_data_t").unwrap();
            let mut prev_state = GoldSrcWeaponData::default();
            let mut prev_rec = prev_state.to_delta(schema);
            for iter in 0..32usize {
                let to_state = if iter % 5 == 0 {
                    prev_state
                } else {
                    random_weapondata(&mut rng)
                };
                let to_rec = to_state.to_delta(schema);
                let force = iter % 2 == 0;

                let mut w_legacy = MsgWriter::new();
                w_legacy.start_bit_writing();
                let c_l = write_delta(&mut w_legacy, schema, &prev_rec, &to_rec, force, time_base);
                w_legacy.end_bit_writing();

                let mut w_fast = MsgWriter::new();
                w_fast.start_bit_writing();
                let c_f =
                    write_struct_delta(&mut w_fast, schema, &prev_state, &to_state, force, time_base);
                w_fast.end_bit_writing();

                assert_eq!(c_f, c_l, "weapon_data_t iter {iter}");
                assert_eq!(w_fast.as_slice(), w_legacy.as_slice(), "weapon_data_t wire iter {iter}");

                if c_l || force {
                    let mut r_l = MsgReader::new(w_legacy.as_slice());
                    r_l.start_bit_reading();
                    let dec_rec = read_delta(&mut r_l, schema, &prev_rec, time_base).unwrap();
                    r_l.end_bit_reading().unwrap();
                    let dec_l = GoldSrcWeaponData::from_delta(&dec_rec, schema);

                    let mut r_f = MsgReader::new(w_fast.as_slice());
                    r_f.start_bit_reading();
                    let dec_f = read_struct_delta(&mut r_f, schema, &prev_state, time_base).unwrap();
                    r_f.end_bit_reading().unwrap();

                    assert_eq!(dec_f, dec_l, "weapon_data_t decode iter {iter}");
                    prev_state = dec_f;
                    prev_rec = dec_rec;
                }
            }
        }

        // 6: `usercmd_t`.
        {
            let schema = reg.get("usercmd_t").unwrap();
            let mut prev_state = GoldSrcUserCmd::default();
            let mut prev_rec = prev_state.to_delta(schema);
            for iter in 0..32usize {
                let to_state = if iter % 5 == 0 {
                    prev_state.clone()
                } else {
                    random_usercmd(&mut rng)
                };
                let to_rec = to_state.to_delta(schema);
                let force = iter % 2 == 0;

                let mut w_legacy = MsgWriter::new();
                w_legacy.start_bit_writing();
                let c_l = write_delta(&mut w_legacy, schema, &prev_rec, &to_rec, force, 0.0);
                w_legacy.end_bit_writing();

                let mut w_fast = MsgWriter::new();
                w_fast.start_bit_writing();
                let c_f =
                    write_struct_delta(&mut w_fast, schema, &prev_state, &to_state, force, 0.0);
                w_fast.end_bit_writing();

                assert_eq!(c_f, c_l, "usercmd_t iter {iter}");
                assert_eq!(w_fast.as_slice(), w_legacy.as_slice(), "usercmd_t wire iter {iter}");

                if c_l || force {
                    let mut r_l = MsgReader::new(w_legacy.as_slice());
                    r_l.start_bit_reading();
                    let dec_rec = read_delta(&mut r_l, schema, &prev_rec, 0.0).unwrap();
                    r_l.end_bit_reading().unwrap();
                    let dec_l = GoldSrcUserCmd::from_delta(&dec_rec, schema);

                    let mut r_f = MsgReader::new(w_fast.as_slice());
                    r_f.start_bit_reading();
                    let dec_f = read_struct_delta(&mut r_f, schema, &prev_state, 0.0).unwrap();
                    r_f.end_bit_reading().unwrap();

                    assert_eq!(dec_f, dec_l, "usercmd_t decode iter {iter}");
                    prev_state = dec_f;
                    prev_rec = dec_rec;
                }
            }
        }
    }

    /// Unknown custom mod fields and duplicate field names in a schema degrade gracefully to zero
    /// and match `DeltaRecord` wire output bit-for-bit.
    #[test]
    fn compiled_plan_handles_unknown_and_duplicate_fields() {
        let custom = DeltaSchema::new(
            "custom_mod_entity".to_string(),
            None,
            vec![
                DeltaFieldDesc {
                    field_type: DT_SIGNED | DT_FLOAT,
                    name: "origin[0]".to_string(),
                    field_offset: 0,
                    field_size: 1,
                    significant_bits: 16,
                    premultiply: 8.0,
                    postmultiply: 1.0,
                },
                DeltaFieldDesc {
                    field_type: DT_STRING,
                    name: "mod_custom_tag".to_string(),
                    field_offset: 4,
                    field_size: 1,
                    significant_bits: 1,
                    premultiply: 1.0,
                    postmultiply: 1.0,
                },
                DeltaFieldDesc {
                    field_type: DT_SIGNED | DT_FLOAT,
                    name: "origin[0]".to_string(),
                    field_offset: 8,
                    field_size: 1,
                    significant_bits: 16,
                    premultiply: 8.0,
                    postmultiply: 1.0,
                },
                DeltaFieldDesc {
                    field_type: DT_INTEGER,
                    name: "modelindex".to_string(),
                    field_offset: 12,
                    field_size: 1,
                    significant_bits: 10,
                    premultiply: 1.0,
                    postmultiply: 1.0,
                },
            ],
        );

        let from = GoldSrcEntityState::default();
        let to = GoldSrcEntityState {
            origin: Vec3::new(128.5, 0.0, 0.0),
            modelindex: 77,
            ..Default::default()
        };
        let from_rec = from.to_delta(&custom);
        let to_rec = to.to_delta(&custom);

        let mut w_l = MsgWriter::new();
        w_l.start_bit_writing();
        assert!(write_delta(&mut w_l, &custom, &from_rec, &to_rec, false, 0.0));
        w_l.end_bit_writing();

        let mut w_f = MsgWriter::new();
        w_f.start_bit_writing();
        assert!(write_struct_delta(&mut w_f, &custom, &from, &to, false, 0.0));
        w_f.end_bit_writing();

        assert_eq!(w_f.as_slice(), w_l.as_slice());

        // Also ensure unknown string field on the wire is skipped cleanly by `read_struct_delta`.
        let mut rec_with_tag = to_rec.clone();
        rec_with_tag.set_str(&custom, "mod_custom_tag", "hello_mod");
        let mut w_tag = MsgWriter::new();
        w_tag.start_bit_writing();
        assert!(write_delta(&mut w_tag, &custom, &from_rec, &rec_with_tag, false, 0.0));
        w_tag.end_bit_writing();

        let mut r = MsgReader::new(w_tag.as_slice());
        r.start_bit_reading();
        let decoded = read_struct_delta(&mut r, &custom, &from, 0.0).unwrap();
        r.end_bit_reading().unwrap();
        assert!((decoded.origin.x - 128.5).abs() < 0.2);
        assert_eq!(decoded.modelindex, 77);
    }
}
