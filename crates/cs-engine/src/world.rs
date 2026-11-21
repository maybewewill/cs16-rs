//! Entities in the world: the area tree, absolute bounds and the leaves an entity touches.
//!
//! Ported from ReHLDS `rehlds/engine/world.cpp` (read 2026-11-21): `SV_ClearWorld`,
//! `SV_CreateAreaNode`, `SV_UnlinkEdict`, `SV_LinkEdict`, `SV_TouchLinks` (box test; the hull test of brush triggers is in `touch.rs`)
//! and `SV_FindTouchedLeafs`. Links are kept by edict index, not by pointer: every area node
//! holds a list of trigger entities and a list of solid ones.
//!
//! Differences from HLDS:
//! - The absolute box is made by the engine, as `SetObjectCollisionBox` of ReGameDLL_CS
//!   (`regamedll/dlls/cbase.cpp`, read 2026-11-21) does in the original, where it is reached
//!   through `pfnSetAbsBox`. That function widens every box by 1 and, for a rotated `SOLID_BSP`
//!   entity, uses the cube of its largest coordinate; it has no `FL_ITEM` case (that is Quake's
//!   `SV_LinkEdict`), so none is made here.
//! - Touches are queued here and dispatched by the server after the current game call
//!   returns (`touch.rs`), not synchronously inside `SV_LinkEdict`. `SV_TouchLinks` (box test,
//!   then the exact hull test for brush triggers) runs at link time, as in HLDS.
//! - `BOX_ON_PLANE_SIDE` is written from memory of the Quake macro (not in the ReHLDS tree).
//! - `ent->groupinfo` is not modelled.

use cs_assets::bsp::{BspMap, Plane};
use std::collections::VecDeque;

use glam::Vec3;

use crate::collision::{CONTENTS_SOLID, MAX_WORLD_COORD};
use crate::edict::{EdictId, EdictPool, EntVars};
use crate::game::EngineApi;
use crate::touch::MAX_TOUCHES_PER_FRAME;

/// Constants of `rehlds/common/const.h` (read 2026-11-21) that entity code uses.
pub mod consts {
    pub const SOLID_NOT: u8 = 0;
    pub const SOLID_TRIGGER: u8 = 1;
    pub const SOLID_BBOX: u8 = 2;
    pub const SOLID_SLIDEBOX: u8 = 3;
    pub const SOLID_BSP: u8 = 4;

    pub const MOVETYPE_NONE: u8 = 0;
    pub const MOVETYPE_WALK: u8 = 3;
    pub const MOVETYPE_STEP: u8 = 4;
    pub const MOVETYPE_FLY: u8 = 5;
    pub const MOVETYPE_TOSS: u8 = 6;
    pub const MOVETYPE_PUSH: u8 = 7;
    pub const MOVETYPE_NOCLIP: u8 = 8;
    pub const MOVETYPE_FLYMISSILE: u8 = 9;
    pub const MOVETYPE_BOUNCE: u8 = 10;
    pub const MOVETYPE_BOUNCEMISSILE: u8 = 11;
    pub const MOVETYPE_FOLLOW: u8 = 12;
    pub const MOVETYPE_PUSHSTEP: u8 = 13;

    pub const FL_FLY: u32 = 1 << 0;
    pub const FL_SWIM: u32 = 1 << 1;
    pub const FL_CONVEYOR: u32 = 1 << 2;
    pub const FL_CLIENT: u32 = 1 << 3;
    pub const FL_INWATER: u32 = 1 << 4;
    pub const FL_MONSTER: u32 = 1 << 5;
    pub const FL_GODMODE: u32 = 1 << 6;
    pub const FL_NOTARGET: u32 = 1 << 7;
    pub const FL_SKIPLOCALHOST: u32 = 1 << 8;
    pub const FL_ONGROUND: u32 = 1 << 9;
    pub const FL_PARTIALGROUND: u32 = 1 << 10;
    pub const FL_WATERJUMP: u32 = 1 << 11;
    pub const FL_FROZEN: u32 = 1 << 12;
    pub const FL_FAKECLIENT: u32 = 1 << 13;
    pub const FL_DUCKING: u32 = 1 << 14;
    pub const FL_FLOAT: u32 = 1 << 15;
    pub const FL_GRAPHED: u32 = 1 << 16;
    pub const FL_IMMUNE_WATER: u32 = 1 << 17;
    pub const FL_IMMUNE_SLIME: u32 = 1 << 18;
    pub const FL_IMMUNE_LAVA: u32 = 1 << 19;
    pub const FL_PROXY: u32 = 1 << 20;
    pub const FL_ALWAYSTHINK: u32 = 1 << 21;
    pub const FL_BASEVELOCITY: u32 = 1 << 22;
    pub const FL_MONSTERCLIP: u32 = 1 << 23;
    pub const FL_ONTRAIN: u32 = 1 << 24;
    pub const FL_WORLDBRUSH: u32 = 1 << 25;
    pub const FL_SPECTATOR: u32 = 1 << 26;
    pub const FL_CUSTOMENTITY: u32 = 1 << 29;
    pub const FL_KILLME: u32 = 1 << 30;
    pub const FL_DORMANT: u32 = 1 << 31;
}

use consts::{FL_KILLME, MOVETYPE_FOLLOW, SOLID_BSP, SOLID_NOT, SOLID_TRIGGER};

/// Depth of the area tree (`AREA_DEPTH` in `world.cpp`, `SV_CreateAreaNode`).
pub const AREA_DEPTH: usize = 4;

