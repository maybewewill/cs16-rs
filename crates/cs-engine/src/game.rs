//! Boundary between the server engine and the game logic: [`GameDll`] is
//! what a game implements, [`EngineApi`] is the handle the engine passes to
//! it on every call.

use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::info::InfoString;
use cs_proto48::messages::ENTITY_NORMAL;
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;

use crate::client::ClientState;
use crate::edict::{EdictId, EdictPool, EntVars};
use crate::map::ServerMap;
use crate::precache::PrecacheTables;
use crate::server::MAX_RELIABLE_BACKLOG;
use crate::signon::default_light_styles;
use crate::world::WorldLinks;

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
    /// Messages `EngineApi::send` dropped because a target's buffer already held more than
    /// `MAX_RELIABLE_BACKLOG` bytes; the server adds them to `NetStats::outbox_dropped`.
    pub(crate) dropped: u64,
    /// The map the game asked to change to (`EngineApi::change_level`); the server's owner takes
    /// it between frames (`Server::take_pending_change_level`).
    pub(crate) change_level: Option<String>,
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
            dropped: 0,
            change_level: None,
        }
    }

    /// Appends `bytes` to the reliable buffer of `slot`, unless it already holds more than
    /// `MAX_RELIABLE_BACKLOG` bytes: then the message is dropped and counted.
    pub(crate) fn push_reliable(&mut self, slot: usize, bytes: &[u8]) {
        let buffer = &mut self.reliable[slot];
        if buffer.len() > MAX_RELIABLE_BACKLOG {
            self.dropped += 1;
        } else {
            buffer.write_bytes(bytes);
        }
    }

    /// [`push_reliable`](Self::push_reliable) for the unreliable buffer of `slot`.
    pub(crate) fn push_unreliable(&mut self, slot: usize, bytes: &[u8]) {
        let buffer = &mut self.unreliable[slot];
        if buffer.len() > MAX_RELIABLE_BACKLOG {
            self.dropped += 1;
        } else {
            buffer.write_bytes(bytes);
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
/// engine goes through its methods (entities, resources and cvars in `api_entities.rs`,
/// messages and sounds in `api_messages.rs`, client slots, userinfo and kicks here), so each
/// can later be wrapped by a mod hook.
pub struct EngineApi<'a> {
    pub(crate) edicts: &'a mut EdictPool,
    pub(crate) precache: &'a mut PrecacheTables,
    pub(crate) cvars: &'a mut CVarRegistry,
    /// The user messages the game registered; `send_usermsg` encodes with their ids.
    pub(crate) usermsgs: &'a UserMsgRegistry,
    pub(crate) map: &'a ServerMap,
    /// The area tree and the leaves of the linked entities.
    pub(crate) world: &'a mut WorldLinks,
    /// The game's time global: `sv_time`, or the due time of the think being run.
    pub(crate) time: f64,
    /// Server time `sv_time` (HLDS `sv.time`), what slot reuse is measured in.
    pub(crate) sv_time: f64,
    /// Seconds of the current server frame (`host_frametime`); 0 outside a frame.
    pub(crate) frametime: f32,
    /// The server's real time of the current frame (HLDS `realtime`); 0 where the engine has
    /// not set it (unit-test fixtures).
    pub(crate) realtime: f64,
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
        world: &'a mut WorldLinks,
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
            world,
            time,
            sv_time: time,
            frametime,
            realtime: 0.0,
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

    /// `CHANGE_LEVEL`: asks the server to switch to map `map`. The engine does not load maps
    /// itself: the request waits for the server's owner, who loads the map and calls
    /// `Server::change_map` (see `Server::take_pending_change_level`). A name the console's
    /// `changelevel` would refuse (see [`valid_map_name`](crate::valid_map_name)) is ignored and
    /// `false` is returned; a later valid request replaces an earlier one.
    pub fn change_level(&mut self, map: &str) -> bool {
        if !crate::console::valid_map_name(map) {
            return false;
        }
        self.outbox.change_level = Some(map.to_string());
        true
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
    /// The rest of `SV_RunCmd` after `run_usercmd` and the engine's `SV_ProcessImpacts` (the
    /// player relinked, the triggers it met and the impacts of its movement touched):
    /// `PlayerPostThink` and `CmdEnd`. Called with the same `eng.time()` as `run_usercmd`, inside
    /// the same lag-compensation window. Default: nothing.
    fn post_usercmd(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _cmd: &GoldSrcUserCmd) {}
    /// Called at the start of every physics pass (HLDS `StartFrame`), after the frame's
    /// datagrams were handled, with the seconds of the frame (`host_frametime`).
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32);
    /// The think of entity `id`, called in the frame its `nextthink` comes due (`0 < nextthink
    /// <= sv_time + frametime`), in the physics pass after `start_frame`, in ascending entity
    /// order. During the call `eng.time()` is `max(nextthink, sv_time)`. The engine has set
    /// `nextthink` to 0 before the call; the game schedules the next think by setting it again.
    fn think(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId) {}
    /// `pfnTouch`: `toucher` (a trigger, or the first entity of an impact) is touched by
    /// `other`. Called after the game call that found the touch has returned, never nested
    /// inside it; at most `MAX_TOUCHES_PER_FRAME` per frame. Both entities were live when the
    /// call was made.
    ///
    /// The touches of the engine's movement physics run at the point of impact, inside the
    /// physics pass, as in HLDS: in `SV_FlyMove` the toucher's `origin` has been written to
    /// where the move got to but the entity is not linked there yet (its `absmin`/`absmax` and
    /// its place in the area tree are the old ones), and what the touch writes to `origin` and
    /// `velocity` is what the move goes on from. A touch during a push (the pushed entity
    /// hitting something) finds the pusher `SOLID_NOT`: the push moves the entity with the
    /// pusher out of the way.
    fn touch(&mut self, _eng: &mut EngineApi<'_>, _toucher: EdictId, _other: EdictId) {}
    /// `pfnBlocked`: `other` blocked the movement of the pusher `pusher`. Called by the
    /// engine's push physics. As in HLDS the pusher is already back where it was, but the
    /// entities it moved earlier in the same push are still at their pushed places when this
    /// runs; they are put back (to where they were) after it returns. The game may remove
    /// `pusher` or `other`; what is left of the moved entities is put back all the same.
    fn blocked(&mut self, _eng: &mut EngineApi<'_>, _pusher: EdictId, _other: EdictId) {}
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData;
    /// A server console command no engine command or cvar claims, with `argv[0]` the command
    /// name as typed (never empty). Return the text to print, or `None` if the game does not
    /// know the command (the console then reports it unknown). The engine replaces control
    /// characters other than newlines in the returned text; a game that echoes client-supplied
    /// text should still keep it short.
    fn server_command(&mut self, _eng: &mut EngineApi<'_>, _argv: &[String]) -> Option<String> {
        None
    }
    /// `SetupVisibility` of the game library: the point `slot`'s client sees the world from, when
    /// it is not the player edict's own eye (`origin + view_ofs`): a spectator watching a
    /// player sees what that player sees. `None`: the edict's own eye. The engine asks once per
    /// update, before it picks the entities the client is sent.
    fn setup_visibility(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Option<Vec3> {
        None
    }
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
        world: WorldLinks,
        outbox: Outbox,
        clients: Vec<ClientView>,
    }

    impl Fixture {
        fn new(max_clients: u8) -> Self {
            let map = ServerMap::stub("de_test");
            Self {
                world: WorldLinks::new(map.world_bounds()),
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
                &mut self.world,
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
