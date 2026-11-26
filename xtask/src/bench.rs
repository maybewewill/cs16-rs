//! Phase 13/16 4-pillar performance benchmark harness (`cargo xtask bench`).
//!
//! Suites:
//! - `server`: 32-player `de_dust2` (1 loopback client + 31 ZBot combat/pathfinding agents)
//!   `avg / p50 / p95 / p99 / max` tick ms + startup readiness ms + bandwidth kbps.
//! - `delta`: Encode + decode `100,000` Protocol 48 deltas across `entity_state_player_t`,
//!   `entity_state_t`, `clientdata_t`, and `usercmd_t` -> `ns/op` and `entities/sec`.
//! - `client`: `1,000` client frames of `Connection` + `Predictor` (`CL_PredictMovement` with
//!   5 unacked cmds) + `ViewBuilder::build_into` (32 studio players + 64 tempents + 24 decals) ->
//!   `avg / p99` CPU `us/frame`.
//! - `gpu` (`--suite gpu` or `--gpu`): Headless `1280x800` `de_dust2` `200` frames with `16`
//!   studio models and `3` active smoke grenade clouds (`45` `gas_puff_01.spr` sprites) ->
//!   `avg / p99` `frame_ms`, `draw_calls/frame`, and `bytes_uploaded/frame`.
//!
//! Runs under whatever Cargo profile `xtask` was compiled with (`dev` by default; never invokes
//! `cargo build --release`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use cs_assets::bsp::{BspMap, Entity};
use cs_assets::scheme::SkyboxTextures;
use cs_client::camera::Camera;
use cs_client::render::scene::{EntRenderMode, ModelRegistry, RenderEntity, SceneFrame, ViewBasis};
use cs_client::render::{RenderSettings, Renderer, SceneDynamicData};
use cs_client::view::{FrameView, MapFxWorld, ViewBuilder, ViewInputs};
use cs_clientcore::tempents::{FxRandom, ModelRef, NoWorld, SmokeCloudConfig, ftent};
use cs_clientcore::{ConnState, Connection, ConnectionConfig, SoundQueue};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_local::{load_server_map, new_server};
use cs_pm::collision::{NO_COLLISION, WorldCollision};
use cs_proto48::bitbuf::{MsgReader, MsgWriter};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{
    DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, read_struct_delta,
    write_struct_delta,
};
use cs_proto48::messages::{PacketEntitiesMsg, SvcMessage, SvcParseContext};
use cs_proto48::tempent::TempEffect;
use cs_transport::{LoopbackHub, PacketTransport, PeerAddr};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

/// Which benchmark pillar(s) to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchSuite {
    All,
    Server,
    Delta,
    Client,
    Gpu,
}

impl BenchSuite {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.to_ascii_lowercase().as_str() {
            "all" => Ok(Self::All),
            "server" => Ok(Self::Server),
            "delta" => Ok(Self::Delta),
            "client" => Ok(Self::Client),
            "gpu" => Ok(Self::Gpu),
            other => Err(format!(
                "--suite: expected all|server|delta|client|gpu, got `{other}`"
            )),
        }
    }
}

/// CLI configuration for `cargo xtask bench`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchConfig {
    pub suite: BenchSuite,
    pub gpu: bool,
    pub ticks: usize,
    pub players: u8,
    pub map: String,
    pub gamedir: Option<PathBuf>,
    pub stub: bool,
    pub json: bool,
    pub delta_ops: usize,
    pub client_frames: usize,
    pub gpu_frames: usize,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            suite: BenchSuite::All,
            gpu: false,
            ticks: 100,
            players: 32,
            map: "de_dust2".to_string(),
            gamedir: None,
            stub: false,
            json: false,
            delta_ops: 100_000,
            client_frames: 1_000,
            gpu_frames: 200,
        }
    }
}

impl BenchConfig {
    pub fn runs_server(&self) -> bool {
        matches!(self.suite, BenchSuite::All | BenchSuite::Server)
    }

    pub fn runs_delta(&self) -> bool {
        matches!(self.suite, BenchSuite::All | BenchSuite::Delta)
    }

    pub fn runs_client(&self) -> bool {
        matches!(self.suite, BenchSuite::All | BenchSuite::Client)
    }

    pub fn runs_gpu(&self) -> bool {
        self.suite == BenchSuite::Gpu || self.gpu
    }
}

/// Suite 1 (`server`) results.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerBenchReport {
    pub map: String,
    pub map_source: &'static str,
    pub players: u8,
    pub ticks: usize,
    pub startup_ms: f64,
    pub tick_avg_ms: f64,
    pub tick_p50_ms: f64,
    pub tick_p95_ms: f64,
    pub tick_p99_ms: f64,
    pub tick_max_ms: f64,
    pub delta_pkt_count: u64,
    pub delta_pkt_avg_bytes: f64,
    pub delta_pkt_max_bytes: usize,
    pub bandwidth_kbps_per_client: f64,
}

/// Suite 2 (`delta`) results.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaBenchReport {
    pub ops: usize,
    pub encode_ns_per_op: f64,
    pub decode_ns_per_op: f64,
    pub total_ns_per_op: f64,
    pub entities_per_sec: f64,
    pub bytes_encoded: u64,
}

/// Suite 3 (`client`) results.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientBenchReport {
    pub map: String,
    pub map_source: &'static str,
    pub frames: usize,
    pub unacked_cmds: usize,
    pub studio_players: usize,
    pub tempents: usize,
    pub decals: usize,
    pub cpu_avg_us: f64,
    pub cpu_p50_us: f64,
    pub cpu_p95_us: f64,
    pub cpu_p99_us: f64,
    pub cpu_max_us: f64,
}

/// Suite 4 (`gpu`) results.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuBenchReport {
    pub adapter: String,
    pub backend: String,
    pub map: String,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    pub studio_models: usize,
    pub smoke_sprites: usize,
    pub frame_avg_ms: f64,
    pub frame_p50_ms: f64,
    pub frame_p95_ms: f64,
    pub frame_p99_ms: f64,
    pub frame_max_ms: f64,
    pub draw_calls_per_frame: f64,
    pub bsp_draw_calls: f64,
    pub mdl_draw_calls: f64,
    pub sprite_draw_calls: f64,
    pub bytes_uploaded_per_frame: f64,
}

/// Combined results emitted by `cargo xtask bench`.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchReport {
    pub profile: &'static str,
    pub server: Option<ServerBenchReport>,
    pub delta: Option<DeltaBenchReport>,
    pub client: Option<ClientBenchReport>,
    pub gpu: Option<GpuBenchReport>,
}

struct MeasuringTransport {
    hub: LoopbackHub,
    recording: bool,
    tx_packets: u64,
    tx_bytes: u64,
    max_packet_bytes: usize,
}

impl MeasuringTransport {
    fn new(hub: LoopbackHub) -> Self {
        Self {
            hub,
            recording: false,
            tx_packets: 0,
            tx_bytes: 0,
            max_packet_bytes: 0,
        }
    }

    fn reset_metrics(&mut self) {
        self.recording = true;
        self.tx_packets = 0;
        self.tx_bytes = 0;
        self.max_packet_bytes = 0;
    }
}

impl PacketTransport for MeasuringTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        self.hub.recv()
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), cs_transport::TransportError> {
        if self.recording {
            self.tx_packets += 1;
            self.tx_bytes += data.len() as u64;
            self.max_packet_bytes = self.max_packet_bytes.max(data.len());
        }
        self.hub.send(to, data)
    }
}

fn parse_positive_usize(flag: &str, raw: &str) -> Result<usize, String> {
    raw.parse::<usize>()
        .ok()
        .filter(|&n| n >= 1)
        .ok_or_else(|| format!("{flag}: expected positive integer, got `{raw}`"))
}

