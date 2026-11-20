//! Pool of entity slots (edicts) and the entity variables sent to clients.
//!
//! Index 0 is worldspawn, indices `1..=max_clients` are player slots; both
//! are live for the whole lifetime of the pool. Other slots are handed out by
//! [`EdictPool::alloc`] and returned with [`EdictPool::free`]. Every slot has
//! a serial that changes when the slot is freed or reset, so an [`EdictId`]
//! kept past that point no longer resolves.

use std::any::Any;

use glam::Vec3;

/// Default size of the edict pool.
pub const DEFAULT_MAX_EDICTS: usize = 900;

/// Seconds a freed slot stays unused before [`EdictPool::alloc`] may reuse it.
pub const EDICT_REUSE_DELAY: f64 = 0.5;

/// Handle to an edict: slot index plus the slot's serial at the time the
/// handle was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdictId {
    pub index: u16,
    pub serial: u32,
}

/// Entity variables (the `entvars_t` subset the network layer sends).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntVars {
    pub classname: String,
    pub model: String,
    pub origin: Vec3,
    pub angles: Vec3,
    pub velocity: Vec3,
    pub v_angle: Vec3,
    pub punchangle: Vec3,
    pub view_ofs: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub modelindex: u16,
    pub weaponmodel: u16,
    pub sequence: u16,
    pub gaitsequence: u16,
    pub frame: f32,
    pub animtime: f32,
    pub framerate: f32,
    pub scale: f32,
    pub body: u8,
    pub skin: i16,
    pub colormap: u16,
    pub effects: u32,
    pub rendermode: u8,
    pub renderamt: u8,
    pub renderfx: u8,
    pub solid: u8,
    pub movetype: u8,
    pub flags: u32,
    pub health: f32,
    pub max_speed: f32,
    pub fov: f32,
    pub waterlevel: i32,
    pub deadflag: i32,
    pub owner: Option<EdictId>,
    pub aiment: Option<EdictId>,
    pub iuser1: i32,
    pub iuser2: i32,
    pub iuser3: i32,
    pub iuser4: i32,
}

/// One entity slot: its variables plus the game's private data.
pub struct Edict {
    pub vars: EntVars,
    /// `Send` so the engine can run on its own thread.
    pub private: Option<Box<dyn Any + Send>>,
}

impl Edict {
    fn empty() -> Self {
        Self {
            vars: EntVars::default(),
            private: None,
        }
    }

    fn clear(&mut self) {
        self.vars = EntVars::default();
        self.private = None;
    }
}

struct Slot {
    edict: Edict,
    serial: u32,
    live: bool,
    /// Time of the last `free`; `None` for a slot that was never freed.
    freed_at: Option<f64>,
}

/// Fixed-size pool of edicts.
pub struct EdictPool {
    slots: Vec<Slot>,
    max_clients: u8,
}

impl EdictPool {
    /// Creates the pool with worldspawn at index 0 and players at
    /// `1..=max_clients`, all live. `max_edicts` is raised to
    /// `max_clients + 1` if smaller and capped at `u16::MAX` slots, so every
    /// index fits an [`EdictId`] and `u16::MAX` itself is never a valid index.
    pub fn new(max_clients: u8, max_edicts: usize) -> Self {
        let fixed = usize::from(max_clients) + 1;
        let len = max_edicts.max(fixed).min(usize::from(u16::MAX));
        let slots = (0..len)
            .map(|index| Slot {
                edict: Edict::empty(),
                serial: 0,
                live: index < fixed,
                freed_at: None,
            })
            .collect();
        Self { slots, max_clients }
    }

    /// A pool like [`new`](Self::new) for the next map: every slot starts at a serial one past
    /// the highest any slot of `self` holds, so no id `self` handed out resolves in it.
    pub fn successor(&self, max_clients: u8, max_edicts: usize) -> Self {
        let next = self
            .slots
            .iter()
            .map(|s| s.serial)
            .max()
            .unwrap_or(0)
            .wrapping_add(1);
        let mut pool = Self::new(max_clients, max_edicts);
        for slot in &mut pool.slots {
            slot.serial = next;
        }
        pool
    }

    /// Worldspawn.
    pub fn world(&self) -> EdictId {
        self.id_at(0)
    }

    /// Edict of player slot `slot` (index `slot + 1`). For `slot >=
    /// max_clients` returns an id that [`get`](Self::get) rejects.
    pub fn client(&self, slot: u8) -> EdictId {
        if slot >= self.max_clients {
            return EdictId {
                index: u16::MAX,
                serial: 0,
            };
        }
        self.id_at(u16::from(slot) + 1)
    }

    /// Takes the lowest free slot above the player slots that was never used
    /// or was freed more than [`EDICT_REUSE_DELAY`] seconds before `now`.
    pub fn alloc(&mut self, now: f64) -> Option<EdictId> {
        let first = usize::from(self.max_clients) + 1;
        let index = (first..self.slots.len()).find(|&i| {
            let slot = &self.slots[i];
            !slot.live && slot.freed_at.is_none_or(|t| now - t > EDICT_REUSE_DELAY)
        })?;
        let slot = &mut self.slots[index];
        slot.live = true;
        slot.freed_at = None;
        Some(EdictId {
            // Lossless: `new` caps the pool at `u16::MAX` slots.
            index: index as u16,
            serial: slot.serial,
        })
    }

