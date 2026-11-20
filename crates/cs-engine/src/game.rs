//! Boundary between the server engine and the game logic: [`GameDll`] is
//! what a game implements, [`EngineApi`] is the handle the engine passes to
//! it on every call.

use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::info::InfoString;
use cs_proto48::messages::ENTITY_NORMAL;
use cs_proto48::usermsg::UserMsgRegistry;

use crate::client::ClientState;
use crate::edict::{EdictId, EdictPool, EntVars};
use crate::map::ServerMap;
use crate::precache::PrecacheTables;
use crate::signon::default_light_styles;

/// `FL_DUCKING` in `EntVars::flags`.
const FL_DUCKING: u32 = 1 << 14;

/// Messages and kicks the game queued during one call, which the server drains afterwards,
/// and the message state that outlives a call: the init buffer and the light styles. Per-client
/// buffers are indexed by slot; a destination is resolved to slots when the game sends, so each
/// buffer keeps the order the game wrote in.
pub(crate) struct Outbox {
    pub(crate) reliable: Vec<MsgWriter>,
    pub(crate) unreliable: Vec<MsgWriter>,
    pub(crate) kicks: Vec<(u8, String)>,
    /// Messages sent to `MsgDest::Init` during map activation, replayed to every client that
    /// enters the server. Cleared when the map changes.
    pub(crate) init: Vec<u8>,
    /// The map is being activated (`spawn_entity` and `server_activate` calls): `Init` messages
    /// are stored.
    pub(crate) init_open: bool,
    /// The 64 light style patterns, sent to every client that enters the server.
    pub(crate) light_styles: Vec<String>,
}

impl Outbox {
    /// One reliable and one unreliable writer per slot.
    pub(crate) fn new(max_clients: u8) -> Self {
        let slots = usize::from(max_clients);
        Self {
            reliable: vec![MsgWriter::new(); slots],
            unreliable: vec![MsgWriter::new(); slots],
            kicks: Vec::new(),
            init: Vec::new(),
            init_open: false,
            light_styles: default_light_styles(),
        }
    }

    /// Forgets what belonged to the previous map: the init buffer and the light styles.
    pub(crate) fn reset_map_state(&mut self) {
        self.init.clear();
        self.init_open = false;
        self.light_styles = default_light_styles();
    }
}

/// What the game may see and change of one client slot.
#[derive(Debug, Default)]
pub(crate) struct ClientView {
    pub(crate) userinfo: InfoString,
    /// The slot holds a connection; the userinfo of a free slot cannot be changed.
    pub(crate) in_use: bool,
    pub(crate) active: bool,
    /// Sign-on state of the slot, copied from the server before every call into the game; it
    /// decides who a message destination reaches.
    pub(crate) state: ClientState,
    /// Set when the game changed `userinfo`; the server broadcasts it.
    pub(crate) userinfo_dirty: bool,
}

/// Engine services available to the game during one call. Everything the game does to the
/// engine goes through its methods (entities, resources and cvars in `api_entities.rs`, clients
/// and messages here), so each can later be wrapped by a mod hook.
pub struct EngineApi<'a> {
    pub(crate) edicts: &'a mut EdictPool,
    pub(crate) precache: &'a mut PrecacheTables,
    pub(crate) cvars: &'a mut CVarRegistry,
    /// The user messages the game registered; `send_usermsg` encodes with their ids.
    pub(crate) usermsgs: &'a UserMsgRegistry,
    pub(crate) map: &'a ServerMap,
    pub(crate) time: f64,
    /// Real seconds of the current server frame; 0 outside a frame.
    pub(crate) frametime: f32,
    pub(crate) outbox: &'a mut Outbox,
    pub(crate) clients: &'a mut [ClientView],
}

impl<'a> EngineApi<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        edicts: &'a mut EdictPool,
        precache: &'a mut PrecacheTables,
        cvars: &'a mut CVarRegistry,
        usermsgs: &'a UserMsgRegistry,
        map: &'a ServerMap,
        time: f64,
        frametime: f32,
        outbox: &'a mut Outbox,
        clients: &'a mut [ClientView],
    ) -> Self {
        debug_assert_eq!(outbox.reliable.len(), clients.len());
        debug_assert_eq!(outbox.unreliable.len(), clients.len());
        Self {
            edicts,
            precache,
            cvars,
            usermsgs,
            map,
            time,
            frametime,
            outbox,
            clients,
        }
    }
}

