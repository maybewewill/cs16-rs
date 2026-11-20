//! Test game and harness shared by the engine's integration tests.
#![allow(dead_code)]

use std::collections::BTreeSet;

use cs_cvar::CVarRegistry;
use cs_engine::{
    EdictId, EngineApi, GameDll, MsgDest, PrecacheError, Server, ServerConfig, ServerMap,
};
use cs_probe::{ProbeClient, ProbeConfig};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::messages::{svc, write_svc_print, write_svc_updateuserinfo};
use cs_proto48::netchan::MAX_MSGLEN;
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{LoopbackEndpoint, LoopbackHub, PacketTransport, PeerAddr, TransportError};
use glam::Vec3;

/// Seconds of one harness frame.
pub const FRAME: f64 = 0.01;

/// A `Server<TestGame>` on a stub map plus a loopback hub the probes connect through.
pub struct Harness {
    pub server: Server<TestGame>,
    pub hub: LoopbackHub,
    pub now: f64,
}

impl Harness {
    pub fn new(max_clients: u8) -> Self {
        Self::with_game(max_clients, TestGame::default())
    }

    pub fn with_game(max_clients: u8, game: TestGame) -> Self {
        let cfg = ServerConfig {
            max_clients,
            ..ServerConfig::default()
        };
        Self::with_config(cfg, game)
    }

    pub fn with_config(cfg: ServerConfig, game: TestGame) -> Self {
        Self::with_map(cfg, ServerMap::stub("test"), game)
    }

    /// A harness whose server starts on `map`.
    pub fn with_map(cfg: ServerConfig, map: ServerMap, game: TestGame) -> Self {
        let server = Server::new(
            cfg,
            map,
            game,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        Self {
            server,
            hub: LoopbackHub::new(),
            now: 0.0,
        }
    }

    /// A probe on a new loopback endpoint; `connect()` was already called.
    pub fn add_probe(&mut self, name: &str) -> ProbeClient<LoopbackEndpoint> {
        let endpoint = self.hub.connect();
        let cfg = ProbeConfig {
            name: name.to_string(),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(format!("probe {}", endpoint.id()).as_bytes()),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        probe.connect();
        probe
    }

    /// Advances `frames` frames of [`FRAME`] seconds: each pumps every listed probe, then
    /// runs one server frame. Probes not listed are not pumped.
    pub fn step(&mut self, probes: &mut [&mut ProbeClient<LoopbackEndpoint>], frames: usize) {
        for _ in 0..frames {
            // Counted in whole frames so the clock does not drift.
            self.now = ((self.now / FRAME).round() + 1.0) * FRAME;
            for probe in probes.iter_mut() {
                probe.pump(self.now);
            }
            self.server.frame(&mut self.hub, self.now);
        }
    }

    /// Advances one frame like [`step`](Self::step), recording what the server reads and sends.
    pub fn step_tapped(&mut self, probes: &mut [&mut ProbeClient<LoopbackEndpoint>]) -> Tap {
        self.now = ((self.now / FRAME).round() + 1.0) * FRAME;
        for probe in probes.iter_mut() {
            probe.pump(self.now);
        }
        let mut tap = TapTransport {
            hub: &mut self.hub,
            log: Tap::default(),
        };
        self.server.frame(&mut tap, self.now);
        tap.log
    }
}

/// What the server did with the transport in one tapped frame.
#[derive(Debug, Default)]
pub struct Tap {
    /// Datagrams the server read.
    pub received: usize,
    /// Address and length of every datagram the server tried to send, in order.
    pub sent: Vec<(PeerAddr, usize)>,
}

impl Tap {
    /// Datagrams the server tried to send to `addr`.
    pub fn sent_to(&self, addr: PeerAddr) -> usize {
        self.sent.iter().filter(|(to, _)| *to == addr).count()
    }
}

/// The hub, with every read and send recorded in a [`Tap`].
struct TapTransport<'a> {
    hub: &'a mut LoopbackHub,
    log: Tap,
}

impl PacketTransport for TapTransport<'_> {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        let datagram = self.hub.recv()?;
        self.log.received += 1;
        Some(datagram)
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        self.log.sent.push((to, data.len()));
        self.hub.send(to, data)
    }
}

