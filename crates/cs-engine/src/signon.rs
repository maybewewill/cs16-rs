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
    write_svc_setview, write_svc_signonnum, write_svc_spawnbaseline_encoded, write_svc_time,
    write_svc_updateuserinfo, write_svc_userregs, write_svc_voiceinit,
};
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;

use crate::api_messages::MAX_LIGHT_STYLE_BYTES;
use crate::lifecycle::MAX_CLIENT_SLOTS;
use crate::oob::MAX_INFO_STRING;

/// Light styles sent in the `spawn` stage: the HLDS table size (`MAX_LIGHTSTYLES`), the limit
/// of the `svc_lightstyle` index.
pub(crate) const MAX_LIGHTSTYLES: u8 = 64;

/// Bytes of the sign-on data a client is sent at once in the `spawn` stage: the init messages
/// and the spawn baselines the engine keeps for it, plus [`SPAWN_TAIL_MAX`] of the rest. The
/// reliable buffer of an HLDS netchan, `NET_MAX_PAYLOAD` (`rehlds/engine/net.h`, read
/// 2026-11-21), which is also the size of its sign-on buffer in a `REHLDS_FIXES` build
/// (`g_rehlds_sv.signonData[NET_MAX_PAYLOAD]`, `sv_main.cpp` `SV_SpawnServer`, read 2026-11-21).
pub const MAX_SIGNON_BUFFER: usize = 65536;

/// Bytes kept back for the `svc_clientdata` of the `spawn` answer (`SV_WriteClientdataToMessage`):
/// the client data against the zero baseline and, for a client with `cl_lw`, the weapon data of
/// up to 64 weapons. A bound over what a game can fill (see the test of its size).
pub const SPAWN_CLIENTDATA_MAX: usize = 8 * 1024;

/// Most bytes the `spawn` answer holds besides the init buffer and the baselines: `svc_setangle`,
/// `svc_time`, one `svc_updateuserinfo` per client slot at the longest userinfo a connect may
/// carry (with its terminating NUL), every light style at its longest stored length, the choke
/// byte and a forced view (`svc_setangle`), [`SPAWN_CLIENTDATA_MAX`] of client data,
/// `svc_signonnum` and `svc_voiceinit`.
pub const SPAWN_TAIL_MAX: usize = 7
    + 5
    + 1
    + 7
    + SPAWN_CLIENTDATA_MAX
    + 3
    + MAX_CLIENT_SLOTS as usize * (1 + 1 + 4 + MAX_INFO_STRING + 1 + 16)
    + MAX_LIGHTSTYLES as usize * (1 + 1 + MAX_LIGHT_STYLE_BYTES + 1)
    + 2;

/// Bytes of [`MAX_SIGNON_BUFFER`] kept back for the spawn baselines when the game writes
/// `MsgDest::Init` messages. The game writes them before the engine knows how big the
/// baselines are; HLDS has no such reserve and errors when the two overflow its buffer.
/// A baseline of a model entity with an origin and angles takes about 19 bytes, so the
/// reserve holds about 1270 of them (a baseline with every field set takes about 49 bytes:
/// about 500); a real map's brush entities take a few KiB (205 entities of cs_office: 3.8 KB).
pub const SIGNON_BASELINE_RESERVE: usize = 24 * 1024;

/// Bytes the init buffer holds: what is left of the sign-on buffer after the spawn tail and
/// the baseline reserve. A `MsgDest::Init` message that would take it past this is sent as
/// `MsgDest::All` instead.
pub const INIT_BUFFER_BUDGET: usize = MAX_SIGNON_BUFFER - SPAWN_TAIL_MAX - SIGNON_BASELINE_RESERVE;

/// The light styles of a fresh map: style 0 is steady full brightness, the rest are empty.
pub(crate) fn default_light_styles() -> Vec<String> {
    (0..MAX_LIGHTSTYLES)
        .map(|style| if style == 0 { "m" } else { "" }.to_string())
        .collect()
}