/// Parses CLI arguments for `cargo xtask bench`.
pub fn parse_bench_args(mut args: impl Iterator<Item = String>) -> Result<BenchConfig, String> {
    let mut cfg = BenchConfig::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--suite" => {
                let raw = args.next().ok_or("--suite needs a value")?;
                cfg.suite = BenchSuite::parse(&raw)?;
            }
            "--gpu" => cfg.gpu = true,
            "--ticks" => {
                let raw = args.next().ok_or("--ticks needs a value")?;
                cfg.ticks = parse_positive_usize("--ticks", &raw)?;
            }
            "--delta-ops" => {
                let raw = args.next().ok_or("--delta-ops needs a value")?;
                cfg.delta_ops = parse_positive_usize("--delta-ops", &raw)?;
            }
            "--client-frames" => {
                let raw = args.next().ok_or("--client-frames needs a value")?;
                cfg.client_frames = parse_positive_usize("--client-frames", &raw)?;
            }
            "--gpu-frames" => {
                let raw = args.next().ok_or("--gpu-frames needs a value")?;
                cfg.gpu_frames = parse_positive_usize("--gpu-frames", &raw)?;
            }
            "--players" => {
                let raw = args.next().ok_or("--players needs a value")?;
                let players = raw
                    .parse::<u8>()
                    .ok()
                    .filter(|&n| (1..=32).contains(&n))
                    .ok_or_else(|| format!("--players: expected 1..=32, got `{raw}`"))?;
                cfg.players = players;
            }
            "--map" => cfg.map = args.next().ok_or("--map needs a value")?,
            "--gamedir" => {
                cfg.gamedir = Some(PathBuf::from(args.next().ok_or("--gamedir needs a value")?));
            }
            "--stub" => cfg.stub = true,
            "--json" => cfg.json = true,
            other => return Err(format!("unknown bench argument `{other}`")),
        }
    }
    Ok(cfg)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn summarize_samples(mut samples: Vec<f64>) -> (f64, f64, f64, f64, f64) {
    let sum: f64 = samples.iter().sum();
    let avg = sum / (samples.len().max(1) as f64);
    samples.sort_by(f64::total_cmp);
    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let p99 = percentile(&samples, 0.99);
    let max = samples.last().copied().unwrap_or(0.0);
    (avg, p50, p95, p99, max)
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Executes the configured benchmark suite(s) on an 8 MiB stack thread (Windows dev safety)
/// and returns measured metrics.
pub fn run_bench_with_config(cfg: &BenchConfig) -> Result<BenchReport, String> {
    let cfg_owned = cfg.clone();
    let handle = std::thread::Builder::new()
        .name("xtask-bench".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || run_bench_inner(&cfg_owned))
        .map_err(|e| format!("failed to spawn bench thread: {e}"))?;
    handle
        .join()
        .map_err(|_| "benchmark thread panicked".to_string())?
}

fn run_bench_inner(cfg: &BenchConfig) -> Result<BenchReport, String> {
    let profile = if cfg!(debug_assertions) { "dev" } else { "release" };
    let server = if cfg.runs_server() {
        Some(run_server_bench(cfg)?)
    } else {
        None
    };
    let delta = if cfg.runs_delta() {
        Some(run_delta_bench(cfg)?)
    } else {
        None
    };
    let client = if cfg.runs_client() {
        Some(run_client_bench(cfg)?)
    } else {
        None
    };
    let gpu = if cfg.runs_gpu() {
        Some(run_gpu_bench(cfg)?)
    } else {
        None
    };
    Ok(BenchReport {
        profile,
        server,
        delta,
        client,
        gpu,
    })
}

fn discover_vfs(cfg: &BenchConfig) -> Result<Option<Vfs>, String> {
    if cfg.stub {
        return Ok(None);
    }
    match Vfs::discover(cfg.gamedir.as_deref(), VfsConfig::default()) {
        Ok(vfs) => Ok(Some(vfs)),
        Err(_) if cfg.gamedir.is_some() => {
            Err("cannot discover Half-Life assets at specified --gamedir".to_string())
        }
        Err(_) => Ok(None),
    }
}

fn stub_world_entities() -> Vec<Entity> {
    let make = |pairs: &[(&str, &str)]| Entity {
        properties: pairs
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    vec![
        make(&[("classname", "worldspawn")]),
        make(&[
            ("classname", "info_player_start"),
            ("origin", "0 0 64"),
            ("angle", "0"),
        ]),
        make(&[
            ("classname", "info_player_deathmatch"),
            ("origin", "512 0 64"),
            ("angle", "180"),
        ]),
    ]
}

fn create_bench_server(
    cfg: &BenchConfig,
    vfs: Option<&Vfs>,
    players: u8,
) -> Result<(Server<CsGame>, &'static str), String> {
    if let Some(vfs) = vfs {
        let map = load_server_map(vfs, &cfg.map)?;
        let mut game = CsGame::new();
        game.load_motd(vfs);
        game.load_map_cycle(vfs);
        game.enable_cvar_bot_quota();
        let srv = new_server(
            ServerConfig {
                hostname: "Bench Real".to_string(),
                max_clients: players,
                ..ServerConfig::default()
            },
            map,
            game,
            vfs,
        );
        Ok((srv, "gamedir"))
    } else {
        let mut game = CsGame::new();
        game.enable_cvar_bot_quota();
        let srv = Server::new(
            ServerConfig {
                hostname: "Bench Stub".to_string(),
                max_clients: players,
                ..ServerConfig::default()
            },
            ServerMap::stub_with_entities(&cfg.map, stub_world_entities()),
            game,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        Ok((srv, "stub"))
    }
}

// -----------------------------------------------------------------------------
// Suite 1: 32-player de_dust2 Server + 31 ZBot tick benchmark
// -----------------------------------------------------------------------------

fn run_server_bench(cfg: &BenchConfig) -> Result<ServerBenchReport, String> {
    let t_start = Instant::now();
    let vfs = discover_vfs(cfg)?;
    let (mut server, map_source) = create_bench_server(cfg, vfs.as_ref(), cfg.players)?;
    let startup_ms = t_start.elapsed().as_secs_f64() * 1000.0;

    let hub = LoopbackHub::new();
    let endpoint = hub.connect();
    let mut net = MeasuringTransport::new(hub);
    let mut conn = Connection::new(
        endpoint,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("bench-probe", 76_561_197_960_265_799, &md5_hex(b"bench")),
    );
    conn.set_userinfo("cl_updaterate", "20");
    conn.set_userinfo("rate", "30000");
    conn.connect();

    let bot_count = cfg.players.saturating_sub(1);
    let _ = server.console_command(&format!("bot_quota {bot_count}"));

    let mut now = 0.0;
    for _ in 0..400 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
        if conn.state() == &ConnState::InGame {
            break;
        }
    }
    if conn.state() != &ConnState::InGame {
        return Err("benchmark loopback client failed to reach InGame".to_string());
    }

    // Warm up until bots/navmesh are initialized and active in the match.
    for _ in 0..100 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
        if bot_count == 0
            || (server.game().bot_nav_built() && server.fake_client_count() >= usize::from(bot_count))
        {
            break;
        }
    }
    for _ in 0..10 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
    }

    net.reset_metrics();
    let mut tick_samples = Vec::with_capacity(cfg.ticks);
    for _ in 0..cfg.ticks {
        now += 0.01;
        conn.pump(now);
        let t0 = Instant::now();
        server.frame(&mut net, now);
        tick_samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let (tick_avg_ms, tick_p50_ms, tick_p95_ms, tick_p99_ms, tick_max_ms) =
        summarize_samples(tick_samples);

    let delta_pkt_avg_bytes = if net.tx_packets == 0 {
        0.0
    } else {
        net.tx_bytes as f64 / net.tx_packets as f64
    };
    let sim_secs = (cfg.ticks as f64) * 0.01;
    let bandwidth_kbps_per_client = if sim_secs > 0.0 {
        (net.tx_bytes as f64 * 8.0) / 1000.0 / sim_secs
    } else {
        0.0
    };

    Ok(ServerBenchReport {
        map: cfg.map.clone(),
        map_source,
        players: cfg.players,
        ticks: cfg.ticks,
        startup_ms,
        tick_avg_ms,
        tick_p50_ms,
        tick_p95_ms,
        tick_p99_ms,
        tick_max_ms,
        delta_pkt_count: net.tx_packets,
        delta_pkt_avg_bytes,
        delta_pkt_max_bytes: net.max_packet_bytes,
        bandwidth_kbps_per_client,
    })
}

// -----------------------------------------------------------------------------
// Suite 2: Protocol 48 Delta Encode + Decode microbenchmark (100,000 ops)
// -----------------------------------------------------------------------------

struct DeltaFixtures {
    base_player: GoldSrcEntityState,
    to_players: [GoldSrcEntityState; 16],
    base_ent: GoldSrcEntityState,
    to_ents: [GoldSrcEntityState; 16],
    base_cd: GoldSrcClientData,
    to_cds: [GoldSrcClientData; 16],
    base_cmd: GoldSrcUserCmd,
    to_cmds: [GoldSrcUserCmd; 16],
}

fn build_delta_fixtures() -> DeltaFixtures {
    let base_player = GoldSrcEntityState {
        number: 2,
        entity_type: 1,
        origin: Vec3::new(128.0, 256.0, 64.0),
        angles: Vec3::new(2.0, 90.0, 0.0),
        modelindex: 12,
        sequence: 4,
        gaitsequence: 4,
        frame: 10.0,
        animtime: 12.40,
        framerate: 1.0,
        weaponmodel: 34,
        movetype: 3,
        solid: 3,
        effects: 0,
        rendermode: 0,
        renderamt: 255,
        renderfx: 0,
        skin: 0,
        body: 0,
        scale: 1.0,
        owner: 0,
        aiment: 0,
        usehull: 0,
        spectator: false,
        mins: Vec3::new(-16.0, -16.0, -36.0),
        maxs: Vec3::new(16.0, 16.0, 36.0),
        colormap: 2,
        eflags: 0,
        rendercolor: [0, 0, 0],
        controller: [127, 127, 127, 127],
        blending: [128, 0],
        friction: 1.0,
        gravity: 1.0,
        basevelocity: Vec3::ZERO,
        startpos: Vec3::ZERO,
        endpos: Vec3::ZERO,
        impacttime: 0.0,
        starttime: 0.0,
        iuser4: 0,
    };
    let to_players = std::array::from_fn(|i| {
        let f = (i + 1) as f32;
        let mut s = base_player.clone();
        s.origin += Vec3::new(f * 3.25, f * -1.75, 0.25 * (i % 3) as f32);
        s.angles = Vec3::new(1.5 + f * 0.4, (90.0 + f * 7.5) % 360.0, 0.0);
        s.frame = (10.0 + f * 2.0) % 256.0;
        s.animtime = 12.40 + f * 0.01;
        s.gaitsequence = if i % 4 == 0 { 5 } else { 4 };
        s.blending = [(128u8).wrapping_add((i as u8) * 3), 0];
        s.effects = if i % 5 == 0 { 2 } else { 0 };
        s
    });

    let base_ent = GoldSrcEntityState {
        number: 48,
        entity_type: 1,
        origin: Vec3::new(-420.0, 812.0, 128.0),
        angles: Vec3::new(0.0, 45.0, 0.0),
        modelindex: 41,
        sequence: 0,
        gaitsequence: 0,
        frame: 0.0,
        animtime: 12.0,
        framerate: 1.0,
        weaponmodel: 0,
        movetype: 6,
        solid: 2,
        effects: 0,
        rendermode: 0,
        renderamt: 255,
        renderfx: 0,
        skin: 0,
        body: 1,
        scale: 1.0,
        owner: 2,
        aiment: 0,
        usehull: 0,
        spectator: false,
        mins: Vec3::new(-8.0, -8.0, -8.0),
        maxs: Vec3::new(8.0, 8.0, 8.0),
        colormap: 0,
        eflags: 0,
        rendercolor: [255, 255, 255],
        controller: [127; 4],
        blending: [0; 2],
        friction: 1.0,
        gravity: 1.0,
        basevelocity: Vec3::ZERO,
        startpos: Vec3::ZERO,
        endpos: Vec3::ZERO,
        impacttime: 0.0,
        starttime: 0.0,
        iuser4: 0,
    };
    let to_ents = std::array::from_fn(|i| {
        let f = (i + 1) as f32;
        let mut s = base_ent.clone();
        s.origin += Vec3::new(f * 6.5, -f * 4.0, f * 1.5);
        s.angles = Vec3::new(f * 5.0, (45.0 + f * 11.0) % 360.0, f * 2.0);
        s.frame = f * 1.5;
        s.animtime = 12.0 + f * 0.015;
        s
    });

    let base_cd = GoldSrcClientData {
        origin: Vec3::new(-180.0, 640.0, 128.0),
        velocity: Vec3::new(190.0, -85.0, 0.0),
        viewmodel: 28,
        punchangle: Vec3::new(-0.5, 0.1, 0.0),
        flags: 1 << 9,
        waterlevel: 0,
        view_ofs: Vec3::new(0.0, 0.0, 17.0),
        health: 100.0,
        b_in_duck: 0,
        weapons: (1 << 31) | (1 << 28) | (1 << 29),
        ammo_nails: 0,
        ammo_shells: 30,
        ammo_cells: 90,
        ammo_rockets: 0,
        fl_time_step_sound: 240,
        fl_duck_time: 0,
        maxspeed: 221.0,
        m_i_id: 28,
        m_fl_next_attack: 0.0,
        fov: 90.0,
        weaponanim: 3,
        deadflag: 0,
        iuser1: 0,
        iuser2: 0,
        iuser3: 0,
        iuser4: 0,
        physinfo: String::new(),
        fl_swim_time: 0,
        waterjumptime: 0,
        pushmsec: 0,
        fuser1: 0.0,
        fuser2: 120.0,
        fuser3: 0.0,
        vuser2: Vec3::ZERO,
        vuser3: Vec3::ZERO,
        vuser4: Vec3::ZERO,
    };
    let to_cds = std::array::from_fn(|i| {
        let f = (i + 1) as f32;
        let mut cd = base_cd.clone();
        cd.origin += Vec3::new(f * 2.1, -f * 0.9, 0.0);
        cd.velocity += Vec3::new(f * 3.0, f * 1.5, 0.0);
        cd.punchangle = Vec3::new(-0.5 - f * 0.08, 0.1 + f * 0.02, 0.0);
        cd.fl_time_step_sound = 240 + (i as i32) * 10;
        cd.m_fl_next_attack = 0.095 - f * 0.004;
        cd.fuser2 = 120.0 + f * 2.0;
        cd
    });

    let base_cmd = GoldSrcUserCmd {
        lerp_msec: 9,
        msec: 10,
        viewangles: Vec3::new(-1.5, 132.0, 0.0),
        forwardmove: 250.0,
        sidemove: 0.0,
        upmove: 0.0,
        lightlevel: 128,
        buttons: 8,
        impulse: 0,
        weaponselect: 0,
        impact_index: 0,
        impact_position: Vec3::ZERO,
    };
    let to_cmds = std::array::from_fn(|i| {
        let f = (i + 1) as f32;
        let mut cmd = base_cmd.clone();
        cmd.msec = 10 + (i % 3) as u8;
        cmd.viewangles = Vec3::new(-1.5 + f * 0.35, (132.0 + f * 2.25) % 360.0, 0.0);
        cmd.forwardmove = 250.0;
        cmd.sidemove = if i % 2 == 0 { 250.0 } else { -250.0 };
        cmd.buttons = 8 | if i % 4 == 0 { 1 } else { 0 };
        cmd
    });

    DeltaFixtures {
        base_player,
        to_players,
        base_ent,
        to_ents,
        base_cd,
        to_cds,
        base_cmd,
        to_cmds,
    }
}

fn run_delta_bench(cfg: &BenchConfig) -> Result<DeltaBenchReport, String> {
    let reg = DeltaRegistry::cs16_default();
    let player_schema = reg
        .get("entity_state_player_t")
        .ok_or("missing entity_state_player_t schema")?;
    let ent_schema = reg
        .get("entity_state_t")
        .ok_or("missing entity_state_t schema")?;
    let cd_schema = reg
        .get("clientdata_t")
        .ok_or("missing clientdata_t schema")?;
    let cmd_schema = reg
        .get("usercmd_t")
        .ok_or("missing usercmd_t schema")?;

    let DeltaFixtures {
        base_player,
        to_players,
        base_ent,
        to_ents,
        base_cd,
        to_cds,
        base_cmd,
        to_cmds,
    } = build_delta_fixtures();

    let total_ops = cfg.delta_ops;
    let time_base = 12.50f64;
    let batch_size = 200usize;
    let mut encode_ns = 0.0f64;
    let mut decode_ns = 0.0f64;
    let mut bytes_encoded = 0u64;

    let mut done = 0usize;
    while done < total_ops {
        let count = (total_ops - done).min(batch_size);

        let t_enc = Instant::now();
        let mut w = MsgWriter::new();
        w.start_bit_writing();
        for offset in 0..count {
            let op = done + offset;
            let variant = (op >> 2) & 15;
            match op & 3 {
                0 => {
                    write_struct_delta(
                        &mut w,
                        player_schema,
                        &base_player,
                        &to_players[variant],
                        true,
                        time_base,
                    );
                }
                1 => {
                    write_struct_delta(
                        &mut w,
                        ent_schema,
                        &base_ent,
                        &to_ents[variant],
                        true,
                        time_base,
                    );
                }
                2 => {
                    write_struct_delta(
                        &mut w,
                        cd_schema,
                        &base_cd,
                        &to_cds[variant],
                        true,
                        time_base,
                    );
                }
                _ => {
                    write_struct_delta(
                        &mut w,
                        cmd_schema,
                        &base_cmd,
                        &to_cmds[variant],
                        true,
                        time_base,
                    );
                }
            }
        }
        w.end_bit_writing();
        let wire = black_box(w.into_bytes());
        encode_ns += t_enc.elapsed().as_nanos() as f64;
        bytes_encoded += wire.len() as u64;

        let t_dec = Instant::now();
        let mut r = MsgReader::new(&wire);
        r.start_bit_reading();
        for offset in 0..count {
            let op = done + offset;
            match op & 3 {
                0 => {
                    let st = read_struct_delta(&mut r, player_schema, &base_player, time_base)
                        .map_err(|e| format!("player delta decode failed: {e}"))?;
                    black_box(st);
                }
                1 => {
                    let st = read_struct_delta(&mut r, ent_schema, &base_ent, time_base)
                        .map_err(|e| format!("entity delta decode failed: {e}"))?;
                    black_box(st);
                }
                2 => {
                    let cd = read_struct_delta(&mut r, cd_schema, &base_cd, time_base)
                        .map_err(|e| format!("clientdata delta decode failed: {e}"))?;
                    black_box(cd);
                }
                _ => {
                    let cmd = read_struct_delta(&mut r, cmd_schema, &base_cmd, time_base)
                        .map_err(|e| format!("usercmd delta decode failed: {e}"))?;
                    black_box(cmd);
                }
            }
        }
        r.end_bit_reading()
            .map_err(|e| format!("delta decode end_bit_reading failed: {e}"))?;
        decode_ns += t_dec.elapsed().as_nanos() as f64;

        done += count;
    }

    let ops_f = total_ops.max(1) as f64;
    let encode_ns_per_op = encode_ns / ops_f;
    let decode_ns_per_op = decode_ns / ops_f;
    let total_ns_per_op = (encode_ns + decode_ns) / ops_f;
    let total_secs = (encode_ns + decode_ns) / 1e9;
    let entities_per_sec = if total_secs > 0.0 {
        ops_f / total_secs
    } else {
        0.0
    };

    Ok(DeltaBenchReport {
        ops: total_ops,
        encode_ns_per_op,
        decode_ns_per_op,
        total_ns_per_op,
        entities_per_sec,
        bytes_encoded,
    })
}

// -----------------------------------------------------------------------------
// Suite 3: Client Connection + Predictor (5 unacked) + ViewBuilder::build_into
// -----------------------------------------------------------------------------

fn run_client_bench(cfg: &BenchConfig) -> Result<ClientBenchReport, String> {
    const TARGET_UNACKED: usize = 5;
    const TARGET_STUDIO_PLAYERS: usize = 32;
    const TARGET_TEMPENTS: usize = 64;
    const TARGET_DECALS: usize = 24;

    let vfs = discover_vfs(cfg)?;
    let (bsp_arc, collision) = if let Some(ref v) = vfs {
        let map = BspMap::load(v, &cfg.map)
            .map_err(|e| format!("failed to load BSP `{}` for client bench: {e}", cfg.map))?;
        let coll = Arc::new(WorldCollision::from_bsp(&map));
        (Some(Arc::new(map)), coll)
    } else {
        (None, Arc::new(NO_COLLISION.clone()))
    };

    let (mut server, map_source) = create_bench_server(cfg, vfs.as_ref(), 32)?;
    let hub = LoopbackHub::new();
    let endpoint = hub.connect();
    let mut net = MeasuringTransport::new(hub);
    let mut conn = Connection::new(
        endpoint,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("bench-client", 76_561_197_960_265_811, &md5_hex(b"cli")),
    );
    conn.set_userinfo("cl_updaterate", "20");
    conn.set_userinfo("rate", "30000");
    conn.connect();

    let _ = server.console_command("bot_quota 31");
    let mut now = 0.0;
    for _ in 0..400 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
        if conn.state() == &ConnState::InGame {
            break;
        }
    }
    if conn.state() != &ConnState::InGame {
        return Err("client benchmark loopback client failed to reach InGame".to_string());
    }

    // Join CT team and pump until bots join and initial clientdata + frame arrive.
    conn.send_stringcmd("jointeam 2");
    conn.send_stringcmd("joinclass 1");
    for _ in 0..120 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
        if conn.frames().valid_sequence != 0
            && conn.client_data().is_some()
            && server.fake_client_count() >= 31
        {
            break;
        }
    }
    // Drain any final datagram queued by the last `server.frame` before injecting the
    // deterministic 32-player snapshot so subsequent `conn.pump` calls do not overwrite it.
    drop(server);
    conn.pump(now);

    let mut view = ViewBuilder::new();
    view.set_world_context(Some(Arc::clone(&collision)), vfs.clone());
    view.set_map_bsp(bsp_arc.clone());
    view.set_map_bounds(Some(&collision));
    conn.set_world_collision(Some(Arc::clone(&collision)));
    conn.set_fx_world(Box::new(MapFxWorld::new(
        Some(Arc::clone(&collision)),
        vfs.clone(),
        view.names(),
    )));

    // Resolve a studio player model index from the precache table.
    let precache_snapshot = conn.precache().clone();
    let player_mdl_idx = precache_snapshot
        .models
        .iter()
        .find(|(_, e)| e.name.eq_ignore_ascii_case("models/player.mdl"))
        .or_else(|| {
            precache_snapshot
                .models
                .iter()
                .find(|(_, e)| e.name.to_ascii_lowercase().ends_with(".mdl"))
        })
        .map(|(&idx, _)| idx)
        .ok_or("no precached .mdl model found for client benchmark")?;

    let base_origin = bsp_arc
        .as_ref()
        .and_then(|m| m.spawns.first().map(|s| s.position + Vec3::new(0.0, 0.0, 36.0)))
        .or_else(|| conn.client_data().map(|c| c.origin))
        .unwrap_or(Vec3::new(0.0, 0.0, 64.0));

    // Feed a deterministic full snapshot containing the local player + 32 studio players so
    // ViewBuilder::build processes all 32 studio models regardless of map PVS.
    let mut ctx = SvcParseContext::new();
    ctx.deltas = DeltaRegistry::cs16_default();
    ctx.max_clients = 32;
    let player_schema = ctx
        .deltas
        .get("entity_state_player_t")
        .ok_or("missing entity_state_player_t schema")?
        .clone();
    let ent_schema = ctx
        .deltas
        .get("entity_state_t")
        .ok_or("missing entity_state_t schema")?
        .clone();
    let cd_schema = ctx
        .deltas
        .get("clientdata_t")
        .ok_or("missing clientdata_t schema")?
        .clone();

    let mut cd = conn.client_data().cloned().unwrap_or_default();
    cd.origin = base_origin;
    cd.velocity = Vec3::new(120.0, 0.0, 0.0);
    cd.view_ofs = Vec3::new(0.0, 0.0, 17.0);
    cd.health = 100.0;
    cd.deadflag = 0;
    cd.flags = 1 << 9; // FL_ONGROUND
    cd.weapons |= 1 << 31; // WEAPON_SUIT
    cd.maxspeed = 250.0;
    cd.fov = 90.0;

    let mut entities = BTreeMap::new();
    let local_num = conn.player_slot().map_or(1u16, |s| u16::from(s) + 1);
    let local_state = GoldSrcEntityState {
        number: local_num,
        entity_type: 1,
        origin: base_origin,
        angles: Vec3::ZERO,
        modelindex: player_mdl_idx,
        sequence: 4,
        gaitsequence: 4,
        framerate: 1.0,
        movetype: 3,
        solid: 3,
        mins: Vec3::new(-16.0, -16.0, -36.0),
        maxs: Vec3::new(16.0, 16.0, 36.0),
        friction: 1.0,
        gravity: 1.0,
        ..GoldSrcEntityState::default()
    };
    entities.insert(local_num, (1u8, local_state.to_delta(&player_schema)));

    let mut placed = 0usize;
    let mut ent_num = 1u16;
    while placed < TARGET_STUDIO_PLAYERS {
        if ent_num == local_num {
            ent_num += 1;
            continue;
        }
        let ang = (placed as f32) * (std::f32::consts::TAU / TARGET_STUDIO_PLAYERS as f32);
        let radius = 140.0 + ((placed % 3) as f32) * 55.0;
        let st = GoldSrcEntityState {
            number: ent_num,
            entity_type: 1,
            origin: base_origin + Vec3::new(ang.cos() * radius, ang.sin() * radius, 0.0),
            angles: Vec3::new(0.0, ang.to_degrees(), 0.0),
            modelindex: player_mdl_idx,
            sequence: 4,
            gaitsequence: 4,
            frame: (placed as f32) * 2.0,
            animtime: now as f32,
            framerate: 1.0,
            movetype: 3,
            solid: 3,
            renderamt: 255,
            scale: 1.0,
            mins: Vec3::new(-16.0, -16.0, -36.0),
            maxs: Vec3::new(16.0, 16.0, 36.0),
            colormap: ent_num,
            controller: [127; 4],
            blending: [128, 0],
            friction: 1.0,
            gravity: 1.0,
            ..GoldSrcEntityState::default()
        };
        let schema = if ent_num <= 32 {
            &player_schema
        } else {
            &ent_schema
        };
        entities.insert(ent_num, (1u8, st.to_delta(schema)));
        placed += 1;
        ent_num += 1;
    }

    let synth_seq = conn.frames().parsecount + 1;
    let cd_msg = SvcMessage::ClientData {
        delta_seq: None,
        record: cd.to_delta(&cd_schema),
        parsed: cd,
        weapon_deltas: Vec::new(),
    };
    conn.frames_mut()
        .handle(&cd_msg, &ctx, &precache_snapshot, synth_seq, now);
    let pkt_msg = SvcMessage::PacketEntities(Box::new(PacketEntitiesMsg {
        is_delta: false,
        delta_from_seq: None,
        expected_count: entities.len() as u16,
        entities,
        removed: Vec::new(),
    }));
    conn.frames_mut()
        .handle(&pkt_msg, &ctx, &precache_snapshot, synth_seq, now);

    // Seed 64 active tempents: 3 smoke grenade clouds (60 puffs) + 4 persistent studio shells.
    conn.fx_mut().tempents.clear();
    let mut fx_rng = FxRandom::new(0x16C5_0016);
    let smoke_cfg = SmokeCloudConfig {
        light_smoke: false,
        max_smokepuffs: 120,
        fastsprites: 0,
    };
    for c in 0..3 {
        let offset = Vec3::new(120.0 + (c as f32) * 65.0, ((c as f32) - 1.0) * 70.0, 0.0);
        conn.fx_mut().tempents.spawn_smoke_clouds_ex(
            base_origin + offset,
            smoke_cfg,
            &NoWorld,
            &mut fx_rng,
            now,
        );
    }
    let existing_temps = conn.fx().tempents.active().len();
    for j in existing_temps..TARGET_TEMPENTS {
        let pos = base_origin + Vec3::new(80.0 + (j as f32) * 8.0, -40.0, 12.0);
        if let Some(t) =
            conn.fx_mut()
                .tempents
                .alloc(pos, ModelRef::Precached(player_mdl_idx), &NoWorld, now)
        {
            t.flags = ftent::PERSIST;
            t.die = now + 600.0;
        }
    }

    // Queue 5 unacked movement commands and keep them unacked across all measured frames so
    // `CL_PredictMovement` replays all 5 commands on every single frame.
    conn.set_cvar("cl_cmdrate", "10");
    conn.suppress_moves(true);
    conn.predictor_mut().reset();
    conn.set_world_collision(Some(Arc::clone(&collision)));
    for i in 0..TARGET_UNACKED {
        conn.predictor_mut().push_cmd(GoldSrcUserCmd {
            lerp_msec: 9,
            msec: 10,
            viewangles: Vec3::new(0.0, (i as f32) * 4.0, 0.0),
            forwardmove: 250.0,
            sidemove: 45.0,
            upmove: 0.0,
            lightlevel: 128,
            buttons: 8,
            impulse: 0,
            weaponselect: 0,
            impact_index: 0,
            impact_position: Vec3::ZERO,
        });
    }

    // Keep total elapsed connection clock <= 5.0s so `outgoing_sequence - ack` stays strictly
    // below `UPDATE_MASK` (63) at `cl_cmdrate 10`, even for large `--client-frames` values.
    let warmup_frames = 8usize;
    let total_steps = warmup_frames + cfg.client_frames;
    let conn_dt = (5.0 / (total_steps.max(1) as f64)).min(0.004);
    let mut decal_rng = FxRandom::new(0xDEC4_0024);
    let mut view_angles = Vec3::new(0.0, 15.0, 0.0);

    let mut frame_view = FrameView::default();
    for step in 0..warmup_frames {
        now += conn_dt;
        view_angles.y = (15.0 + (step as f32) * 0.25) % 360.0;
        conn.pump(now);
        let inputs = ViewInputs {
            now,
            frametime: 0.01,
            ..ViewInputs::default()
        };
        let _ = view.build_into(&mut frame_view, &mut conn, view_angles, &inputs);
        black_box(&frame_view);
    }

    let mut samples_us = Vec::with_capacity(cfg.client_frames);
    let mut last_studio = 0usize;
    let mut last_tempents = 0usize;
    let mut last_decals = 0usize;
    let mut tmp_sounds = SoundQueue::default();

    for frame_idx in 0..cfg.client_frames {
        now += conn_dt;
        view_angles.y = (17.0 + (frame_idx as f32) * 0.25) % 360.0;

        // Enqueue 24 decals for `ViewBuilder::build_into` to process on this frame.
        tmp_sounds.clear();
        for d in 0..TARGET_DECALS {
            let ang = ((frame_idx + d) as f32) * (std::f32::consts::TAU / TARGET_DECALS as f32);
            let pos = base_origin + Vec3::new(ang.cos() * 96.0, ang.sin() * 96.0, -32.0);
            conn.fx_mut().tempents.spawn_effect(
                &TempEffect::WorldDecal {
                    position: pos,
                    decal: (d as u8) % 8,
                },
                now,
                &precache_snapshot,
                &NoWorld,
                &mut decal_rng,
                &mut tmp_sounds,
            );
        }

        let inputs = ViewInputs {
            now,
            frametime: 0.01,
            ..ViewInputs::default()
        };

        let t0 = Instant::now();
        conn.pump(now);
        let _ = view.build_into(&mut frame_view, &mut conn, view_angles, &inputs);
        let elapsed_us = t0.elapsed().as_secs_f64() * 1_000_000.0;

        last_studio = frame_view.stats.studio;
        last_tempents = frame_view.stats.tempents;
        last_decals = frame_view.decals.len();
        black_box(&frame_view);
        samples_us.push(elapsed_us);
    }

    if conn.predictor().unacked() != TARGET_UNACKED {
        return Err(format!(
            "client bench expected {TARGET_UNACKED} unacked cmds, got {}",
            conn.predictor().unacked()
        ));
    }
    if !conn.predictor().prediction().predicted {
        return Err("client bench expected CL_PredictMovement to run (predicted == false)".into());
    }
    if last_studio < TARGET_STUDIO_PLAYERS {
        return Err(format!(
            "client bench expected >= {TARGET_STUDIO_PLAYERS} studio players, got {last_studio}"
        ));
    }
    if last_tempents < TARGET_TEMPENTS {
        return Err(format!(
            "client bench expected >= {TARGET_TEMPENTS} tempents, got {last_tempents}"
        ));
    }
    if last_decals != TARGET_DECALS {
        return Err(format!(
            "client bench expected {TARGET_DECALS} decals, got {last_decals}"
        ));
    }

    let (cpu_avg_us, cpu_p50_us, cpu_p95_us, cpu_p99_us, cpu_max_us) =
        summarize_samples(samples_us);

    Ok(ClientBenchReport {
        map: cfg.map.clone(),
        map_source,
        frames: cfg.client_frames,
        unacked_cmds: TARGET_UNACKED,
        studio_players: last_studio,
        tempents: last_tempents,
        decals: last_decals,
        cpu_avg_us,
        cpu_p50_us,
        cpu_p95_us,
        cpu_p99_us,
        cpu_max_us,
    })
}

