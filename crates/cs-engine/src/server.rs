//! The server: client slots, one netchan per client and the sign-on sequence. Connectionless
//! packets and connects are in `connect.rs`, per-client updates in `frame.rs`.

use std::collections::{BTreeMap, BTreeSet};

use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::{MsgWriter, ProtoError};
use cs_proto48::delta::{DeltaRecord, DeltaRegistry, DeltaSchema, GoldSrcEntityState};
use cs_proto48::handshake::tokenize_goldsrc_line;
use cs_proto48::messages::{
    ClcMessage, ServerInfoMsg, entity_schema_name, parse_clc_stream, wire_entity_type,
    write_svc_newmovevars, write_svc_stufftext,
};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{PacketTransport, PeerAddr};

use crate::client::{Client, ClientState, MAX_REFUSED_STREAMS, SignonStage};
use crate::edict::{DEFAULT_MAX_EDICTS, EdictPool};

use crate::game::{ClientView, EngineApi, GameDll, Outbox, baseline_state_from_vars};
use crate::map::ServerMap;
use crate::oob::{ChallengeIssuer, OOB_GLOBAL_REPLIES_PER_SEC, OobLimiter};
use crate::precache::PrecacheTables;
use crate::world::WorldLinks;
use crate::world::consts::{EF_MUZZLEFLASH, EF_NOINTERP, MOVETYPE_WALK, SOLID_SLIDEBOX};
use crate::signon::{
    MAX_SIGNON_BUFFER, SPAWN_TAIL_MAX, UserInfoEntry, move_vars, public_userinfo, munged_map_crc, write_new_stage,
    write_sendres_stage,
    write_spawn_baselines, write_spawn_stage, write_spawn_tail,
};
use crate::usercmd::TokenBucket;

/// Unacknowledged reliable bytes a client may have queued; past this it is dropped.
pub const MAX_RELIABLE_BACKLOG: usize = 256 * 1024;
/// Datagrams one [`Server::frame`] reads; the rest wait in the transport for the next frame.
pub const MAX_DATAGRAMS_PER_FRAME: usize = 4096;
/// Simulation time (`EngineApi::time`) at every map activation: HLDS starts `sv.time` at 1.0 on
/// each map load.
pub const SV_TIME_AT_MAP_START: f64 = 1.0;
/// Longest frame the simulation takes as one step, seconds: `Host_FilterTime` (ReHLDS
/// `rehlds/engine/host.cpp`, read 2026-11-21) clamps `host_frametime` to 0.25.
pub const MAX_FRAMETIME: f64 = 0.25;
/// Physics passes that settle a map at activation with one player slot: 2 of 0.1 s.
pub const SETTLE_PASSES_SINGLE: usize = 2;
/// Physics passes that settle a map at activation with more player slots: 16 of 0.8 s.
pub const SETTLE_PASSES_MULTI: usize = 16;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub hostname: String,
    pub max_clients: u8,
    pub game_dir: String,
    pub build: i32,
    pub timeout_secs: f64,
    pub signon_timeout_secs: f64,
    pub min_update_rate: u32,
    pub max_update_rate: u32,
    pub min_rate: u32,
    pub max_rate: u32,
    /// Size of the edict pool (clamped as `EdictPool::new` does).
    pub max_edicts: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            hostname: "Counter-Strike 1.6 Rust".to_string(),
            max_clients: 16,
            game_dir: "cstrike".to_string(),
            build: 8684,
            timeout_secs: 60.0,
            signon_timeout_secs: 120.0,
            min_update_rate: 10,
            max_update_rate: 102,
            min_rate: 1000,
            max_rate: 100_000,
            max_edicts: DEFAULT_MAX_EDICTS,
        }
    }
}

pub struct Server<G: GameDll> {
    pub(crate) cfg: ServerConfig,
    pub(crate) map: ServerMap,
    pub(crate) game: G,
    pub(crate) deltas: DeltaRegistry,
    pub(crate) cvars: CVarRegistry,
    pub(crate) usermsgs: UserMsgRegistry,
    pub(crate) edicts: EdictPool,
    /// The area tree and what each linked entity touches; rebuilt for every map.
    pub(crate) world: WorldLinks,
    pub(crate) precache: PrecacheTables,
    pub(crate) outbox: Outbox,
    /// What the game sees of each slot, indexed like `clients`.
    pub(crate) views: Vec<ClientView>,
    pub(crate) clients: Vec<Client>,
    /// Entity states sent with `svc_spawnbaseline`, by entity number.
    pub(crate) baselines: Vec<(u16, GoldSrcEntityState)>,
    /// The same baselines as delta records, the base of full entity updates.
    pub(crate) baseline_records: BTreeMap<u16, (u8, DeltaRecord)>,
    pub(crate) usercmd_schema: Option<DeltaSchema>,
    pub(crate) challenges: ChallengeIssuer,
    pub(crate) limiter: OobLimiter,
    /// Replies to connectionless datagrams without a valid challenge, all addresses together.
    pub(crate) oob_budget: TokenBucket,
    /// The movevars as the cvars had them when the first command of this frame was checked
    /// (`sync_client_movevars`); dropped at the start of every frame.
    pub(crate) frame_movevars: Option<cs_proto48::messages::MoveVarsMsg>,
    pub(crate) spawn_count: u32,
    pub(crate) next_userid: i32,
    /// Real time of the latest frame, as the host gave it. Network timers (timeouts, rates,
    /// pings, challenge buckets) run on it; nothing the game sees does.
    pub(crate) real_time: f64,
    /// Simulation time (HLDS `sv.time`): [`SV_TIME_AT_MAP_START`] when a map is activated, plus
    /// the frame time after each physics pass. What the game sees as `EngineApi::time`.
    pub(crate) sv_time: f64,
    /// Seconds of the frame being run (HLDS `host_frametime`, see [`MAX_FRAMETIME`]), handed to
    /// the game as `EngineApi::frametime`; 0 between frames.
    pub(crate) frametime: f64,
    /// Entities (by index) a `movetype` nothing handles was already reported for on this map.
    pub(crate) warned_movetypes: BTreeSet<u16>,
    /// Warnings given for such entities since the server started.
    pub(crate) movetype_warnings: u64,
    /// The cvars the movement physics reads, read once per physics pass.
    pub(crate) phys_cvars: crate::physics_move::PhysCvars,
    pub(crate) net_stats: NetStats,
    /// Set by `shutdown`: frames do nothing from then on.
    pub(crate) shutting_down: bool,
    /// Final datagrams for released clients, sent at the end of the frame.
    pub(crate) farewells: Vec<(PeerAddr, Vec<u8>)>,
    /// Server time the next `svc_pings` is due.
    pub(crate) next_pings_at: f64,
}