/// A userinfo as `SV_FullClientUpdate` sends it (ReHLDS `sv_main.cpp`, read 2026-11-22): without
/// the keys that start with `_`, which are private to the server (`Info_RemovePrefixedKeys`).
pub(crate) fn public_userinfo(info: &cs_proto48::info::InfoString) -> String {
    let mut info = info.clone();
    info.remove_prefixed('_');
    info.to_wire_string()
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

/// Movement variables from the cvars that exist, defaults otherwise (`SV_SetMoveVars`, ReHLDS
/// `sv_main.cpp`, read 2026-11-22: every field has its own cvar; `sv_zmax` is `zmax`,
/// `sv_wateramp` the wave height, `edgefriction` the edge friction, `mp_footsteps` the footsteps
/// flag, `sv_skyname` the sky; `entgravity` is always 1). The sky name is the world entity's
/// `skyname` key when it has one, else the cvar when it is not empty. Either way it is cut to
/// what `movevars_t::skyName` holds, 32 bytes with the terminator (`Q_strncpy(movevars->skyName,
/// sv_skyname.string, sizeof(movevars->skyName) - 1)`, `SV_SetMoveVars`, ReHLDS `sv_main.cpp`,
/// read 2026-11-22), so 31 bytes at most (the finding's "32 characters" counted the
/// terminator; the source wins).
///
/// DEVIATION (documented): a cvar value that is not finite (NaN, infinity) becomes 0.0. NaN
/// compares unequal to itself, so a NaN on the wire (and in `pm`) would make the movevars resend
/// of `sync_client_movevars` fire on every command.
pub(crate) fn move_vars(cvars: &CVarRegistry, entities: &[Entity]) -> MoveVarsMsg {
    let mut mv = MoveVarsMsg {
        // The first assignment of `SV_SetMoveVars`; the game scales it per player.
        entgravity: 1.0,
        ..MoveVarsMsg::default()
    };
    let [sky_r, sky_g, sky_b] = &mut mv.skycolor;
    let [vec_x, vec_y, vec_z] = &mut mv.skyvec;
    let fields: [(&str, &mut f32); 23] = [
        ("sv_gravity", &mut mv.gravity),
        ("sv_stopspeed", &mut mv.stopspeed),
        ("sv_maxspeed", &mut mv.maxspeed),
        ("sv_spectatormaxspeed", &mut mv.spectatormaxspeed),
        ("sv_accelerate", &mut mv.accelerate),
        ("sv_airaccelerate", &mut mv.airaccelerate),
        ("sv_wateraccelerate", &mut mv.wateraccelerate),
        ("sv_friction", &mut mv.friction),
        ("edgefriction", &mut mv.edgefriction),
        ("sv_waterfriction", &mut mv.waterfriction),
        ("sv_bounce", &mut mv.bounce),
        ("sv_stepsize", &mut mv.stepsize),
        ("sv_maxvelocity", &mut mv.maxvelocity),
        ("sv_zmax", &mut mv.zmax),
        ("sv_wateramp", &mut mv.wave_height),
        ("sv_rollangle", &mut mv.rollangle),
        ("sv_rollspeed", &mut mv.rollspeed),
        ("sv_skycolor_r", sky_r),
        ("sv_skycolor_g", sky_g),
        ("sv_skycolor_b", sky_b),
        ("sv_skyvec_x", vec_x),
        ("sv_skyvec_y", vec_y),
        ("sv_skyvec_z", vec_z),
    ];
    for (name, field) in fields {
        if let Some(cvar) = cvars.get(name) {
            let value = cvar.as_f32();
            *field = if value.is_finite() { value } else { 0.0 };
        }
    }
    if let Some(cvar) = cvars.get("mp_footsteps") {
        // `movevars->footsteps = sv_footsteps.value` stores a float in an int: 0.5 is off.
        mv.footsteps = cvar.as_f32() as i32 != 0;
    }
    let lump_sky = entities
        .iter()
        .find(|e| e.classname() == "worldspawn")
        .and_then(|e| e.get("skyname"));
    if let Some(sky) = lump_sky {
        mv.sky_name = sky.to_string();
    } else if let Some(cvar) = cvars.get("sv_skyname")
        && !cvar.value.is_empty()
    {
        mv.sky_name = cvar.value.clone();
    }
    // A cut inside a multi-byte character drops the whole character (a name is ASCII in practice).
    let mut end = mv.sky_name.len().min(SKY_NAME_MAX);
    while !mv.sky_name.is_char_boundary(end) {
        end -= 1;
    }
    mv.sky_name.truncate(end);
    mv
}

/// `sizeof(movevars_t::skyName) - 1`: the longest sky name the client is sent.
const SKY_NAME_MAX: usize = 31;

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
    encode: Option<cs_proto48::messages::EntityEncodeFn<'_>>,
) {
    write_svc_spawnbaseline_encoded(w, deltas, max_clients, baselines, time, encode);
}

/// `SV_WriteSpawn` (ReHLDS `sv_main.cpp`, read 2026-11-22) after the game put the player in:
/// `svc_time`, the userinfo of every connected client (`SV_FullClientUpdate`), the stored light
/// styles and the view angles (`v_angle` pitch and yaw, roll 0). The client data follows
/// ([`Server::write_spawn_clientdata`](crate::server::Server)) and then [`write_spawn_tail`].
pub(crate) fn write_spawn_stage(
    w: &mut MsgWriter,
    view_angles: Vec3,
    time: f64,
    userinfos: &[UserInfoEntry],
    light_styles: &[String],
) {
    write_svc_time(w, time as f32);
    write_userinfos(w, userinfos);
    for (style, pattern) in light_styles.iter().enumerate().take(MAX_LIGHTSTYLES.into()) {
        write_svc_lightstyle(w, style as u8, pattern);
    }
    write_svc_setangle(w, Vec3::new(view_angles.x, view_angles.y, 0.0));
}

