//! Boundary between the server engine and the game logic: [`GameDll`] is
//! what a game implements, [`EngineApi`] is the handle the engine passes to
//! it on every call.

use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::info::InfoString;
use cs_proto48::messages::ENTITY_NORMAL;
use cs_proto48::usermsg::UserMsgRegistry;

use crate::edict::{EdictPool, EntVars};
use crate::map::ServerMap;
use crate::precache::PrecacheTables;

/// `FL_DUCKING` in `EntVars::flags`.
const FL_DUCKING: u32 = 1 << 14;

/// Messages and kicks the game queued during one call; the server drains
/// them afterwards. Per-client buffers are indexed by slot.
pub(crate) struct Outbox {
    pub(crate) reliable: Vec<MsgWriter>,
    pub(crate) unreliable: Vec<MsgWriter>,
    pub(crate) reliable_all: MsgWriter,
    pub(crate) kicks: Vec<(u8, String)>,
}

impl Outbox {
    /// One reliable and one unreliable writer per slot.
    #[cfg_attr(not(test), expect(dead_code, reason = "constructed by the server"))]
    pub(crate) fn new(max_clients: u8) -> Self {
        let slots = usize::from(max_clients);
        Self {
            reliable: vec![MsgWriter::new(); slots],
            unreliable: vec![MsgWriter::new(); slots],
            reliable_all: MsgWriter::new(),
            kicks: Vec::new(),
        }
    }
}

/// What the game may see and change of one client slot.
#[derive(Debug, Default)]
pub(crate) struct ClientView {
    pub(crate) userinfo: InfoString,
    pub(crate) active: bool,
    /// Set when the game changed `userinfo`; the server broadcasts it.
    pub(crate) userinfo_dirty: bool,
}

/// Engine services available to the game during one call.
pub struct EngineApi<'a> {
    pub edicts: &'a mut EdictPool,
    pub precache: &'a mut PrecacheTables,
    pub cvars: &'a mut CVarRegistry,
    pub map: &'a ServerMap,
    pub time: f64,
    pub(crate) outbox: &'a mut Outbox,
    pub(crate) clients: &'a mut [ClientView],
    /// Handed out for out-of-range slots; its contents are discarded.
    scratch: MsgWriter,
}

impl<'a> EngineApi<'a> {
    #[cfg_attr(not(test), expect(dead_code, reason = "constructed by the server"))]
    pub(crate) fn new(
        edicts: &'a mut EdictPool,
        precache: &'a mut PrecacheTables,
        cvars: &'a mut CVarRegistry,
        map: &'a ServerMap,
        time: f64,
        outbox: &'a mut Outbox,
        clients: &'a mut [ClientView],
    ) -> Self {
        Self {
            edicts,
            precache,
            cvars,
            map,
            time,
            outbox,
            clients,
            scratch: MsgWriter::new(),
        }
    }
}

impl EngineApi<'_> {
    /// Reliable buffer of `slot`.
    pub fn reliable(&mut self, slot: u8) -> &mut MsgWriter {
        match self.outbox.reliable.get_mut(usize::from(slot)) {
            Some(writer) => writer,
            None => discard(&mut self.scratch),
        }
    }

    /// Unreliable buffer of `slot`.
    pub fn unreliable(&mut self, slot: u8) -> &mut MsgWriter {
        match self.outbox.unreliable.get_mut(usize::from(slot)) {
            Some(writer) => writer,
            None => discard(&mut self.scratch),
        }
    }

    /// Reliable buffer sent to every client.
    pub fn reliable_all(&mut self) -> &mut MsgWriter {
        &mut self.outbox.reliable_all
    }

    pub fn client_key(&self, slot: u8, key: &str) -> Option<&str> {
        self.clients.get(usize::from(slot))?.userinfo.get(key)
    }

    /// Sets a userinfo key and marks the slot's userinfo for broadcast.
    pub fn set_client_key(&mut self, slot: u8, key: &str, value: &str) {
        if let Some(client) = self.clients.get_mut(usize::from(slot)) {
            client.userinfo.set(key, value);
            client.userinfo_dirty = true;
        }
    }

    /// Queues a kick; a second kick of the same slot keeps the first reason.
    pub fn kick(&mut self, slot: u8, reason: &str) {
        if usize::from(slot) >= self.clients.len()
            || self.outbox.kicks.iter().any(|(s, _)| *s == slot)
        {
            return;
        }
        self.outbox.kicks.push((slot, reason.to_string()));
    }

    pub fn is_client_active(&self, slot: u8) -> bool {
        self.clients
            .get(usize::from(slot))
            .is_some_and(|c| c.active)
    }
}