/// Calls the game runs against the engine at the start of the next frame, queued by the test
/// with [`TestGame::act`].
#[derive(Default)]
pub struct Actions(Vec<Action>);

type Action = Box<dyn FnOnce(&mut EngineApi<'_>)>;

impl std::fmt::Debug for Actions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Actions({})", self.0.len())
    }
}

/// Sends `bytes` to `dest` in messages the engine accepts (each at most `MAX_MSGLEN` bytes).
/// Only for filler such as `svc_nop` bytes, which stay valid when cut anywhere.
pub fn send_chunked(eng: &mut EngineApi<'_>, dest: MsgDest, bytes: &[u8]) {
    for chunk in bytes.chunks(MAX_MSGLEN) {
        eng.send(dest, chunk);
    }
}

/// `svc_print` of `text` as bytes.
pub fn print_bytes(text: &str) -> Vec<u8> {
    let mut w = MsgWriter::new();
    write_svc_print(&mut w, text);
    w.into_bytes()
}

/// Minimal game: places players on a line, moves them forward by
/// `forwardmove`, and records every call.
#[derive(Debug, Default)]
pub struct TestGame {
    /// Reason `client_connect` refuses with, if any.
    pub refuse: Option<String>,
    /// Reason `client_connect` kicks the connecting slot with (and then accepts), if any.
    pub kick_on_connect: Option<String>,
    /// Bytes `client_command("flood")` writes into that client's reliable buffer.
    pub flood_bytes: usize,
    /// While non-zero, every `run_usercmd` writes this many bytes into that client's reliable
    /// buffer.
    pub flood_per_usercmd: usize,
    /// When set, `client_connect` also writes `flood_bytes` into the connecting slot's
    /// reliable buffer.
    pub flood_on_connect: bool,
    /// When set, `update_client_data` (called for every update sent to an active client)
    /// sends an `svc_nop` to `MsgDest::All` and sets a userinfo key on that client.
    pub broadcast_every_frame: bool,
    /// When set, `client_put_in_server` sends an `svc_print` of [`SHOUT`] to `MsgDest::All`.
    pub shout_on_put_in_server: bool,
    /// Entities with `modelindex = 2` that `server_activate` creates.
    pub extra_entities: usize,
    /// When set, the extra entities get `EF_NODRAW` in `effects`.
    pub nodraw_extras: bool,
    /// When set, every `update_client_data` call gives each extra entity field values that
    /// differ from its spawn baseline, from the other extras and from the previous call.
    pub churn_extras: bool,
    /// When set, `client_put_in_server` gives each extra entity field values that differ from
    /// its spawn baseline and from the other extras, once.
    pub scatter_extras: bool,
    /// Entity numbers `add_to_full_pack` was called for.
    pub add_to_full_pack_ents: BTreeSet<u16>,
    /// `(viewer slot, entity number)` of every `add_to_full_pack` call.
    pub add_to_full_pack_pairs: BTreeSet<(u8, u16)>,
    /// When set, `server_deactivate` writes an `svc_print` of [`DEACTIVATE_PRINT`] into the
    /// reliable buffer of every slot.
    pub print_on_deactivate: bool,
    /// `skin` the extra entities get in `server_activate`.
    pub extra_skin: i16,
    /// Slots `EngineApi::is_client_active` reported active during the last `server_activate`.
    pub active_at_activate: Vec<u8>,
    /// Result of `precache_model("models/late.mdl")` in `client_put_in_server`.
    pub late_precache: Option<Result<u16, PrecacheError>>,
    /// Every `argv` received by `client_command`.
    pub commands: Vec<Vec<String>>,
    /// Every `random_seed` received by `run_usercmd`, in call order.
    pub random_seeds: Vec<u32>,
    /// The `msec` of every command `run_usercmd` received, in call order.
    pub usercmd_msecs: Vec<u8>,

    /// `(classname, keys)` of every `spawn_entity` call, in call order.
    pub spawned: Vec<(String, Vec<(String, String)>)>,
    /// `"spawn:<classname>"` for every `spawn_entity` call and `"activate"` for every
    /// `server_activate`, in call order.
    pub call_order: Vec<String>,
    /// Result of the `precache_model` that `spawn_entity` made for each accepted entity with a
    /// `model` key.
    pub spawn_precache: Vec<Result<u16, PrecacheError>>,
    /// `eng.time()` of every `spawn_entity` call.
    pub spawn_times: Vec<f64>,

