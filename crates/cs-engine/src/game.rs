//! Boundary between the server engine and the game logic: [`GameDll`] is
//! what a game implements, [`EngineApi`] is the handle the engine passes to
//! it on every call.

use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{DeltaMarks, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::info::InfoString;
use cs_proto48::messages::{ENTITY_BEAM, ENTITY_NORMAL};
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;

use crate::client::ClientState;
use crate::edict::{EdictId, EdictPool, EntVars};
use crate::map::ServerMap;
use crate::precache::PrecacheTables;
use crate::server::MAX_RELIABLE_BACKLOG;
use crate::signon::default_light_styles;
use crate::world::{LinkSnapshot, WorldLinks};

/// `FL_DUCKING` in `EntVars::flags`.
const FL_DUCKING: u32 = 1 << 14;
/// `EFLAG_SLERP` of `entity_state_t::eflags`: the client interpolates the studio model.
const EFLAG_SLERP: u8 = 1;
use crate::world::consts::{MOVETYPE_WALK, SOLID_SLIDEBOX};

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
    /// The events queued for each client, waiting for its next update (see `events.rs`).
    pub(crate) events: Vec<crate::events::EventQueue>,
    /// The `event_t` delta schema `svc_event_reliable` is encoded with; `None` where the
    /// server has not set it (a reliable event is then not sent).
    pub(crate) event_schema: Option<cs_proto48::delta::DeltaSchema>,
    /// While the game puts the client of this slot in the server (`ClientPutInServer`), what it
    /// sends to that client alone (`MsgDest::One`, `MsgDest::OneUnreliable`) is thrown away: the
    /// original clears the client's reliable and unreliable buffers after the call.
    pub(crate) discard_direct: Option<usize>,
    /// `sv.reliable_datagram`: what the game sent to `MsgDest::All` since the last flush. The
    /// server appends it to the reliable stream of every client in the game at the end of the
    /// frame (`SV_UpdateToReliableMessages`), after what the game sent to single clients.
    pub(crate) reliable_all: MsgWriter,
    /// Slots `EngineApi::create_fake_client` took since the server last looked: the game sees the
    /// slot taken at once (its `ClientView`), the server gives it its `Client` before the next
    /// call into the game and before it drains (`Server::adopt_fake_clients`).
    pub(crate) fake_created: Vec<u8>,
    /// The commands `EngineApi::run_player_move` queued, run in order after the game call that
    /// queued them returns (the game is never called from inside a game call).
    pub(crate) fake_moves: Vec<FakeMove>,
    /// Moves `EngineApi::run_player_move` refused (not finite, not a fake client's slot, queue
    /// full); the server adds them to `NetStats::fake_moves_dropped`.
    pub(crate) fake_moves_dropped: u64,
}

/// One `PF_RunPlayerMove_I` waiting to run.
#[derive(Debug, Clone)]
pub(crate) struct FakeMove {
    pub(crate) slot: u8,
    pub(crate) cmd: GoldSrcUserCmd,
}

/// Moves `EngineApi::run_player_move` may have queued at once; further ones are refused.
pub(crate) const MAX_QUEUED_FAKE_MOVES: usize = 1024;

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
            events: vec![crate::events::EventQueue::default(); slots],
            event_schema: None,
            discard_direct: None,
            reliable_all: MsgWriter::new(),
            fake_created: Vec::new(),
            fake_moves: Vec::new(),
            fake_moves_dropped: 0,
        }
    }

    /// `self` with the `event_t` schema reliable events are encoded with.
    pub(crate) fn with_event_schema(mut self, schema: Option<cs_proto48::delta::DeltaSchema>) -> Self {
        self.event_schema = schema;
        self
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

    /// Appends `bytes` to [`reliable_all`](Self::reliable_all) under the same cap as
    /// [`push_reliable`](Self::push_reliable).
    pub(crate) fn push_reliable_all(&mut self, bytes: &[u8]) {
        if self.reliable_all.len() > MAX_RELIABLE_BACKLOG {
            self.dropped += 1;
        } else {
            self.reliable_all.write_bytes(bytes);
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
        self.reliable_all = MsgWriter::new();
        self.light_styles = default_light_styles();
    }
}

/// What the game may see and change of one client slot.
#[derive(Debug, Default)]
pub(crate) struct ClientView {
    pub(crate) userinfo: InfoString,
    /// `client_t::physinfo`: what the game set with `SET_PHYSICS_KEY_VALUE`; empty at connect
    /// (ReHLDS `sv_main.cpp` clears it there). It goes to the client in its client data.
    pub(crate) physinfo: InfoString,
    /// The slot holds a connection; the userinfo of a free slot cannot be changed.
    pub(crate) in_use: bool,
    pub(crate) active: bool,
    /// Sign-on state of the slot, copied from the server before every call into the game; it
    /// decides who a message destination reaches.
    pub(crate) state: ClientState,
    /// Set when the game changed `userinfo`; the server broadcasts it.
    pub(crate) userinfo_dirty: bool,
    /// `client_t::fakeclient`: the slot holds a bot made by `EngineApi::create_fake_client`.
    pub(crate) fake: bool,
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
    /// The studio poses the traces of this call made (`rgStudioCache`).
    pub(crate) studio_cache: crate::studio::SharedStudioCache,
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
            studio_cache: Default::default(),
        }
    }
}