    /// Returns the edict to the pool. Ignores stale ids, worldspawn and
    /// player slots.
    pub fn free(&mut self, id: EdictId, now: f64) {
        if id.index <= u16::from(self.max_clients) || self.get(id).is_none() {
            return;
        }
        let slot = &mut self.slots[usize::from(id.index)];
        slot.edict.clear();
        slot.serial = slot.serial.wrapping_add(1);
        slot.live = false;
        slot.freed_at = Some(now);
    }

    /// Clears player slot `slot` and changes its serial; the slot stays live.
    /// Ignores `slot >= max_clients`.
    pub fn reset_client(&mut self, slot: u8) {
        if slot >= self.max_clients {
            return;
        }
        let slot = &mut self.slots[usize::from(slot) + 1];
        slot.edict.clear();
        slot.serial = slot.serial.wrapping_add(1);
    }

    pub fn get(&self, id: EdictId) -> Option<&Edict> {
        self.slots
            .get(usize::from(id.index))
            .filter(|s| s.live && s.serial == id.serial)
            .map(|s| &s.edict)
    }

    pub fn get_mut(&mut self, id: EdictId) -> Option<&mut Edict> {
        self.slots
            .get_mut(usize::from(id.index))
            .filter(|s| s.live && s.serial == id.serial)
            .map(|s| &mut s.edict)
    }

    /// Live edict at `index`, with its current id.
    pub fn by_index(&self, index: u16) -> Option<(EdictId, &Edict)> {
        let slot = self.slots.get(usize::from(index)).filter(|s| s.live)?;
        Some((
            EdictId {
                index,
                serial: slot.serial,
            },
            &slot.edict,
        ))
    }

