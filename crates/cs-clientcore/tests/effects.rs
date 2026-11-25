//! The effect modules against the stock-server golden captures (`hlds_join`,
//! `hlds_office_join`): the svc stream of a real join goes through `ClientEffects` the way the
//! connection feeds it, and the HUD state, the sounds and the temp entities are checked.
//!
//! The captures are not in the repository. The tests read `CS16_GOLDEN_DIR` and return without
//! assertions when the variable or a capture is absent.

use std::collections::BTreeMap;
use std::path::Path;

use cs_clientcore::{ClientEffects, FxEnv, NoWorld, PrecacheTables, SoundCommand};
use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::delta::GoldSrcEntityState;
use cs_proto48::messages::{
    SvcMessage, SvcParseContext, entity_schema_name, parse_svc_stream,
};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};

const MAGIC: &[u8; 8] = b"CS48CAP1";

/// The received datagrams of a `CS48CAP1` capture.
fn received_datagrams(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut rest = bytes.strip_prefix(MAGIC.as_slice()).expect("not a capture");
    let mut out = Vec::new();
    while !rest.is_empty() {
        let dir = rest[0];
        let len = u32::from_le_bytes(rest[9..13].try_into().unwrap()) as usize;
        if dir == 0 {
            out.push(rest[13..13 + len].to_vec());
        }
        rest = &rest[13 + len..];
    }
    out
}

/// What a replay left behind.
struct Replay {
    fx: ClientEffects,
    precache: PrecacheTables,
    messages: usize,
    sounds_seen: usize,
    temp_entities_seen: usize,
}

/// Feeds every svc message of a capture to a [`ClientEffects`] with the client's own order:
/// the precache tables fill when the resource list arrives, the entity map after each update.
fn replay(bytes: &[u8]) -> Replay {
    let mut chan = NetChan::new(FRAGMENT_C2S_SIZE);
    let mut ctx = SvcParseContext::new();
    let mut splits = SplitPacketAssembler::default();
    let mut precache = PrecacheTables::default();
    let mut entities: BTreeMap<u16, GoldSrcEntityState> = BTreeMap::new();
    let mut fx = ClientEffects::with_seed(1);
    let mut out = Replay {
        fx: ClientEffects::default(),
        precache: PrecacheTables::default(),
        messages: 0,
        sounds_seen: 0,
        temp_entities_seen: 0,
    };
    let mut time = 0.0;

    for datagram in received_datagrams(bytes) {
        let Some(datagram) = splits.push(&datagram) else {
            continue;
        };
        if datagram.len() >= 4 && datagram[..4] == CONNECTIONLESS_HEADER.to_le_bytes() {
            continue;
        }
        let Some(incoming) = chan.process_packet(&datagram).expect("netchan") else {
            continue;
        };
        ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;
        let streams = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for stream in streams {
            for msg in parse_svc_stream(&stream, &mut ctx).expect("svc stream") {
                time += 0.001;
                out.messages += 1;
                match &msg {
                    SvcMessage::ResourceList { resources, .. } => precache.add(resources),
                    SvcMessage::Sound(_)
                    | SvcMessage::SpawnStaticSound(_)
                    | SvcMessage::StopSound(_)
                    | SvcMessage::SoundFade(_) => out.sounds_seen += 1,
                    SvcMessage::TempEntity { .. } => out.temp_entities_seen += 1,
                    _ => {}
                }
                let mut env = FxEnv::new(time, &precache, &entities, &NoWorld);
                env.max_clients = u16::from(ctx.max_clients);
                fx.handle(&msg, &env);
                if let SvcMessage::PacketEntities(update) = &msg {
                    entities = update
                        .entities
                        .iter()
                        .filter_map(|(&number, (ent_type, rec))| {
                            let schema = ctx.deltas.get(entity_schema_name(
                                number,
                                *ent_type,
                                ctx.max_clients,
                            ))?;
                            Some((
                                number,
                                GoldSrcEntityState::from_delta(number, *ent_type, rec, schema),
                            ))
                        })
                        .collect();
                }
            }
        }
    }
    out.fx = fx;
    out.precache = precache;
    out
}

fn capture(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var_os("CS16_GOLDEN_DIR")?;
    std::fs::read(Path::new(&dir).join(name)).ok()
}