/// Leaves an entity keeps (`MAX_ENT_LEAFS` in `rehlds/public/rehlds/edict.h`, read 2026-11-21).
/// An entity touching more leaves keeps the top node of its box instead.
pub const MAX_ENT_LEAFS: usize = 48;

/// What a linked entity touches in the BSP, for the visibility test: `num_leafs`,
/// `leafnums` and `headnode` of the HLDS edict. A leaf number `n` stands for leaf `n + 1` of
/// the map (leaf 0 is solid) and for bit `n` of a PVS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntLeaves {
    num_leafs: usize,
    leafnums: [i32; MAX_ENT_LEAFS],
    /// Top BSP node of the entity's box when it touches more than [`MAX_ENT_LEAFS`] leaves;
    /// -1 otherwise.
    headnode: i32,
}

impl EntLeaves {
    const NONE: Self = Self {
        num_leafs: 0,
        leafnums: [-1; MAX_ENT_LEAFS],
        headnode: -1,
    };

    /// The leaf numbers kept.
    #[cfg(test)]
    pub(crate) fn leafnums(&self) -> &[i32] {
        &self.leafnums[..self.num_leafs]
    }

    /// Whether any of the leaves (or, on overflow, any leaf under the head node) is in `pvs`
    /// (`SV_HeadnodeVisible` and the leaf loop of `SV_AddToFullPack`).
    pub(crate) fn visible(&self, bsp: &BspMap, pvs: &[bool]) -> bool {
        if self.headnode >= 0 {
            return headnode_visible(bsp, pvs, self.headnode, &mut walk_budget(bsp));
        }
        self.leafnums[..self.num_leafs]
            .iter()
            .any(|&n| usize::try_from(n).is_ok_and(|n| pvs.get(n).copied().unwrap_or(false)))
    }
}

/// `SV_HeadnodeVisible`: whether any leaf under node `child` is in `pvs`. `budget` bounds the
/// nodes visited, so a cyclic node tree in a hostile map cannot make it run away.
fn headnode_visible(bsp: &BspMap, pvs: &[bool], child: i32, budget: &mut u32) -> bool {
    if child < 0 {
        let leaf = !child as usize;
        return leaf != 0
            && bsp
                .leafs
                .get(leaf)
                .is_some_and(|l| l.contents != CONTENTS_SOLID)
            && pvs.get(leaf - 1).copied().unwrap_or(false);
    }
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    let Some(node) = bsp.nodes.get(child as usize) else {
        return false;
    };
    node.children
        .iter()
        .any(|&c| headnode_visible(bsp, pvs, i32::from(c), budget))
}

/// Nodes a walk of the BSP may visit: every node once, plus slack. A valid tree never needs
/// more.
fn walk_budget(bsp: &BspMap) -> u32 {
    u32::try_from(bsp.nodes.len() + bsp.leafs.len() + 16).unwrap_or(u32::MAX)
}

/// `BOX_ON_PLANE_SIDE`: bit 1 when part of the box is in front of `plane`, bit 2 when part is
/// behind. An axial plane (type 0..=2) compares against the box's extent on that axis.
fn box_on_plane_side(mins: Vec3, maxs: Vec3, plane: &Plane) -> u8 {
    if (0..3).contains(&plane.plane_type) {
        let axis = plane.plane_type as usize;
        return if plane.dist <= mins[axis] {
            1
        } else if plane.dist >= maxs[axis] {
            2
        } else {
            3
        };
    }
    let (mut front, mut back) = (0.0f32, 0.0f32);
    for axis in 0..3 {
        let n = plane.normal[axis];
        if n >= 0.0 {
            front += n * maxs[axis];
            back += n * mins[axis];
        } else {
            front += n * mins[axis];
            back += n * maxs[axis];
        }
    }
    u8::from(front >= plane.dist) | (u8::from(back < plane.dist) << 1)
}

/// `SV_FindTouchedLeafs` over the world model for the box `mins..maxs`. A box in solid, or
/// outside the tree, touches no leaf.
pub(crate) fn find_touched_leafs(bsp: &BspMap, mins: Vec3, maxs: Vec3) -> EntLeaves {
    struct Walk<'a> {
        bsp: &'a BspMap,
        mins: Vec3,
        maxs: Vec3,
        out: EntLeaves,
        overflow: bool,
        topnode: i32,
        budget: u32,
    }
    impl Walk<'_> {
        fn visit(&mut self, child: i32) {
            if child < 0 {
                let leaf = !child as usize;
                // Leaf 0 is the solid leaf.
                if leaf == 0
                    || self
                        .bsp
                        .leafs
                        .get(leaf)
                        .is_none_or(|l| l.contents == CONTENTS_SOLID)
                {
                    return;
                }
                if self.out.num_leafs >= MAX_ENT_LEAFS {
                    // Keep counting: the entity overran its list.
                    self.overflow = true;
                } else {
                    self.out.leafnums[self.out.num_leafs] = (leaf - 1) as i32;
                    self.out.num_leafs += 1;
                }
                return;
            }
            if self.budget == 0 {
                return;
            }
            self.budget -= 1;
            let Some(node) = self.bsp.nodes.get(child as usize) else {
                return;
            };
            let Some(plane) = self.bsp.planes.get(node.plane as usize) else {
                return;
            };
            let sides = box_on_plane_side(self.mins, self.maxs, plane);
            if sides == 3 && self.topnode == -1 {
                self.topnode = child;
            }
            let children = node.children;
            if sides & 1 != 0 {
                self.visit(i32::from(children[0]));
            }
            if sides & 2 != 0 {
                self.visit(i32::from(children[1]));
            }
        }
    }

    let Some(root) = bsp.models.first().map(|m| m.head_nodes[0]) else {
        return EntLeaves::NONE;
    };
    let mut walk = Walk {
        bsp,
        mins,
        maxs,
        out: EntLeaves::NONE,
        overflow: false,
        topnode: -1,
        budget: walk_budget(bsp),
    };
    walk.visit(root);
    if walk.overflow {
        // Use the head node instead of the leaf list.
        walk.out = EntLeaves {
            headnode: walk.topnode,
            ..EntLeaves::NONE
        };
    }
    walk.out
}