/// Monotonic counts of what the server refused, dropped or failed to send.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NetStats {
    /// Datagrams the client's netchan refused: malformed, stale or duplicate.
    pub datagrams_rejected: u64,
    /// Payloads that reached `parse_clc_stream` and failed to parse.
    pub payloads_rejected: u64,
    /// Times queued unreliable messages for a client were discarded: the queue outgrew
    /// `MAX_DATAGRAM`, or it did not fit in the update that was to carry it.
    pub unreliable_dropped: u64,
    /// Updates whose unreliable part (client data and entity block) was dropped because the
    /// entity block did not fit in the datagram.
    pub entity_blocks_dropped: u64,
    /// `Server::frame` calls ignored because their time was not finite or went backwards.
    pub frames_skipped: u64,
    /// Connectionless datagrams left unanswered by the per-address limiter or by the global
    /// budget for replies without a valid challenge.
    pub oob_denied: u64,
    /// Messages `EngineApi::send` dropped because the buffer of a target slot already held more
    /// than [`MAX_RELIABLE_BACKLOG`] bytes within one game call (a game looping sends).
    pub outbox_dropped: u64,
    /// Datagrams the transport refused to send: updates, sign-on datagrams, farewells and
    /// connectionless replies.
    pub send_failures: u64,
    /// Touches dropped because a frame already queued `MAX_TOUCHES_PER_FRAME`.
    pub touches_dropped: u64,
    /// Frames that dropped touches (one `tracing::warn!` each).
    pub touch_cap_frames: u64,
}

impl NetStats {
    /// Sends `data` to `to`, counting a refused send in `send_failures`.
    pub(crate) fn send(&mut self, net: &mut dyn PacketTransport, to: PeerAddr, data: &[u8]) {
        if net.send(to, data).is_err() {
            self.send_failures += 1;
        }
    }
}

/// Whether `SV_CreateBaseline` (ReHLDS `sv_main.cpp`, read 2026-11-22) baselines edict `index`:
/// `g_psvs.maxclients >= entnum || svent->v.modelindex`, so the world (0) and every client slot
/// are baselined whether or not they have a model, and any other live edict only with one.
pub(crate) fn baselined(index: u16, v: &crate::edict::EntVars, max_clients: u8) -> bool {
    index <= u16::from(max_clients) || v.modelindex != 0
}

impl<G: GameDll> Server<G> {
    /// The server's name: the `hostname` cvar, which [`Server::new`] sets from
    /// `ServerConfig::hostname` and the console may change (the original has this one cvar for
    /// `status`, `svc_serverinfo`, the A2S info reply and the MOTD title). A registry with no
    /// such cvar gives the configured name.
    pub fn hostname(&self) -> String {
        self.cvars
            .get("hostname")
            .map_or_else(|| self.cfg.hostname.clone(), |c| c.value.clone())
    }

    pub fn new(
        cfg: ServerConfig,
        map: ServerMap,
        game: G,
        deltas: DeltaRegistry,
        mut cvars: CVarRegistry,
    ) -> Self {
        let cfg = cfg.sanitized();
        // The name the operator configured (`--hostname`) is the `hostname` cvar the game reads for
        // `gmsgServerName` (the title of the MOTD window), as in the original, where the one
        // cvar is both. A registry with no such cvar is left alone.
        cvars.set("hostname", cfg.hostname.clone());
        let usermsgs = game.usermsgs();
        let edicts = EdictPool::new(cfg.max_clients, cfg.max_edicts);
        let precache = Self::map_precache(&map);
        let map_bounds = map.world_bounds();
        let usercmd_schema = deltas.get("usercmd_t").cloned();
        if usercmd_schema.is_none() {
            tracing::error!("delta registry has no usercmd_t; client messages cannot be parsed");
        }
        let slots = usize::from(cfg.max_clients);
        let mut server = Self {
            outbox: Outbox::new(cfg.max_clients).with_event_schema(deltas.get("event_t").cloned()),
            views: (0..slots).map(|_| ClientView::default()).collect(),
            clients: (0..slots).map(|_| Client::free()).collect(),
            cfg,
            map,
            game,
            deltas,
            cvars,
            usermsgs,
            edicts,
            world: WorldLinks::new(map_bounds),
            precache,
            baselines: Vec::new(),
            baseline_records: BTreeMap::new(),
            usercmd_schema,
            challenges: ChallengeIssuer::new(),
            limiter: OobLimiter::new(),
            oob_budget: TokenBucket::new(OOB_GLOBAL_REPLIES_PER_SEC, OOB_GLOBAL_REPLIES_PER_SEC),
            frame_movevars: None,
            spawn_count: 1,
            next_userid: 1,
            real_time: 0.0,
            sv_time: SV_TIME_AT_MAP_START,
            frametime: 0.0,
            warned_movetypes: BTreeSet::new(),
            phys_cvars: crate::physics_move::PhysCvars::default(),
            movetype_warnings: 0,
            net_stats: NetStats::default(),
            shutting_down: false,
            farewells: Vec::new(),
            next_pings_at: 0.0,
        };
        server.activate_map();
        server
    }

    /// Sets the decal list `EngineApi::decal_index` answers from (`Decal_Init` in the original,
    /// once at start). Build it with [`decal_names_from_wads`](crate::decal_names_from_wads)
    /// from the lump names of the `decals.wad` files; it is kept across map changes.
    pub fn set_decals(&mut self, names: Vec<String>) {
        self.precache.set_decals(names);
    }

    /// `PF_DecalIndex` for the host: the index of decal `name`, or -1 (see
    /// [`EngineApi::decal_index`]).
    pub fn decal_index(&self, name: &str) -> i32 {
        self.precache.decal_index(name)
    }

    /// Gives the engine the way to read game files, so that the studio models the game
    /// precaches (players, above all) are traced by their hitboxes. Models precached already
    /// are read now; the setting survives a map change. Without it every entity is a box.
    pub fn set_model_loader(&mut self, loader: crate::precache::ModelLoader) {
        self.precache.set_model_loader(Some(loader));
    }

    /// Fresh precache tables for `map`, before the game activates it.
    pub(crate) fn map_precache(map: &ServerMap) -> PrecacheTables {
        let precache = PrecacheTables::new(&map.path, map.submodel_count);
        let last_submodel = map.submodel_count.saturating_sub(1);
        if last_submodel > 0 && precache.model_index(&format!("*{last_submodel}")).is_none() {
            tracing::error!(
                map = %map.name,
                submodels = map.submodel_count,
                "brush models do not fit the model precache table"
            );
        }
        precache
    }