impl EngineApi<'_> {
    pub fn client_key(&self, slot: u8, key: &str) -> Option<&str> {
        self.clients.get(usize::from(slot))?.userinfo.get(key)
    }

    /// Sets a userinfo key and marks the slot's userinfo for broadcast; a free slot is left
    /// alone.
    pub fn set_client_key(&mut self, slot: u8, key: &str, value: &str) {
        if let Some(client) = self.clients.get_mut(usize::from(slot))
            && client.in_use
        {
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

/// What a game implements. The engine calls it with a fresh [`EngineApi`]
/// on every call.
pub trait GameDll {
    fn usermsgs(&self) -> UserMsgRegistry;
    fn server_activate(&mut self, eng: &mut EngineApi<'_>);
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>);
    /// An entry of the map's entity lump, offered before `server_activate`. `id` is a fresh
    /// edict (the world for the first `worldspawn` entry) whose variables the engine has already
    /// filled from `keys`, and `model` is set when it was precached; `keys` are the
    /// `(key, value)` pairs, at most `MAX_ENTITY_KEYS` of them, `classname` first. Return
    /// `false` to refuse the entry: the engine frees the edict (the world is never removed).
    /// A game may remove the entity itself and then return `false`.
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool;
    /// A client asks to connect; `Err(text)` refuses it with `text`. A client that never
    /// finishes sign-on gets no `client_disconnect` afterwards (that is only called for clients
    /// that were put in the server), so a game must not allocate per-client state here; do it
    /// in `client_put_in_server`.
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
    /// Called once at the start of every server frame, before its datagrams are handled, with
    /// the seconds elapsed since the previous frame.
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32);
    /// The think of entity `id`, called in the frame its `nextthink` comes due (`0 < nextthink
    /// <= time`), after `start_frame`. The engine has set `nextthink` to 0 before the call; the
    /// game schedules the next think by setting it again. At most `MAX_THINKS_PER_FRAME` per
    /// frame.
    fn think(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId) {}
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
    use crate::api_messages::MsgDest;
    use crate::edict::DEFAULT_MAX_EDICTS;
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
        usermsgs: UserMsgRegistry,
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
                usermsgs: UserMsgRegistry::cs16_default(),
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
                &self.usermsgs,
                &self.map,
                1.0,
                0.0,
                &mut self.outbox,
                &mut self.clients,
            )
        }
    }

    #[test]
    fn out_of_range_slots_do_not_panic() {
        let mut f = Fixture::new(2);
        let mut eng = f.api();
        eng.send(MsgDest::One(2), &[1]);
        eng.send(MsgDest::OneUnreliable(255), &[2]);
        assert_eq!(eng.client_key(2, "name"), None);
        eng.set_client_key(2, "name", "x");
        eng.kick(2, "bye");
        assert!(!eng.is_client_active(2));
        assert!(!eng.is_client_active(255));
        assert!(f.outbox.reliable.iter().all(MsgWriter::is_empty));
        assert!(f.outbox.unreliable.iter().all(MsgWriter::is_empty));
        assert!(f.outbox.kicks.is_empty());
        assert!(f.clients.iter().all(|c| !c.userinfo_dirty));
    }

    #[test]
    fn set_client_key_marks_dirty_and_reads_back() {
        let mut f = Fixture::new(2);
        f.clients[1].in_use = true;
        f.clients[1].active = true;
        let mut eng = f.api();
        assert_eq!(eng.client_key(1, "model"), None);
        assert!(eng.is_client_active(1));
        assert!(!eng.is_client_active(0));
        eng.set_client_key(1, "model", "gign");
        assert_eq!(eng.client_key(1, "model"), Some("gign"));
        assert_eq!(eng.client_key(0, "model"), None);
        assert!(f.clients[1].userinfo_dirty);
        assert!(!f.clients[0].userinfo_dirty);
    }

    #[test]
    fn set_client_key_ignores_a_free_slot() {
        let mut f = Fixture::new(2);
        let mut eng = f.api();
        eng.set_client_key(0, "name", "Ghost");
        assert_eq!(eng.client_key(0, "name"), None);
        assert!(!f.clients[0].userinfo_dirty);
    }

    #[test]
    fn kick_records_once() {
        let mut f = Fixture::new(2);
        let mut eng = f.api();
        eng.kick(1, "first");
        eng.kick(1, "second");
        eng.kick(0, "other");
        assert_eq!(
            f.outbox.kicks,
            vec![(1, "first".to_string()), (0, "other".to_string())]
        );
    }
}