/// One node of the area tree. `axis` is `None` for a leaf of the tree; `children[0]` is the
/// side with the larger coordinates.
struct AreaNode {
    axis: Option<usize>,
    dist: f32,
    children: [usize; 2],
    trigger: Vec<u16>,
    solid: Vec<u16>,
}

/// Where a linked entity sits.
#[derive(Clone)]
struct Link {
    id: EdictId,
    leaves: EntLeaves,
    /// Area node and list the entity is in; `None` for an entity that is not in the tree
    /// (non-solid, or an unusable box).
    area: Option<(usize, bool)>,
}

/// `ED_Free` for the engine: takes `id` out of the area tree and frees its slot at server time
/// `now`. A stale id, the world and the player slots are left alone.
pub(crate) fn free_entity(edicts: &mut EdictPool, world: &mut WorldLinks, id: EdictId, now: f64) {
    if id.index > u16::from(edicts.max_clients()) && edicts.get(id).is_some() {
        world.forget(id.index);
    }
    edicts.free(id, now);
}

/// Where a queued touch came from; decides what is re-checked when it is dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TouchKind {
    /// `SV_TouchLinks`: `toucher` is a trigger, which must still be `SOLID_TRIGGER`.
    Trigger,
    /// `SV_Impact`.
    Impact,
}

/// A touch waiting for dispatch.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingTouch {
    pub(crate) toucher: EdictId,
    pub(crate) other: EdictId,
    pub(crate) kind: TouchKind,
}

/// The area tree and what each linked entity touches, plus the touches found by linking.
pub(crate) struct WorldLinks {
    nodes: Vec<AreaNode>,
    /// By edict index.
    links: Vec<Option<Link>>,
    /// `(toucher, other)` pairs waiting for the server to dispatch them, oldest first.
    pending_touches: VecDeque<PendingTouch>,
    /// Touches queued since the frame began (dispatched or not); bounded by
    /// [`MAX_TOUCHES_PER_FRAME`].
    frame_touches: usize,
    /// Touches refused this frame because the bound was reached.
    touches_dropped: usize,
    /// The server is inside its touch dispatch loop (`touch.rs`).
    pub(crate) dispatching: bool,
    /// The game's `force_retouch` global (see `EngineApi::set_force_retouch`).
    pub(crate) force_retouch: f32,
}

impl WorldLinks {
    /// `SV_ClearWorld`: an empty tree over the world bounds `mins..maxs`.
    pub(crate) fn new(bounds: (Vec3, Vec3)) -> Self {
        let mut world = Self {
            nodes: Vec::with_capacity((1 << (AREA_DEPTH + 1)) - 1),
            links: Vec::new(),
            pending_touches: VecDeque::new(),
            frame_touches: 0,
            touches_dropped: 0,
            dispatching: false,
            force_retouch: 0.0,
        };
        world.create_node(0, bounds.0, bounds.1);
        world
    }

    /// `SV_CreateAreaNode`.
    fn create_node(&mut self, depth: usize, mins: Vec3, maxs: Vec3) -> usize {
        let index = self.nodes.len();
        self.nodes.push(AreaNode {
            axis: None,
            dist: 0.0,
            children: [0, 0],
            trigger: Vec::new(),
            solid: Vec::new(),
        });
        if depth == AREA_DEPTH {
            return index;
        }
        let size = maxs - mins;
        let axis = if size.x > size.y { 0 } else { 1 };
        let dist = 0.5 * (maxs[axis] + mins[axis]);
        let (mut maxs1, mut mins2) = (maxs, mins);
        maxs1[axis] = dist;
        mins2[axis] = dist;
        let front = self.create_node(depth + 1, mins2, maxs);
        let back = self.create_node(depth + 1, mins, maxs1);
        let node = &mut self.nodes[index];
        node.axis = Some(axis);
        node.dist = dist;
        node.children = [front, back];
        index
    }

    fn link_of(&self, index: u16) -> Option<&Link> {
        self.links.get(usize::from(index))?.as_ref()
    }

    /// `SV_UnlinkEdict`: takes the entity out of the tree.
    pub(crate) fn unlink(&mut self, index: u16) {
        let Some(Some(link)) = self.links.get_mut(usize::from(index)) else {
            return;
        };
        if let Some((node, trigger)) = link.area.take() {
            let node = &mut self.nodes[node];
            let list = if trigger {
                &mut node.trigger
            } else {
                &mut node.solid
            };
            list.retain(|&i| i != index);
        }
    }

    /// Takes the entity out of the tree and forgets its leaves; for an entity that is gone.
    pub(crate) fn forget(&mut self, index: u16) {
        self.unlink(index);
        if let Some(slot) = self.links.get_mut(usize::from(index)) {
            *slot = None;
        }
    }

