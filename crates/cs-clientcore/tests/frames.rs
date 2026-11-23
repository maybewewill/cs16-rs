//! Frames, the entity store and interpolation: synthetic updates for the exact numbers, golden
//! captures for the real wire, a loopback server for the wiring in the connection.

use std::collections::BTreeMap;

use cs_clientcore::entities::{ef, movetype};
use cs_clientcore::frames::UPDATE_MASK;
use cs_clientcore::resources::res_type;
use cs_clientcore::{
    ClientFrames, ConnState, Connection, ConnectionConfig, DeltaRefusal, EmitInputs, EntityKind,
    FrameOutcome, PrecacheTables,
};
use cs_cvar::CVarRegistry;
use cs_engine::{EdictId, EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::messages::{
    ENTITY_NORMAL, PacketEntitiesMsg, ResourceEntry, SvcMessage, SvcParseContext, parse_svc_stream,
};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{LoopbackHub, PeerAddr};
use glam::Vec3;

// ---- synthetic rig ----------------------------------------------------------------------

const PROP_MODEL: u16 = 3;
const PLAYER_MODEL: u16 = 4;

fn models() -> PrecacheTables {
    let mut t = PrecacheTables::default();
    let entry = |name: &str, index: u16| ResourceEntry {
        res_type: res_type::MODEL,
        filename: name.to_string(),
        index,
        download_size: 0,
        flags: 0,
        md5: None,
        reserved: None,
    };
    t.add(&[
        entry("maps/test.bsp", 1),
        entry("models/w_c4.mdl", PROP_MODEL),
        entry("models/player.mdl", PLAYER_MODEL),
    ]);
    t
}

struct Rig {
    frames: ClientFrames,
    ctx: SvcParseContext,
    models: PrecacheTables,
}

impl Rig {
    fn new() -> Self {
        let mut ctx = SvcParseContext::new();
        ctx.max_clients = 8;
        let mut frames = ClientFrames::new();
        frames.entities.max_clients = 8;
        frames.entities.playernum = 0;
        Self {
            frames,
            ctx,
            models: models(),
        }
    }

    fn record(&self, state: &GoldSrcEntityState) -> (u8, cs_proto48::delta::DeltaRecord) {
        let name = cs_proto48::messages::entity_schema_name(state.number, ENTITY_NORMAL, 8);
        let schema = self.ctx.deltas.get(name).expect("schema");
        (ENTITY_NORMAL, state.to_delta(schema))
    }

    fn update(
        &self,
        is_delta: bool,
        from: Option<u8>,
        states: &[GoldSrcEntityState],
    ) -> SvcMessage {
        let entities: BTreeMap<_, _> = states
            .iter()
            .map(|s| (s.number, self.record(s)))
            .collect();
        SvcMessage::PacketEntities(Box::new(PacketEntitiesMsg {
            is_delta,
            delta_from_seq: from,
            expected_count: entities.len() as u16,
            entities,
            removed: Vec::new(),
        }))
    }

    /// One packet: `svc_time`, then the entity update. The client clock is the server's.
    fn packet(&mut self, seq: u32, mtime: f32, update: SvcMessage) -> FrameOutcome {
        self.ctx.last_incoming_seq = (seq & 0xFF) as u8;
        let t = SvcMessage::Time(mtime);
        self.frames.handle(&t, &self.ctx, &self.models, seq, 0.0);
        self.frames.clock.time = f64::from(mtime);
        self.frames
            .handle(&update, &self.ctx, &self.models, seq, 0.0)
    }
}

fn prop(number: u16, x: f32, animtime: f32) -> GoldSrcEntityState {
    GoldSrcEntityState {
        number,
        entity_type: ENTITY_NORMAL,
        origin: Vec3::new(x, 0.0, 0.0),
        modelindex: PROP_MODEL,
        // Tossed: the server does not step it, the client interpolates the history.
        movetype: 6,
        animtime,
        ..GoldSrcEntityState::default()
    }
}

fn player(number: u16, x: f32, animtime: f32) -> GoldSrcEntityState {
    GoldSrcEntityState {
        number,
        entity_type: ENTITY_NORMAL,
        origin: Vec3::new(x, 0.0, 0.0),
        modelindex: PLAYER_MODEL,
        movetype: movetype::WALK,
        animtime,
        ..GoldSrcEntityState::default()
    }
}

fn emit_at(rig: &mut Rig, time: f64) -> cs_clientcore::EmitResult {
    rig.frames.clock.time = time;
    rig.frames.emit(&EmitInputs::default())
}

fn find(r: &cs_clientcore::EmitResult, number: u16) -> &cs_clientcore::VisibleEntity {
    r.entities
        .iter()
        .find(|e| e.number == number)
        .unwrap_or_else(|| panic!("entity {number} is not visible"))
}

#[test]
fn interpolates_between_two_frames() {
    let mut rig = Rig::new();
    // Entities 2 (a player) and 20 (a prop) at x = 0 at server time 1.0 ...
    let a = [player(2, 0.0, 1.0), prop(20, 0.0, 1.0)];
    let msg = rig.update(false, None, &a);
    assert_eq!(rig.packet(1, 1.0, msg), FrameOutcome::Applied);
    // ... and at x = 100 at 1.1.
    let b = [player(2, 100.0, 1.1), prop(20, 100.0, 1.1)];
    let msg = rig.update(true, Some(1), &b);
    assert_eq!(rig.packet(2, 1.1, msg), FrameOutcome::Applied);

    // The render time is the client time minus ex_interp (0.1): 1.15 shows the middle.
    let r = emit_at(&mut rig, 1.15);
    assert!((find(&r, 2).origin.x - 50.0).abs() < 0.05, "{:?}", find(&r, 2).origin);
    assert!((find(&r, 20).origin.x - 50.0).abs() < 0.05, "{:?}", find(&r, 20).origin);
    assert_eq!(find(&r, 2).kind, EntityKind::Player);
    assert_eq!(find(&r, 20).kind, EntityKind::Normal);
    assert_eq!(find(&r, 20).modelindex, PROP_MODEL);

    // A quarter of the way.
    let r = emit_at(&mut rig, 1.125);
    assert!((find(&r, 2).origin.x - 25.0).abs() < 0.05);

    // Past the newest update: a player is extrapolated up to 1.2 intervals, a prop stops at
    // the newest position (1.5 intervals past would be x = 150).
    let r = emit_at(&mut rig, 1.25);
    assert!((find(&r, 2).origin.x - 120.0).abs() < 0.05, "{:?}", find(&r, 2).origin);
    assert!((find(&r, 20).origin.x - 100.0).abs() < 0.05, "{:?}", find(&r, 20).origin);
}

#[test]
fn angles_are_interpolated_along_the_short_way() {
    let mut rig = Rig::new();
    let mut a = player(2, 0.0, 1.0);
    a.angles = Vec3::new(0.0, 350.0, 0.0);
    let mut b = player(2, 0.0, 1.1);
    b.angles = Vec3::new(0.0, 10.0, 0.0);
    let msg = rig.update(false, None, &[a]);
    rig.packet(1, 1.0, msg);
    let msg = rig.update(true, Some(1), &[b]);
    rig.packet(2, 1.1, msg);
    let r = emit_at(&mut rig, 1.15);
    // Halfway between 350 (-10 after normalisation) and 10 is 0, not 180.
    let yaw = find(&r, 2).angles.y;
    assert!(yaw.abs() < 0.01, "yaw {yaw}");
}

#[test]
fn nointerp_snaps() {
    let mut rig = Rig::new();
    rig.frames.settings.nointerp = true;
    let msg = rig.update(false, None, &[player(2, 0.0, 1.0), prop(20, 0.0, 1.0)]);
    rig.packet(1, 1.0, msg);
    let msg = rig.update(true, Some(1), &[player(2, 100.0, 1.1), prop(20, 100.0, 1.1)]);
    rig.packet(2, 1.1, msg);
    let r = emit_at(&mut rig, 1.15);
    // A player takes the newest state with `cl_nointerp`. (`CL_InterpolateModel` does not
    // read the cvar, so a prop is still interpolated.)
    assert_eq!(find(&r, 2).origin.x, 100.0);
    assert!((find(&r, 20).origin.x - 50.0).abs() < 0.05);
}

#[test]
fn a_teleport_snaps_and_ef_nointerp_resets_the_latched_state() {
    let mut rig = Rig::new();
    let msg = rig.update(false, None, &[player(2, 0.0, 1.0), player(3, 0.0, 1.0)]);
    rig.packet(1, 1.0, msg);
    // Player 2 jumps 1000 units in 0.05 s: more than sv_maxvelocity (2000) per update
    // interval (1/20 s) allows, so its history restarts and it is drawn at the new place.
    // Player 3 has EF_NOINTERP: `CL_ResetLatchedVars` for the studio renderer (the position
    // history is not touched, so it still moves over the interval).
    let mut far = player(3, 100.0, 1.05);
    far.effects = ef::NOINTERP;
    let msg = rig.update(true, Some(1), &[player(2, 1000.0, 1.05), far]);
    rig.packet(2, 1.05, msg);
    let r = emit_at(&mut rig, 1.1);
    assert_eq!(find(&r, 2).origin.x, 1000.0);
    let p3 = find(&r, 3);
    assert_eq!(p3.origin.x, 0.0);
    assert_eq!(p3.latched.prevorigin.x, 100.0);
    assert_eq!(p3.latched.prevanimtime, 1.05);
    assert_eq!(p3.state.animtime, 1.05);
}

#[test]
fn a_new_entity_waits_for_its_render_time_and_then_appears_in_place() {
    let mut rig = Rig::new();
    let msg = rig.update(false, None, &[player(2, 0.0, 1.0)]);
    rig.packet(1, 1.0, msg);
    // Entity 20 shows up in the second frame: it has no history, so it is not drawn until
    // the render time (client time minus ex_interp) reaches its first update, and it is not
    // dragged in from the origin.
    let msg = rig.update(true, Some(1), &[player(2, 0.0, 1.1), prop(20, 500.0, 1.1)]);
    rig.packet(2, 1.1, msg);
    let r = emit_at(&mut rig, 1.15);
    assert!(r.entities.iter().all(|e| e.number != 20));
    let r = emit_at(&mut rig, 1.25);
    assert_eq!(find(&r, 20).origin.x, 500.0);
}

#[test]
fn delta_from_an_unacked_frame_is_refused() {
    let mut rig = Rig::new();
    let msg = rig.update(false, None, &[player(2, 0.0, 1.0), prop(20, 0.0, 1.0)]);
    assert_eq!(rig.packet(1, 1.0, msg), FrameOutcome::Applied);
    assert_eq!(rig.frames.valid_sequence, 1);

    // Based on packet 2, which this client never received.
    let msg = rig.update(true, Some(2), &[player(2, 10.0, 1.1)]);
    assert_eq!(
        rig.packet(3, 1.1, msg),
        FrameOutcome::Refused(DeltaRefusal::UnknownFrame)
    );
    assert_eq!(rig.frames.valid_sequence, 0, "no frame may be acknowledged");
    assert!(rig.frames.current_frame().is_none());
    assert!(rig.frames.emit(&EmitInputs::default()).entities.is_empty());
    // The entity states of the last good frame are untouched.
    assert_eq!(rig.frames.entities.get(2).unwrap().curstate.origin.x, 0.0);

    // Based on the frame it arrived in.
    let msg = rig.update(true, Some(4), &[player(2, 10.0, 1.2)]);
    assert_eq!(
        rig.packet(4, 1.2, msg),
        FrameOutcome::Refused(DeltaRefusal::TooOld)
    );

    // Based on a frame the ring has long forgotten.
    let msg = rig.update(true, Some(1), &[player(2, 10.0, 6.0)]);
    let seq = 1 + UPDATE_MASK;
    assert_eq!(
        rig.packet(seq, 6.0, msg),
        FrameOutcome::Refused(DeltaRefusal::TooOld)
    );

    // A full update recovers.
    let msg = rig.update(false, None, &[player(2, 20.0, 6.1)]);
    assert_eq!(rig.packet(seq + 1, 6.1, msg), FrameOutcome::Applied);
    assert_eq!(rig.frames.valid_sequence, seq + 1);
    assert!(!rig.frames.emit(&EmitInputs::default()).entities.is_empty());
}

#[test]
fn deltas_keep_working_past_sequence_255() {
    let mut rig = Rig::new();
    let first = 254u32;
    let msg = rig.update(false, None, &[player(2, 0.0, 1.0), prop(20, 0.0, 1.0)]);
    assert_eq!(rig.packet(first, 1.0, msg), FrameOutcome::Applied);
    let mut t = 1.0f32;
    for seq in (first + 1)..=260 {
        t += 0.1;
        let x = (seq - first) as f32 * 100.0;
        let msg = rig.update(
            true,
            Some(((seq - 1) & 0xFF) as u8),
            &[player(2, x, t), prop(20, x, t)],
        );
        assert_eq!(rig.packet(seq, t, msg), FrameOutcome::Applied, "seq {seq}");
        let f = rig.frames.current_frame().unwrap();
        assert!(f.entities.iter().all(|e| !e.is_new), "seq {seq}: an entity was reset");
    }
    // Render time between the last two updates: the middle of 500 and 600.
    let r = emit_at(&mut rig, f64::from(t) - 0.05 + 0.1);
    assert!((find(&r, 2).origin.x - 550.0).abs() < 0.1, "{:?}", find(&r, 2).origin);
    assert!((find(&r, 20).origin.x - 550.0).abs() < 0.1, "{:?}", find(&r, 20).origin);
}

#[test]
fn a_packet_without_an_entity_update_changes_nothing() {
    let mut rig = Rig::new();
    let msg = rig.update(false, None, &[player(2, 0.0, 1.0), prop(20, 0.0, 1.0)]);
    rig.packet(1, 1.0, msg);
    let before = emit_at(&mut rig, 1.1);
    assert!(!before.entities.is_empty());
    // Packet 2 carries a time and a print only.
    rig.frames.handle(&SvcMessage::Time(1.1), &rig.ctx, &rig.models, 2, 0.0);
    rig.frames
        .handle(&SvcMessage::Print("hi".into()), &rig.ctx, &rig.models, 2, 0.0);
    assert_eq!(rig.frames.parsecount, 1);
    let after = emit_at(&mut rig, 1.1);
    assert_eq!(before, after);
}

#[test]
fn a_delta_from_a_frame_that_was_itself_refused_is_refused() {
    let mut rig = Rig::new();
    let msg = rig.update(false, None, &[player(2, 0.0, 1.0)]);
    rig.packet(1, 1.0, msg);
    let msg = rig.update(true, Some(9), &[player(2, 1.0, 1.1)]);
    assert!(matches!(rig.packet(2, 1.1, msg), FrameOutcome::Refused(_)));
    // Packet 2 was thrown away, so it is no base for packet 3.
    let msg = rig.update(true, Some(2), &[player(2, 2.0, 1.2)]);
    assert_eq!(
        rig.packet(3, 1.2, msg),
        FrameOutcome::Refused(DeltaRefusal::UnknownFrame)
    );
}

#[test]
fn local_player_is_not_interpolated() {
    let mut rig = Rig::new();
    // Slot 0 is entity 1.
    let msg = rig.update(false, None, &[player(1, 0.0, 1.0)]);
    rig.packet(1, 1.0, msg);
    let msg = rig.update(true, Some(1), &[player(1, 100.0, 1.1)]);
    rig.packet(2, 1.1, msg);
    rig.frames.clock.time = 1.15;
    let r = rig.frames.emit(&EmitInputs {
        simorg: Some(Vec3::new(77.0, 0.0, 0.0)),
        standing_on: None,
    });
    let e = find(&r, 1);
    assert!(e.local_player);
    assert_eq!(e.origin.x, 77.0);
}

#[test]
fn baselines_and_clientdata_are_kept() {
    let mut rig = Rig::new();
    let base = prop(20, 5.0, 0.0);
    let (ty, rec) = rig.record(&base);
    let msg = SvcMessage::SpawnBaseline {
        baselines: BTreeMap::from([(20, (ty, rec))]),
        instance_baselines: Vec::new(),
    };
    rig.frames.handle(&msg, &rig.ctx, &rig.models, 1, 0.0);
    assert_eq!(rig.frames.entities.get(20).unwrap().baseline.origin.x, 5.0);

    let cd = GoldSrcClientData {
        health: 77.0,
        ..GoldSrcClientData::default()
    };
    let msg = SvcMessage::ClientData {
        delta_seq: None,
        record: cs_proto48::delta::DeltaRegistry::cs16_default()
            .get("clientdata_t")
            .unwrap()
            .zero_record(),
        parsed: cd,
        weapon_deltas: Vec::new(),
    };
    rig.frames.handle(&msg, &rig.ctx, &rig.models, 2, 0.0);
    let f = rig.frames.ring.get(2).unwrap();
    assert_eq!(f.clientdata.as_ref().unwrap().health, 77.0);
}

// ---- golden captures --------------------------------------------------------------------

/// `CS48CAP1` records: `u8 dir` (0 received), `f64 time`, `u32 len`, the datagram.
fn read_capture(bytes: &[u8]) -> Vec<(bool, f64, Vec<u8>)> {
    let mut rest = bytes.strip_prefix(b"CS48CAP1".as_slice()).expect("magic");
    let mut out = Vec::new();
    while !rest.is_empty() {
        let (h, after) = rest.split_at(13);
        let time = f64::from_le_bytes(h[1..9].try_into().unwrap());
        let len = u32::from_le_bytes(h[9..13].try_into().unwrap()) as usize;
        let (data, after) = after.split_at(len);
        out.push((h[0] == 0, time, data.to_vec()));
        rest = after;
    }
    out
}

#[test]
fn replays_a_golden_capture_into_the_entity_store() {
    let Some(dir) = std::env::var_os("CS16_GOLDEN_DIR") else {
        return;
    };
    let mut replayed = 0;
    for file in ["hlds_join.cs48cap", "hlds_office_join.cs48cap"] {
        let Ok(bytes) = std::fs::read(std::path::Path::new(&dir).join(file)) else {
            continue;
        };
        replayed += 1;
        let records = read_capture(&bytes);

        let mut chan = NetChan::new(FRAGMENT_C2S_SIZE);
        let mut ctx = SvcParseContext::new();
        let mut splits = SplitPacketAssembler::default();
        let mut frames = ClientFrames::new();
        let mut models = PrecacheTables::default();
        let (mut applied, mut refused, mut emitted) = (0u32, 0u32, 0usize);
        let mut seen: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
        let mut max_players_seen = 0usize;

        for (received, time, data) in records {
            if !received {
                continue;
            }
            let Some(datagram) = splits.push(&data) else {
                continue;
            };
            if datagram.len() >= 4 && datagram[..4] == CONNECTIONLESS_HEADER.to_le_bytes() {
                continue;
            }
            let Some(incoming) = chan.process_packet(&datagram).expect("netchan") else {
                continue;
            };
            ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;
            let streams =
                std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
            for stream in streams {
                for msg in parse_svc_stream(&stream, &mut ctx).expect("svc stream") {
                    if let SvcMessage::ResourceList { resources, .. } = &msg {
                        models.add(resources);
                    }
                    match frames.handle(&msg, &ctx, &models, incoming.sequence, time) {
                        FrameOutcome::Applied => {
                            applied += 1;
                            // The server's frame is the parser's frame.
                            let current = frames.current_frame().expect("current frame");
                            if let SvcMessage::PacketEntities(update) = &msg {
                                let numbers: Vec<u16> =
                                    current.entities.iter().map(|e| e.state.number).collect();
                                let wire: Vec<u16> = update.entities.keys().copied().collect();
                                assert_eq!(numbers, wire, "{file}: frame {}", incoming.sequence);
                            }
                            // An entity the base frame holds is not new.
                            if let SvcMessage::PacketEntities(update) = &msg
                                && let Some(from) = update.delta_from_seq
                            {
                                let base = incoming.sequence
                                    - (incoming.sequence.wrapping_sub(u32::from(from)) & 0xFF);
                                let held = &seen[&base];
                                for e in &current.entities {
                                    assert_eq!(
                                        e.is_new,
                                        !held.contains(&e.state.number),
                                        "{file}: seq {} entity {}",
                                        incoming.sequence,
                                        e.state.number
                                    );
                                }
                            }
                            seen.insert(
                                incoming.sequence,
                                current.entities.iter().map(|e| e.state.number).collect(),
                            );
                            // Render the frame as the client would, a frame step later.
                            frames.advance(0.01);
                            let r = frames.emit(&EmitInputs::default());
                            emitted += r.entities.len();
                            max_players_seen = max_players_seen.max(
                                r.entities
                                    .iter()
                                    .filter(|e| e.kind == EntityKind::Player)
                                    .count(),
                            );
                            for e in &r.entities {
                                assert!(e.origin.is_finite(), "{file}: {e:?}");
                                assert!(e.angles.is_finite(), "{file}: {e:?}");
                            }
                        }
                        FrameOutcome::Refused(why) => {
                            refused += 1;
                            eprintln!("{file}: refused {why:?} at {}", incoming.sequence);
                        }
                        FrameOutcome::Ignored => {}
                    }
                }
            }
        }
        eprintln!(
            "{file}: {applied} frames, {refused} refused, {emitted} visible entities, \
             {} entities known, up to {max_players_seen} players",
            frames.entities.len()
        );
        assert!(applied > 0, "{file}: no entity update applied");
        assert_eq!(refused, 0, "{file}: the server's deltas must be based on frames we hold");
        assert!(emitted > 0, "{file}: nothing was ever visible");
        assert!(frames.entities.len() > 1, "{file}: only one entity known");
        assert!(frames.entities.get(0).is_some(), "{file}: no world entity");
    }
    assert!(replayed > 0 || std::env::var_os("CS16_GOLDEN_DIR").is_none());
}

// ---- the wiring in the connection -------------------------------------------------------

struct Bare;

impl GameDll for Bare {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }
    fn server_activate(&mut self, _: &mut EngineApi<'_>) {}
    fn server_deactivate(&mut self, _: &mut EngineApi<'_>) {}
    fn spawn_entity(
        &mut self,
        _: &mut EngineApi<'_>,
        _: EdictId,
        _: &str,
        _: &[(String, String)],
    ) -> bool {
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

#[test]
fn the_connection_feeds_the_frame_store() {
    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        ServerMap::stub("test"),
        Bare,
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    let mut hub = LoopbackHub::new();
    let endpoint = hub.connect();
    let cfg = ConnectionConfig::new("core", 76_561_197_960_265_730, &md5_hex(b"frames loopback"));
    let mut conn = Connection::new(endpoint, PeerAddr::Loopback(0), cfg);
    conn.connect();

    let mut now = 0.0;
    for _ in 0..800 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut hub, now);
        conn.frames_mut().advance(0.01);
        if conn.state() == &ConnState::InGame && conn.stats().delta_updates >= 3 {
            break;
        }
    }
    assert_eq!(conn.state(), &ConnState::InGame, "{:?}", conn.messages());
    assert!(conn.stats().delta_updates >= 3, "no delta update arrived");

    let frames = conn.frames();
    assert_eq!(frames.entities.max_clients, 4);
    assert_eq!(Some(frames.entities.playernum), conn.player_slot());
    let current = frames.current_frame().expect("a valid current frame");
    // The frame store holds the same entities the connection's own view does.
    let numbers: Vec<u16> = current.entities.iter().map(|e| e.state.number).collect();
    let view: Vec<u16> = conn.entities().keys().copied().collect();
    assert_eq!(numbers, view);
    assert!(frames.valid_sequence != 0);
}