#[test]
fn hud_state_after_golden_join() {
    let Some(bytes) = capture("hlds_join.cs48cap") else {
        return;
    };
    let r = replay(&bytes);
    let hud = &r.fx.hud;
    // The values the stock server sent a freshly joined counter-terrorist.
    assert_eq!((hud.health, hud.armor, hud.armor_type), (100, 0, 0));
    assert_eq!(hud.money, 800);
    assert_eq!(hud.fov, 90);
    assert_eq!(hud.hidden, 2);
    assert_eq!(hud.round_time, 286);
    assert_eq!(hud.server_name, "Counter-Strike 1.6 Server");
    // The weapon table: 29 weapons, e.g. the AK-47 in slot 0 position 1 on ammo type 2.
    assert_eq!(hud.weapons.len(), 29);
    let ak = &hud.weapons[&28];
    assert_eq!(ak.name, "weapon_ak47");
    assert_eq!(
        (ak.slot, ak.position, ak.ammo1_type, ak.ammo1_max),
        (0, 1, 2, 90)
    );
    assert_eq!(hud.weapons[&29].name, "weapon_knife");
    // The spawn kit: a knife and a USP with 12 rounds, 24 spare. The stock server sends
    // `CurWeapon(active, USP, 12)` and then `CurWeapon(inactive, knife, -1)`; an inactive
    // `CurWeapon` only updates that weapon's clip, so the USP stays the current weapon.
    let cur = hud.current_weapon.unwrap();
    assert_eq!((cur.weapon_id, cur.clip), (16, 12));
    assert_eq!(hud.clips[&16], 12);
    assert_eq!(hud.ammo[&6], 24);
    assert_eq!(hud.weapon_pickups, [29, 16]);
    // Team and scores.
    assert_eq!(hud.players[&1].team, "CT");
    assert_eq!(hud.players[&1].team_id, 2);
    assert_eq!(hud.team_scores["CT"], 0);
    assert_eq!(hud.team_scores["TERRORIST"], 0);
    // The buy zone icon, green.
    let icon = &hud.status_icons["buyzone"];
    assert_eq!((icon.status, icon.color), (1, Some([0, 160, 0])));
    // The text messages and the MOTD, whole.
    let first = &hud.text[0];
    assert_eq!(first.message, "#Game_connected");
    assert_eq!(first.params, ["cs-probe"]);
    assert!(hud.text.iter().any(|t| t.message == "#Game_join_ct"));
    assert!(hud.motd.starts_with("<!DOCTYPE HTML>"));
    assert!(hud.motd.trim_end().ends_with("</html>"));
    assert!(hud.motd_complete);
    // The registered messages arrive by number, the first being the engine's own `VoiceMask`.
    assert_eq!(hud.user_messages[&64].name, "VoiceMask");
    assert!(hud.user_messages.values().any(|m| m.name == "Health"));
    // Build 10210 newer messages (`Account`, `HealthInfo`, `ReqState`) now reduce directly into
    // `HudState` instead of falling through to `unknown_messages`.
    assert_eq!(hud.player_health.get(&1).copied(), Some(100));
    assert_eq!(hud.player_money.get(&1).copied(), Some(800));
    assert!(hud.req_state_count > 0);
    // The damage message every spawn sends, with the spawn origin.
    assert_eq!(hud.damage.unwrap().origin.x, 448.0);
}

#[test]
fn golden_sounds_become_commands_with_their_names() {
    for file in ["hlds_join.cs48cap", "hlds_office_join.cs48cap"] {
        let Some(bytes) = capture(file) else {
            continue;
        };
        let mut r = replay(&bytes);
        // Only the sound-start/stop/fade messages are counted in `sounds_seen`; `svc_roomtype`
        // and `svc_cdtrack` also become commands (room DSP, music) and are left out here.
        let commands: Vec<_> = r
            .fx
            .sounds
            .drain()
            .into_iter()
            .filter(|c| !matches!(c, SoundCommand::RoomType(_) | SoundCommand::CdTrack { .. }))
            .collect();
        assert_eq!(
            commands.len() + r.fx.sounds.unknown_sounds as usize,
            r.sounds_seen,
            "{file}"
        );
        for c in &commands {
            let (SoundCommand::Start(s) | SoundCommand::Ambient(s)) = c else {
                continue;
            };
            let name = s.name.as_deref().expect("a precached sound has a name");
            assert_eq!(Some(name), r.precache.sound(s.index), "{file}");
            assert!((0.0..=1.0).contains(&s.volume), "{file}");
            assert!(s.pitch >= 2, "{file}");
        }
    }
    let Some(bytes) = capture("hlds_join.cs48cap") else {
        return;
    };
    let mut r = replay(&bytes);
    // The join also carries `svc_cdtrack 0 0` (stop music); only the entity sounds are read here.
    let commands: Vec<_> = r
        .fx
        .sounds
        .drain()
        .into_iter()
        .filter(|c| matches!(c, SoundCommand::Start(_) | SoundCommand::Ambient(_)))
        .collect();
    let SoundCommand::Start(first) = &commands[0] else {
        panic!("{:?}", commands[0]);
    };
    assert_eq!(first.name.as_deref(), Some("weapons/knife_deploy1.wav"));
    assert_eq!((first.entity, first.channel), (1, 3));
    let SoundCommand::Start(pickup) = &commands[1] else {
        panic!();
    };
    assert_eq!(pickup.name.as_deref(), Some("items/gunpickup2.wav"));
}

#[test]
fn golden_temp_entities_stay_within_the_pool() {
    for file in ["hlds_join.cs48cap", "hlds_office_join.cs48cap"] {
        let Some(bytes) = capture(file) else {
            continue;
        };
        let r = replay(&bytes);
        assert!(r.messages > 0, "{file}");
        assert!(r.fx.tempents.active().len() <= cs_clientcore::tempents::MAX_TEMP_ENTITIES);
    }
}