    /// The area node whose list the box belongs to: the first node the box crosses.
    fn node_for(&self, absmin: Vec3, absmax: Vec3) -> usize {
        let mut node = 0;
        while let Some(axis) = self.nodes[node].axis {
            let n = &self.nodes[node];
            if absmin[axis] > n.dist {
                node = n.children[0];
            } else if absmax[axis] < n.dist {
                node = n.children[1];
            } else {
                break;
            }
        }
        node
    }

    /// Indices of the entities in the tree whose absolute box touches `mins..maxs`, with their
    /// list, node by node in the order `SV_TouchLinks` visits them (a node's triggers, then its
    /// solids, then the larger-coordinate child's subtree, then the other).
    fn collect(
        &self,
        edicts: &EdictPool,
        mins: Vec3,
        maxs: Vec3,
        want_trigger: bool,
        want_solid: bool,
        out: &mut Vec<EdictId>,
    ) {
        let mut stack = vec![0usize];
        while let Some(node) = stack.pop() {
            let n = &self.nodes[node];
            let lists = [(want_trigger, &n.trigger), (want_solid, &n.solid)];
            for (wanted, list) in lists {
                if !wanted {
                    continue;
                }
                for &index in list {
                    let Some(link) = self.link_of(index) else {
                        continue;
                    };
                    // A stale entry (the slot was freed behind our back) resolves to nothing.
                    let Some(edict) = edicts.get(link.id) else {
                        continue;
                    };
                    let v = &edict.vars;
                    if bounds_intersect(mins, maxs, v.absmin, v.absmax) {
                        out.push(link.id);
                    }
                }
            }
            if let Some(axis) = n.axis {
                // The stack is last in, first out: `children[1]` goes in first so that
                // `children[0]` is visited first, as `SV_TouchLinks` recurses.
                if n.dist > mins[axis] {
                    stack.push(n.children[1]);
                }
                if maxs[axis] > n.dist {
                    stack.push(n.children[0]);
                }
            }
        }
    }

    /// The solid-list entities whose absolute box touches `mins..maxs`, in the order
    /// `SV_ClipToLinks` visits them.
    pub(crate) fn solids_in_box(
        &self,
        edicts: &EdictPool,
        mins: Vec3,
        maxs: Vec3,
        out: &mut Vec<EdictId>,
    ) {
        self.collect(edicts, mins, maxs, false, true, out);
    }

    /// The solid-list entities of the nodes `SV_LinkContents` visits for `pos` (the root down
    /// to the node `pos` selects), in its order. Boxes are not tested.
    pub(crate) fn solids_along_point(&self, edicts: &EdictPool, pos: Vec3) -> Vec<EdictId> {
        let mut out = Vec::new();
        let mut node = 0;
        loop {
            let n = &self.nodes[node];
            out.extend(
                n.solid
                    .iter()
                    .filter_map(|&index| self.link_of(index))
                    .map(|link| link.id)
                    .filter(|&id| edicts.get(id).is_some()),
            );
            let Some(axis) = n.axis else {
                return out;
            };
            node = if pos[axis] > n.dist {
                n.children[0]
            } else if pos[axis] < n.dist {
                n.children[1]
            } else {
                return out;
            };
        }
    }

    /// Whether the entity's own leaves (not a box) are in `pvs`; `None` when the entity is not
    /// linked.
    pub(crate) fn leaves_visible(&self, bsp: &BspMap, pvs: &[bool], id: EdictId) -> Option<bool> {
        let link = self.link_of(id.index).filter(|l| l.id == id)?;
        Some(link.leaves.visible(bsp, pvs))
    }

    /// The leaves kept for `id`; `None` when it is not linked.
    #[cfg(test)]
    pub(crate) fn leaves_of(&self, id: EdictId) -> Option<&EntLeaves> {
        self.link_of(id.index)
            .filter(|l| l.id == id)
            .map(|l| &l.leaves)
    }

    /// Takes the oldest queued touch.
    pub(crate) fn pop_touch(&mut self) -> Option<PendingTouch> {
        self.pending_touches.pop_front()
    }

    /// How many touches are queued and not yet taken.
    pub(crate) fn pending_touch_count(&self) -> usize {
        self.pending_touches.len()
    }

    /// The touches queued and not yet taken, as `(toucher, other)`.
    #[cfg(test)]
    pub(crate) fn pending_touches(&self) -> Vec<(EdictId, EdictId)> {
        self.pending_touches
            .iter()
            .map(|t| (t.toucher, t.other))
            .collect()
    }

    /// Queues `touches` together, or none of them when fewer than `touches.len()` are left of
    /// this frame's [`MAX_TOUCHES_PER_FRAME`]; the refused ones are counted.
    pub(crate) fn queue_touches(&mut self, touches: &[PendingTouch]) {
        if self.frame_touches + touches.len() > MAX_TOUCHES_PER_FRAME {
            self.touches_dropped += touches.len();
            return;
        }
        self.frame_touches += touches.len();
        self.pending_touches.extend(touches.iter().copied());
    }

    /// [`queue_touches`](Self::queue_touches) for one trigger touch.
    pub(crate) fn queue_touch(&mut self, trigger: EdictId, other: EdictId) {
        self.queue_touches(&[PendingTouch {
            toucher: trigger,
            other,
            kind: TouchKind::Trigger,
        }]);
    }

    /// Starts a new frame's touch budget; returns the touches refused during the last one.
    pub(crate) fn reset_touch_budget(&mut self) -> usize {
        self.frame_touches = 0;
        std::mem::take(&mut self.touches_dropped)
    }
}