/// Client userinfo key advertising support for extended WASM mod usermsgs and auto-download.
pub const MOD_CAPABILITY_KEY: &str = "_cs16rs_mods";

impl EngineApi<'_> {
    /// Returns true if the client in `slot` advertised mod capability (`_cs16rs_mods 1`).
    pub fn client_supports_mods(&self, slot: u8) -> bool {
        self.client_key(slot, MOD_CAPABILITY_KEY) == Some("1")
    }

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

    /// `SET_PHYSICS_KEY_VALUE` (`PF_SetPhysicsKeyValue`, ReHLDS `pr_cmds.cpp`, read 2026-11-22):
    /// sets a key of the slot's physics info; a free slot is left alone.
    pub fn set_physics_key(&mut self, slot: u8, key: &str, value: &str) {
        if let Some(client) = self.clients.get_mut(usize::from(slot))
            && client.in_use
        {
            client.physinfo.set(key, value);
        }
    }

    /// `ENGINE_GETPHYSINFO`: the slot's physics info as an info string (`\keyalue...`),
    /// empty for a free slot.
    pub fn physics_info(&self, slot: u8) -> String {
        self.clients
            .get(usize::from(slot))
            .filter(|c| c.in_use)
            .map_or_else(String::new, |c| c.physinfo.to_wire_string())
    }

    /// `PF_CreateFakeClient_I` / `CreateFakeClient_internal` (ReHLDS `pr_cmds.cpp` 1975-2033, read
    /// 2026-11-23): takes the first free slot for a bot called `name` and returns its number;
    /// `None` when every slot is taken. The slot has no address and no netchan peer, and the
    /// engine never sends it anything: it counts as in the game from the start (`active`,
    /// `spawned`, `connected`), the edict gets `FL_FAKECLIENT | FL_CLIENT` (the flags are
    /// assigned, as in the source), and the userinfo is `name`, `model` `gordon`, `topcolor` 1,
    /// `bottomcolor` 1 and `*bot` 1 (the source has no `*bot`: ReGameDLL's `AddBot` sets it
    /// right after; here it is there from the start), marked for broadcast (`sendinfo`).
    ///
    /// As in the source `ClientConnect` is not called and the game puts the bot in the server
    /// itself (`ClientPutInServer`). Two deviations, both because the engine never calls the
    /// game from inside a game call: `SV_ExtractFromUserinfo` of the source calls
    /// `pfnClientUserInfoChanged`, which is not done here; and the name is sanitised and made
    /// unique as a connecting client's is (`SV_CheckForDuplicateNames`, which the source also
    /// runs from `SV_ExtractFromUserinfo`), so the bot may be called `(1)name`. The userid
    /// comes from the same counter as the clients' (`g_userid++`), taken when the server adopts
    /// the slot.
    pub fn create_fake_client(&mut self, name: &str) -> Option<u8> {
        let slot = self.clients.iter().position(|c| !c.in_use)?;
        let slot_u8 = u8::try_from(slot).ok()?;
        let name = crate::connect::unique_name_among(self.clients, &crate::oob::sanitize_name(name), None);
        let mut userinfo = InfoString::default();
        userinfo.set("name", name);
        userinfo.set("model", "gordon");
        userinfo.set("topcolor", "1");
        userinfo.set("bottomcolor", "1");
        userinfo.set("*bot", "1");
        let edict = self.edicts.client(slot_u8);
        self.edicts.get_mut(edict)?.vars.flags =
            crate::world::consts::FL_FAKECLIENT | crate::world::consts::FL_CLIENT;
        self.clients[slot] = ClientView {
            userinfo,
            in_use: true,
            active: true,
            state: ClientState::Active,
            userinfo_dirty: true,
            fake: true,
            ..ClientView::default()
        };
        self.outbox.fake_created.push(slot_u8);
        Some(slot_u8)
    }

    /// Whether `slot` holds a fake client (`client_t::fakeclient`); false for a real client, a
    /// free slot or a slot out of range.
    pub fn is_fake_client(&self, slot: u8) -> bool {
        self.clients.get(usize::from(slot)).is_some_and(|c| c.in_use && c.fake)
    }

    /// `PF_RunPlayerMove_I` (ReHLDS `pr_cmds.cpp` 2035-2066, `SV_RunCmd` in `sv_user.cpp`, read
    /// 2026-11-23): runs one user command for the fake client in `slot`, as `SV_RunCmd` does for
    /// a client's, with the game's `run_usercmd` and `post_usercmd` and the engine's
    /// `SV_ProcessImpacts` in between. The command is built from the arguments (`random_seed`
    /// 0, every other field zero), a command of more than 50 ms is split in halves, and the
    /// time base is `host_frametime + sv.time - msec / 1000`.
    ///
    /// The command runs after the game call that made this one returns, not inside it (the
    /// engine never calls the game from inside a game call), in the order queued. A fake client
    /// does not take part in lag compensation (`SV_SetupMove` and `SV_RestoreMove` are skipped
    /// for it) and is never sent movevars. Nothing is queued for a slot that is not a fake
    /// client (the source would run it for any client; that is not supported here), for numbers
    /// that are not finite, or when `MAX_QUEUED_FAKE_MOVES` are already waiting; each refusal is
    /// counted in `NetStats::fake_moves_dropped`.
    #[allow(clippy::too_many_arguments)]
    pub fn run_player_move(
        &mut self,
        slot: u8,
        viewangles: Vec3,
        forwardmove: f32,
        sidemove: f32,
        upmove: f32,
        buttons: u16,
        impulse: u8,
        msec: u8,
    ) {
        if !self.is_fake_client(slot)
            || !viewangles.is_finite()
            || !forwardmove.is_finite()
            || !sidemove.is_finite()
            || !upmove.is_finite()
            || self.outbox.fake_moves.len() >= MAX_QUEUED_FAKE_MOVES
        {
            self.outbox.fake_moves_dropped += 1;
            return;
        }
        let cmd = GoldSrcUserCmd {
            viewangles,
            forwardmove,
            sidemove,
            upmove,
            buttons,
            impulse,
            msec,
            ..GoldSrcUserCmd::default()
        };
        self.outbox.fake_moves.push(FakeMove { slot, cmd });
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

    /// Marks the client of `slot` active or not without a client behind it (`client->active`
    /// is set by the engine when a client enters the game): for a game's tests that stand a
    /// player in a slot by calling `client_put_in_server` themselves. A slot out of range is
    /// ignored. Test-only (`test-hooks` feature).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn test_set_client_active(&mut self, slot: u8, active: bool) {
        if let Some(client) = self.clients.get_mut(usize::from(slot)) {
            client.active = active;
        }
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
    /// `pfnGetWeaponData`: fills `out`, the `weapon_data_t` list of `slot`'s player (index =
    /// weapon id, 64 entries, entries the game does not set stay as they are: default), for the
    /// `svc_clientdata` of the client's next update. The engine asks only for a client whose
    /// `cl_lw` is on (see [`EngineApi::client_lw`]), after [`update_client_data`](Self::update_client_data)
    /// of the same update. Return `false` to send no weapon data (the default).
    fn get_weapon_data(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _slot: u8,
        _out: &mut [GoldSrcWeaponData; 64],
    ) -> bool {
        false
    }
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
    /// `AddToFullPack` of the game library (`pfnAddToFullPack`): whether the entity `ctx.ent`
    /// goes into the packet of `ctx.viewer`, and the state it carries. `state` is the plain copy
    /// of the edict ([`entity_state_from_vars`]); the game may edit it. The engine asks for every
    /// client slot, then for the other live edicts in index order until the list is full; it
    /// skips the edicts the source's first two checks refuse (`EF_NODRAW` on another entity, no
    /// model), which need no game state. Every other filter is the game's: see [`FullPackCtx`]. The
    /// default is the engine's own cull: potentially visible, or the viewer itself.
    fn add_to_full_pack(
        &mut self,
        _eng: &mut EngineApi<'_>,
        ctx: &FullPackCtx<'_>,
        _state: &mut GoldSrcEntityState,
    ) -> bool {
        ctx.in_pvs || ctx.ent_id == ctx.viewer_id
    }
    /// A conditional delta encoder of the game (`DELTA_ADDENCODER` in `RegisterEncoders`): runs
    /// for each entity delta whose `delta.lst` schema names `encoder` (`Entity_Encode`,
    /// `Player_Encode`, `Custom_Encode`), after the changed fields are marked, and may set and
    /// clear marks. `from` is the state the delta starts from (zero for a new entity written
    /// against no base), `to` the state it goes to, `current_player` the slot of the client the
    /// packet is for (`ENGINE_CURRENT_PLAYER`). The default leaves every mark as it is.
    fn delta_encode(
        &mut self,
        _encoder: &str,
        _current_player: u8,
        _from: &GoldSrcEntityState,
        _to: &GoldSrcEntityState,
        _marks: &mut DeltaMarks<'_>,
    ) {
    }

    /// Whether [`delta_encode`](Self::delta_encode) does anything. A game that does not override
    /// it returns `false`, and the engine then skips the encoder hook (and the decoding of the
    /// `from` state it needs) for every entity of every update. The default is `true`, which is
    /// always correct.
    fn has_delta_encoder(&self) -> bool {
        true
    }
}