/// Empties the scratch writer and hands it out.
fn discard(scratch: &mut MsgWriter) -> &mut MsgWriter {
    *scratch = MsgWriter::new();
    scratch
}

/// What a game implements. The engine calls it with a fresh [`EngineApi`]
/// on every call.
pub trait GameDll {
    fn usermsgs(&self) -> UserMsgRegistry;
    fn server_activate(&mut self, eng: &mut EngineApi<'_>);
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>);
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String>;
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8);
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8);
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]);
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8);
    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    );
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32);
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData;
    /// Whether `ent` goes into `viewer`'s packet; may adjust `state`.
    fn add_to_full_pack(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _viewer: u8,
        _ent: u16,
        _state: &mut GoldSrcEntityState,
    ) -> bool {
        true
    }
}

/// Network state of edict `index` built from its variables.
pub fn entity_state_from_vars(index: u16, v: &EntVars) -> GoldSrcEntityState {
    GoldSrcEntityState {
        number: index,
        entity_type: ENTITY_NORMAL,
        origin: v.origin,
        angles: v.angles,
        modelindex: v.modelindex,
        sequence: v.sequence,
        gaitsequence: v.gaitsequence,
        frame: v.frame,
        animtime: v.animtime,
        framerate: v.framerate,
        weaponmodel: v.weaponmodel,
        movetype: v.movetype,
        solid: v.solid,
        effects: v.effects,
        rendermode: v.rendermode,
        renderamt: v.renderamt,
        renderfx: v.renderfx,
        skin: v.skin,
        body: v.body,
        scale: v.scale,
        owner: v.owner.map_or(0, |id| id.index),
        aiment: v.aiment.map_or(0, |id| id.index),
        usehull: u8::from(v.flags & FL_DUCKING != 0),
        spectator: false,
        mins: v.mins,
        maxs: v.maxs,
        colormap: v.colormap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edict::{DEFAULT_MAX_EDICTS, EdictId};
    use glam::Vec3;

    #[test]
    fn entity_state_copies_vars() {
        let v = EntVars {
            origin: Vec3::new(1.0, 2.0, 3.0),
            modelindex: 7,
            sequence: 4,
            effects: 8,
            ..Default::default()
        };
        let s = entity_state_from_vars(21, &v);
        assert_eq!(
            (s.number, s.origin, s.modelindex, s.sequence, s.effects),
            (21, v.origin, 7, 4, 8)
        );
    }

    #[test]
    fn entity_state_copies_every_field() {
        let v = EntVars {
            origin: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::new(4.0, 5.0, 6.0),
            mins: Vec3::new(-16.0, -16.0, -36.0),
            maxs: Vec3::new(16.0, 16.0, 36.0),
            modelindex: 7,
            weaponmodel: 9,
            sequence: 4,
            gaitsequence: 3,
            frame: 0.5,
            animtime: 1.25,
            framerate: 2.0,
            scale: 1.5,
            body: 2,
            skin: -1,
            colormap: 0x1234,
            effects: 8,
            rendermode: 5,
            renderamt: 200,
            renderfx: 3,
            solid: 2,
            movetype: 6,
            flags: FL_DUCKING | 1,
            owner: Some(EdictId {
                index: 5,
                serial: 9,
            }),
            aiment: Some(EdictId {
                index: 6,
                serial: 0,
            }),
            ..Default::default()
        };
        let s = entity_state_from_vars(21, &v);
        assert_eq!(
            s,
            GoldSrcEntityState {
                number: 21,
                entity_type: ENTITY_NORMAL,
                origin: v.origin,
                angles: v.angles,
                modelindex: 7,
                sequence: 4,
                gaitsequence: 3,
                frame: 0.5,
                animtime: 1.25,
                framerate: 2.0,
                weaponmodel: 9,
                movetype: 6,
                solid: 2,
                effects: 8,
                rendermode: 5,
                renderamt: 200,
                renderfx: 3,
                skin: -1,
                body: 2,
                scale: 1.5,
                owner: 5,
                aiment: 6,
                usehull: 1,
                spectator: false,
                mins: v.mins,
                maxs: v.maxs,
                colormap: 0x1234,
            }
        );

        let standing = entity_state_from_vars(3, &EntVars::default());
        assert_eq!(
            (standing.owner, standing.aiment, standing.usehull),
            (0, 0, 0)
        );
        assert_eq!(standing.entity_type, ENTITY_NORMAL);
    }

    struct Fixture {
        edicts: EdictPool,
        precache: PrecacheTables,
        cvars: CVarRegistry,
        map: ServerMap,
        outbox: Outbox,
        clients: Vec<ClientView>,
    }

    impl Fixture {
        fn new(max_clients: u8) -> Self {
            let map = ServerMap::stub("de_test");
            Self {
                edicts: EdictPool::new(max_clients, DEFAULT_MAX_EDICTS),
                precache: PrecacheTables::new(&map.path, map.submodel_count),
                cvars: CVarRegistry::new_empty(),
                map,
                outbox: Outbox::new(max_clients),
                clients: (0..max_clients).map(|_| ClientView::default()).collect(),
            }
        }

        fn api(&mut self) -> EngineApi<'_> {
            EngineApi::new(
                &mut self.edicts,
                &mut self.precache,
                &mut self.cvars,
                &self.map,
                1.0,
                &mut self.outbox,
                &mut self.clients,
            )
        }
    }

    #[test]
    fn per_slot_writers_are_independent() {
        let mut f = Fixture::new(2);
        let mut eng = f.api();
        eng.reliable(0).write_u8(1);
        eng.reliable(1).write_u8(2);
        eng.reliable(1).write_u8(3);
        eng.unreliable(0).write_u8(4);
        eng.reliable_all().write_u8(5);
        drop(eng);
        assert_eq!(f.outbox.reliable[0].as_slice(), &[1]);
        assert_eq!(f.outbox.reliable[1].as_slice(), &[2, 3]);
        assert_eq!(f.outbox.unreliable[0].as_slice(), &[4]);
        assert!(f.outbox.unreliable[1].is_empty());
        assert_eq!(f.outbox.reliable_all.as_slice(), &[5]);
    }

    #[test]
    fn out_of_range_slots_do_not_panic() {
        let mut f = Fixture::new(2);
        let mut eng = f.api();
        eng.reliable(2).write_u8(1);
        eng.unreliable(255).write_u8(2);
        // The scratch writer starts empty every time it is handed out.
        assert!(eng.reliable(3).is_empty());
        assert_eq!(eng.client_key(2, "name"), None);
        eng.set_client_key(2, "name", "x");
        eng.kick(2, "bye");
        assert!(!eng.is_client_active(2));
        assert!(!eng.is_client_active(255));
        drop(eng);
        assert!(f.outbox.reliable.iter().all(MsgWriter::is_empty));
        assert!(f.outbox.unreliable.iter().all(MsgWriter::is_empty));
        assert!(f.outbox.reliable_all.is_empty());
        assert!(f.outbox.kicks.is_empty());
        assert!(f.clients.iter().all(|c| !c.userinfo_dirty));
    }

    #[test]
    fn set_client_key_marks_dirty_and_reads_back() {
        let mut f = Fixture::new(2);
        f.clients[1].active = true;
        let mut eng = f.api();
        assert_eq!(eng.client_key(1, "model"), None);
        assert!(eng.is_client_active(1));
        assert!(!eng.is_client_active(0));
        eng.set_client_key(1, "model", "gign");
        assert_eq!(eng.client_key(1, "model"), Some("gign"));
        assert_eq!(eng.client_key(0, "model"), None);
        drop(eng);
        assert!(f.clients[1].userinfo_dirty);
        assert!(!f.clients[0].userinfo_dirty);
    }

    #[test]
    fn kick_records_once() {
        let mut f = Fixture::new(2);
        let mut eng = f.api();
        eng.kick(1, "first");
        eng.kick(1, "second");
        eng.kick(0, "other");
        drop(eng);
        assert_eq!(
            f.outbox.kicks,
            vec![(1, "first".to_string()), (0, "other".to_string())]
        );
    }
}