/// `BoundsIntersect`: the boxes overlap or touch.
fn bounds_intersect(mins1: Vec3, maxs1: Vec3, mins2: Vec3, maxs2: Vec3) -> bool {
    mins1.cmple(maxs2).all() && maxs1.cmpge(mins2).all()
}

/// Clamps a finite box to the world's coordinate range.
fn clamp_to_world(v: Vec3) -> Vec3 {
    v.clamp(Vec3::splat(-MAX_WORLD_COORD), Vec3::splat(MAX_WORLD_COORD))
}

/// `SetObjectCollisionBox` of the game: the entity's absolute box, widened by 1. A rotated
/// `SOLID_BSP` entity gets the cube of its largest coordinate around its origin.
fn absolute_box(v: &EntVars) -> (Vec3, Vec3) {
    let (mut absmin, mut absmax) = if v.solid == SOLID_BSP && v.angles != Vec3::ZERO {
        let radius = v.mins.abs().max_element().max(v.maxs.abs().max_element());
        (
            v.origin - Vec3::splat(radius),
            v.origin + Vec3::splat(radius),
        )
    } else {
        (v.origin + v.mins, v.origin + v.maxs)
    };
    absmin -= Vec3::ONE;
    absmax += Vec3::ONE;
    (absmin, absmax)
}