    /// `nextthink` the extra entities get in `server_activate`; 0 schedules nothing.
    pub extra_nextthink: f64,
    /// When set, every `think` sets the entity's `nextthink` to the current time.
    pub think_reschedules: bool,
    /// When set, every `think` removes the entity itself.
    pub think_removes_self: bool,
    /// When set, every `think` removes the live entity with this index.
    pub think_removes_index: Option<u16>,
    /// `nextthink` of the n-th extra entity (0-based) where it overrides `extra_nextthink`.
    pub extra_nextthink_overrides: Vec<f64>,
    /// When set, a think of a `test_extra` entity creates a `test_child` that is due at once;
    /// it does so after `think_removes_index` took effect, so the child may take that slot.
    pub think_creates_child: bool,
    /// Index of every `test_child` created by a think, in creation order.
    pub children: Vec<u16>,
    /// `(entity index, eng.time())` of every `think` call, in call order.
    pub think_log: Vec<(u16, f64)>,

    pub server_activate_calls: usize,
    pub server_deactivate_calls: usize,
    pub client_connect_calls: usize,
    pub client_put_in_server_calls: usize,
    pub client_disconnect_calls: usize,
    pub client_command_calls: usize,
    pub client_userinfo_changed_calls: usize,
    pub run_usercmd_calls: usize,
    pub start_frame_calls: usize,
    pub update_client_data_calls: usize,
    pub add_to_full_pack_calls: usize,

    /// Sounds `server_activate` precaches.
    pub precache_sounds: Vec<String>,
    /// `(destination, bytes)` sent by `server_activate`, in order.
    pub activate_sends: Vec<(MsgDest, Vec<u8>)>,
    /// `(destination, bytes)` sent by `spawn_entity` for every entity of class
    /// `test_initmsg` (which it keeps).
    pub spawn_sends: Vec<(MsgDest, Vec<u8>)>,
    /// Where `client_put_in_server` puts each slot (index = slot), with a view offset of 17
    /// above it; slots without an entry stay on the default line.
    pub spawn_origins: Vec<Vec3>,
    /// When set, every `start_frame` sends 2 KB of `svc_nop` to `Broadcast`, 2 KB to `Pvs` at
    /// the world origin and 1 KB to `Init`, each in messages of 1000 bytes.
    pub message_flood: bool,
    /// Calls queued by the test, run at the start of the next frame.
    pub actions: Actions,
}

