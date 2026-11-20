//! What the server sends at each sign-on stage. Every writer is a pure function appending
//! messages to a [`MsgWriter`]; the order is a working assumption about HLDS until a golden
//! capture confirms it.

use cs_assets::bsp::Entity;
use cs_cvar::CVarRegistry;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::crypto::com_munge2;
use cs_proto48::delta::{DeltaRegistry, GoldSrcEntityState};
use cs_proto48::messages::{
    MoveVarsMsg, ResourceEntry, ServerInfoMsg, write_svc_cdtrack, write_svc_deltadescriptions,
    write_svc_lightstyle, write_svc_newmovevars, write_svc_print, write_svc_resourcelist,
    write_svc_resourcerequest, write_svc_sendextrainfo, write_svc_serverinfo, write_svc_setangle,
    write_svc_setview, write_svc_signonnum, write_svc_spawnbaseline, write_svc_time,
    write_svc_updateuserinfo, write_svc_userregs,
};
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;

/// Light styles sent in the `spawn` stage.
pub(crate) const MAX_LIGHTSTYLES: u8 = 64;

/// The light styles of a fresh map: style 0 is steady full brightness, the rest are empty.
pub(crate) fn default_light_styles() -> Vec<String> {
    (0..MAX_LIGHTSTYLES)
        .map(|style| if style == 0 { "m" } else { "" }.to_string())
        .collect()
}

/// One `svc_updateuserinfo` entry.
pub(crate) struct UserInfoEntry {
    pub(crate) slot: u8,
    pub(crate) userid: i32,
    pub(crate) info: String,
}

/// Console banner printed at the start of the `new` stage.
pub(crate) fn banner(build: i32, spawn_count: u32) -> String {
    format!("\x02\nBUILD {build} SERVER (0 CRC)\nServer # {spawn_count}\n")
}

/// Movement variables from the `sv_*` cvars that exist, defaults otherwise; the sky name
/// comes from the world entity's `skyname` key.
pub(crate) fn move_vars(cvars: &CVarRegistry, entities: &[Entity]) -> MoveVarsMsg {
    let mut mv = MoveVarsMsg::default();
    let fields: [(&str, &mut f32); 7] = [
        ("sv_gravity", &mut mv.gravity),
        ("sv_stopspeed", &mut mv.stopspeed),
        ("sv_maxspeed", &mut mv.maxspeed),
        ("sv_accelerate", &mut mv.accelerate),
        ("sv_airaccelerate", &mut mv.airaccelerate),
        ("sv_friction", &mut mv.friction),
        ("sv_stepsize", &mut mv.stepsize),
    ];
    for (name, field) in fields {
        if let Some(cvar) = cvars.get(name) {
            *field = cvar.as_f32();
        }
    }
    if let Some(sky) = entities
        .iter()
        .find(|e| e.classname() == "worldspawn")
        .and_then(|e| e.get("skyname"))
    {
        mv.sky_name = sky.to_string();
    }
    mv
}

/// The map CRC a client answers `spawn` with: munged with `COM_Munge2` keyed by
/// `(-1 - spawn_count) & 0xFF`, read as a signed integer.
pub(crate) fn munged_map_crc(map_crc: u32, spawn_count: u32) -> i32 {
    let mut crc = map_crc.to_le_bytes();
    com_munge2(&mut crc, (-1i32).wrapping_sub(spawn_count as i32) & 0xFF);
    i32::from_le_bytes(crc)
}

fn write_userinfos(w: &mut MsgWriter, userinfos: &[UserInfoEntry]) {
    for u in userinfos {
        write_svc_updateuserinfo(w, u.slot, u.userid, &u.info, &[0; 16]);
    }
}

/// Answer to `new`: banner, server info, extra info, delta descriptions, movement variables,
/// CD track, view entity, user message registrations and the userinfo of `userinfos`.
pub(crate) fn write_new_stage(
    w: &mut MsgWriter,
    info: &ServerInfoMsg,
    build: i32,
    deltas: &DeltaRegistry,
    move_vars: &MoveVarsMsg,
    usermsgs: &UserMsgRegistry,
    userinfos: &[UserInfoEntry],
) {
    write_svc_print(w, &banner(build, info.spawn_count));
    write_svc_serverinfo(w, info);
    write_svc_sendextrainfo(w, "", false);
    write_svc_deltadescriptions(w, deltas);
    write_svc_newmovevars(w, move_vars);
    write_svc_cdtrack(w, 0, 0);
    write_svc_setview(w, u16::from(info.player_slot) + 1);
    write_svc_userregs(w, usermsgs);
    write_userinfos(w, userinfos);
}