// -----------------------------------------------------------------------------
// Suite 4: Headless 1280x800 GPU Frame Benchmark (de_dust2 + 16 MDL + 3 smokes)
// -----------------------------------------------------------------------------

fn run_gpu_bench(cfg: &BenchConfig) -> Result<GpuBenchReport, String> {
    const WIDTH: u32 = 1280;
    const HEIGHT: u32 = 800;
    const STUDIO_COUNT: usize = 16;
    const SMOKE_CLOUDS: usize = 3;
    const PUFFS_PER_CLOUD: usize = 15;
    const SMOKE_SPRITE_COUNT: usize = SMOKE_CLOUDS * PUFFS_PER_CLOUD;

    let Some(vfs) = discover_vfs(cfg)? else {
        return Err(
            "gpu benchmark requires Half-Life/CS 1.6 assets (remove --stub or set CS16_GAMEDIR / --gamedir)"
                .to_string(),
        );
    };

    let map = BspMap::load(&vfs, &cfg.map)
        .map_err(|e| format!("failed to load BSP `{}` for gpu bench: {e}", cfg.map))?;
    let sky = SkyboxTextures::load(&vfs, &map.skyname);

    let mut renderer = Renderer::new_headless(WIDTH, HEIGHT)?;
    let adapter = renderer.adapter_name.clone();
    let backend = renderer.backend_name.clone();
    renderer.load_map(&map, &sky);

    let world_name = format!("maps/{}.bsp", cfg.map);
    let model_names = [
        "",
        world_name.as_str(),
        "models/player/urban/urban.mdl",
        "models/player/leet/leet.mdl",
        "models/player/sas/sas.mdl",
        "models/player/terror/terror.mdl",
        "models/p_ak47.mdl",
        "sprites/gas_puff_01.spr",
    ];
    let registry = ModelRegistry::from_precache(&model_names);
    let player_handles = [
        registry.resolve(2).ok_or("urban.mdl handle missing")?,
        registry.resolve(3).ok_or("leet.mdl handle missing")?,
        registry.resolve(4).ok_or("sas.mdl handle missing")?,
        registry.resolve(5).ok_or("terror.mdl handle missing")?,
    ];
    let weapon_handle = vfs
        .exists("models/p_ak47.mdl")
        .then(|| registry.resolve(6))
        .flatten();
    let smoke_handle = registry.resolve(7).ok_or("gas_puff_01.spr handle missing")?;
    renderer.set_model_registry(registry);

    let (cam_pos, base_yaw) = map
        .spawns
        .first()
        .map(|s| (s.position + Vec3::new(0.0, 0.0, 64.0), s.yaw_deg))
        .unwrap_or((Vec3::new(0.0, 0.0, 64.0), 0.0));
    let mut camera = Camera {
        position: cam_pos,
        yaw_deg: base_yaw,
        pitch_deg: 2.0,
        roll_deg: 0.0,
    };
    let cam_fwd = camera.forward();
    let cam_right = camera.right();

    // Place 16 studio player models + 3 active smoke grenade clouds (45 gas_puff_01.spr sprites)
    // directly in front of the camera so every model and puff renders every frame.
    let mut base_entities = Vec::with_capacity(STUDIO_COUNT + SMOKE_SPRITE_COUNT);
    for i in 0..STUDIO_COUNT {
        let row = (i / 8) as f32;
        let col = (i % 8) as f32 - 3.5;
        let dist = 190.0 + row * 105.0;
        let lateral = col * 42.0;
        let origin = cam_pos + cam_fwd * dist + cam_right * lateral + Vec3::new(0.0, 0.0, -28.0);
        let mut ent = RenderEntity::new((i + 1) as u16, player_handles[i % player_handles.len()], origin);
        ent.angles = Vec3::new(0.0, (base_yaw + 180.0 + col * 10.0).rem_euclid(360.0), 0.0);
        ent.player = true;
        ent.sequence = 4;
        ent.gait = Some((4, (i as f32) * 3.5));
        ent.framerate = 1.0;
        ent.attached = weapon_handle;
        base_entities.push(ent);
    }

    let cloud_offsets = [(150.0f32, -75.0f32), (215.0, 5.0), (275.0, 80.0)];
    let mut rng = FxRandom::new(0x500C_E045);
    let mut next_idx = (STUDIO_COUNT + 1) as u16;
    for &(dist, lateral) in &cloud_offsets {
        let center = cam_pos + cam_fwd * dist + cam_right * lateral + Vec3::new(0.0, 0.0, -16.0);
        for p in 0..PUFFS_PER_CLOUD {
            let mut pos = center;
            if p != 0 {
                let ang = rng.float(0.0, std::f32::consts::TAU);
                let rad = rng.float(15.0, 68.0);
                pos.x += ang.cos() * rad;
                pos.y += ang.sin() * rad;
                pos.z += rng.float(6.0, 48.0);
            } else {
                pos.z += 24.0;
            }
            let c = rng.long(105, 140) as u8;
            let mut spr = RenderEntity::new(next_idx, smoke_handle, pos);
            spr.rendermode = EntRenderMode::TransAlpha;
            spr.renderamt = 255;
            spr.rendercolor = [c, c, c];
            spr.scale = 2.9 + rng.float(-0.2, 0.25);
            spr.frame = 0.0;
            base_entities.push(spr);
            next_idx += 1;
        }
    }

    let dyn_data = SceneDynamicData {
        vfs: Some(&vfs),
        ..SceneDynamicData::default()
    };

    // Warmup frames so shaders, textures, MDL/SPR GPU caches, and PVS buffers are resident.
    let warmup_frames = 5usize;
    for i in 0..warmup_frames {
        let t = (i as f32) * 0.01;
        let settings = RenderSettings {
            time_sec: t,
            ..RenderSettings::default()
        };
        let view = ViewBasis::from_angles(
            camera.position,
            Vec3::new(camera.pitch_deg, camera.yaw_deg, camera.roll_deg),
        );
        renderer.set_entity_scene(Some(SceneFrame {
            time: t,
            view,
            entities: base_entities.clone(),
            shadows: Vec::new(),
        }));
        let _ = renderer.render_frame(
            &camera,
            &settings,
            &dyn_data,
            egui::TexturesDelta::default(),
            &[],
            1.0,
            None,
        );
        let _ = renderer.device.poll(wgpu::PollType::wait_indefinitely());
    }

    let mut frame_samples_ms = Vec::with_capacity(cfg.gpu_frames);
    let mut sum_draw_calls = 0u64;
    let mut sum_bsp_calls = 0u64;
    let mut sum_mdl_calls = 0u64;
    let mut sum_sprite_calls = 0u64;
    let mut sum_bytes_uploaded = 0u64;

    for i in 0..cfg.gpu_frames {
        let t = ((i + warmup_frames) as f32) * 0.01;
        // Subtle yaw oscillation keeps the camera in the same PVS leaf while updating matrices.
        camera.yaw_deg = (base_yaw + (t * 2.0).sin() * 1.5).rem_euclid(360.0);
        let settings = RenderSettings {
            time_sec: t,
            ..RenderSettings::default()
        };
        let view = ViewBasis::from_angles(
            camera.position,
            Vec3::new(camera.pitch_deg, camera.yaw_deg, camera.roll_deg),
        );
        let scene_frame = SceneFrame {
            time: t,
            view,
            entities: base_entities.clone(),
            shadows: Vec::new(),
        };
        renderer.set_entity_scene(Some(scene_frame));

        let t0 = Instant::now();
        let _ = renderer.render_frame(
            &camera,
            &settings,
            &dyn_data,
            egui::TexturesDelta::default(),
            &[],
            1.0,
            None,
        );
        let _ = renderer.device.poll(wgpu::PollType::wait_indefinitely());
        frame_samples_ms.push(t0.elapsed().as_secs_f64() * 1000.0);

        let st = renderer.last_frame_stats();
        sum_draw_calls += st.draw_calls as u64;
        sum_bsp_calls += st.bsp_draw_calls as u64;
        sum_mdl_calls += st.mdl_draw_calls as u64;
        sum_sprite_calls += st.sprite_draw_calls as u64;
        sum_bytes_uploaded += st.bytes_uploaded;
    }

    let (frame_avg_ms, frame_p50_ms, frame_p95_ms, frame_p99_ms, frame_max_ms) =
        summarize_samples(frame_samples_ms);
    let nf = cfg.gpu_frames.max(1) as f64;

    Ok(GpuBenchReport {
        adapter,
        backend,
        map: cfg.map.clone(),
        width: WIDTH,
        height: HEIGHT,
        frames: cfg.gpu_frames,
        studio_models: STUDIO_COUNT,
        smoke_sprites: SMOKE_SPRITE_COUNT,
        frame_avg_ms,
        frame_p50_ms,
        frame_p95_ms,
        frame_p99_ms,
        frame_max_ms,
        draw_calls_per_frame: sum_draw_calls as f64 / nf,
        bsp_draw_calls: sum_bsp_calls as f64 / nf,
        mdl_draw_calls: sum_mdl_calls as f64 / nf,
        sprite_draw_calls: sum_sprite_calls as f64 / nf,
        bytes_uploaded_per_frame: sum_bytes_uploaded as f64 / nf,
    })
}

