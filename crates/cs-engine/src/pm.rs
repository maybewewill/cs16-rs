//! The server's side of the player movement services: building the `physents` and `moveents` of
//! one player's command from edicts (`SV_AddLinksToPM`'s output, `SV_CopyEdictToPhysent`, read
//! 2026-11-22 from `rehlds/engine/sv_user.cpp`). The traces, position tests, point contents,
//! ladders and the player hull tables themselves are in `cs-pm` (`cs_pm::physent`), which has no
//! engine in it so that a client can run them over its own entities; they are re-exported here
//! under their old names. The entry points on [`EngineApi`](crate::EngineApi) are in
//! `api_entities.rs`.

use glam::Vec3;

use crate::collision::WorldCollision;
use crate::edict::{EdictId, EntVars};
use crate::trace_ents::brush_model;
use crate::world::consts::{SOLID_BBOX, SOLID_BSP, SOLID_NOT};

pub use cs_pm::physent::{
    CONTENTS_LADDER, MAX_MOVEENTS, MAX_PHYSENTS, PM_GLASS_IGNORE, PM_NORMAL, PM_PLAYER_MAXS,
    PM_PLAYER_MINS, PM_STUDIO_BOX, PM_STUDIO_IGNORE, PM_WORLD_ONLY, PhysEntView, PmTraceResult,
};
pub(crate) use cs_pm::physent::{
    PhysEnt, PhysModel, ladder_contains, player_trace, point_contents, test_player_position,
    trace_model, world_physent,
};

/// Reach, in units each way from the player's origin, of `SV_AddLinksToPM`.
const PM_REACH: f32 = 256.0;

/// `SV_AddLinksToPM` snapshot for one player's command: physents (world first) and moveents.
/// The entries are `cs-pm`'s; this keeps the edict behind each physent next to them.
#[derive(Clone, Debug, Default)]
pub struct PmPhysEnts {
    ents: cs_pm::physent::PmPhysEnts,
    /// The edict of each of `ents.physents`, in the same order.
    edicts: Vec<EdictId>,
}

impl PmPhysEnts {
    /// `numphysent`.
    pub fn num_physents(&self) -> usize {
        self.ents.num_physents()
    }

    /// `nummoveent`.
    pub fn num_moveents(&self) -> usize {
        self.ents.num_moveents()
    }

    /// Physent `index` (`physents[index]`), `None` when out of range.
    pub fn physent(&self, index: i32) -> Option<PhysEntView> {
        self.ents.physent(index)
    }

    /// Moveent `index` (`moveents[index]`), `None` when out of range.
    pub fn moveent(&self, index: usize) -> Option<PhysEntView> {
        self.ents.moveent(index)
    }

    /// The edict behind `physents[index].info` (`EDICT_NUM(info)`), `None` for a bad index.
    pub fn edict_of(&self, index: i32) -> Option<EdictId> {
        let index = usize::try_from(index).ok()?;
        self.edicts.get(index).copied()
    }

    pub(crate) fn push_physent(&mut self, pe: PhysEnt, edict: EdictId) {
        self.ents.push_physent(pe);
        self.edicts.push(edict);
    }

    pub(crate) fn push_moveent(&mut self, pe: PhysEnt) {
        self.ents.push_moveent(pe);
    }

    pub(crate) fn physents_slice(&self) -> &[PhysEnt] {
        self.ents.physents_slice()
    }

    pub(crate) fn moveent_at(&self, index: usize) -> Option<&PhysEnt> {
        self.ents.moveent_at(index)
    }
}

/// The physent a brush or studio `model` of entity `v` makes: a brush model of the map, or
/// "some other model".
fn model_of(v: &EntVars, coll: Option<&WorldCollision>) -> PhysModel {
    match brush_model(v).filter(|&m| coll.is_some_and(|c| m < c.model_count())) {
        Some(m) => PhysModel::Brush(m),
        None => PhysModel::Other,
    }
}

/// `SV_CopyEdictToPhysent` (`rehlds/engine/sv_user.cpp`, read 2026-11-22) for entity `id`.
/// `client_slot` is "edict number in `1..=maxclients`" (then `player` is the edict number and
/// the origin is `true_origin` when lag compensation moved the player).
pub(crate) fn physent_from_edict(
    id: EdictId,
    v: &EntVars,
    client_slot: bool,
    true_origin: Option<Vec3>,
    coll: Option<&WorldCollision>,
) -> PhysEnt {
    let info = i32::from(id.index);
    let origin = if client_slot {
        true_origin.unwrap_or(v.origin)
    } else {
        v.origin
    };
    let (model, mins, maxs) = match v.solid {
        SOLID_BSP => (model_of(v, coll), Vec3::ZERO, Vec3::ZERO),
        SOLID_NOT => {
            let model = if v.modelindex != 0 {
                model_of(v, coll)
            } else {
                PhysModel::None
            };
            (model, Vec3::ZERO, Vec3::ZERO)
        }
        // `SOLID_BBOX`: the model is only kept for studio hitboxes (unsupported, module doc).
        SOLID_BBOX => (PhysModel::None, v.mins, v.maxs),
        _ => (PhysModel::None, v.mins, v.maxs),
    };
    // The bounds are the loaded ones, with the one-unit spread of `Mod_LoadSubmodels` (see
    // `WorldCollision::from_bsp`), as `PM_GetModelBounds` reads them in the original.
    let (model_mins, model_maxs) = match (model, coll) {
        (PhysModel::Brush(m), Some(c)) => c.model_bounds(m).unwrap_or((Vec3::ZERO, Vec3::ZERO)),
        _ => (Vec3::ZERO, Vec3::ZERO),
    };
    PhysEnt {
        view: PhysEntView {
            info,
            player: if client_slot { info } else { 0 },
            origin,
            angles: v.angles,
            mins,
            maxs,
            solid: i32::from(v.solid),
            skin: i32::from(v.skin),
            rendermode: i32::from(v.rendermode),
            movetype: i32::from(v.movetype),
            is_brush_model: matches!(model, PhysModel::Brush(_)),
            model_mins,
            model_maxs,
        },
        model,
    }
}

/// `SV_AddLinksToPM`'s reach box around `origin`: `origin +- 256`.
pub(crate) fn reach_box(origin: Vec3) -> (Vec3, Vec3) {
    let reach = Vec3::splat(PM_REACH);
    (origin - reach, origin + reach)
}