impl TestGame {
    /// Runs `f` against the engine at the start of the next frame.
    pub fn act(&mut self, f: impl FnOnce(&mut EngineApi<'_>) + 'static) {
        self.actions.0.push(Box::new(f));
    }
}

impl GameDll for TestGame {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }

    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.server_activate_calls += 1;
        self.call_order.push("activate".to_string());
        self.active_at_activate = (0..32).filter(|&s| eng.is_client_active(s)).collect();
        // Lands at model index 2 on a stub map.
        eng.precache_model("models/test.mdl")
            .expect("precache before lock");
        for sound in &self.precache_sounds {
            eng.precache_sound(sound).expect("precache before lock");
        }
        for (dest, bytes) in &self.activate_sends {
            eng.send(*dest, bytes);
        }
        for n in 1..=self.extra_entities {
            let id = eng.create_entity("test_extra").expect("free edict");
            let vars = eng.ent_mut(id).expect("just allocated");
            vars.modelindex = 2;
            vars.origin = Vec3::new(0.0, 0.0, 16.0 * n as f32);
            vars.skin = self.extra_skin;
            vars.nextthink = self
                .extra_nextthink_overrides
                .get(n - 1)
                .copied()
                .unwrap_or(self.extra_nextthink);
            if self.nodraw_extras {
                vars.effects |= EF_NODRAW;
            }
        }
    }

    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.spawned.push((classname.to_string(), keys.to_vec()));
        self.call_order.push(format!("spawn:{classname}"));
        self.spawn_times.push(eng.time());
        if classname == "test_selfremove" {
            // Removes the entity itself and still refuses it.
            eng.remove_entity(id);
            return false;
        }
        if classname == "test_initmsg" {
            for (dest, bytes) in &self.spawn_sends {
                eng.send(*dest, bytes);
            }
            return true;
        }
        if !classname.starts_with("test_keep") {
            return false;
        }
        let model = keys.iter().find(|(k, _)| k == "model").map(|(_, v)| v);
        if let Some(model) = model
            && eng.ent(id).is_some_and(|v| v.model.is_empty())
        {
            let result = eng.precache_model(model);
            self.spawn_precache.push(result);
            eng.set_model(id, model);
        }
        true
    }

    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.server_deactivate_calls += 1;
        if self.print_on_deactivate {
            for slot in 0..32 {
                eng.send(MsgDest::One(slot), &print_bytes(DEACTIVATE_PRINT));
            }
        }
    }

    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.client_connect_calls += 1;
        if let Some(reason) = &self.kick_on_connect {
            eng.kick(slot, reason);
        }
        if self.flood_on_connect {
            send_chunked(eng, MsgDest::One(slot), &vec![1; self.flood_bytes]);
        }
        match &self.refuse {
            Some(text) => Err(text.clone()),
            None => Ok(()),
        }
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.client_put_in_server_calls += 1;
        let id = eng.client_edict(slot);
        if let Some(vars) = eng.ent_mut(id) {
            vars.origin = Vec3::new(64.0 * f32::from(slot), 0.0, 0.0);
            if let Some(origin) = self.spawn_origins.get(usize::from(slot)) {
                vars.origin = *origin;
                vars.view_ofs = Vec3::new(0.0, 0.0, 17.0);
            }
            vars.modelindex = 2;
            vars.health = 100.0;
        }
        self.late_precache = Some(eng.precache_model("models/late.mdl"));
        if self.shout_on_put_in_server {
            eng.send(MsgDest::All, &print_bytes(SHOUT));
        }
        if self.scatter_extras {
            churn_extras(eng, 0);
        }
    }

    fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {
        self.client_disconnect_calls += 1;
    }

    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.client_command_calls += 1;
        if argv.first().is_some_and(|c| c == "flood") {
            // `svc_nop` bytes: harmless if they ever reached the client.
            send_chunked(eng, MsgDest::One(slot), &vec![1; self.flood_bytes]);
        }
        if argv.first().is_some_and(|c| c == "kickme") {
            eng.kick(slot, GAME_KICK_REASON);
        }
        if argv.first().is_some_and(|c| c == "rekey") {
            // A new value on every call, so every call is a real change.
            let value = (self.client_command_calls).to_string();
            eng.set_client_key(slot, "rekey", &value);
        }
        if let [cmd, slot, key, value] = argv
            && cmd == "setkey"
            && let Ok(slot) = slot.parse::<u8>()
        {
            // `setkey <slot> <key> <value>`: sets a userinfo key of any slot.
            eng.set_client_key(slot, key, value);
        }
        if argv.first().is_some_and(|c| c == "bigprint") {
            // Enough `svc_print` messages of `BIG_PRINT` to be sent as a fragmented stream.
            let text = big_print();
            for _ in 0..BIG_PRINT_COUNT {
                eng.send(MsgDest::One(slot), &print_bytes(&text));
            }
        }
        if argv.first().is_some_and(|c| c == "biginfo") {
            // A userinfo update of slot 15 whose text is `BIG_INFO`.
            let mut w = MsgWriter::new();
            write_svc_updateuserinfo(&mut w, 15, 99, &big_info(), &[0; 16]);
            eng.send(MsgDest::One(slot), w.as_slice());
        }
        self.commands.push(argv.to_vec());
    }

    fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {
        self.client_userinfo_changed_calls += 1;
    }

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        self.run_usercmd_calls += 1;
        self.random_seeds.push(random_seed);
        self.usercmd_msecs.push(cmd.msec);
        if self.flood_per_usercmd > 0 {
            // `svc_nop` bytes.
            send_chunked(
                eng,
                MsgDest::One(slot),
                &vec![svc::NOP; self.flood_per_usercmd],
            );
        }
        let id = eng.client_edict(slot);
        if let Some(vars) = eng.ent_mut(id) {
            vars.origin.x += cmd.forwardmove * f32::from(cmd.msec) / 1000.0;
        }
    }

    fn start_frame(&mut self, eng: &mut EngineApi<'_>, _frametime: f32) {
        self.start_frame_calls += 1;
        for action in std::mem::take(&mut self.actions.0) {
            action(eng);
        }
        if self.message_flood {
            for _ in 0..2 {
                eng.send(MsgDest::Broadcast, &[svc::NOP; 1000]);
                eng.send(MsgDest::Pvs(Vec3::ZERO), &[svc::NOP; 1000]);
            }
            eng.send(MsgDest::Init, &[svc::NOP; 1000]);
        }
    }

    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let now = eng.time();
        self.think_log.push((id.index, now));
        if self.think_reschedules
            && let Some(vars) = eng.ent_mut(id)
        {
            vars.nextthink = now;
        }
        if let Some(index) = self.think_removes_index
            && let Some(victim) = eng.entity_ids().into_iter().find(|v| v.index == index)
        {
            eng.remove_entity(victim);
        }
        if self.think_creates_child
            && eng.ent(id).is_some_and(|v| v.classname == "test_extra")
            && let Some(child) = eng.create_entity("test_child")
        {
            if let Some(vars) = eng.ent_mut(child) {
                vars.nextthink = now;
            }
            self.children.push(child.index);
        }
        if self.think_removes_self {
            eng.remove_entity(id);
        }
    }

    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.update_client_data_calls += 1;
        if self.broadcast_every_frame {
            eng.send(MsgDest::All, &[svc::NOP]);
            let frame = self.update_client_data_calls.to_string();
            eng.set_client_key(slot, "frame", &frame);
        }
        if self.churn_extras {
            churn_extras(eng, self.update_client_data_calls);
        }
        let id = eng.client_edict(slot);
        let vars = eng.ent(id).cloned().unwrap_or_default();
        GoldSrcClientData {
            origin: vars.origin,
            health: vars.health,
            maxspeed: 250.0,
            ..Default::default()
        }
    }

    fn add_to_full_pack(
        &mut self,
        _eng: &mut EngineApi<'_>,
        viewer: u8,
        ent: u16,
        _state: &mut GoldSrcEntityState,
    ) -> bool {
        self.add_to_full_pack_calls += 1;
        self.add_to_full_pack_ents.insert(ent);
        self.add_to_full_pack_pairs.insert((viewer, ent));
        true
    }
}