/// What the engine tells the game about one `AddToFullPack` question: the arguments of the
/// original (`ent`, `host`, `hostflags`, `pSet`) as the game can use them.
#[derive(Debug, Clone, Copy)]
pub struct FullPackCtx<'a> {
    /// The entity asked about (`e`): its edict number.
    pub ent: u16,
    /// Its edict.
    pub ent_id: EdictId,
    /// The client the packet is for: its slot (`ENTINDEX(host) - 1`).
    pub viewer: u8,
    /// The viewer's player edict (`host`).
    pub viewer_id: EdictId,
    /// `hostflags`: bit 0 is the client's `cl_lw`.
    pub hostflags: u8,
    /// `ENGINE_CHECK_VISIBILITY(ent, pSet)`: the entity is potentially visible from the viewpoint
    /// `SetupVisibility` chose (true when the viewer sees everything). The engine always
    /// computes it; the original only asks when the entity has not been in the viewer's PVS for
    /// a second.
    pub in_pvs: bool,
    /// The viewer edict's `headnode` and `leafnums` (`client->headnode`, `client->leafnums`).
    pub viewer_leaves: &'a LinkSnapshot,
}

/// Network state of edict `index` built from its variables: the copy `AddToFullPack` of the
/// game library makes into `entity_state_t` (ReGameDLL_CS `regamedll/dlls/client.cpp`, vanilla
/// branch, read 2026-11-22), statement by statement. The engine calls it for every entity it
/// offers; `player` of the original is `index` in `1..=max_clients` (`SV_IsPlayerIndex`).
///
/// Quirks kept: the state starts all zero, so a non-player never carries `weaponmodel`,
/// `gaitsequence`, `spectator`, `friction`, `gravity`, `usehull` or `basevelocity`; `animtime` is
/// truncated to milliseconds through a `double` (`int(1000.0 * animtime) / 1000.0`); a non-player
/// that has an animation time and no velocity gets `EFLAG_SLERP`; `owner` is sent only when it is
/// a client (`1..=max_clients`); `aiment` is the edict number of the followed entity.
pub fn entity_state_from_vars(index: u16, v: &EntVars, max_clients: u8) -> GoldSrcEntityState {
    let is_client = |n: u16| (1..=u16::from(max_clients)).contains(&n);
    let player = is_client(index);
    let mut state = GoldSrcEntityState {
        number: index,
        entity_type: if v.flags & crate::world::consts::FL_CUSTOMENTITY != 0 {
            ENTITY_BEAM
        } else {
            ENTITY_NORMAL
        },
        // `int(1000.0 * animtime) / 1000.0`: the product and the quotient are `double`.
        animtime: (f64::from((1000.0 * f64::from(v.animtime)) as i32) / 1000.0) as f32,
        origin: v.origin,
        angles: v.angles,
        mins: v.mins,
        maxs: v.maxs,
        startpos: v.startpos,
        endpos: v.endpos,
        impacttime: v.impacttime,
        starttime: v.starttime,
        modelindex: v.modelindex,
        frame: v.frame,
        skin: v.skin,
        effects: v.effects,
        scale: v.scale,
        solid: v.solid,
        colormap: v.colormap,
        movetype: v.movetype,
        sequence: v.sequence,
        framerate: v.framerate,
        body: v.body,
        controller: v.controller,
        blending: v.blending,
        rendermode: v.rendermode,
        renderamt: v.renderamt,
        renderfx: v.renderfx,
        rendercolor: v.rendercolor,
        aiment: v.aiment.map_or(0, |id| id.index),
        owner: v.owner.map_or(0, |id| id.index),
        iuser4: v.iuser4,
        ..Default::default()
    };
    // Add studio interpolation if a non-player entity is moving (the original's own question
    // is "why?").
    if !player && v.animtime != 0.0 && v.velocity == Vec3::ZERO {
        state.eflags |= EFLAG_SLERP;
    }
    if !is_client(state.owner) {
        state.owner = 0;
    }
    if player {
        state.basevelocity = v.basevelocity;
        state.weaponmodel = v.weaponmodel;
        state.gaitsequence = v.gaitsequence;
        state.spectator = v.flags & crate::world::consts::FL_SPECTATOR != 0;
        state.friction = v.friction;
        state.gravity = v.gravity;
        state.usehull = u8::from(v.flags & FL_DUCKING != 0);
    }
    state
}