fn format_report_json(r: &BenchReport) -> String {
    let mut out = format!("{{\"profile\":\"{}\"", r.profile);
    if let Some(ref s) = r.server {
        let _ = write!(
            out,
            ",\"server\":{{\"map\":\"{}\",\"map_source\":\"{}\",\"players\":{},\"ticks\":{},\"startup_ms\":{:.3},\"tick_avg_ms\":{:.4},\"tick_p50_ms\":{:.4},\"tick_p95_ms\":{:.4},\"tick_p99_ms\":{:.4},\"tick_max_ms\":{:.4},\"delta_pkt_count\":{},\"delta_pkt_avg_bytes\":{:.1},\"delta_pkt_max_bytes\":{},\"bandwidth_kbps_per_client\":{:.2}}}",
            json_escape(&s.map),
            s.map_source,
            s.players,
            s.ticks,
            s.startup_ms,
            s.tick_avg_ms,
            s.tick_p50_ms,
            s.tick_p95_ms,
            s.tick_p99_ms,
            s.tick_max_ms,
            s.delta_pkt_count,
            s.delta_pkt_avg_bytes,
            s.delta_pkt_max_bytes,
            s.bandwidth_kbps_per_client,
        );
    }
    if let Some(ref d) = r.delta {
        let _ = write!(
            out,
            ",\"delta\":{{\"ops\":{},\"encode_ns_per_op\":{:.2},\"decode_ns_per_op\":{:.2},\"total_ns_per_op\":{:.2},\"entities_per_sec\":{:.1},\"bytes_encoded\":{}}}",
            d.ops,
            d.encode_ns_per_op,
            d.decode_ns_per_op,
            d.total_ns_per_op,
            d.entities_per_sec,
            d.bytes_encoded,
        );
    }
    if let Some(ref c) = r.client {
        let _ = write!(
            out,
            ",\"client\":{{\"map\":\"{}\",\"map_source\":\"{}\",\"frames\":{},\"unacked_cmds\":{},\"studio_players\":{},\"tempents\":{},\"decals\":{},\"cpu_avg_us\":{:.2},\"cpu_p50_us\":{:.2},\"cpu_p95_us\":{:.2},\"cpu_p99_us\":{:.2},\"cpu_max_us\":{:.2}}}",
            json_escape(&c.map),
            c.map_source,
            c.frames,
            c.unacked_cmds,
            c.studio_players,
            c.tempents,
            c.decals,
            c.cpu_avg_us,
            c.cpu_p50_us,
            c.cpu_p95_us,
            c.cpu_p99_us,
            c.cpu_max_us,
        );
    }
    if let Some(ref g) = r.gpu {
        let _ = write!(
            out,
            ",\"gpu\":{{\"adapter\":\"{}\",\"backend\":\"{}\",\"map\":\"{}\",\"width\":{},\"height\":{},\"frames\":{},\"studio_models\":{},\"smoke_sprites\":{},\"frame_avg_ms\":{:.4},\"frame_p50_ms\":{:.4},\"frame_p95_ms\":{:.4},\"frame_p99_ms\":{:.4},\"frame_max_ms\":{:.4},\"draw_calls_per_frame\":{:.1},\"bsp_draw_calls\":{:.1},\"mdl_draw_calls\":{:.1},\"sprite_draw_calls\":{:.1},\"bytes_uploaded_per_frame\":{:.1}}}",
            json_escape(&g.adapter),
            json_escape(&g.backend),
            json_escape(&g.map),
            g.width,
            g.height,
            g.frames,
            g.studio_models,
            g.smoke_sprites,
            g.frame_avg_ms,
            g.frame_p50_ms,
            g.frame_p95_ms,
            g.frame_p99_ms,
            g.frame_max_ms,
            g.draw_calls_per_frame,
            g.bsp_draw_calls,
            g.mdl_draw_calls,
            g.sprite_draw_calls,
            g.bytes_uploaded_per_frame,
        );
    }
    out.push('}');
    out
}