/// Answer to `sendres`: the resource request and the resource list.
pub(crate) fn write_sendres_stage(
    w: &mut MsgWriter,
    spawn_count: u32,
    resources: &[ResourceEntry],
) {
    write_svc_resourcerequest(w, spawn_count as i32);
    write_svc_resourcelist(w, resources);
}

/// First part of the answer to `spawn`, sent before the game puts the player in.
pub(crate) fn write_spawn_baselines(
    w: &mut MsgWriter,
    deltas: &DeltaRegistry,
    max_clients: u8,
    baselines: &[(u16, GoldSrcEntityState)],
    time: f64,
) {
    write_svc_spawnbaseline(w, deltas, max_clients, baselines, time);
}

/// Rest of the answer to `spawn`, after the game put the player in: view angles, time, every
/// connected client's userinfo, the stored light styles, the init buffer (messages the game
/// sent to `MsgDest::Init` while the map was activated, complete as it wrote them) and
/// `svc_signonnum 1`.
pub(crate) fn write_spawn_stage(
    w: &mut MsgWriter,
    view_angles: Vec3,
    time: f64,
    userinfos: &[UserInfoEntry],
    light_styles: &[String],
    init: &[u8],
) {
    write_svc_setangle(w, view_angles);
    write_svc_time(w, time as f32);
    write_userinfos(w, userinfos);
    for (style, pattern) in light_styles.iter().enumerate().take(MAX_LIGHTSTYLES.into()) {
        write_svc_lightstyle(w, style as u8, pattern);
    }
    w.write_bytes(init);
    write_svc_signonnum(w, 1);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cs_proto48::crypto::com_unmunge2;
    use cs_proto48::messages::{ENTITY_NORMAL, SvcMessage, SvcParseContext, parse_svc_stream, svc};

    use super::*;

    fn parse(w: MsgWriter) -> Vec<SvcMessage> {
        parse_svc_stream(&w.into_bytes(), &mut SvcParseContext::new()).expect("valid stream")
    }

    fn info() -> ServerInfoMsg {
        ServerInfoMsg {
            protocol: 48,
            spawn_count: 3,
            map_crc: 0x1234_5678,
            client_dll_md5: [0; 16],
            max_clients: 8,
            player_slot: 2,
            is_deathmatch: true,
            game_dir: "cstrike".to_string(),
            hostname: "host".to_string(),
            map_path: "maps/test.bsp".to_string(),
            map_cycle: String::new(),
            vac_secure: false,
        }
    }

    fn entry(slot: u8, userid: i32, info: &str) -> UserInfoEntry {
        UserInfoEntry {
            slot,
            userid,
            info: info.to_string(),
        }
    }

    #[test]
    fn new_stage_messages_in_order() {
        let deltas = DeltaRegistry::cs16_default();
        let usermsgs = UserMsgRegistry::cs16_default();
        let mv = MoveVarsMsg::default();
        let mut w = MsgWriter::new();
        write_new_stage(
            &mut w,
            &info(),
            8684,
            &deltas,
            &mv,
            &usermsgs,
            &[entry(0, 4, "\\name\\Other")],
        );
        let msgs = parse(w);

        let schemas = deltas.iter().count();
        let regs = usermsgs.iter().count();
        assert_eq!(msgs.len(), 3 + schemas + 3 + regs + 1);
        assert_eq!(
            msgs[0],
            SvcMessage::Print("\x02\nBUILD 8684 SERVER (0 CRC)\nServer # 3\n".to_string())
        );
        assert_eq!(msgs[1], SvcMessage::ServerInfo(Box::new(info())));
        assert_eq!(
            msgs[2],
            SvcMessage::SendExtraInfo {
                fallback_dir: String::new(),
                cheats: false
            }
        );
        assert!(
            msgs[3..3 + schemas]
                .iter()
                .all(|m| m.opcode() == svc::DELTADESCRIPTION)
        );
        let rest = &msgs[3 + schemas..];
        assert_eq!(rest[0], SvcMessage::NewMoveVars(Box::new(mv)));
        assert_eq!(
            rest[1],
            SvcMessage::CdTrack {
                track: 0,
                loop_track: 0
            }
        );
        assert_eq!(rest[2], SvcMessage::SetView(3));
        assert!(
            rest[3..3 + regs]
                .iter()
                .all(|m| m.opcode() == svc::NEWUSERMSG)
        );
        assert_eq!(
            rest[3 + regs],
            SvcMessage::UpdateUserInfo {
                slot: 0,
                userid: 4,
                userinfo: "\\name\\Other".to_string(),
                cdkey_hash: [0; 16],
            }
        );
    }

    #[test]
    fn sendres_stage_requests_and_lists_resources() {
        let resources = vec![ResourceEntry {
            res_type: 2,
            filename: "models/test.mdl".to_string(),
            index: 2,
            download_size: 0,
            flags: 0,
            md5: None,
        }];
        let mut w = MsgWriter::new();
        write_sendres_stage(&mut w, 5, &resources);
        assert_eq!(
            parse(w),
            vec![
                SvcMessage::ResourceRequest {
                    spawn_count: 5,
                    reserved: 0
                },
                SvcMessage::ResourceList {
                    resources,
                    consistency_indices: Vec::new()
                },
            ]
        );
    }

    #[test]
    fn spawn_stage_messages_in_order() {
        let deltas = DeltaRegistry::cs16_default();
        let state = GoldSrcEntityState {
            number: 40,
            entity_type: ENTITY_NORMAL,
            modelindex: 2,
            origin: Vec3::new(0.0, 0.0, 16.0),
            ..Default::default()
        };
        let mut w = MsgWriter::new();
        write_spawn_baselines(&mut w, &deltas, 8, &[(40, state)], 1.0);
        let mut init = MsgWriter::new();
        write_svc_print(&mut init, "init\n");
        write_spawn_stage(
            &mut w,
            Vec3::new(0.0, 90.0, 0.0),
            1.5,
            &[entry(0, 1, "\\name\\A"), entry(1, 2, "\\name\\B")],
            &default_light_styles(),
            init.as_slice(),
        );
        let msgs = parse(w);
        assert_eq!(msgs.len(), 1 + 1 + 1 + 2 + 64 + 1 + 1);
        let SvcMessage::SpawnBaseline { baselines, .. } = &msgs[0] else {
            panic!("expected spawnbaseline, got {:?}", msgs[0]);
        };
        assert_eq!(baselines.keys().copied().collect::<Vec<_>>(), vec![40]);
        let SvcMessage::SetAngle(angles) = msgs[1] else {
            panic!("expected setangle");
        };
        assert!((angles.y - 90.0).abs() < 0.01, "{angles:?}");
        assert_eq!(msgs[2], SvcMessage::Time(1.5));
        assert_eq!(msgs[3].opcode(), svc::UPDATEUSERINFO);
        assert_eq!(msgs[4].opcode(), svc::UPDATEUSERINFO);
        assert_eq!(
            msgs[5],
            SvcMessage::LightStyle {
                index: 0,
                pattern: "m".to_string()
            }
        );
        assert_eq!(
            msgs[5 + 63],
            SvcMessage::LightStyle {
                index: 63,
                pattern: String::new()
            }
        );
        assert_eq!(msgs[5 + 64], SvcMessage::Print("init\n".to_string()));
        assert_eq!(msgs[5 + 65], SvcMessage::SignonNum(1));
    }

    #[test]
    fn move_vars_follow_cvars_and_sky() {
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("sv_gravity", "600");
        cvars.set("sv_stepsize", "20");
        let world = Entity {
            properties: HashMap::from([
                ("classname".to_string(), "worldspawn".to_string()),
                ("skyname".to_string(), "space".to_string()),
            ]),
        };
        let mv = move_vars(&cvars, &[world]);
        assert_eq!((mv.gravity, mv.stepsize), (600.0, 20.0));
        assert_eq!(mv.stopspeed, cvars.get_f32("sv_stopspeed"));
        assert_eq!(mv.sky_name, "space");

        let empty = move_vars(&CVarRegistry::new_empty(), &[]);
        assert_eq!(empty, MoveVarsMsg::default());
    }

    #[test]
    fn munged_crc_unmunges_to_map_crc() {
        let munged = munged_map_crc(0xDEAD_BEEF, 7);
        let mut bytes = munged.to_le_bytes();
        com_unmunge2(&mut bytes, (-1 - 7) & 0xFF);
        assert_eq!(u32::from_le_bytes(bytes), 0xDEAD_BEEF);
    }
}
