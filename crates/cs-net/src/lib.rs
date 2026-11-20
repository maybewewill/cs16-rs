//! Compact UDP networking protocol (`postcard`), client-side prediction reconciliation,
//! and server-side lag-compensation history ring buffer.

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use cs_assets::bsp::SpawnTeam;
use cs_sim::grenades::{ActiveGrenade, SmokeCloud};
use cs_sim::pmove::{MoveParams, PlayerMoveState, step_player_move};
use cs_sim::rules::{
    BulletTracer, ChatMessage, Corpse, HostageState, KillFeedEntry, MatchWorld, PlantedC4,
    PlayerInput, PlayerState, RoundPhase, SimEvent, TICK_DT, WorldWeapon,
};
use cs_sim::trace::BspCollision;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

pub const DEFAULT_PORT: u16 = 27015;
pub const MAX_PACKET_SIZE: usize = 65000;
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum NetError {
    #[error("network I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Codec(#[from] postcard::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanServerInfo {
    pub addr: String,
    pub hostname: String,
    pub map_name: String,
    pub players: usize,
    pub max_players: usize,
    pub ping_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientPacket {
    Hello {
        name: String,
        preferred_team: Option<SpawnTeam>,
    },
    UserCmd {
        seq: u32,
        input: PlayerInput,
    },
    Buy(String),
    ChooseTeam(SpawnTeam),
    Chat {
        team_only: bool,
        text: String,
    },
    LanPing,
    Disconnect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub tick: u64,
    pub ack_seq: u32,
    pub map_name: String,
    pub phase: RoundPhase,
    pub phase_timer: f32,
    pub round_number: u32,
    pub ct_score: u32,
    pub t_score: u32,
    pub players: Vec<PlayerState>,
    pub planted_c4: Option<PlantedC4>,
    pub grenades: Vec<ActiveGrenade>,
    pub smokes: Vec<SmokeCloud>,
    pub dropped_weapons: Vec<WorldWeapon>,
    pub corpses: Vec<Corpse>,
    pub hostages: Vec<HostageState>,
    pub broken_models: Vec<usize>,
    pub door_offsets: Vec<(usize, Vec3)>,
    pub tracers: Vec<BulletTracer>,
    pub killfeed: Vec<KillFeedEntry>,
    pub chat: Vec<ChatMessage>,
    pub banner: Option<(String, f32)>,
}

impl WorldSnapshot {
    pub fn capture(world: &MatchWorld, ack_seq: u32) -> Self {
        Self {
            tick: world.tick,
            ack_seq,
            map_name: world.map_name.clone(),
            phase: world.phase,
            phase_timer: world.phase_timer,
            round_number: world.round_number,
            ct_score: world.ct_score,
            t_score: world.t_score,
            players: world.players.clone(),
            planted_c4: world.planted_c4.clone(),
            grenades: world.grenades.clone(),
            smokes: world.smokes.clone(),
            dropped_weapons: world.dropped_weapons.clone(),
            corpses: world.corpses.clone(),
            hostages: world.hostages.clone(),
            broken_models: world.broken_models.clone(),
            door_offsets: world.door_offsets.clone(),
            tracers: world.tracers.clone(),
            killfeed: world.killfeed.clone(),
            chat: world.chat.clone(),
            banner: world.banner.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerPacket {
    Welcome {
        player_id: u32,
        map_name: String,
    },
    Snapshot(Box<WorldSnapshot>),
    Events {
        tick: u64,
        events: Vec<SimEvent>,
    },
    LanInfo(LanServerInfo),
    Kicked(String),
}

/// Probe localhost & LAN broadcast ports (`27015..27018`) for running `cs-server` instances.
pub fn discover_lan_servers(timeout: Duration) -> Vec<LanServerInfo> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(20)));
    let Ok(ping) = postcard::to_allocvec(&ClientPacket::LanPing) else {
        return Vec::new();
    };

    let start = Instant::now();
    for port in 27015..=27018u16 {
        let _ = sock.send_to(&ping, format!("127.0.0.1:{port}"));
        let _ = sock.send_to(&ping, format!("255.255.255.255:{port}"));
    }

    let mut found: HashMap<String, LanServerInfo> = HashMap::new();
    let mut buf = [0u8; 2048];
    while start.elapsed() < timeout {
        match sock.recv_from(&mut buf) {
            Ok((len, src)) => {
                if let Ok(ServerPacket::LanInfo(mut info)) =
                    postcard::from_bytes::<ServerPacket>(&buf[..len])
                {
                    info.addr = src.to_string();
                    info.ping_ms = start.elapsed().as_millis().max(1) as u32;
                    found.insert(info.addr.clone(), info);
                }
            }
            Err(_) => continue,
        }
    }
    found.into_values().collect()
}

/// Client-side `pm_shared` prediction & reconciliation buffer.
#[derive(Debug, Default)]
pub struct PredictionBuffer {
    pending: VecDeque<(u32, PlayerInput)>,
    next_seq: u32,
}

impl PredictionBuffer {
    pub fn record_command(&mut self, input: PlayerInput) -> u32 {
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        let seq = self.next_seq;
        self.pending.push_back((seq, input));
        if self.pending.len() > 128 {
            self.pending.pop_front();
        }
        seq
    }

    /// Reconcile local `PlayerMoveState` against the server's authoritative state at `ack_seq`
    /// by replaying all unacknowledged commands through `step_player_move`.
    pub fn reconcile(
        &mut self,
        auth_mv: PlayerMoveState,
        ack_seq: u32,
        weapon_max_speed: f32,
        params: &MoveParams,
        collision: &BspCollision,
    ) -> PlayerMoveState {
        while let Some(&(seq, _)) = self.pending.front() {
            if seq <= ack_seq {
                self.pending.pop_front();
            } else {
                break;
            }
        }
        let mut predicted = auth_mv;
        for &(_, inp) in &self.pending {
            let _ = step_player_move(
                &mut predicted,
                &inp.move_cmd,
                weapon_max_speed,
                params,
                collision,
                TICK_DT,
            );
        }
        predicted
    }
}

pub type LagPose = (u32, Vec3, f32, bool);

/// Server-side 250 ms lag-compensation pose history (25 snapshots at 100 Hz).
#[derive(Debug, Default)]
pub struct LagCompHistory {
    frames: VecDeque<(u64, Vec<LagPose>)>,
}

impl LagCompHistory {
    pub fn record(&mut self, world: &MatchWorld) {
        let poses: Vec<LagPose> = world
            .players
            .iter()
            .filter(|p| p.alive)
            .map(|p| (p.id, p.mv.origin, p.yaw_deg, p.mv.ducking))
            .collect();
        self.frames.push_back((world.tick, poses));
        if self.frames.len() > 25 {
            self.frames.pop_front();
        }
    }

    pub fn poses_at_tick(&self, target_tick: u64) -> Option<&[LagPose]> {
        self.frames
            .iter()
            .min_by_key(|(t, _)| t.abs_diff(target_tick))
            .map(|(_, poses)| poses.as_slice())
    }
}

/// Non-blocking UDP client used by `cs-client` when connected to a dedicated `cs-server`.
pub struct NetClient {
    socket: UdpSocket,
    server_addr: SocketAddr,
    pub player_id: Option<u32>,
    pub prediction: PredictionBuffer,
    rx_buf: Vec<u8>,
}

impl NetClient {
    pub fn connect(addr: &str, player_name: &str) -> Result<Self, NetError> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let server_addr: SocketAddr = addr
            .parse()
            .or_else(|_| format!("{addr}:{DEFAULT_PORT}").parse())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        let client = Self {
            socket,
            server_addr,
            player_id: None,
            prediction: PredictionBuffer::default(),
            rx_buf: vec![0u8; MAX_PACKET_SIZE],
        };
        client.send(&ClientPacket::Hello {
            name: player_name.to_string(),
            preferred_team: None,
        })?;
        Ok(client)
    }

    pub fn send(&self, packet: &ClientPacket) -> Result<(), NetError> {
        let bytes = postcard::to_allocvec(packet)?;
        self.socket.send_to(&bytes, self.server_addr)?;
        Ok(())
    }

    pub fn send_input(&mut self, input: PlayerInput) -> Result<u32, NetError> {
        let seq = self.prediction.record_command(input);
        self.send(&ClientPacket::UserCmd { seq, input })?;
        Ok(seq)
    }

    pub fn poll(&mut self) -> Vec<ServerPacket> {
        let mut packets = Vec::new();
        loop {
            match self.socket.recv_from(&mut self.rx_buf) {
                Ok((len, _)) => {
                    if let Ok(pkt) = postcard::from_bytes::<ServerPacket>(&self.rx_buf[..len]) {
                        if let ServerPacket::Welcome { player_id, .. } = &pkt {
                            self.player_id = Some(*player_id);
                        }
                        packets.push(pkt);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        packets
    }

    pub fn server_addr(&self) -> SocketAddr {
        self.server_addr
    }
}

struct ConnectedClient {
    player_id: u32,
    last_seen: Instant,
    last_seq: u32,
    latest_input: PlayerInput,
}

/// Non-blocking authoritative UDP server driving a `MatchWorld` at 100 Hz.
pub struct NetServer {
    socket: UdpSocket,
    clients: HashMap<SocketAddr, ConnectedClient>,
    pub lag_history: LagCompHistory,
    rx_buf: Vec<u8>,
}

impl NetServer {
    pub fn bind(bind_addr: &str) -> Result<Self, NetError> {
        let socket = UdpSocket::bind(bind_addr)?;
        socket.set_nonblocking(true)?;
        info!(addr = %socket.local_addr()?, "CS 1.6 Rust UDP server listening");
        Ok(Self {
            socket,
            clients: HashMap::new(),
            lag_history: LagCompHistory::default(),
            rx_buf: vec![0u8; MAX_PACKET_SIZE],
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, NetError> {
        Ok(self.socket.local_addr()?)
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Receive all pending client packets, step the authoritative `MatchWorld`, and broadcast snapshots.
    pub fn tick(&mut self, world: &mut MatchWorld, dt: f32) {
        let now = Instant::now();

        // 1. Drain incoming UDP packets
        loop {
            match self.socket.recv_from(&mut self.rx_buf) {
                Ok((len, src)) => {
                    if let Ok(pkt) = postcard::from_bytes::<ClientPacket>(&self.rx_buf[..len]) {
                        self.handle_packet(world, src, pkt, now);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    warn!(%e, "UDP recv error");
                    break;
                }
            }
        }

        // 2. Drop timed-out clients
        let timed_out: Vec<SocketAddr> = self
            .clients
            .iter()
            .filter(|(_, c)| now.duration_since(c.last_seen) > CLIENT_TIMEOUT)
            .map(|(&a, _)| a)
            .collect();
        for addr in timed_out {
            if let Some(c) = self.clients.remove(&addr) {
                world.players.retain(|p| p.id != c.player_id);
                info!(%addr, player_id = c.player_id, "Client timed out");
            }
        }

        // 3. Step authoritative simulation
        let inputs: Vec<(u32, PlayerInput)> = self
            .clients
            .values()
            .map(|c| (c.player_id, c.latest_input))
            .collect();
        world.step(&inputs, dt);
        self.lag_history.record(world);

        // 4. Broadcast events immediately and snapshots every 2 ticks (50 Hz)
        let events = world.take_events();
        if !events.is_empty()
            && let Ok(bytes) = postcard::to_allocvec(&ServerPacket::Events {
                tick: world.tick,
                events,
            })
        {
            for &addr in self.clients.keys() {
                let _ = self.socket.send_to(&bytes, addr);
            }
        }

        if world.tick.is_multiple_of(2) {
            for (&addr, client) in &self.clients {
                let snap = WorldSnapshot::capture(world, client.last_seq);
                if let Ok(bytes) = postcard::to_allocvec(&ServerPacket::Snapshot(Box::new(snap))) {
                    let _ = self.socket.send_to(&bytes, addr);
                }
            }
        }
    }

    fn handle_packet(
        &mut self,
        world: &mut MatchWorld,
        src: SocketAddr,
        pkt: ClientPacket,
        now: Instant,
    ) {
        match pkt {
            ClientPacket::Hello {
                name,
                preferred_team,
            } => {
                let player_id = if let Some(existing) = self.clients.get_mut(&src) {
                    existing.last_seen = now;
                    existing.player_id
                } else {
                    let ct = world
                        .players
                        .iter()
                        .filter(|p| p.team == SpawnTeam::CounterTerrorist)
                        .count();
                    let t = world
                        .players
                        .iter()
                        .filter(|p| p.team == SpawnTeam::Terrorist)
                        .count();
                    let team = preferred_team.unwrap_or(if ct <= t {
                        SpawnTeam::CounterTerrorist
                    } else {
                        SpawnTeam::Terrorist
                    });
                    let pid = world.add_player(name.clone(), team, false);
                    info!(%src, player_id = pid, name = %name, ?team, "Player connected");
                    self.clients.insert(
                        src,
                        ConnectedClient {
                            player_id: pid,
                            last_seen: now,
                            last_seq: 0,
                            latest_input: PlayerInput::default(),
                        },
                    );
                    pid
                };

                if let Ok(bytes) = postcard::to_allocvec(&ServerPacket::Welcome {
                    player_id,
                    map_name: world.map_name.clone(),
                }) {
                    let _ = self.socket.send_to(&bytes, src);
                }
            }
            ClientPacket::UserCmd { seq, input } => {
                if let Some(c) = self.clients.get_mut(&src) {
                    c.last_seen = now;
                    if seq >= c.last_seq {
                        c.last_seq = seq;
                        c.latest_input = input;
                    }
                }
            }
            ClientPacket::Buy(item) => {
                if let Some(c) = self.clients.get_mut(&src) {
                    c.last_seen = now;
                    let _ = world.buy_item(c.player_id, &item);
                }
            }
            ClientPacket::ChooseTeam(team) => {
                if let Some(c) = self.clients.get_mut(&src) {
                    c.last_seen = now;
                    if let Some(p) = world.player_mut(c.player_id) {
                        p.team = team;
                    }
                }
            }
            ClientPacket::Chat { team_only, text } => {
                if let Some(c) = self.clients.get_mut(&src) {
                    c.last_seen = now;
                    if let Some(p) = world.player(c.player_id) {
                        let sender = p.name.clone();
                        let team = if team_only { Some(p.team) } else { None };
                        world.push_chat(sender, team, text);
                    }
                }
            }
            ClientPacket::LanPing => {
                let info = LanServerInfo {
                    addr: self
                        .socket
                        .local_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_default(),
                    hostname: "Counter-Strike 1.6 Rust Dedicated".to_string(),
                    map_name: world.map_name.clone(),
                    players: world.players.len(),
                    max_players: 16,
                    ping_ms: 1,
                };
                if let Ok(bytes) = postcard::to_allocvec(&ServerPacket::LanInfo(info)) {
                    let _ = self.socket.send_to(&bytes, src);
                }
            }
            ClientPacket::Disconnect => {
                if let Some(c) = self.clients.remove(&src) {
                    world.players.retain(|p| p.id != c.player_id);
                    info!(%src, player_id = c.player_id, "Player disconnected");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use cs_assets::bsp::BspMap;
    use cs_assets::materials::MaterialTable;
    use cs_sim::pmove::MoveInputCmd;
    use cs_sim::rules::{MatchConfig, MatchWorld, PlayerInput, TICK_DT};
    use cs_vfs::{Vfs, VfsConfig};

    use super::*;

    #[test]
    fn localhost_udp_handshake_movement_and_snapshot_sync() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        let map = BspMap::load(&vfs, "de_dust2").expect("load de_dust2");
        let mats = MaterialTable::load(&vfs);
        let mut world = MatchWorld::new(
            &map,
            mats,
            MatchConfig {
                freeze_time_sec: 0.0,
                ..Default::default()
            },
        );
        world.set_bot_quota(3);

        let mut server = NetServer::bind("127.0.0.1:0").expect("bind ephemeral UDP port");
        let addr = server.local_addr().expect("local addr").to_string();

        let mut client = NetClient::connect(&addr, "Tester").expect("connect client");
        let mut got_welcome = false;
        let mut got_snapshot = None;

        for step in 0..20 {
            if got_welcome {
                let _ = client.send_input(PlayerInput {
                    move_cmd: MoveInputCmd {
                        forward: 1.0,
                        ..Default::default()
                    },
                    ..Default::default()
                });
            }
            server.tick(&mut world, TICK_DT);
            std::thread::sleep(Duration::from_millis(2));
            for pkt in client.poll() {
                match pkt {
                    ServerPacket::Welcome { map_name, .. } => {
                        assert_eq!(map_name, "de_dust2");
                        got_welcome = true;
                    }
                    ServerPacket::Snapshot(s) => {
                        got_snapshot = Some(s);
                    }
                    _ => {}
                }
            }
            if step > 6 && got_welcome && got_snapshot.is_some() {
                break;
            }
        }

        assert!(got_welcome, "client should receive Welcome packet");
        let snap = got_snapshot.expect("client should receive WorldSnapshot");
        assert_eq!(snap.map_name, "de_dust2");
        assert!(snap.players.len() >= 4);
        assert!(snap.ack_seq >= 1);
    }
}