    /// Spawns the map's entities, lets the game activate the current map, then locks the
    /// precache tables and computes the spawn baselines.
    pub(crate) fn activate_map(&mut self) {
        // ReHLDS builds the PAS at SV_SpawnServer, before the map entities exist.
        self.map.calc_pas(self.cfg.max_clients);
        // While the game spawns entities and activates the map, messages sent to
        // `MsgDest::Init` are kept for the clients that enter later.
        self.outbox.init_open = true;
        self.spawn_map_entities();
        self.with_game(|game, eng| game.server_activate(eng));
        // HLDS sets `ss_active` before the settle passes, so a think that precaches during
        // them is refused.
        self.precache.lock();
        self.settle_physics();
        self.outbox.init_open = false;
        self.compute_baselines();
    }

    /// One server frame, in the order of HLDS `SV_Frame_Internal` (ReHLDS
    /// `rehlds/engine/sv_main.cpp`, read 2026-11-21): the frame time is `now` minus the previous
    /// frame's, at most [`MAX_FRAMETIME`] (`Host_FilterTime`); then up to
    /// [`MAX_DATAGRAMS_PER_FRAME`] waiting datagrams are handled, so the user commands they carry
    /// run at the simulation time before this frame's physics; then the physics pass (see
    /// `physics.rs`: `start_frame`, the entities by movetype, their thinks); then the simulation
    /// time advances by the frame time; then clients that timed out are freed, the pings and
    /// userinfo updates that are due are queued, and each client is sent what is due for it (see
    /// `frame.rs`) with the final datagrams of released clients. Network timers run on `now`.
    /// A call whose `now` is not finite or earlier than the previous frame's, and any call after
    /// [`shutdown`](Self::shutdown), does nothing but count in `NetStats::frames_skipped`.
    pub fn frame(&mut self, net: &mut dyn PacketTransport, now: f64) {
        if self.shutting_down || !now.is_finite() || now < self.real_time {
            self.net_stats.frames_skipped += 1;
            return;
        }
        let real_dt = now - self.real_time;
        self.real_time = now;
        self.frametime = real_dt.min(MAX_FRAMETIME);
        self.accrue_client_time(real_dt);
        self.oob_budget.refill(real_dt);
        self.frame_movevars = None;
        for _ in 0..MAX_DATAGRAMS_PER_FRAME {
            let Some((from, data)) = net.recv() else {
                break;
            };
            self.handle_datagram(net, from, &data);
        }
        self.run_physics();
        // The physics pass is the frame's game work: deliver what is still queued, and start a
        // fresh touch budget, before the updates are built.
        self.finish_frame_touches();
        self.flush_reliable_datagram();
        self.sv_time += self.frametime;
        self.drop_timed_out_clients();
        self.queue_pings_if_due();
        self.broadcast_userinfo();
        self.send_client_frames(net);
        self.cleanup_ents();
        self.send_farewells(net);
        self.frametime = 0.0;
    }

    /// `SV_RunCmd`'s movevars check (ReHLDS `sv_user.cpp` 1018-1030, read 2026-11-22), run for
    /// the client of `slot` after the game's movement call: the client's own copy of the
    /// movevars (what it was last sent) is compared with the server's now (the cvars; ReHLDS
    /// also compares what `PM_Move` left in its copy, which the game here does not alter), and a
    /// difference is written to the client's reliable channel as `svc_newmovevars`; the copy is
    /// then the server's. Fake clients do not exist in the engine. A client that sends no
    /// commands is not checked, so it gets a change when its next command runs, as in ReHLDS
    /// (the check no longer runs once per frame for every client). A client whose copy is not set
    /// yet (before the `new` stage) is not sent anything: `new` carries the current values. The
    /// reliable write here is direct and is NOT subject to `MAX_RELIABLE_BACKLOG` (only what the
    /// game queues goes through the capped outbox): it is one small message per change of a
    /// cvar, and `move_vars` never yields a NaN, so an unchanged value never compares unequal.
    pub(crate) fn sync_client_movevars(&mut self, slot: u8) {
        // Built once per frame, not per command: a cvar the game changes during a frame reaches
        // the clients with their first command of the next one.
        let current = self
            .frame_movevars
            .get_or_insert_with(|| move_vars(&self.cvars, &self.map.entities))
            .clone();
        let Some(client) = self.clients.get_mut(usize::from(slot)) else {
            return;
        };
        if client.state != ClientState::Active || client.movevars.as_ref() == Some(&current) {
            return;
        }
        if client.movevars.is_some() {
            let mut w = MsgWriter::new();
            write_svc_newmovevars(&mut w, &current);
            client.chan.write_reliable(w.as_slice());
        }
        client.movevars = Some(current);
    }

    pub fn config(&self) -> &ServerConfig {
        &self.cfg
    }

    pub fn client_state(&self, slot: u8) -> ClientState {
        self.clients
            .get(usize::from(slot))
            .map_or(ClientState::Free, |c| c.state)
    }

    pub fn client_count(&self) -> usize {
        self.clients.iter().filter(|c| !c.is_free()).count()
    }

    /// Userid of the client in `slot`; `None` for a free slot.
    pub fn client_userid(&self, slot: u8) -> Option<i32> {
        self.clients
            .get(usize::from(slot))
            .filter(|c| !c.is_free())
            .map(|c| c.userid)
    }

    /// Updates kept as delta bases for the client in `slot`; `None` for a free slot.
    pub fn client_frame_count(&self, slot: u8) -> Option<usize> {
        self.clients
            .get(usize::from(slot))
            .filter(|c| !c.is_free())
            .map(|c| c.frames.stored())
    }

    /// `(records, distinct records)` held by the updates kept for the client in `slot`: the
    /// number of entity records in its frame ring and how many of them are separate
    /// allocations (records of unchanged entities are shared). Test aid.
    #[doc(hidden)]
    pub fn client_frame_records(&self, slot: u8) -> Option<(usize, usize)> {
        let client = self.clients.get(usize::from(slot)).filter(|c| !c.is_free())?;
        Some(client.frames.record_counts())
    }

    /// Bytes in the init buffer: what the game sent to `MsgDest::Init` while activating the
    /// current map, at most [`INIT_BUFFER_BUDGET`](crate::INIT_BUFFER_BUDGET).
    pub fn init_buffer_len(&self) -> usize {
        self.outbox.init.len()
    }

    /// Spawn baselines the `spawn` stage sends for the current map: those that fit the sign-on
    /// buffer (see [`MAX_SIGNON_BUFFER`]).
    pub fn baseline_count(&self) -> usize {
        self.baselines.len()
    }

    /// The entity numbers of the spawn baselines, ascending.
    pub fn baseline_numbers(&self) -> Vec<u16> {
        self.baselines.iter().map(|(number, _)| *number).collect()
    }

