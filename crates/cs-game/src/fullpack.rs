//! `AddToFullPack`: which entities a client is sent (ReGameDLL_CS `regamedll/dlls/client.cpp`,
//! vanilla branch, read 2026-11-22). The engine builds the plain `entity_state_t` copy of the
//! edict ([`cs_engine::entity_state_from_vars`]) and asks; this decides, in the order of the
//! source:
//!
//! 1. `EF_NODRAW` on an entity that is not the host;
//! 2. no model (`!ent->v.modelindex`; `!STRING(ent->v.model)` is never true, `STRING(0)` is an
//!    empty string, not a null pointer);
//! 3. `FL_SPECTATOR` on an entity that is not the host;
//! 4. the PVS history of the host: when the leaves the host edict touches changed, everything the
//!    history knows is forgotten (`CheckPlayerPVSLeafChanged`, `ResetPlayerPVS`);
//! 5. for an entity that is not the host: unless it was in the host's PVS less than a second ago
//!    (`CheckEntityRecentlyInPVS`: it stays in the packet), it must be visible
//!    (`ENGINE_CHECK_VISIBILITY`, the engine's answer in [`FullPackCtx::in_pvs`]); the history
//!    notes when it was seen (`MarkEntityInPVS`);
//! 6. `FL_SKIPLOCALHOST` on an entity the host owns, for a host whose client predicts weapons
//!    (`hostflags & 1`).
//!
//! Not ported: the `groupinfo` filter (`UTIL_SetGroupTrace`; the engine has no `groupinfo`, and
//! nothing in the vanilla game sets it) and everything under `REGAMEDLL_ADD`
//! (`EF_OWNER_NO_VISIBILITY`, `EF_FORCEVISIBILITY`, `EF_NOSLERP`, ...).

use cs_engine::consts::{EF_NODRAW, FL_SKIPLOCALHOST, FL_SPECTATOR};
use cs_engine::{EngineApi, FullPackCtx};

/// `ENTITYPVSSTATUS` and `PLAYERPVSSTATUS` of the source (`g_PVSStatus`): per client, the leaves
/// of its edict when the history was last reset, and per entity the time it was last seen in the
/// client's PVS (0: not seen).
#[derive(Debug, Default)]
pub struct PvsHistory {
    clients: Vec<ClientPvs>,
}

#[derive(Debug, Default)]
struct ClientPvs {
    headnode: i32,
    leafnums: Vec<i32>,
    /// `m_fTimeEnteredPVS` by entity number.
    entered: Vec<f32>,
}

impl PvsHistory {
    /// `CheckPlayerPVSLeafChanged` and, when it is true, `ResetPlayerPVS`: a host whose edict
    /// touches other leaves than at the last reset starts a history over. The history of a client
    /// that never asked is the zero-filled `g_PVSStatus` (`headnode` 0, no leaves).
    fn client(&mut self, slot: usize, headnode: i32, leafnums: &[i32]) -> &mut ClientPvs {
        if self.clients.len() <= slot {
            self.clients.resize_with(slot + 1, ClientPvs::default);
        }
        let pvs = &mut self.clients[slot];
        if pvs.headnode != headnode || pvs.leafnums != leafnums {
            pvs.headnode = headnode;
            pvs.leafnums = leafnums.to_vec();
            pvs.entered.clear();
        }
        pvs
    }

    /// Forgets every client (a new map).
    pub fn clear(&mut self) {
        self.clients.clear();
    }
}

impl ClientPvs {
    /// `CheckEntityRecentlyInPVS`: `es->m_fTimeEnteredPVS && es->m_fTimeEnteredPVS + 1.0f >=
    /// currenttime`, in `float`.
    fn recently_in_pvs(&self, ent: usize, now: f32) -> bool {
        let entered = self.entered.get(ent).copied().unwrap_or(0.0);
        entered != 0.0 && entered + 1.0 >= now
    }

    /// `MarkEntityInPVS`: the time the entity was seen, 0 when it was not.
    fn mark(&mut self, ent: usize, now: f32, in_pvs: bool) {
        if self.entered.len() <= ent {
            self.entered.resize(ent + 1, 0.0);
        }
        self.entered[ent] = if in_pvs { now } else { 0.0 };
    }
}

/// `AddToFullPack` (client.cpp), the filters: whether `ctx.ent` is sent to `ctx.viewer`. The
/// state itself is the engine's copy of the edict, which already holds what the rest of the
/// function sets.
pub fn add_to_full_pack(history: &mut PvsHistory, eng: &EngineApi<'_>, ctx: &FullPackCtx<'_>) -> bool {
    let (Some(ent), Some(_host)) = (eng.ent(ctx.ent_id), eng.ent(ctx.viewer_id)) else {
        return false;
    };
    let is_host = ctx.ent_id == ctx.viewer_id;

    if !is_host && ent.effects & EF_NODRAW != 0 {
        return false;
    }
    if ent.modelindex == 0 {
        return false;
    }
    if ent.flags & FL_SPECTATOR != 0 && !is_host {
        return false;
    }

    // `gpGlobals->time`, a `float`.
    let now = eng.time() as f32;
    let pvs = history.client(
        usize::from(ctx.viewer),
        ctx.viewer_leaves.headnode,
        &ctx.viewer_leaves.leafnums,
    );
    if !is_host && !pvs.recently_in_pvs(usize::from(ctx.ent), now) {
        if !ctx.in_pvs {
            pvs.mark(usize::from(ctx.ent), now, false);
            return false;
        }
        pvs.mark(usize::from(ctx.ent), now, true);
    }

    !(ent.flags & FL_SKIPLOCALHOST != 0
        && ctx.hostflags & 1 != 0
        && ent.owner == Some(ctx.viewer_id))
}