/// Reason of the kick `client_command("kickme")` issues.
pub const GAME_KICK_REASON: &str = "Kicked by the game";

/// Text of the `svc_print` that `print_on_deactivate` writes.
pub const DEACTIVATE_PRINT: &str = "old map\n";

/// `svc_print` messages `client_command("bigprint")` writes, each of [`big_print`].
pub const BIG_PRINT_COUNT: usize = 40;

/// Text of the `svc_print` messages `client_command("bigprint")` writes: 400 bytes.
pub fn big_print() -> String {
    format!("{}\n", "b".repeat(399))
}

/// Text of the `svc_print` that `shout_on_put_in_server` broadcasts.
pub const SHOUT: &str = "shout\n";

/// Text of the userinfo update `client_command("biginfo")` sends: about 1900 bytes.
pub fn big_info() -> String {
    (0..95).map(|i| format!("\\key{i:03}\\{i:012}")).collect()
}

/// `EF_NODRAW` in `EntVars::effects`.
pub const EF_NODRAW: u32 = 128;

/// Gives every `test_extra` entity values derived from its index and `call`.
fn churn_extras(eng: &mut EngineApi<'_>, call: usize) {
    let mut ids = Vec::new();
    while let Some(id) = eng.find_by_classname(ids.last().copied(), "test_extra") {
        ids.push(id);
    }
    let k = call as f32;
    for id in ids {
        let n = f32::from(id.index);
        let vars = eng.ent_mut(id).expect("live");
        vars.origin = Vec3::new(8.0 * n + k, -4.0 * n - k, 16.0 * n + 0.5 * k);
        vars.angles = Vec3::new((n + k) % 90.0, (3.0 * n + k) % 360.0, (n * 7.0) % 180.0);
        vars.frame = (n + k) % 200.0;
        vars.framerate = 1.0 + (n % 7.0) * 0.25;
        vars.sequence = id.index % 50;
        vars.skin = (id.index % 13) as i16;
        vars.body = (id.index % 11) as u8;
        vars.renderamt = (id.index % 251) as u8;
    }
}