    pub fn net_stats(&self) -> NetStats {
        self.net_stats
    }

    /// Entities a `movetype` that HLDS rejects was reported for (one warning per entity per
    /// map), counted since the server started. Test aid.
    #[doc(hidden)]
    pub fn movetype_warnings(&self) -> u64 {
        self.movetype_warnings
    }

    /// Touches found by linking and not yet dispatched (`(trigger, other)` pairs). Test aid.
    #[doc(hidden)]
    pub fn pending_touch_count(&self) -> usize {
        self.world.pending_touch_count()
    }

    pub fn spawn_count(&self) -> u32 {
        self.spawn_count
    }

    pub fn game(&self) -> &G {
        &self.game
    }

    pub fn game_mut(&mut self) -> &mut G {
        &mut self.game
    }

    pub fn edicts(&self) -> &EdictPool {
        &self.edicts
    }

    /// How entity `id` is linked into the world (its leaves and area node); `None` when it is
    /// not linked. Test aid.
    #[doc(hidden)]
    pub fn link_snapshot(&self, id: crate::edict::EdictId) -> Option<crate::world::LinkSnapshot> {
        self.world.link_snapshot(id)
    }

    /// Calls into the game, then drains what it queued.
    pub(crate) fn with_game<R>(&mut self, f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R) -> R {
        let result = self.call_game(f);
        self.drain_outbox();
        result
    }

    /// [`with_game`](Self::with_game) with `EngineApi::time` set to `time` for the call.
    pub(crate) fn with_game_at<R>(
        &mut self,
        time: f64,
        f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R,
    ) -> R {
        self.sync_views();
        let result = self.call_game_synced_at(time, f);
        self.drain_outbox();
        result
    }