impl EngineApi<'_> {
    /// Moves entity `id` to `origin` and links it (`PF_setorigin_I`). A stale or foreign id, or
    /// an `origin` outside the world (non-finite, or a coordinate beyond `MAX_WORLD_COORD`),
    /// changes nothing.
    pub fn set_origin(&mut self, id: EdictId, origin: Vec3) {
        if !crate::api_entities::in_world(origin) {
            return;
        }
        let Some(edict) = self.edicts.get_mut(id) else {
            return;
        };
        edict.vars.origin = origin;
        self.link_entity(id, false);
    }

    /// Sets the size of entity `id` and links it (`PF_setsize_I` through `SetMinMaxSize`):
    /// `mins`, `maxs` and `size = maxs - mins`. HLDS stops the server when `mins` is above
    /// `maxs` on an axis; here, as for a stale id or numbers outside the world, nothing
    /// changes.
    pub fn set_size(&mut self, id: EdictId, mins: Vec3, maxs: Vec3) {
        if !crate::api_entities::in_world(mins)
            || !crate::api_entities::in_world(maxs)
            || mins.cmpgt(maxs).any()
        {
            return;
        }
        let Some(edict) = self.edicts.get_mut(id) else {
            return;
        };
        edict.vars.mins = mins;
        edict.vars.maxs = maxs;
        edict.vars.size = maxs - mins;
        self.link_entity(id, false);
    }

    /// `SV_LinkEdict`: recomputes the absolute box of entity `id`, the leaves it touches and
    /// its place in the area tree. An entity that is `SOLID_NOT` (with `skin >= -1`) is not
    /// put in the tree; a `SOLID_BSP` entity without a model is not either. With
    /// `touch_triggers` the `SOLID_TRIGGER` entities whose box meets the entity's (and, for a
    /// brush trigger, whose hull holds its origin) are queued as touches, which the server
    /// dispatches after the current game call (see `touch.rs`). The world, a stale id and an entity
    /// whose box is not finite are left out of the tree; such an entity gets a zero box.
    pub fn link_entity(&mut self, id: EdictId, touch_triggers: bool) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let (absmin, absmax) = absolute_box(v);
        let (solid, movetype, aiment, skin) = (v.solid, v.movetype, v.aiment, v.skin);
        let has_model = v.modelindex != 0;
        let no_model = !has_model && v.model.is_empty();
        let killed = v.flags & FL_KILLME != 0;
        let (ent_origin, ent_mins, ent_maxs) = (v.origin, v.mins, v.maxs);

        // Whatever the entity was linked as is gone.
        self.world.unlink(id.index);
        if id.index == 0 {
            return;
        }
        if !absmin.is_finite() || !absmax.is_finite() {
            self.store_link(id, Vec3::ZERO, Vec3::ZERO, EntLeaves::NONE, None);
            return;
        }
        let (absmin, absmax) = (clamp_to_world(absmin), clamp_to_world(absmax));

        let leaves = if movetype == MOVETYPE_FOLLOW && aiment.is_some() {
            // A follower is seen where the entity it follows is.
            aiment
                .and_then(|a| self.world.link_of(a.index).filter(|l| l.id == a))
                .map_or(EntLeaves::NONE, |l| l.leaves.clone())
        } else {
            match self.map.bsp() {
                Some(bsp) if has_model => find_touched_leafs(bsp, absmin, absmax),
                _ => EntLeaves::NONE,
            }
        };

        let in_tree = !(solid == SOLID_NOT && skin >= -1) && !(solid == SOLID_BSP && no_model);
        let area = in_tree.then(|| (self.world.node_for(absmin, absmax), solid == SOLID_TRIGGER));
        self.store_link(id, absmin, absmax, leaves, area);

        if area.is_some() && touch_triggers && !killed {
            let mut found = Vec::new();
            self.world
                .collect(self.edicts, absmin, absmax, true, false, &mut found);
            for touch in found {
                let Some(t) = self.edicts.get(touch).map(|e| &e.vars) else {
                    continue;
                };
                if touch.index != id.index
                    && t.flags & FL_KILLME == 0
                    && t.solid == SOLID_TRIGGER
                    && self.brush_trigger_holds(t, ent_origin, ent_mins, ent_maxs)
                {
                    self.world.queue_touch(touch, id);
                }
            }
        }
    }

    /// Writes the box to the entity and records the link.
    fn store_link(
        &mut self,
        id: EdictId,
        absmin: Vec3,
        absmax: Vec3,
        leaves: EntLeaves,
        area: Option<(usize, bool)>,
    ) {
        if let Some(edict) = self.edicts.get_mut(id) {
            edict.vars.absmin = absmin;
            edict.vars.absmax = absmax;
        }
        let index = usize::from(id.index);
        if self.world.links.len() <= index {
            self.world.links.resize(index + 1, None);
        }
        if let Some((node, trigger)) = area {
            let node = &mut self.world.nodes[node];
            let list = if trigger {
                &mut node.trigger
            } else {
                &mut node.solid
            };
            list.push(id.index);
        }
        self.world.links[index] = Some(Link { id, leaves, area });
    }

    /// Entities in the area tree whose absolute box touches `mins..maxs`, in ascending index
    /// order. The filter is that of the game's `UTIL_EntitiesInBox` (ReGameDLL_CS
    /// `regamedll/dlls/util.cpp`, read 2026-11-21; ReHLDS has no entities-in-box call of its own):
    /// free slots and the world (index 0) are skipped and the absolute boxes must overlap,
    /// touching counting as overlap (`CBaseEntity::Intersects`); there is no flag mask. Unlike
    /// that linear scan, this one reads the area tree, so an entity that was not linked, or is
    /// not in the tree because it is not solid, is not found, and a moved entity is found where
    /// it was last linked. Non-finite bounds give an empty list.
    pub fn entities_in_box(&self, mins: Vec3, maxs: Vec3) -> Vec<EdictId> {
        if !mins.is_finite() || !maxs.is_finite() {
            return Vec::new();
        }
        let mut found = Vec::new();
        self.world
            .collect(self.edicts, mins, maxs, true, true, &mut found);
        found.sort_unstable_by_key(|id| id.index);
        found
    }

    /// The touches `link_entity(_, true)` found and nothing has taken yet, as
    /// `(trigger, other)`.
    #[cfg(test)]
    pub(crate) fn pending_touches(&self) -> Vec<(EdictId, EdictId)> {
        self.world.pending_touches()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use cs_assets::bsp::{BspMap, BspModel, Leaf, LightmapAtlas, Node};
    use cs_cvar::CVarRegistry;
    use cs_proto48::usermsg::UserMsgRegistry;

    use super::consts::*;
    use super::*;
    use crate::game::{ClientView, Outbox};
    use crate::map::ServerMap;
    use crate::precache::PrecacheTables;

    struct Fixture {
        edicts: EdictPool,
        precache: PrecacheTables,
        cvars: CVarRegistry,
        usermsgs: UserMsgRegistry,
        map: ServerMap,
        world: WorldLinks,
        outbox: Outbox,
        clients: Vec<ClientView>,
    }

    impl Fixture {
        fn on(map: ServerMap) -> Self {
            Self {
                edicts: EdictPool::new(1, 64),
                precache: PrecacheTables::new(&map.path, map.submodel_count),
                cvars: CVarRegistry::new_empty(),
                usermsgs: UserMsgRegistry::cs16_default(),
                world: WorldLinks::new(map.world_bounds()),
                map,
                outbox: Outbox::new(1),
                clients: vec![ClientView::default()],
            }
        }

        fn api(&mut self) -> EngineApi<'_> {
            EngineApi::new(
                &mut self.edicts,
                &mut self.precache,
                &mut self.cvars,
                &self.usermsgs,
                &self.map,
                &mut self.world,
                1.0,
                0.0,
                &mut self.outbox,
                &mut self.clients,
            )
        }
    }

    /// Four leaves in a row along x, split at -100, 0 and 200: leaf 1 (x < -100), leaf 2
    /// (-100..0), leaf 3 (0..200), leaf 4 (x >= 200). The PVS of leaf 1 holds leaf 2 only.
    pub(crate) fn strip_bsp() -> BspMap {
        let plane = |dist: f32| Plane {
            normal: Vec3::X,
            dist,
            plane_type: 0,
        };
        let leaf = |contents: i32, vis_offset: i32| Leaf {
            contents,
            vis_offset,
            mins: [0; 3],
            maxs: [0; 3],
            first_mark_surface: 0,
            num_mark_surfaces: 0,
            ambient_levels: [0; 4],
        };
        let node = |plane: u32, children: [i16; 2]| Node {
            plane,
            children,
            mins: [0; 3],
            maxs: [0; 3],
            first_face: 0,
            num_faces: 0,
        };
        BspMap {
            name: "strip".into(),
            skyname: String::new(),
            entities: Vec::new(),
            spawns: Vec::new(),
            hostage_spawns: Vec::new(),
            brush_entities: Vec::new(),
            planes: vec![plane(-100.0), plane(0.0), plane(200.0)],
            vertices_raw: Vec::new(),
            // Bit 1: leaf 2.
            visibility: vec![0b10],
            // Front (x >= dist) first; leaf k is child `-k - 1`.
            nodes: vec![node(0, [1, -2]), node(1, [2, -3]), node(2, [-5, -4])],
            texinfos: Vec::new(),
            faces: Vec::new(),
            lighting: Vec::new(),
            clipnodes: Vec::new(),
            leafs: vec![
                leaf(CONTENTS_SOLID, -1),
                leaf(-1, 0),
                leaf(-1, -1),
                leaf(-1, -1),
                leaf(-1, -1),
            ],
            mark_surfaces: Vec::new(),
            edges: Vec::new(),
            surf_edges: Vec::new(),
            models: vec![BspModel {
                mins: Vec3::splat(-1000.0),
                maxs: Vec3::splat(1000.0),
                origin: Vec3::ZERO,
                head_nodes: [0, -1, -1, -1],
                vis_leafs: 4,
                first_face: 0,
                num_faces: 0,
            }],
            textures: Vec::new(),
            lightmap_atlas: LightmapAtlas {
                width: 0,
                height: 0,
                rgba: Vec::new(),
                packed_faces: 0,
            },
            render_vertices: Vec::new(),
            render_indices: Vec::new(),
            batches: Vec::new(),
        }
    }

    fn strip_fixture() -> Fixture {
        Fixture::on(ServerMap::with_bsp_for_test(Arc::new(strip_bsp())))
    }

    #[test]
    fn area_tree_has_the_depth_of_hlds() {
        let world = WorldLinks::new((Vec3::splat(-1000.0), Vec3::splat(1000.0)));
        assert_eq!(world.nodes.len(), (1 << (AREA_DEPTH + 1)) - 1);
        assert_eq!(world.nodes[0].axis, Some(1), "square world splits on y");
        assert_eq!(world.nodes[0].dist, 0.0);
        // Preorder: the front child comes right after its parent.
        assert_eq!(world.nodes[0].children[0], 1);
        let leaves = world.nodes.iter().filter(|n| n.axis.is_none()).count();
        assert_eq!(leaves, 1 << AREA_DEPTH);
    }

    #[test]
    fn leaves_of_a_box_follow_the_splits() {
        let bsp = strip_bsp();
        let at = |lo: f32, hi: f32| {
            find_touched_leafs(&bsp, Vec3::new(lo, -1.0, -1.0), Vec3::new(hi, 1.0, 1.0))
        };
        assert_eq!(at(-300.0, -200.0).leafnums(), [0]);
        assert_eq!(at(10.0, 20.0).leafnums(), [2]);
        let mut all = at(-300.0, 450.0).leafnums().to_vec();
        all.sort_unstable();
        assert_eq!(all, [0, 1, 2, 3]);
        // Touching a split plane exactly only counts the side the box is on.
        assert_eq!(at(0.0, 20.0).leafnums(), [2]);
        let pvs = bsp.decompress_pvs(1);
        assert_eq!(pvs, [false, true, false, false]);
        assert!(!at(-300.0, -200.0).visible(&bsp, &pvs));
        assert!(at(-300.0, 450.0).visible(&bsp, &pvs));
        assert!(!at(10.0, 20.0).visible(&bsp, &pvs));
    }

    #[test]
    fn a_box_over_a_hidden_centre_and_corners_is_still_visible() {
        let mut f = strip_fixture();
        let eye = Vec3::new(-200.0, 0.0, 0.0);
        // Centre x = 100 is leaf 3, the ends are leaves 1 and 4, all outside the PVS of leaf
        // 1; the box still crosses leaf 2.
        let (mins, maxs) = (Vec3::new(-250.0, -8.0, -8.0), Vec3::new(450.0, 8.0, 8.0));
        let eng = f.api();
        assert!(eng.can_see(eye, mins, maxs));
        assert!(!eng.can_see(eye, Vec3::new(50.0, -8.0, -8.0), Vec3::new(150.0, 8.0, 8.0)));
    }

    #[test]
    fn head_node_stands_for_the_leaves_under_it() {
        let bsp = strip_bsp();
        let pvs = bsp.decompress_pvs(1);
        let under = |headnode| EntLeaves {
            headnode,
            ..EntLeaves::NONE
        };
        // Node 0 holds every leaf, node 2 only leaves 3 and 4, which the PVS does not hold.
        assert!(under(0).visible(&bsp, &pvs));
        assert!(under(1).visible(&bsp, &pvs));
        assert!(!under(2).visible(&bsp, &pvs));
        assert!(!EntLeaves::NONE.visible(&bsp, &pvs));
    }

    #[test]
    fn linking_finds_leaves_and_follow_copies_them() {
        let mut f = strip_fixture();
        let mut eng = f.api();
        let a = eng.create_entity("a").unwrap();
        eng.ent_mut(a).unwrap().modelindex = 1;
        eng.ent_mut(a).unwrap().solid = SOLID_BBOX;
        eng.set_size(a, Vec3::splat(-5.0), Vec3::splat(5.0));
        eng.set_origin(a, Vec3::new(-50.0, 0.0, 0.0));
        let b = eng.create_entity("b").unwrap();
        {
            let v = eng.ent_mut(b).unwrap();
            v.modelindex = 1;
            v.movetype = MOVETYPE_FOLLOW;
            v.aiment = Some(a);
        }
        eng.set_origin(b, Vec3::new(500.0, 0.0, 0.0));
        let leaves = |eng: &EngineApi<'_>, id| eng.world.leaves_of(id).unwrap().leafnums().to_vec();
        assert_eq!(leaves(&eng, a), [1]);
        // The follower takes the leaves of the entity it follows, not its own.
        assert_eq!(leaves(&eng, b), [1]);
        // An entity without a model touches no leaf.
        let c = eng.create_entity("c").unwrap();
        eng.set_origin(c, Vec3::new(-50.0, 0.0, 0.0));
        assert!(leaves(&eng, c).is_empty());
    }

    #[test]
    fn touches_are_queued_for_trigger_overlaps() {
        let mut f = Fixture::on(ServerMap::stub("touch"));
        let mut eng = f.api();
        let trigger = eng.create_entity("trigger").unwrap();
        eng.ent_mut(trigger).unwrap().solid = SOLID_TRIGGER;
        eng.set_size(trigger, Vec3::splat(-32.0), Vec3::splat(32.0));
        eng.set_origin(trigger, Vec3::new(100.0, 0.0, 0.0));
        let far_trigger = eng.create_entity("far").unwrap();
        eng.ent_mut(far_trigger).unwrap().solid = SOLID_TRIGGER;
        eng.set_size(far_trigger, Vec3::splat(-32.0), Vec3::splat(32.0));
        eng.set_origin(far_trigger, Vec3::new(1000.0, 0.0, 0.0));
        let solid_box = eng.create_entity("box").unwrap();
        eng.ent_mut(solid_box).unwrap().solid = SOLID_BBOX;
        eng.set_size(solid_box, Vec3::splat(-16.0), Vec3::splat(16.0));
        // Not touching yet and not asked to touch: nothing queued.
        eng.set_origin(solid_box, Vec3::new(100.0, 0.0, 0.0));
        assert!(eng.pending_touches().is_empty());

        eng.link_entity(solid_box, true);
        assert_eq!(eng.pending_touches(), [(trigger, solid_box)]);
        // A trigger does not touch a non-trigger that is out of reach, and an entity does not
        // touch itself.
        eng.ent_mut(solid_box).unwrap().origin = Vec3::new(500.0, 0.0, 0.0);
        eng.link_entity(solid_box, true);
        eng.link_entity(trigger, true);
        assert_eq!(eng.pending_touches().len(), 1);
        // A killed entity touches nothing.
        eng.ent_mut(solid_box).unwrap().origin = Vec3::new(100.0, 0.0, 0.0);
        eng.ent_mut(solid_box).unwrap().flags |= FL_KILLME;
        eng.link_entity(solid_box, true);
        assert_eq!(eng.pending_touches().len(), 1);
    }

    #[test]
    fn touches_come_in_the_order_the_tree_is_walked() {
        let mut f = Fixture::on(ServerMap::stub("order"));
        let mut eng = f.api();
        let trigger = |eng: &mut EngineApi<'_>, y: f32| {
            let id = eng.create_entity("trigger").unwrap();
            eng.ent_mut(id).unwrap().solid = SOLID_TRIGGER;
            eng.set_size(id, Vec3::splat(-8.0), Vec3::splat(8.0));
            eng.set_origin(id, Vec3::new(0.0, y, 0.0));
            id
        };
        // The lower index sits in the child with the smaller coordinates.
        let back = trigger(&mut eng, -500.0);
        let front = trigger(&mut eng, 500.0);
        let big = eng.create_entity("big").unwrap();
        eng.ent_mut(big).unwrap().solid = SOLID_BBOX;
        eng.set_size(big, Vec3::splat(-1000.0), Vec3::splat(1000.0));
        eng.link_entity(big, true);
        assert_eq!(eng.pending_touches(), [(front, big), (back, big)]);
    }

    #[test]
    fn touch_queue_is_capped_per_frame() {
        let mut f = Fixture::on(ServerMap::stub("cap"));
        let mut eng = f.api();
        let trigger = eng.create_entity("trigger").unwrap();
        eng.ent_mut(trigger).unwrap().solid = SOLID_TRIGGER;
        eng.set_size(trigger, Vec3::splat(-32.0), Vec3::splat(32.0));
        let ball = eng.create_entity("ball").unwrap();
        eng.ent_mut(ball).unwrap().solid = SOLID_BBOX;
        eng.set_size(ball, Vec3::splat(-1.0), Vec3::splat(1.0));
        for _ in 0..MAX_TOUCHES_PER_FRAME + 10 {
            eng.link_entity(ball, true);
        }
        assert_eq!(eng.pending_touches().len(), MAX_TOUCHES_PER_FRAME);
        assert_eq!(f.world.reset_touch_budget(), 10);
        // The budget is back; the queue itself is not touched by the reset.
        assert_eq!(f.world.pending_touches().len(), MAX_TOUCHES_PER_FRAME);
        assert_eq!(f.world.reset_touch_budget(), 0);
    }

    #[test]
    fn follower_of_an_unlinked_entity_has_no_leaves() {
        let mut f = strip_fixture();
        let mut eng = f.api();
        let target = eng.create_entity("target").unwrap();
        let b = eng.create_entity("b").unwrap();
        {
            let v = eng.ent_mut(b).unwrap();
            v.modelindex = 1;
            v.movetype = MOVETYPE_FOLLOW;
            v.aiment = Some(target);
        }
        eng.link_entity(b, false);
        assert!(eng.world.leaves_of(b).unwrap().leafnums().is_empty());
    }

    #[test]
    fn hostile_bsp_does_not_loop_or_panic() {
        let mut bsp = strip_bsp();
        // A node that is its own child, and one with a plane and a child out of range.
        bsp.nodes[0].children = [0, 0];
        bsp.nodes[1].plane = 99;
        bsp.nodes[2].children = [i16::MAX, -9999];
        // The box crosses the split of the looping node, so each visit would branch twice.
        let _ = find_touched_leafs(&bsp, Vec3::splat(-300.0), Vec3::splat(300.0));
        let leaves = EntLeaves {
            headnode: 0,
            ..EntLeaves::NONE
        };
        assert!(!leaves.visible(&bsp, &[true; 4]));
        bsp.models.clear();
        let none = find_touched_leafs(&bsp, Vec3::ZERO, Vec3::ONE);
        assert_eq!(none, EntLeaves::NONE);
    }
}
