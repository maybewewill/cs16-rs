//! Phase 14 Task 11 server/game integration tests:
//! `HealthInfo` and `Account` observer anti-leak rules and `.dem` replay inclusion.

mod common;

use std::collections::HashMap;

use cs_assets::bsp::Entity;
use cs_cvar::CVarRegistry;
use cs_engine::consts::FL_PROXY;
use cs_engine::{EdictId, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::messages::{SvcMessage, SvcParseContext, parse_svc_stream};
use cs_proto48::usermsg::CsUserMsg;
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};

fn entity(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

struct MultiBench {
    server: Server<common::Rig>,
    hub: LoopbackHub,
    probes: Vec<ProbeClient<LoopbackEndpoint>>,
    now: f64,
}

impl MultiBench {
    fn new(slots: u8) -> Self {
        let cfg = ServerConfig {
            max_clients: slots,
            max_edicts: 256,
            max_update_rate: 102,
            ..ServerConfig::default()
        };
        let rig = common::Rig {
            game: CsGame::new(),
            actions: Vec::new(),
            precache_events: Vec::new(),
        };
        let mut entities = vec![entity(&[("classname", "worldspawn")])];
        for i in 0..8 {
            for class in ["info_player_start", "info_player_deathmatch"] {
                let y = if class == "info_player_start" { 0 } else { 500 };
                entities.push(entity(&[
                    ("classname", class),
                    ("origin", &format!("{} {} 1", (i + 1) * 64, y)),
                    ("angles", "0 90 0"),
                ]));
            }
        }
        let map = ServerMap::stub_with_entities("test", entities);
        let server = Server::new(
            cfg,
            map,
            rig,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        Self {
            server,
            hub: LoopbackHub::new(),
            probes: Vec::new(),
            now: 0.0,
        }
    }

    fn join(&mut self, name: &str) -> usize {
        let endpoint = self.hub.connect();
        let cfg = ProbeConfig {
            name: name.to_string(),
            steam_id: 76_561_197_960_265_730 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(name.as_bytes()),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        probe.set_userinfo("cl_updaterate", "60");
        probe.connect();
        self.probes.push(probe);
        let idx = self.probes.len() - 1;
        for _ in 0..600 {
            self.step(1);
            if self.probes[idx].state() == &ProbeState::InGame {
                break;
            }
        }
        assert_eq!(self.probes[idx].state(), &ProbeState::InGame);
        idx
    }

    fn step(&mut self, frames: usize) {
        for _ in 0..frames {
            for p in &mut self.probes {
                p.pump(self.now);
            }
            self.now = ((self.now / 0.01).round() + 1.0) * 0.01;
            self.server.frame(&mut self.hub, self.now);
        }
    }

    fn usermsgs(&self, probe_idx: usize) -> Vec<CsUserMsg> {
        self.probes[probe_idx]
            .messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::UserMsg { decoded, .. } => Some(decoded.clone()),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn health_info_and_account_never_leak_to_live_enemies_and_reach_observers_and_demo_replay() {
    let mut b = MultiBench::new(6);
    let ct_live = b.join("CT_Live");
    let t_live = b.join("T_Live");
    let pure_spec = b.join("PureSpec");
    let ct_dead_obs = b.join("CT_Dead");
    let hltv_proxy = b.join("HLTV_Proxy");

    // Enable `mp_forcecamera 1` and server-side `.dem` capture before spawning players.
    b.server.enable_replay_capture(true);
    let mut replay_ctx = SvcParseContext::new();
    replay_ctx.max_clients = b.server.config().max_clients;
    let signon = b.server.encode_replay_signon();
    let _ = parse_svc_stream(&signon, &mut replay_ctx).expect("replay signon parses");
    let _ = b.server.take_replay_svc_packets();

    b.server.game_mut().actions.push(Box::new(move |game, eng| {
        eng.set_cvar("mp_forcecamera", "1");
        let proxy_edict: EdictId = eng.client_edict(hltv_proxy as u8);
        if let Some(v) = eng.ent_mut(proxy_edict) {
            v.flags |= FL_PROXY;
        }
        assert!(game.spawn_player(eng, ct_live as u8, Team::Ct, 0));
        assert!(game.spawn_player(eng, t_live as u8, Team::Terrorist, 0));
        assert!(game.spawn_player(eng, ct_dead_obs as u8, Team::Ct, 0));
        game.client_command(eng, pure_spec as u8, &["jointeam".into(), "6".into()]);
        // Trigger `NeededPlayersCheck` (`#Game_Commencing` 3s) and let Round 1 begin cleanly.
        game.client_command(eng, ct_dead_obs as u8, &["kill".into()]);
    }));
    b.step(350);

    // Now Round 1 is live (`game_started = true`); kill `ct_dead_obs` and wait 4s for `start_death_cam`.
    b.server.game_mut().actions.push(Box::new(move |game, eng| {
        game.client_command(eng, ct_dead_obs as u8, &["kill".into()]);
    }));
    b.step(400);

    let obs_alive = b.server.game().game.state().players[ct_dead_obs]
        .as_ref()
        .unwrap()
        .alive;
    assert!(!obs_alive, "ct_dead_obs should be in observer mode");

    let ct_seen_before = b.usermsgs(ct_live).len();
    let t_seen_before = b.usermsgs(t_live).len();
    let spec_seen_before = b.usermsgs(pure_spec).len();
    let dead_seen_before = b.usermsgs(ct_dead_obs).len();
    let hltv_seen_before = b.usermsgs(hltv_proxy).len();

    // Mutate HP and money on CT_Live and T_Live (`damage_player` + `vest` -$650).
    b.server.game_mut().actions.push(Box::new(move |game, eng| {
        eng.set_cvar("mp_buy_anywhere", "1");
        assert!(game.damage_player(eng, ct_live as u8, None, 37.0, 0));
        assert!(game.damage_player(eng, t_live as u8, None, 59.0, 0));
        game.client_command(eng, ct_live as u8, &["vest".into()]);
        game.client_command(eng, t_live as u8, &["vest".into()]);
    }));
    b.step(20);

    let ct_id = (ct_live + 1) as u8;
    let t_id = (t_live + 1) as u8;

    let ct_msgs = &b.usermsgs(ct_live)[ct_seen_before..];
    let t_msgs = &b.usermsgs(t_live)[t_seen_before..];
    let spec_msgs = &b.usermsgs(pure_spec)[spec_seen_before..];
    let dead_msgs = &b.usermsgs(ct_dead_obs)[dead_seen_before..];
    let hltv_msgs = &b.usermsgs(hltv_proxy)[hltv_seen_before..];

    // 1. Live players MUST NEVER receive enemy HealthInfo or Account!
    assert!(
        !ct_msgs.iter().any(|m| matches!(
            m,
            CsUserMsg::HealthInfo { player_id, .. } | CsUserMsg::Account { player_id, .. }
            if *player_id == t_id
        )),
        "live CT must never receive enemy T HealthInfo/Account: {ct_msgs:?}"
    );
    assert!(
        !t_msgs.iter().any(|m| matches!(
            m,
            CsUserMsg::HealthInfo { player_id, .. } | CsUserMsg::Account { player_id, .. }
            if *player_id == ct_id
        )),
        "live T must never receive enemy CT HealthInfo/Account: {t_msgs:?}"
    );

    // 2. Dead CT observer under `mp_forcecamera 1` receives teammate CT HealthInfo/Account, NEVER enemy T!
    assert!(
        dead_msgs.contains(&CsUserMsg::HealthInfo {
            player_id: ct_id,
            health: 63,
        }),
        "dead CT observer missing teammate HealthInfo(63): {dead_msgs:?}"
    );
    assert!(
        dead_msgs.contains(&CsUserMsg::Account {
            player_id: ct_id,
            amount: 150,
        }),
        "dead CT observer missing teammate Account(150): {dead_msgs:?}"
    );
    assert!(
        !dead_msgs.iter().any(|m| matches!(
            m,
            CsUserMsg::HealthInfo { player_id, .. } | CsUserMsg::Account { player_id, .. }
            if *player_id == t_id
        )),
        "mp_forcecamera 1 dead CT must not receive enemy T HealthInfo/Account: {dead_msgs:?}"
    );

    // 3. Pure Spectator and HLTV proxy receive BOTH CT and T HealthInfo/Account!
    for (label, msgs) in [("spectator", spec_msgs), ("hltv", hltv_msgs)] {
        assert!(
            msgs.contains(&CsUserMsg::HealthInfo {
                player_id: ct_id,
                health: 63,
            }),
            "{label} missing CT HealthInfo: {msgs:?}"
        );
        assert!(
            msgs.contains(&CsUserMsg::HealthInfo {
                player_id: t_id,
                health: 41,
            }),
            "{label} missing T HealthInfo: {msgs:?}"
        );
        assert!(
            msgs.contains(&CsUserMsg::Account {
                player_id: ct_id,
                amount: 150,
            }),
            "{label} missing CT Account: {msgs:?}"
        );
        assert!(
            msgs.contains(&CsUserMsg::Account {
                player_id: t_id,
                amount: 150,
            }),
            "{label} missing T Account: {msgs:?}"
        );
    }

    // 4. Round `.dem` replay stream contains BOTH CT and T HealthInfo + Account!
    let mut replay_usermsgs = Vec::new();
    for pkt in b.server.take_replay_svc_packets() {
        for m in parse_svc_stream(&pkt, &mut replay_ctx).expect("replay packet parses") {
            if let SvcMessage::UserMsg { decoded, .. } = m {
                replay_usermsgs.push(decoded);
            }
        }
    }
    assert!(replay_usermsgs.contains(&CsUserMsg::HealthInfo {
        player_id: ct_id,
        health: 63,
    }));
    assert!(replay_usermsgs.contains(&CsUserMsg::HealthInfo {
        player_id: t_id,
        health: 41,
    }));
    assert!(replay_usermsgs.contains(&CsUserMsg::Account {
        player_id: ct_id,
        amount: 150,
    }));
    assert!(replay_usermsgs.contains(&CsUserMsg::Account {
        player_id: t_id,
        amount: 150,
    }));

    // 5. Switching `mp_forcecamera 0` allows dead CT observer to receive enemy T updates too.
    let dead_before_open = b.usermsgs(ct_dead_obs).len();
    b.server.game_mut().actions.push(Box::new(move |game, eng| {
        eng.set_cvar("mp_forcecamera", "0");
        assert!(game.damage_player(eng, t_live as u8, None, 12.0, 0));
    }));
    b.step(20);
    let dead_open_msgs = &b.usermsgs(ct_dead_obs)[dead_before_open..];
    // `t_live` has `Vest` (`armor = 100`), so `ARMOR_RATIO = 0.5` halves `12.0` damage to `6` (`41 -> 35`).
    assert!(dead_open_msgs.contains(&CsUserMsg::HealthInfo {
        player_id: t_id,
        health: 35,
    }));
}