/// The end of the answer to `spawn`: `svc_signonnum 1` and `SV_WriteVoiceCodec` (an empty codec
/// of quality 0, `SV_WriteVoiceCodec_internal`).
pub(crate) fn write_spawn_tail(w: &mut MsgWriter) {
    write_svc_signonnum(w, 1);
    write_svc_voiceinit(w, "", 0);
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
        // `SV_Spawn_f`: the init buffer, the baselines, then `SV_WriteSpawn`.
        let mut w = MsgWriter::new();
        let mut init = MsgWriter::new();
        write_svc_print(&mut init, "init\n");
        w.write_bytes(init.as_slice());
        write_spawn_baselines(&mut w, &deltas, 8, &[(40, state)], 1.0, None);
        write_spawn_stage(
            &mut w,
            Vec3::new(0.0, 90.0, 5.0),
            1.5,
            &[entry(0, 1, r"\name\A"), entry(1, 2, r"\name\B")],
            &default_light_styles(),
        );
        write_spawn_tail(&mut w);
        let msgs = parse(w);
        assert_eq!(msgs.len(), 1 + 1 + 1 + 2 + 64 + 1 + 1 + 1);
        assert_eq!(msgs[0], SvcMessage::Print("init\n".to_string()));
        let SvcMessage::SpawnBaseline { baselines, .. } = &msgs[1] else {
            panic!("expected spawnbaseline, got {:?}", msgs[1]);
        };
        assert_eq!(baselines.keys().copied().collect::<Vec<_>>(), vec![40]);
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
        // The roll is zero whatever `v_angle` holds.
        let SvcMessage::SetAngle(angles) = msgs[5 + 64] else {
            panic!("expected setangle");
        };
        assert!((angles.y - 90.0).abs() < 0.01 && angles.z == 0.0, "{angles:?}");
        assert_eq!(msgs[5 + 65], SvcMessage::SignonNum(1));
        assert_eq!(msgs[5 + 66].opcode(), svc::VOICEINIT);
    }

    #[test]
    fn move_vars_follow_cvars_and_sky() {
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("sv_gravity", "600");
        cvars.set("sv_stepsize", "20");
        cvars.set("sv_maxvelocity", "1500");
        cvars.set("sv_bounce", "0.5");
        let world = Entity {
            properties: HashMap::from([
                ("classname".to_string(), "worldspawn".to_string()),
                ("skyname".to_string(), "space".to_string()),
            ]),
        };
        let mv = move_vars(&cvars, &[world]);
        assert_eq!((mv.gravity, mv.stepsize), (600.0, 20.0));
        // `SV_SetMoveVars` (`sv_main.cpp`, read 2026-11-21) copies these two as well.
        assert_eq!((mv.maxvelocity, mv.bounce), (1500.0, 0.5));
        assert_eq!(mv.stopspeed, cvars.get_f32("sv_stopspeed"));
        assert_eq!(mv.sky_name, "space");

        // Without a `skyname` key the cvar is the sky; `zmax` and the wave height are cvars.
        cvars.set("sv_skyname", "des");
        cvars.set("sv_zmax", "8192");
        cvars.set("sv_wateramp", "2");
        let mv = move_vars(&cvars, &[]);
        assert_eq!(
            (mv.sky_name.as_str(), mv.zmax, mv.wave_height),
            ("des", 8192.0, 2.0)
        );

        // A long name is cut to 31 bytes, from the lump and from the cvar alike.
        let long = "s".repeat(40);
        let world = Entity {
            properties: HashMap::from([
                ("classname".to_string(), "worldspawn".to_string()),
                ("skyname".to_string(), long.clone()),
            ]),
        };
        assert_eq!(move_vars(&cvars, &[world]).sky_name, "s".repeat(31));
        cvars.set("sv_skyname", &long);
        assert_eq!(move_vars(&cvars, &[]).sky_name, "s".repeat(31));
        cvars.set("sv_skyname", "s".repeat(31));
        assert_eq!(move_vars(&cvars, &[]).sky_name.len(), 31);

        let empty = move_vars(&CVarRegistry::new_empty(), &[]);
        assert_eq!(empty, MoveVarsMsg::default());
    }

    #[test]
    fn move_vars_turn_a_non_finite_cvar_into_zero() {
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("sv_gravity", "nan");
        cvars.set("sv_friction", "inf");
        cvars.set("sv_stopspeed", "-inf");
        let mv = move_vars(&cvars, &[]);
        assert_eq!((mv.gravity, mv.friction, mv.stopspeed), (0.0, 0.0, 0.0));
        // Equal to itself, so a resend compare does not fire every command.
        assert_eq!(mv, move_vars(&cvars, &[]));
    }

    #[test]
    fn munged_crc_unmunges_to_map_crc() {
        let munged = munged_map_crc(0xDEAD_BEEF, 7);
        let mut bytes = munged.to_le_bytes();
        com_unmunge2(&mut bytes, (-1 - 7) & 0xFF);
        assert_eq!(u32::from_le_bytes(bytes), 0xDEAD_BEEF);
    }
}