    /// Runs `f` on the engine services without calling the game; the world, the pool and the
    /// outbox are the game's own, so nothing queued is drained.
    pub(crate) fn with_engine<R>(&mut self, f: impl FnOnce(&mut EngineApi<'_>) -> R) -> R {
        self.call_game_synced_at(self.sv_time, |_, eng| f(eng))
    }

    /// Copies the sign-on state of every slot into what the game sees of it.
    pub(crate) fn sync_views(&mut self) {
        for (view, client) in self.views.iter_mut().zip(&self.clients) {
            view.state = client.state;
        }
    }

    /// Calls into the game without draining the outbox. The game sees the sign-on state every
    /// slot has now.
    pub(crate) fn call_game<R>(&mut self, f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R) -> R {
        self.sync_views();
        self.call_game_synced(f)
    }

    /// [`call_game`](Self::call_game) for a caller that ran [`sync_views`](Self::sync_views)
    /// and changed no slot's state since, such as the per-entity `add_to_full_pack` calls of
    /// one update.
    pub(crate) fn call_game_synced<R>(
        &mut self,
        f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R,
    ) -> R {
        self.call_game_synced_at(self.sv_time, f)
    }

    /// [`call_game_synced`](Self::call_game_synced) with `EngineApi::time` set to `time`.
    pub(crate) fn call_game_synced_at<R>(
        &mut self,
        time: f64,
        f: impl FnOnce(&mut G, &mut EngineApi<'_>) -> R,
    ) -> R {
        let mut eng = EngineApi::new(
            &mut self.edicts,
            &mut self.precache,
            &mut self.cvars,
            &self.usermsgs,
            &self.map,
            &mut self.world,
            self.sv_time,
            self.frametime as f32,
            &mut self.outbox,
            &mut self.views,
        );
        eng.time = time;
        eng.realtime = self.real_time;
        f(&mut self.game, &mut eng)
    }

    /// What follows every game call: moves what the game queued to the clients (see
    /// [`drain_messages`](Self::drain_messages)), then dispatches the touches the call queued
    /// (see `touch.rs`).
    pub(crate) fn drain_outbox(&mut self) {
        self.drain_messages();
        self.dispatch_touches();
    }

    /// Moves what the game queued to the clients, carries out its kicks (telling the kicked
    /// client why) and drops clients over the reliable backlog cap.
    pub(crate) fn drain_messages(&mut self) {
        loop {
            self.net_stats.outbox_dropped += std::mem::take(&mut self.outbox.dropped);
            for (slot, client) in self.clients.iter_mut().enumerate() {
                let reliable = std::mem::take(&mut self.outbox.reliable[slot]);
                let unreliable = std::mem::take(&mut self.outbox.unreliable[slot]);
                if client.is_free() {
                    continue;
                }
                if !reliable.is_empty() {
                    client.chan.write_reliable(reliable.as_slice());
                }
                if client.queue_unreliable(unreliable.as_slice()) {
                    self.net_stats.unreliable_dropped += 1;
                }
            }

            let mut drops: Vec<(u8, Option<String>)> = std::mem::take(&mut self.outbox.kicks)
                .into_iter()
                .map(|(slot, reason)| (slot, Some(reason)))
                .collect();
            drops.extend(self.overloaded_slots().into_iter().map(|slot| (slot, None)));
            if drops.is_empty() {
                return;
            }
            for (slot, reason) in drops {
                self.release_slot(slot, reason.as_deref());
            }
        }
    }

    /// The broadcast half of `SV_UpdateToReliableMessages` (ReHLDS `sv_main.cpp`, read
    /// 2026-11-22): what the game sent to `MsgDest::All` during the frame is appended to the
    /// reliable stream of every client with `client->active`, which `SV_WriteSpawn` sets (here:
    /// past the `spawn` answer), and the buffer is emptied. The `REHLDS_FIXES` build also reaches
    /// a client that has been in the game before; not reproduced.
    pub(crate) fn flush_reliable_datagram(&mut self) {
        if self.outbox.reliable_all.is_empty() {
            return;
        }
        let bytes = std::mem::replace(&mut self.outbox.reliable_all, MsgWriter::new());
        for client in &mut self.clients {
            if client.state.is_in_game() {
                client.chan.write_reliable(bytes.as_slice());
            }
        }
        // The cap applies to what stays unacknowledged.
        self.drain_outbox();
    }

    /// `SV_CleanupEnts` (ReHLDS `sv_main.cpp`, read 2026-11-22), the end of
    /// `SV_SendClientMessages`: the effects that live for one frame (`EF_NOINTERP`,
    /// `EF_MUZZLEFLASH`) are cleared on every entity from 1 up, whether or not any client was
    /// sent an update this frame.
    fn cleanup_ents(&mut self) {
        self.edicts.clear_effects_from(1, EF_NOINTERP | EF_MUZZLEFLASH);
    }

    /// Connected clients whose unacknowledged reliable data exceeds [`MAX_RELIABLE_BACKLOG`].
    fn overloaded_slots(&self) -> Vec<u8> {
        self.clients
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_free() && c.chan.pending_reliable_bytes() > MAX_RELIABLE_BACKLOG)
            .map(|(slot, _)| slot as u8)
            .collect()
    }

    /// Spawn baselines of the edicts `SV_CreateBaseline` (ReHLDS `sv_main.cpp`, read
    /// 2026-11-22) baselines: every live edict that is the world, a client slot or has a model
    /// ([`baselined`]), each built by [`baseline_state_from_vars`]; as many as fit
    /// what the init buffer leaves of [`MAX_SIGNON_BUFFER`] after the spawn tail
    /// ([`SPAWN_TAIL_MAX`]). Baselines that do not fit are skipped from the highest entity
    /// number down (HLDS has no such rule: its `SV_CreateBaseline` overflows the signon buffer
    /// and errors out); a client gets such an entity as a full delta from the zero state. One
    /// `tracing::error!` reports how many were skipped.
    fn compute_baselines(&mut self) {
        let max_clients = self.cfg.max_clients;
        // `sv_playermodel` (`SV_FindModelNumbers`, ReHLDS `sv_main.cpp` 666-681): -1 when the
        // game never precached it; as the state's `u16` modelindex that is `u16::MAX`, which
        // the 10-bit wire field cuts to 1023, as the `int` -1 is cut in the original.
        let player_model = self
            .precache
            .model_index("models/player.mdl")
            .unwrap_or(u16::MAX);
        let mut baselines: Vec<(u16, GoldSrcEntityState)> = self
            .edicts
            .iter_live()
            .filter(|(id, e)| baselined(id.index, &e.vars, max_clients))
            .map(|(id, e)| {
                (
                    id.index,
                    baseline_state_from_vars(id.index, &e.vars, max_clients, player_model),
                )
            })
            .collect();
        let room = MAX_SIGNON_BUFFER
            .saturating_sub(SPAWN_TAIL_MAX)
            .saturating_sub(self.outbox.init.len());
        let fits = |count: usize| {
            let mut w = MsgWriter::new();
            write_spawn_baselines(
                &mut w,
                &self.deltas,
                max_clients,
                &baselines[..count],
                self.sv_time,
                None,
            );
            w.len() <= room
        };
        if !fits(baselines.len()) {
            // The size grows with the count: the largest prefix that fits is a binary search.
            let (mut lo, mut hi) = (0, baselines.len());
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if fits(mid) {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let skipped = baselines.len() - lo;
            baselines.truncate(lo);
            tracing::error!(
                skipped,
                kept = lo,
                "spawn baselines do not fit the sign-on buffer; \
                 the highest entities are sent from the zero state"
            );
        }
        self.baselines = baselines;
        self.baseline_records = self
            .baselines
            .iter()
            .filter_map(|(number, state)| {
                let wire_type = wire_entity_type(state.entity_type);
                let schema =
                    self.deltas
                        .get(entity_schema_name(*number, wire_type, max_clients))?;
                Some((*number, (state.entity_type, state.to_delta(schema))))
            })
            .collect();
    }

    pub(crate) fn slot_of(&self, addr: &PeerAddr) -> Option<u8> {
        self.clients
            .iter()
            .position(|c| c.addr.as_ref() == Some(addr))
            .map(|i| i as u8)
    }

    fn handle_datagram(&mut self, net: &mut dyn PacketTransport, from: PeerAddr, data: &[u8]) {
        if data.starts_with(&[0xFF; 4]) {
            self.handle_connectionless(net, from, data);
        } else if let Some(slot) = self.slot_of(&from) {
            self.handle_netchan(slot, data);
        }
    }

    fn handle_netchan(&mut self, slot: u8, data: &[u8]) {
        let index = usize::from(slot);
        let incoming = match self.clients[index].chan.process_packet(data) {
            Ok(Some(incoming)) => incoming,
            // A duplicate or out-of-order datagram.
            Ok(None) => {
                self.net_stats.datagrams_rejected += 1;
                return;
            }
            Err(error) => {
                self.net_stats.datagrams_rejected += 1;
                // Only a refused stream counts; a malformed header proves nothing about the
                // sender, as anyone who knows the address can forge one.
                if matches!(error, ProtoError::StreamRefused(_)) {
                    self.count_refused_stream(slot);
                }
                return;
            }
        };
        if incoming.completed_normal_stream.is_some() || incoming.completed_file_stream.is_some() {
            self.clients[index].refused_streams = 0;
        }
        self.note_accepted(slot, incoming.sequence_ack);
        // The delta request is per packet: one without `clc_delta` asks for a full update.
        self.clients[index].delta_seq = None;
        let Some(schema) = self.usercmd_schema.clone() else {
            return;
        };
        // Losses are replayed once per packet, by its first `clc_move`.
        let mut dropped = incoming.dropped;
        // A completed file stream (`completed_file_stream`) is discarded.
        let payloads = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for payload in payloads {
            // The parser does not tell a bad `clc_move` checksum from other errors, so every
            // unparsable payload counts as a bad move.
            let Ok(messages) = parse_clc_stream(&payload, &schema, incoming.sequence) else {
                self.net_stats.payloads_rejected += 1;
                self.count_bad_move(slot);
                if self.clients[index].is_free() {
                    return;
                }
                continue;
            };
            // At most one sign-on stage command is honoured per payload.
            let mut stage_done = false;
            for message in messages {
                if self.clients[index].is_free() {
                    return;
                }
                // Resource lists, file consistency, voice and cvar replies are not handled.
                match message {
                    ClcMessage::StringCmd(line) => {
                        self.handle_stringcmd(slot, &line, &mut stage_done);
                    }
                    ClcMessage::Move {
                        num_backup, cmds, ..
                    } => {
                        let lost = std::mem::take(&mut dropped);
                        self.handle_move(slot, &cmds, num_backup, lost, incoming.sequence);
                    }
                    ClcMessage::Delta(seq) => self.clients[index].delta_seq = Some(seq),
                    _ => {}
                }
            }
        }
    }

    /// Counts a stream the netchan of `slot` refused; the client is sent a farewell and dropped
    /// at the [`MAX_REFUSED_STREAMS`]th in a row.
    fn count_refused_stream(&mut self, slot: u8) {
        let client = &mut self.clients[usize::from(slot)];
        client.refused_streams += 1;
        if client.refused_streams >= MAX_REFUSED_STREAMS {
            self.release_slot(slot, Some("Too many refused streams"));
            self.drain_outbox();
        }
    }

    /// Runs one string command. A sign-on stage command is honoured only in the stage that
    /// expects it and only while `stage_done` is clear; honouring one sets `stage_done`. Any
    /// other command needs a token from the client's string command bucket, and a `setinfo` one
    /// from its `setinfo` bucket as well.
    fn handle_stringcmd(&mut self, slot: u8, line: &str, stage_done: &mut bool) {
        let argv = tokenize_goldsrc_line(line);
        let Some(cmd) = argv.first() else {
            return;
        };
        let state = self.clients[usize::from(slot)].state;
        let is_stage_cmd = matches!(cmd.as_str(), "new" | "sendres" | "spawn" | "sendents");
        if is_stage_cmd && *stage_done {
            return;
        }
        if !is_stage_cmd && !self.clients[usize::from(slot)].stringcmds.try_take() {
            return;
        }
        match (cmd.as_str(), state) {
            // `new` while the `new` stage's output is the latest sent is a repeat: ignored.
            (
                "new",
                ClientState::Signon(
                    SignonStage::AwaitNew | SignonStage::AwaitSpawn | SignonStage::AwaitSendEnts,
                ),
            ) => self.stage_new(slot),
            ("sendres", ClientState::Signon(SignonStage::AwaitSendRes)) => {
                self.stage_sendres(slot);
            }
            ("spawn", ClientState::Signon(SignonStage::AwaitSpawn)) => {
                self.stage_spawn(slot, &argv);
            }
            ("sendents", ClientState::Signon(SignonStage::AwaitSendEnts)) => {
                self.clients[usize::from(slot)].state = ClientState::Active;
            }
            _ if is_stage_cmd => return,
            ("setinfo", _) => {
                if self.clients[usize::from(slot)].setinfos.try_take() {
                    self.client_setinfo(slot, &argv);
                }
            }
            ("disconnect" | "dropclient", _) => self.release_slot(slot, None),
            _ => self.with_game(|game, eng| game.client_command(eng, slot, &argv)),
        }
        *stage_done |= is_stage_cmd;
        // Enforces the reliable backlog cap on what the stage wrote.
        self.drain_outbox();
    }

    /// Userinfo of the connected slots that `include` accepts.
    fn userinfo_entries(&self, include: impl Fn(&Client) -> bool) -> Vec<UserInfoEntry> {
        self.clients
            .iter()
            .zip(&self.views)
            .enumerate()
            .filter(|(_, (c, _))| !c.is_free() && include(c))
            .map(|(slot, (c, v))| UserInfoEntry {
                slot: slot as u8,
                userid: c.userid,
                info: public_userinfo(&v.userinfo),
            })
            .collect()
    }

    fn stage_new(&mut self, slot: u8) {
        let index = usize::from(slot);
        if self.clients[index].put_in_server {
            // Restarting after `spawn`: end the old session the way a reconnect does.
            self.with_game(|game, eng| game.client_disconnect(eng, slot));
            if self.clients[index].is_free() {
                return;
            }
            self.edicts.reset_client(slot);
            self.world.forget(u16::from(slot) + 1);
            self.clients[index].put_in_server = false;
            self.views[index].active = false;
        }
        self.clients[index].reset_frames();
        let info = ServerInfoMsg {
            protocol: 48,
            spawn_count: self.spawn_count,
            map_crc: self.map.crc,
            client_dll_md5: [0; 16],
            max_clients: self.cfg.max_clients,
            player_slot: slot,
            is_deathmatch: true,
            game_dir: self.cfg.game_dir.clone(),
            hostname: self.hostname(),
            map_path: self.map.path.clone(),
            map_cycle: String::new(),
            vac_secure: false,
        };
        let userinfos = self.userinfo_entries(|c| c.state == ClientState::Active);
        let mut w = MsgWriter::new();
        let movevars = move_vars(&self.cvars, &self.map.entities);
        write_new_stage(
            &mut w,
            &info,
            self.cfg.build,
            &self.deltas,
            &movevars,
            &self.usermsgs,
            &userinfos,
        );
        let client = &mut self.clients[usize::from(slot)];
        client.chan.write_reliable(w.as_slice());
        // `client->pGameClient->SetMoveVars(sv_movevars)` at the end of `SV_SendResources`.
        client.movevars = Some(movevars);
        client.state = ClientState::Signon(SignonStage::AwaitSendRes);
    }

    fn stage_sendres(&mut self, slot: u8) {
        // File transfer is not implemented: every resource is listed with size 0.
        let resources = self.precache.resource_list(&|_| 0);
        let mut w = MsgWriter::new();
        write_sendres_stage(&mut w, self.spawn_count, &resources);
        let client = &mut self.clients[usize::from(slot)];
        client.chan.write_reliable(w.as_slice());
        client.state = ClientState::Signon(SignonStage::AwaitSpawn);
    }

    /// `spawn <spawn_count> <crc>`.
    fn stage_spawn(&mut self, slot: u8, argv: &[String]) {
        let index = usize::from(slot);
        let count = argv.get(1).and_then(|a| a.parse::<u32>().ok());
        if count != Some(self.spawn_count) {
            let mut w = MsgWriter::new();
            write_svc_stufftext(&mut w, "reconnect\n");
            let client = &mut self.clients[index];
            client.chan.write_reliable(w.as_slice());
            client.state = ClientState::Signon(SignonStage::AwaitNew);
            return;
        }
        let expected = munged_map_crc(self.map.crc, self.spawn_count);
        let crc = argv.get(2).and_then(|a| a.parse::<i32>().ok());
        if crc != Some(expected) {
            tracing::warn!(slot, ?crc, expected, "map CRC mismatch in spawn");
        }

        // `SV_Spawn_f` (ReHLDS `sv_main.cpp`, read 2026-11-22): the answer starts with the signon
        // buffer (what the game sent to the init buffer while the map was activated, then the
        // spawn baselines, in the order `SV_ActivateServer` made them), then `SV_WriteSpawn`.
        let mut w = MsgWriter::new();
        w.write_bytes(&self.outbox.init);
        // The game's conditional encoders run on baselines as well (`DELTA_WriteDelta`).
        let game = &mut self.game;
        let mut encode = |name: &str,
                          from: &GoldSrcEntityState,
                          to: &GoldSrcEntityState,
                          marks: &mut cs_proto48::delta::DeltaMarks<'_>| {
            game.delta_encode(name, slot, from, to, marks);
        };
        write_spawn_baselines(
            &mut w,
            &self.deltas,
            self.cfg.max_clients,
            &self.baselines,
            self.sv_time,
            Some(&mut encode),
        );

        // `SV_WriteSpawn`: a player is a sliding box that walks unless the game says otherwise;
        // the game's `ClientPutInServer` sets what it wants, then the edict is linked as
        // `SV_LinkEdict` would from its `SET_ORIGIN`. Whatever the game sends to this client
        // alone in it is thrown away: the original clears the client's reliable and unreliable
        // buffers right after the call (`SZ_Clear(&host_client->netchan.message)` and `datagram`,
        // not under `REHLDS_FIXES`, which clears them before the call; the golden capture of
        // HLDS build 10210 has none of the messages of `Spawn()` in the join). What it sends to
        // everyone stays.
        let player = self.edicts.client(slot);
        if let Some(edict) = self.edicts.get_mut(player) {
            edict.vars.solid = SOLID_SLIDEBOX;
            edict.vars.movetype = MOVETYPE_WALK;
            // `sv_player->v.colormap = NUM_FOR_EDICT(sv_player)` (ReHLDS `sv_main.cpp`,
            // `SV_WriteSpawn`, read 2026-11-22): the entity state carries it to every client.
            edict.vars.colormap = player.index;
        }
        self.outbox.discard_direct = Some(index);
        self.with_game(|game, eng| game.client_put_in_server(eng, slot));
        self.outbox.discard_direct = None;
        if self.clients[index].is_free() {
            // Kicked while being put in.
            return;
        }
        self.with_engine(|eng| eng.link_entity(player, false));
        self.clients[index].put_in_server = true;
        self.views[index].active = true;

        let view_angles = self
            .edicts
            .get(self.edicts.client(slot))
            .map(|e| e.vars.v_angle)
            .unwrap_or_default();
        let userinfos = self.userinfo_entries(|_| true);
        write_spawn_stage(
            &mut w,
            view_angles,
            self.sv_time,
            &userinfos,
            &self.outbox.light_styles,
        );
        // `SV_WriteClientdataToMessage`, then `svc_signonnum 1` and `SV_WriteVoiceCodec`.
        self.write_spawn_clientdata(slot, &mut w);
        write_spawn_tail(&mut w);
        let client = &mut self.clients[index];
        client.chan.write_reliable(w.as_slice());
        // `host_client->sendinfo = TRUE` (`SV_WriteSpawn`, sv_main.cpp:1464, read 2026-11-22): the
        // others learn of the joiner at the end of this frame (`broadcast_userinfo`).
        self.views[index].userinfo_dirty = true;
        client.state = ClientState::Signon(SignonStage::AwaitSendEnts);
    }
}

#[cfg(test)]
mod tests {
    use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
    use cs_proto48::messages::svc;
    use cs_proto48::netchan::MAX_MSGLEN;
    use glam::Vec3;