/// CLI entrypoint for `cargo xtask bench`.
pub fn run_bench(args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let cfg = parse_bench_args(args)?;
    let r = run_bench_with_config(&cfg)?;
    if cfg.json {
        println!("{}", format_report_json(&r));
    } else {
        println!("xtask bench (profile: {})", r.profile);
        if let Some(ref s) = r.server {
            println!(
                "[suite: server] map: {} [{}], players: {}, ticks: {}",
                s.map, s.map_source, s.players, s.ticks
            );
            println!("  startup_readiness_ms   : {:.3} ms", s.startup_ms);
            println!("  server_tick_avg_ms     : {:.4} ms", s.tick_avg_ms);
            println!(
                "  server_tick_p50/95/99  : {:.4} / {:.4} / {:.4} ms (max {:.4} ms)",
                s.tick_p50_ms, s.tick_p95_ms, s.tick_p99_ms, s.tick_max_ms
            );
            println!(
                "  delta_snapshot_packets : {} pkts, avg {:.1} B, max {} B ({:.2} kbps @ cl_updaterate 20)",
                s.delta_pkt_count,
                s.delta_pkt_avg_bytes,
                s.delta_pkt_max_bytes,
                s.bandwidth_kbps_per_client
            );
        }
        if let Some(ref d) = r.delta {
            println!("[suite: delta] ops: {} (player/entity/clientdata/usercmd)", d.ops);
            println!(
                "  encode / decode / sum  : {:.2} / {:.2} / {:.2} ns/op",
                d.encode_ns_per_op, d.decode_ns_per_op, d.total_ns_per_op
            );
            println!(
                "  throughput             : {:.1} entities/sec ({} B wire)",
                d.entities_per_sec, d.bytes_encoded
            );
        }
        if let Some(ref c) = r.client {
            println!(
                "[suite: client] map: {} [{}], frames: {}, unacked: {}, studio: {}, tempents: {}, decals: {}",
                c.map, c.map_source, c.frames, c.unacked_cmds, c.studio_players, c.tempents, c.decals
            );
            println!("  client_cpu_avg_us      : {:.2} us/frame", c.cpu_avg_us);
            println!(
                "  client_cpu_p50/95/99   : {:.2} / {:.2} / {:.2} us/frame (max {:.2} us)",
                c.cpu_p50_us, c.cpu_p95_us, c.cpu_p99_us, c.cpu_max_us
            );
        }
        if let Some(ref g) = r.gpu {
            println!(
                "[suite: gpu] map: {}, {}x{}, frames: {}, studio: {}, smokes: {} sprites ({} / {})",
                g.map,
                g.width,
                g.height,
                g.frames,
                g.studio_models,
                g.smoke_sprites,
                g.adapter,
                g.backend
            );
            println!("  gpu_frame_avg_ms       : {:.4} ms", g.frame_avg_ms);
            println!(
                "  gpu_frame_p50/95/99    : {:.4} / {:.4} / {:.4} ms (max {:.4} ms)",
                g.frame_p50_ms, g.frame_p95_ms, g.frame_p99_ms, g.frame_max_ms
            );
            println!(
                "  draw_calls_per_frame   : {:.1} (bsp {:.1}, mdl {:.1}, sprite {:.1})",
                g.draw_calls_per_frame, g.bsp_draw_calls, g.mdl_draw_calls, g.sprite_draw_calls
            );
            println!(
                "  bytes_uploaded/frame   : {:.1} B ({:.2} KiB)",
                g.bytes_uploaded_per_frame,
                g.bytes_uploaded_per_frame / 1024.0
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bench_flags_and_rejects_invalid() {
        let args = vec![
            "--suite".into(),
            "delta".into(),
            "--gpu".into(),
            "--ticks".into(),
            "50".into(),
            "--delta-ops".into(),
            "1000".into(),
            "--client-frames".into(),
            "25".into(),
            "--gpu-frames".into(),
            "10".into(),
            "--players".into(),
            "32".into(),
            "--map".into(),
            "cs_office".into(),
            "--stub".into(),
            "--json".into(),
        ];
        let cfg = parse_bench_args(args.into_iter()).unwrap();
        assert_eq!(cfg.suite, BenchSuite::Delta);
        assert!(cfg.gpu);
        assert!(cfg.runs_delta());
        assert!(cfg.runs_gpu());
        assert!(!cfg.runs_server());
        assert!(!cfg.runs_client());
        assert_eq!(cfg.ticks, 50);
        assert_eq!(cfg.delta_ops, 1000);
        assert_eq!(cfg.client_frames, 25);
        assert_eq!(cfg.gpu_frames, 10);
        assert_eq!(cfg.players, 32);
        assert_eq!(cfg.map, "cs_office");
        assert!(cfg.stub);
        assert!(cfg.json);

        for bad in [
            vec!["--suite", "bogus"],
            vec!["--players", "0"],
            vec!["--players", "33"],
            vec!["--ticks", "0"],
            vec!["--delta-ops", "0"],
            vec!["--client-frames", "0"],
            vec!["--gpu-frames", "0"],
            vec!["--unknown"],
        ] {
            let owned: Vec<String> = bad.into_iter().map(String::from).collect();
            assert!(parse_bench_args(owned.into_iter()).is_err());
        }
    }

    #[test]
    fn bench_smoke_measures_32_player_ticks_and_delta_packets() {
        let cfg = BenchConfig {
            suite: BenchSuite::All,
            gpu: false,
            ticks: 20,
            players: 32,
            map: "de_dust2".to_string(),
            gamedir: None,
            stub: true,
            json: false,
            delta_ops: 400,
            client_frames: 20,
            gpu_frames: 5,
        };
        let report = run_bench_with_config(&cfg).unwrap();
        let s = report.server.as_ref().expect("server report");
        assert_eq!(s.players, 32);
        assert_eq!(s.ticks, 20);
        assert!(s.startup_ms >= 0.0);
        assert!(s.tick_avg_ms > 0.0);
        assert!(s.delta_pkt_count > 0);
        assert!(s.delta_pkt_avg_bytes > 0.0);

        let d = report.delta.as_ref().expect("delta report");
        assert_eq!(d.ops, 400);
        assert!(d.total_ns_per_op > 0.0);
        assert!(d.entities_per_sec > 0.0);
        assert!(d.bytes_encoded > 0);

        let c = report.client.as_ref().expect("client report");
        assert_eq!(c.frames, 20);
        assert_eq!(c.unacked_cmds, 5);
        assert!(c.studio_players >= 32);
        assert!(c.tempents >= 64);
        assert_eq!(c.decals, 24);
        assert!(c.cpu_avg_us > 0.0);

        assert!(report.gpu.is_none());
        let json = format_report_json(&report);
        assert!(json.contains("\"server\":"));
        assert!(json.contains("\"delta\":"));
        assert!(json.contains("\"client\":"));
    }
}