/// The baseline of edict `index`: `CreateBaseline` of the game library (ReGameDLL_CS
/// `regamedll/dlls/client.cpp`, read 2026-11-22) with the entity type and number that
/// `SV_CreateBaseline` (ReHLDS `rehlds/engine/sv_main.cpp`, read 2026-11-22) sets around the
/// call. Only the fields the game copies are set; the rest stay zero, as in the original (the
/// baseline is only the base the first delta of an entity is written against).
///
/// A client slot gets the fixed player baseline: hull 0 bounds, `colormap` = the slot number,
/// `modelindex` = `player_model` (`sv_playermodel`, the index of `models/player.mdl`), friction
/// and gravity 1, `MOVETYPE_WALK`, `SOLID_SLIDEBOX`, `framerate` 1.
pub fn baseline_state_from_vars(
    index: u16,
    v: &EntVars,
    max_clients: u8,
    player_model: u16,
) -> GoldSrcEntityState {
    let mut state = GoldSrcEntityState {
        number: index,
        entity_type: if v.flags & crate::world::consts::FL_CUSTOMENTITY != 0 {
            ENTITY_BEAM
        } else {
            ENTITY_NORMAL
        },
        origin: v.origin,
        angles: v.angles,
        frame: v.frame,
        skin: v.skin,
        rendermode: v.rendermode,
        renderamt: v.renderamt,
        rendercolor: v.rendercolor,
        renderfx: v.renderfx,
        scale: v.scale,
        ..Default::default()
    };
    if (1..=u16::from(max_clients)).contains(&index) {
        state.mins = crate::pm::PM_PLAYER_MINS[0];
        state.maxs = crate::pm::PM_PLAYER_MAXS[0];
        state.colormap = index;
        state.modelindex = player_model;
        state.friction = 1.0;
        state.movetype = MOVETYPE_WALK;
        state.solid = SOLID_SLIDEBOX;
        state.framerate = 1.0;
        state.gravity = 1.0;
    } else {
        state.mins = v.mins;
        state.maxs = v.maxs;
        state.colormap = 0;
        state.modelindex = v.modelindex;
        state.movetype = v.movetype;
        state.solid = v.solid;
        state.framerate = v.framerate;
        state.gravity = v.gravity;
    }
    state
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
        let s = entity_state_from_vars(21, &v, 8);
        assert_eq!(
            (s.number, s.origin, s.modelindex, s.sequence, s.effects),
            (21, v.origin, 7, 4, 8)
        );
    }

    fn rich_vars() -> EntVars {
        EntVars {
            origin: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::new(4.0, 5.0, 6.0),
            mins: Vec3::new(-16.0, -16.0, -36.0),
            maxs: Vec3::new(16.0, 16.0, 36.0),
            startpos: Vec3::new(7.0, 8.0, 9.0),
            endpos: Vec3::new(10.0, 11.0, 12.0),
            starttime: 3.5,
            impacttime: 4.5,
            basevelocity: Vec3::new(1.0, 0.0, -1.0),
            modelindex: 7,
            weaponmodel: 9,
            sequence: 4,
            gaitsequence: 3,
            frame: 0.5,
            animtime: 1.2509,
            framerate: 2.0,
            controller: [1, 2, 3, 4],
            blending: [128, 127],
            scale: 1.5,
            body: 2,
            skin: -1,
            colormap: 0x1234,
            effects: 8,
            rendermode: 5,
            renderamt: 200,
            renderfx: 3,
            rendercolor: [10, 20, 30],
            solid: 2,
            movetype: 6,
            friction: 0.5,
            gravity: 0.25,
            iuser4: 2,
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
        }
    }

    #[test]
    fn player_state_carries_every_delta_field_of_its_edict() {
        let v = rich_vars();
        // Edict 3 of 8 clients is a player.
        let s = entity_state_from_vars(3, &v, 8);
        assert_eq!(
            s,
            GoldSrcEntityState {
                number: 3,
                entity_type: ENTITY_NORMAL,
                origin: v.origin,
                angles: v.angles,
                modelindex: 7,
                sequence: 4,
                gaitsequence: 3,
                frame: 0.5,
                // `int(1000.0 * 1.2509) / 1000.0`: milliseconds, truncated.
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
                // A player never gets `EFLAG_SLERP`.
                eflags: 0,
                rendercolor: [10, 20, 30],
                controller: [1, 2, 3, 4],
                blending: [128, 127],
                friction: 0.5,
                gravity: 0.25,
                basevelocity: Vec3::new(1.0, 0.0, -1.0),
                startpos: Vec3::new(7.0, 8.0, 9.0),
                endpos: Vec3::new(10.0, 11.0, 12.0),
                impacttime: 4.5,
                starttime: 3.5,
                iuser4: 2,
            }
        );
        let standing = entity_state_from_vars(3, &EntVars::default(), 8);
        assert_eq!(
            (standing.owner, standing.aiment, standing.usehull),
            (0, 0, 0)
        );
        assert_eq!(standing.entity_type, ENTITY_NORMAL);
    }

    #[test]
    fn non_player_state_leaves_the_player_only_fields_zero() {
        let v = rich_vars();
        let s = entity_state_from_vars(21, &v, 8);
        // Only `AddToFullPack`'s `player` branch sets these.
        assert_eq!(
            (s.weaponmodel, s.gaitsequence, s.usehull, s.spectator),
            (0, 0, 0, false)
        );
        assert_eq!((s.friction, s.gravity, s.basevelocity), (0.0, 0.0, Vec3::ZERO));
        // The owner is sent only when it is a client; edict 5 is one of 8, 12 is not.
        assert_eq!(s.owner, 5);
        let far = EntVars {
            owner: Some(EdictId {
                index: 12,
                serial: 0,
            }),
            ..rich_vars()
        };
        assert_eq!(entity_state_from_vars(21, &far, 8).owner, 0);
        // A moving-animated non-player with no velocity is interpolated by the client.
        assert_eq!(s.eflags, 1);
        let moving = EntVars {
            velocity: Vec3::new(0.0, 1.0, 0.0),
            ..rich_vars()
        };
        assert_eq!(entity_state_from_vars(21, &moving, 8).eflags, 0);
        let custom = EntVars {
            flags: crate::world::consts::FL_CUSTOMENTITY,
            ..Default::default()
        };
        assert_eq!(entity_state_from_vars(21, &custom, 8).entity_type, ENTITY_BEAM);
    }

    #[test]
    fn baselines_follow_create_baseline() {
        let v = rich_vars();
        let player = baseline_state_from_vars(2, &v, 8, 36);
        assert_eq!(
            (
                player.colormap,
                player.modelindex,
                player.friction,
                player.gravity,
                player.movetype,
                player.solid,
                player.framerate
            ),
            (2, 36, 1.0, 1.0, MOVETYPE_WALK, SOLID_SLIDEBOX, 1.0)
        );
        assert_eq!(
            (player.mins, player.maxs),
            (crate::pm::PM_PLAYER_MINS[0], crate::pm::PM_PLAYER_MAXS[0])
        );
        // The copy `CreateBaseline` makes for every entity.
        assert_eq!(
            (player.origin, player.angles, player.frame, player.skin),
            (v.origin, v.angles, 0.5, -1)
        );
        assert_eq!(
            (player.rendermode, player.renderamt, player.renderfx, player.rendercolor),
            (5, 200, 3, [10, 20, 30])
        );
        // What it leaves alone stays zero.
        assert_eq!(
            (player.sequence, player.body, player.effects, player.weaponmodel),
            (0, 0, 0, 0)
        );
        let other = baseline_state_from_vars(21, &v, 8, 36);
        assert_eq!(
            (
                other.colormap,
                other.modelindex,
                other.movetype,
                other.solid,
                other.framerate,
                other.gravity,
                other.mins,
                other.maxs
            ),
            (0, 7, 6, 2, 2.0, 0.25, v.mins, v.maxs)
        );
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

    /// A game that overrides nothing it need not.
    struct Bare;

    impl GameDll for Bare {
        fn usermsgs(&self) -> UserMsgRegistry {
            UserMsgRegistry::cs16_default()
        }
        fn server_activate(&mut self, _: &mut EngineApi<'_>) {}
        fn server_deactivate(&mut self, _: &mut EngineApi<'_>) {}
        fn spawn_entity(&mut self, _: &mut EngineApi<'_>, _: EdictId, _: &str, _: &[(String, String)]) -> bool {
            true
        }
        fn client_connect(&mut self, _: &mut EngineApi<'_>, _: u8) -> Result<(), String> {
            Ok(())
        }
        fn client_put_in_server(&mut self, _: &mut EngineApi<'_>, _: u8) {}
        fn client_disconnect(&mut self, _: &mut EngineApi<'_>, _: u8) {}
        fn client_command(&mut self, _: &mut EngineApi<'_>, _: u8, _: &[String]) {}
        fn client_userinfo_changed(&mut self, _: &mut EngineApi<'_>, _: u8) {}
        fn run_usercmd(&mut self, _: &mut EngineApi<'_>, _: u8, _: &GoldSrcUserCmd, _: u32) {}
        fn start_frame(&mut self, _: &mut EngineApi<'_>, _: f32) {}
        fn update_client_data(&mut self, _: &mut EngineApi<'_>, _: u8) -> GoldSrcClientData {
            GoldSrcClientData::default()
        }
    }

    /// The default `add_to_full_pack` is the engine's own cull: what the viewer can see, and the
    /// viewer itself.
    #[test]
    fn the_default_add_to_full_pack_culls_by_pvs() {
        let mut f = Fixture::new(2);
        let viewer = f.edicts.client(0);
        let other = f.edicts.client(1);
        let leaves = LinkSnapshot { leafnums: Vec::new(), headnode: -1, area: None };
        let ctx = |ent_id: EdictId, in_pvs: bool| FullPackCtx {
            ent: ent_id.index,
            ent_id,
            viewer: 0,
            viewer_id: viewer,
            hostflags: 0,
            in_pvs,
            viewer_leaves: &leaves,
        };
        let mut eng = f.api();
        let mut state = GoldSrcEntityState::default();
        let mut game = Bare;
        assert!(game.add_to_full_pack(&mut eng, &ctx(other, true), &mut state));
        assert!(!game.add_to_full_pack(&mut eng, &ctx(other, false), &mut state));
        assert!(game.add_to_full_pack(&mut eng, &ctx(viewer, false), &mut state));
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