    /// Worldspawn, player slots and allocated edicts in ascending index order.
    pub fn iter_live(&self) -> impl Iterator<Item = (EdictId, &Edict)> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.live)
            .map(|(i, s)| {
                (
                    EdictId {
                        // Lossless: `new` caps the pool at `u16::MAX` slots.
                        index: i as u16,
                        serial: s.serial,
                    },
                    &s.edict,
                )
            })
    }

    pub fn max_clients(&self) -> u8 {
        self.max_clients
    }

    fn id_at(&self, index: u16) -> EdictId {
        EdictId {
            index,
            serial: self.slots[usize::from(index)].serial,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = {
        const fn assert_send<T: Send>() {}
        assert_send::<EdictPool>();
    };

    #[test]
    fn successor_rejects_every_id_of_the_previous_pool() {
        let mut old = EdictPool::new(2, 16);
        let world = old.world();
        let freed = old.alloc(0.0).unwrap();
        let live = old.alloc(0.0).unwrap();
        old.free(freed, 1.0);
        old.reset_client(1);
        old.reset_client(1);
        // A player id taken before and after the resets.
        let player = old.client(0);
        let reset_player = old.client(1);

        let mut new = old.successor(2, 16);
        assert_eq!(new.max_clients(), 2);
        for id in [world, freed, live, player, reset_player] {
            assert!(new.get(id).is_none(), "{id:?}");
        }
        // The successor's own ids resolve, at the same indices.
        assert!(new.get(new.world()).is_some());
        assert!(new.get(new.client(0)).is_some());
        assert!(new.get(new.client(1)).is_some());
        let first = new.alloc(0.0).unwrap();
        assert_eq!(first.index, freed.index);
        assert!(new.get(first).is_some());
        let second = new.alloc(0.0).unwrap();
        assert_eq!(second.index, live.index);
        assert!(new.get(second).is_some());
        assert!(new.get(live).is_none());
        assert_eq!(new.iter_live().count(), 5);
    }

    #[test]
    fn first_alloc_is_after_player_slots() {
        let mut pool = EdictPool::new(16, DEFAULT_MAX_EDICTS);
        assert_eq!(pool.alloc(0.0).unwrap().index, 17);
    }

    #[test]
    fn stale_id_is_rejected_after_free() {
        let mut pool = EdictPool::new(1, 3);
        let old = pool.alloc(0.0).unwrap();
        pool.free(old, 1.0);
        assert!(pool.get(old).is_none());
        let new = pool.alloc(2.0).unwrap();
        assert_eq!(new.index, old.index);
        assert_ne!(new.serial, old.serial);
        assert!(pool.get(old).is_none());
        assert!(pool.get(new).is_some());
    }

    #[test]
    fn freed_edict_is_not_reused_within_half_a_second() {
        let mut pool = EdictPool::new(1, DEFAULT_MAX_EDICTS);
        let old = pool.alloc(0.0).unwrap();
        pool.free(old, 10.0);
        let early = pool.alloc(10.4).unwrap();
        assert_ne!(early.index, old.index);
        let late = pool.alloc(10.6).unwrap();
        assert_eq!(late.index, old.index);
    }

    #[test]
    fn pool_exhaustion_returns_none() {
        let mut pool = EdictPool::new(1, 4);
        assert!(pool.alloc(0.0).is_some());
        assert!(pool.alloc(0.0).is_some());
        assert!(pool.alloc(0.0).is_none());
    }

    #[test]
    fn world_and_clients_cannot_be_freed() {
        let mut pool = EdictPool::new(2, 8);
        let world = pool.world();
        let client = pool.client(0);
        pool.free(world, 0.0);
        pool.free(client, 0.0);
        assert!(pool.get(world).is_some());
        assert!(pool.get(client).is_some());
        assert_eq!(pool.world(), world);
        assert_eq!(pool.client(0), client);
    }

    #[test]
    fn reset_client_invalidates_old_id() {
        let mut pool = EdictPool::new(2, 8);
        let old = pool.client(1);
        pool.get_mut(old).unwrap().vars.health = 100.0;
        pool.get_mut(old).unwrap().private = Some(Box::new(7u32));
        pool.reset_client(1);
        assert!(pool.get(old).is_none());
        let new = pool.client(1);
        assert_eq!(new.index, old.index);
        assert_ne!(new.serial, old.serial);
        let edict = pool.get(new).unwrap();
        assert_eq!(edict.vars, EntVars::default());
        assert!(edict.private.is_none());
    }

    #[test]
    fn free_with_stale_id_does_nothing() {
        let mut pool = EdictPool::new(1, 4);
        let old = pool.alloc(0.0).unwrap();
        pool.free(old, 1.0);
        let new = pool.alloc(2.0).unwrap();
        assert_eq!(new.index, old.index);
        pool.get_mut(new).unwrap().vars.health = 5.0;
        pool.free(old, 3.0);
        assert_eq!(pool.get(new).unwrap().vars.health, 5.0);
    }

    #[test]
    fn free_clears_vars_and_private() {
        let mut pool = EdictPool::new(1, 4);
        let id = pool.alloc(0.0).unwrap();
        pool.get_mut(id).unwrap().vars.classname = "info_target".into();
        pool.get_mut(id).unwrap().private = Some(Box::new(1u8));
        pool.free(id, 1.0);
        let again = pool.alloc(2.0).unwrap();
        assert_eq!(again.index, id.index);
        let edict = pool.get(again).unwrap();
        assert_eq!(edict.vars, EntVars::default());
        assert!(edict.private.is_none());
    }

    #[test]
    fn out_of_range_client_and_index_are_rejected() {
        let mut pool = EdictPool::new(2, 8);
        let bogus = pool.client(2);
        assert!(pool.get(bogus).is_none());
        assert!(pool.get_mut(bogus).is_none());
        assert!(pool.get(pool.client(255)).is_none());
        let client = pool.client(1);
        let allocated = pool.alloc(0.0).unwrap();
        // Slot 3 is the first allocatable one.
        assert_eq!(allocated.index, 3);
        pool.reset_client(2);
        pool.reset_client(255);
        assert!(pool.get(client).is_some());
        assert_eq!(pool.client(1), client);
        assert!(pool.get(allocated).is_some());
        assert_eq!(pool.by_index(3).unwrap().0, allocated);
        assert!(
            pool.get(EdictId {
                index: 100,
                serial: 0
            })
            .is_none()
        );
        assert!(pool.by_index(8).is_none());
        assert!(pool.by_index(u16::MAX).is_none());
        // Slot 4 is the next allocatable one and is not yet live.
        assert!(pool.by_index(4).is_none());
        assert_eq!(pool.alloc(0.0).unwrap().index, 4);
    }

    #[test]
    fn iter_live_is_ascending_and_skips_free_slots() {
        let mut pool = EdictPool::new(2, 8);
        let a = pool.alloc(0.0).unwrap();
        let b = pool.alloc(0.0).unwrap();
        let c = pool.alloc(0.0).unwrap();
        pool.free(b, 1.0);
        let live: Vec<u16> = pool.iter_live().map(|(id, _)| id.index).collect();
        assert_eq!(live, vec![0, 1, 2, a.index, c.index]);
        for (id, _) in pool.iter_live() {
            assert!(pool.get(id).is_some());
            assert_eq!(pool.by_index(id.index).unwrap().0, id);
        }
    }

    #[test]
    fn alloc_returns_none_while_only_recently_freed_slots_exist() {
        let mut pool = EdictPool::new(1, 4);
        let a = pool.alloc(0.0).unwrap();
        let b = pool.alloc(0.0).unwrap();
        pool.free(a, 10.0);
        pool.free(b, 10.0);
        assert!(pool.alloc(10.5).is_none());
        assert_eq!(pool.alloc(10.6).unwrap().index, a.index);
    }

    #[test]
    fn small_max_edicts_is_raised_to_cover_player_slots() {
        let mut pool = EdictPool::new(4, 1);
        assert_eq!(pool.max_clients(), 4);
        assert!(pool.get(pool.client(3)).is_some());
        assert_eq!(pool.iter_live().count(), 5);
        assert!(pool.alloc(0.0).is_none());
    }
}