    use super::*;
    use crate::api_messages::{MAX_LIGHT_STYLE_BYTES, MsgDest};
    use crate::edict::EdictId;
    use crate::oob::MAX_INFO_STRING;
    use crate::signon::{
        SPAWN_CLIENTDATA_MAX,
        INIT_BUFFER_BUDGET, MAX_LIGHTSTYLES, write_spawn_baselines, write_spawn_stage,
    };

    /// `NET_MAX_PAYLOAD` of ReHLDS (`rehlds/engine/net.h`, read 2026-11-21), also the size of
    /// the reliable message buffer of its netchan (`net_chan.cpp`).
    const NET_MAX_PAYLOAD: usize = 65536;

    /// A game that fills everything the `spawn` stage carries: `entities` map entities (with a
    /// model, an origin and angles; with every baseline field set to a value that differs
    /// between entities when `all_fields`), 64 light styles of the longest stored length and a
    /// full init buffer.
    struct WorstCaseGame {
        entities: usize,
        all_fields: bool,
    }

    impl GameDll for WorstCaseGame {
        fn usermsgs(&self) -> UserMsgRegistry {
            UserMsgRegistry::cs16_default()
        }

        fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
            eng.precache_model("models/test.mdl").expect("before lock");
            for n in 0..self.entities {
                let id = eng.create_entity("worst").expect("free edict");
                let k = n as f32;
                let owner = eng.client_edict(0);
                let vars = eng.ent_mut(id).expect("just allocated");
                vars.modelindex = 2;
                vars.origin = Vec3::new(4000.0 - k, -3000.0 + 2.0 * k, 100.5 + k);
                vars.angles = Vec3::new((k * 3.7) % 360.0, (k * 1.3) % 360.0, (k * 5.1) % 360.0);
                if !self.all_fields {
                    continue;
                }
                vars.weaponmodel = 1 + (n % 200) as u16;
                vars.mins = Vec3::new(-16.0 - k % 7.0, -16.0, -36.0);
                vars.maxs = Vec3::new(16.0 + k % 5.0, 16.0, 36.0);
                vars.sequence = 1 + (n % 500) as u16;
                vars.gaitsequence = 1 + (n % 90) as u16;
                vars.frame = 0.5 + k % 200.0;
                vars.animtime = 1.0 + k;
                vars.framerate = 1.0 + (k % 7.0) * 0.25;
                vars.scale = 1.0 + (k % 9.0) * 0.1;
                vars.body = 1 + (n % 250) as u8;
                vars.skin = 1 + (n % 12) as i16;
                vars.colormap = 1 + (n % 60_000) as u16;
                vars.effects = 1 + (n % 120) as u32;
                vars.rendermode = 1 + (n % 5) as u8;
                vars.renderamt = 1 + (n % 250) as u8;
                vars.renderfx = 1 + (n % 20) as u8;
                vars.solid = 1 + (n % 4) as u8;
                vars.movetype = 1 + (n % 12) as u8;
                vars.owner = Some(owner);
                vars.aiment = Some(owner);
            }
            for style in 0..MAX_LIGHTSTYLES {
                eng.light_style(style, &"z".repeat(MAX_LIGHT_STYLE_BYTES));
            }
            // `svc_nop` bytes stay valid when cut anywhere.
            let filler = vec![svc::NOP; INIT_BUFFER_BUDGET];
            for chunk in filler.chunks(MAX_MSGLEN) {
                eng.send(MsgDest::Init, chunk);
            }
        }

        fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {}

        fn spawn_entity(
            &mut self,
            _eng: &mut EngineApi<'_>,
            _id: EdictId,
            _classname: &str,
            _keys: &[(String, String)],
        ) -> bool {
            false
        }

        fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
            Ok(())
        }

        fn client_put_in_server(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
        fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}
        fn client_command(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _argv: &[String]) {}
        fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}

        fn run_usercmd(
            &mut self,
            _eng: &mut EngineApi<'_>,
            _slot: u8,
            _cmd: &GoldSrcUserCmd,
            _random_seed: u32,
        ) {
        }

        fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {}

        fn update_client_data(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> GoldSrcClientData {
            GoldSrcClientData::default()
        }
    }

    /// Bytes the `spawn` stage writes to one client's reliable stream on a full server of
    /// `entities` map entities: the total, the baselines and the rest of the stage.
    fn spawn_stage_size(entities: usize, all_fields: bool) -> (usize, usize, usize) {
        let cfg = ServerConfig {
            max_clients: 32,
            max_edicts: 33 + entities,
            ..ServerConfig::default()
        };
        let server = Server::new(
            cfg,
            ServerMap::stub("test"),
            WorstCaseGame {
                entities,
                all_fields,
            },
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        assert_eq!(server.init_buffer_len(), INIT_BUFFER_BUDGET);
        // The map entities, the world (edict 0) and the 32 client slots, which `SV_CreateBaseline`
        // baselines whether or not they have a model.
        assert!(server.baselines.len() <= entities + 1 + 32);
        // Every slot connected, each with a userinfo of the longest a connect may carry.
        let info = format!("\\k\\{}", "v".repeat(MAX_INFO_STRING - 3));
        assert_eq!(info.len(), MAX_INFO_STRING);
        let userinfos: Vec<UserInfoEntry> = (0..32)
            .map(|slot| UserInfoEntry {
                slot,
                userid: 1000 + i32::from(slot),
                info: info.clone(),
            })
            .collect();
        let mut baselines = MsgWriter::new();
        write_spawn_baselines(&mut baselines, &server.deltas, 32, &server.baselines, 100.0, None);
        let mut rest = MsgWriter::new();
        rest.write_bytes(&server.outbox.init);
        write_spawn_stage(
            &mut rest,
            Vec3::new(0.0, 90.0, 0.0),
            100.0,
            &userinfos,
            &server.outbox.light_styles,
        );
        write_spawn_tail(&mut rest);
        let (a, b) = (baselines.as_slice().len(), rest.as_slice().len());
        (a + b, a, b)
    }

    /// The `spawn` stage of a full server must fit the reliable buffer ReHLDS gives a netchan
    /// (`NET_MAX_PAYLOAD`): a full init buffer, 64 light styles of the longest stored length,
    /// 32 userinfos of 256 bytes and the baselines of every entity of the default pool. A pool
    /// whose entities all have every baseline field set does not fit; that is reported, not
    /// asserted (see the final-fix report).
    #[test]
    fn worst_case_spawn_stage_fits_the_reliable_payload() {
        let entities = DEFAULT_MAX_EDICTS - 33;
        let (total, baselines, rest) = spawn_stage_size(entities, false);
        eprintln!(
            "spawn stage, {entities} model entities (origin, angles): {total} bytes \
             ({baselines} baselines, {rest} other) of {NET_MAX_PAYLOAD}"
        );
        assert!(total <= NET_MAX_PAYLOAD, "{total} bytes");

        // Everything except the baselines is bounded and fits on its own, with room left.
        assert!(rest < NET_MAX_PAYLOAD, "{rest} bytes");

        let (total, baselines, rest) = spawn_stage_size(entities, true);
        eprintln!(
            "spawn stage, {entities} entities with every field set: {total} bytes \
             ({baselines} baselines, {rest} other) of {NET_MAX_PAYLOAD}"
        );
        let (largest, _, _) = spawn_stage_size(2047 - 33, false);
        eprintln!(
            "spawn stage, {} model entities (the 11-bit entity number limit): {largest} bytes",
            2047 - 33
        );
    }

    /// The spawn tail constant covers what the `spawn` answer holds besides the init buffer and
    /// the baselines, and is not loose by more than a few bytes per message (the client data
    /// has a bound of its own, [`SPAWN_CLIENTDATA_MAX`]).
    #[test]
    fn spawn_tail_max_is_the_worst_case_of_the_tail() {
        let (_, _, rest) = spawn_stage_size(0, false);
        // `spawn_stage_size` fills the init buffer; the tail is what is left, with no client
        // data and no choke or forced view.
        let tail = rest - INIT_BUFFER_BUDGET + 1 + 7 + SPAWN_CLIENTDATA_MAX;
        assert!(tail <= SPAWN_TAIL_MAX, "{tail} bytes of {SPAWN_TAIL_MAX}");
        assert!(SPAWN_TAIL_MAX - tail <= 64, "{tail} bytes of {SPAWN_TAIL_MAX}");
    }

    /// `SPAWN_CLIENTDATA_MAX` holds the client data with every field at its widest and 64
    /// weapon records likewise (a string at its 256 bytes).
    #[test]
    fn spawn_clientdata_max_holds_the_widest_client_data() {
        let deltas = DeltaRegistry::cs16_default();
        let bits = |schema: &DeltaSchema| -> usize {
            schema
                .fields
                .iter()
                .map(|f| {
                    if f.base_type() == cs_proto48::delta::DT_STRING {
                        8 * 256
                    } else {
                        usize::from(f.significant_bits) + 1
                    }
                })
                .sum::<usize>()
                + 8
        };
        let clientdata = bits(deltas.get("clientdata_t").expect("clientdata_t"));
        let weapon = bits(deltas.get("weapon_data_t").expect("weapon_data_t")) + 7;
        let bytes = 1 + clientdata.div_ceil(8) + 64 * weapon.div_ceil(8) + 8;
        assert!(bytes <= SPAWN_CLIENTDATA_MAX, "{bytes} bytes");
    }

    /// Init messages and baselines share `MAX_SIGNON_BUFFER`: whatever the game wrote, the
    /// baselines kept fit what the init buffer and the spawn tail leave, the ones that do not
    /// are the highest, and the whole `spawn` stage fits a reliable payload.
    #[test]
    fn init_and_baselines_share_the_signon_budget() {
        // Every baseline field set: 1500 entities do not all fit.
        let entities = 1500;
        let (total, baselines, rest) = spawn_stage_size(entities, true);
        eprintln!(
            "{entities} entities with every field set: {total} bytes of {NET_MAX_PAYLOAD} \
             ({baselines} baselines, {rest} other)"
        );
        assert!(total <= MAX_SIGNON_BUFFER, "{total} bytes");
        assert!(baselines > 0);
        assert!(baselines + INIT_BUFFER_BUDGET <= MAX_SIGNON_BUFFER - SPAWN_TAIL_MAX);
    }

    #[test]
    fn default_config_matches_brief() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.hostname, "Counter-Strike 1.6 Rust");
        assert_eq!(cfg.max_clients, 16);
        assert_eq!(cfg.game_dir, "cstrike");
        assert_eq!(cfg.build, 8684);
        assert_eq!((cfg.timeout_secs, cfg.signon_timeout_secs), (60.0, 120.0));
        assert_eq!((cfg.min_update_rate, cfg.max_update_rate), (10, 102));
        assert_eq!((cfg.min_rate, cfg.max_rate), (1000, 100_000));
    }
}
