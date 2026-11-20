//! GoldSrc Protocol 48 `svc_*` (server → client) and `clc_*` (client → server)
//! message codecs (`ReHLDS/engine/net.h`, `sv_main.cpp`, `sv_user.cpp`).

use std::collections::BTreeMap;

use glam::Vec3;

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::crypto::{
    com_block_sequence_crc_byte, com_munge, com_munge3, com_unmunge, com_unmunge3,
};
use crate::delta::{
    DeltaRecord, DeltaRegistry, DeltaSchema, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd,
    has_delta_changes, read_delta, read_delta_description, write_delta, write_delta_description,
};
use crate::usermsg::{CsUserMsg, USERMSG_BASE_ID, UserMsgDesc, UserMsgRegistry, decode_cs_usermsg, encode_cs_usermsg};
use crate::PROTOCOL_VERSION;

pub mod svc {
    pub const BAD: u8 = 0;
    pub const NOP: u8 = 1;
    pub const DISCONNECT: u8 = 2;
    pub const EVENT: u8 = 3;
    pub const VERSION: u8 = 4;
    pub const SETVIEW: u8 = 5;
    pub const SOUND: u8 = 6;
    pub const TIME: u8 = 7;
    pub const PRINT: u8 = 8;
    pub const STUFFTEXT: u8 = 9;
    pub const SETANGLE: u8 = 10;
    pub const SERVERINFO: u8 = 11;
    pub const LIGHTSTYLE: u8 = 12;
    pub const UPDATEUSERINFO: u8 = 13;
    pub const DELTADESCRIPTION: u8 = 14;
    pub const CLIENTDATA: u8 = 15;
    pub const STOPSOUND: u8 = 16;
    pub const PINGS: u8 = 17;
    pub const PARTICLE: u8 = 18;
    pub const DAMAGE: u8 = 19;
    pub const SPAWNSTATIC: u8 = 20;
    pub const EVENT_RELIABLE: u8 = 21;
    pub const SPAWNBASELINE: u8 = 22;
    pub const TEMP_ENTITY: u8 = 23;
    pub const SETPAUSE: u8 = 24;
    pub const SIGNONNUM: u8 = 25;
    pub const CENTERPRINT: u8 = 26;
    pub const KILLEDMONSTER: u8 = 27;
    pub const FOUNDSECRET: u8 = 28;
    pub const SPAWNSTATICSOUND: u8 = 29;
    pub const INTERMISSION: u8 = 30;
    pub const FINALE: u8 = 31;
    pub const CDTRACK: u8 = 32;
    pub const RESTORE: u8 = 33;
    pub const CUTSCENE: u8 = 34;
    pub const WEAPONANIM: u8 = 35;
    pub const DECALNAME: u8 = 36;
    pub const ROOMTYPE: u8 = 37;
    pub const ADDANGLE: u8 = 38;
    pub const NEWUSERMSG: u8 = 39;
    pub const PACKETENTITIES: u8 = 40;
    pub const DELTAPACKETENTITIES: u8 = 41;
    pub const CHOKE: u8 = 42;
    pub const RESOURCELIST: u8 = 43;
    pub const NEWMOVEVARS: u8 = 44;
    pub const RESOURCEREQUEST: u8 = 45;
    pub const CUSTOMIZATION: u8 = 46;
    pub const CROSSHAIRANGLE: u8 = 47;
    pub const SOUNDFADE: u8 = 48;
    pub const FILETXFERFAILED: u8 = 49;
    pub const HLTV: u8 = 50;
    pub const DIRECTOR: u8 = 51;
    pub const VOICEINIT: u8 = 52;
    pub const VOICEDATA: u8 = 53;
    pub const SENDEXTRAINFO: u8 = 54;
    pub const TIMESCALE: u8 = 55;
    pub const RESOURCELOCATION: u8 = 56;
    pub const SENDCVARVALUE: u8 = 57;
    pub const SENDCVARVALUE2: u8 = 58;
    pub const EXEC: u8 = 59;

    /// Every `svc_*` opcode as `(lowercase name, value)`, in opcode order.
    pub const ALL: &[(&str, u8)] = &[
        ("bad", BAD),
        ("nop", NOP),
        ("disconnect", DISCONNECT),
        ("event", EVENT),
        ("version", VERSION),
        ("setview", SETVIEW),
        ("sound", SOUND),
        ("time", TIME),
        ("print", PRINT),
        ("stufftext", STUFFTEXT),
        ("setangle", SETANGLE),
        ("serverinfo", SERVERINFO),
        ("lightstyle", LIGHTSTYLE),
        ("updateuserinfo", UPDATEUSERINFO),
        ("deltadescription", DELTADESCRIPTION),
        ("clientdata", CLIENTDATA),
        ("stopsound", STOPSOUND),
        ("pings", PINGS),
        ("particle", PARTICLE),
        ("damage", DAMAGE),
        ("spawnstatic", SPAWNSTATIC),
        ("event_reliable", EVENT_RELIABLE),
        ("spawnbaseline", SPAWNBASELINE),
        ("temp_entity", TEMP_ENTITY),
        ("setpause", SETPAUSE),
        ("signonnum", SIGNONNUM),
        ("centerprint", CENTERPRINT),
        ("killedmonster", KILLEDMONSTER),
        ("foundsecret", FOUNDSECRET),
        ("spawnstaticsound", SPAWNSTATICSOUND),
        ("intermission", INTERMISSION),
        ("finale", FINALE),
        ("cdtrack", CDTRACK),
        ("restore", RESTORE),
        ("cutscene", CUTSCENE),
        ("weaponanim", WEAPONANIM),
        ("decalname", DECALNAME),
        ("roomtype", ROOMTYPE),
        ("addangle", ADDANGLE),
        ("newusermsg", NEWUSERMSG),
        ("packetentities", PACKETENTITIES),
        ("deltapacketentities", DELTAPACKETENTITIES),
        ("choke", CHOKE),
        ("resourcelist", RESOURCELIST),
        ("newmovevars", NEWMOVEVARS),
        ("resourcerequest", RESOURCEREQUEST),
        ("customization", CUSTOMIZATION),
        ("crosshairangle", CROSSHAIRANGLE),
        ("soundfade", SOUNDFADE),
        ("filetxferfailed", FILETXFERFAILED),
        ("hltv", HLTV),
        ("director", DIRECTOR),
        ("voiceinit", VOICEINIT),
        ("voicedata", VOICEDATA),
        ("sendextrainfo", SENDEXTRAINFO),
        ("timescale", TIMESCALE),
        ("resourcelocation", RESOURCELOCATION),
        ("sendcvarvalue", SENDCVARVALUE),
        ("sendcvarvalue2", SENDCVARVALUE2),
        ("exec", EXEC),
    ];
}

pub mod clc {
    pub const BAD: u8 = 0;
    pub const NOP: u8 = 1;
    pub const MOVE: u8 = 2;
    pub const STRINGCMD: u8 = 3;
    pub const DELTA: u8 = 4;
    pub const RESOURCELIST: u8 = 5;
    pub const TMOVE: u8 = 6;
    pub const FILECONSISTENCY: u8 = 7;
    pub const VOICEDATA: u8 = 8;
    pub const HLTV: u8 = 9;
    pub const CVARVALUE: u8 = 10;
    pub const CVARVALUE2: u8 = 11;

    /// Every `clc_*` opcode as `(lowercase name, value)`, in opcode order.
    pub const ALL: &[(&str, u8)] = &[
        ("bad", BAD),
        ("nop", NOP),
        ("move", MOVE),
        ("stringcmd", STRINGCMD),
        ("delta", DELTA),
        ("resourcelist", RESOURCELIST),
        ("tmove", TMOVE),
        ("fileconsistency", FILECONSISTENCY),
        ("voicedata", VOICEDATA),
        ("hltv", HLTV),
        ("cvarvalue", CVARVALUE),
        ("cvarvalue2", CVARVALUE2),
    ];
}

pub const ENTITY_NORMAL: u8 = 1;
pub const ENTITY_BEAM: u8 = 2;

const RESOURCE_INDEX_BITS: u32 = 12;
const RES_CUSTOM: u32 = 1 << 2;

const SND_FL_VOLUME: u32 = 1 << 0;
const SND_FL_ATTENUATION: u32 = 1 << 1;
const SND_FL_LARGE_INDEX: u32 = 1 << 2;
const SND_FL_PITCH: u32 = 1 << 3;

/// Parsed `svc_serverinfo` payload (`SV_SendServerinfo_internal`, `sv_main.cpp:1129-1205`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfoMsg {
    pub protocol: u32,
    pub spawn_count: u32,
    /// Unmunged map BSP CRC (`worldmapCRC`).
    pub map_crc: u32,
    pub client_dll_md5: [u8; 16],
    pub max_clients: u8,
    pub player_slot: u8,
    pub is_deathmatch: bool,
    pub game_dir: String,
    pub hostname: String,
    pub map_path: String,
    pub map_cycle: String,
    pub vac_secure: bool,
}

/// Parsed `svc_newmovevars` payload (`SV_WriteMovevarsToClient`, `sv_main.cpp:1014-1043`).
#[derive(Debug, Clone, PartialEq)]
pub struct MoveVarsMsg {
    pub gravity: f32,
    pub stopspeed: f32,
    pub maxspeed: f32,
    pub spectatormaxspeed: f32,
    pub accelerate: f32,
    pub airaccelerate: f32,
    pub wateraccelerate: f32,
    pub friction: f32,
    pub edgefriction: f32,
    pub waterfriction: f32,
    pub entgravity: f32,
    pub bounce: f32,
    pub stepsize: f32,
    pub maxvelocity: f32,
    pub zmax: f32,
    pub wave_height: f32,
    pub footsteps: bool,
    pub rollangle: f32,
    pub rollspeed: f32,
    pub skycolor: [f32; 3],
    pub skyvec: [f32; 3],
    pub sky_name: String,
}

impl Default for MoveVarsMsg {
    fn default() -> Self {
        Self {
            gravity: 800.0,
            stopspeed: 75.0,
            maxspeed: 320.0,
            spectatormaxspeed: 500.0,
            accelerate: 5.0,
            airaccelerate: 10.0,
            wateraccelerate: 10.0,
            friction: 4.0,
            edgefriction: 2.0,
            waterfriction: 1.0,
            entgravity: 1.0,
            bounce: 1.0,
            stepsize: 18.0,
            maxvelocity: 2000.0,
            zmax: 4096.0,
            wave_height: 0.0,
            footsteps: true,
            rollangle: 0.0,
            rollspeed: 200.0,
            skycolor: [0.0; 3],
            skyvec: [0.0; 3],
            sky_name: "desert".to_string(),
        }
    }
}

/// Entry inside `svc_resourcelist` (`SV_SendResources_internal`, `sv_main.cpp:1247-1281`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    pub res_type: u8,
    pub filename: String,
    pub index: u16,
    pub download_size: u32,
    pub flags: u8,
    pub md5: Option<[u8; 16]>,
}

/// Parsed `svc_sound` payload (`sv_main.cpp:808-821`).
#[derive(Debug, Clone, PartialEq)]
pub struct SoundMsg {
    pub flags: u16,
    pub volume: u8,
    pub attenuation: f32,
    pub channel: u8,
    pub entity: u16,
    pub sound_index: u16,
    pub origin: Vec3,
    pub pitch: u8,
}

/// Parsed `svc_packetentities` / `svc_deltapacketentities` snapshot update.
#[derive(Debug, Clone, PartialEq)]
pub struct PacketEntitiesMsg {
    pub is_delta: bool,
    pub delta_from_seq: Option<u8>,
    pub expected_count: u16,
    pub entities: BTreeMap<u16, (u8, DeltaRecord)>,
    pub removed: Vec<u16>,
}

/// High-level decoded GoldSrc `svc_*` server message.
#[derive(Debug, Clone, PartialEq)]
pub enum SvcMessage {
    Nop,
    Disconnect(String),
    Version(u32),
    SetView(u16),
    Sound(SoundMsg),
    Time(f32),
    Print(String),
    StuffText(String),
    SetAngle(Vec3),
    AddAngle(f32),
    ServerInfo(Box<ServerInfoMsg>),
    LightStyle {
        index: u8,
        pattern: String,
    },
    UpdateUserInfo {
        slot: u8,
        userid: i32,
        userinfo: String,
        cdkey_hash: [u8; 16],
    },
    DeltaDescription(Box<DeltaSchema>),
    ClientData {
        delta_seq: Option<u8>,
        record: DeltaRecord,
        parsed: GoldSrcClientData,
        weapon_deltas: Vec<(u8, DeltaRecord)>,
    },
    StopSound(u16),
    Pings(Vec<(u8, u16, u8)>),
    SpawnBaseline {
        baselines: BTreeMap<u16, (u8, DeltaRecord)>,
        instance_baselines: Vec<DeltaRecord>,
    },
    TempEntity {
        te_type: u8,
        raw: Vec<u8>,
    },
    SetPause(bool),
    SignonNum(u8),
    CenterPrint(String),
    CdTrack {
        track: i8,
        loop_track: i8,
    },
    WeaponAnim {
        anim: u8,
        body: u8,
    },
    DecalName {
        index: u8,
        name: String,
    },
    RoomType(u16),
    NewUserMsg(UserMsgDesc),
    PacketEntities(Box<PacketEntitiesMsg>),
    Choke,
    ResourceList {
        resources: Vec<ResourceEntry>,
        consistency_indices: Vec<u16>,
    },
    NewMoveVars(Box<MoveVarsMsg>),
    ResourceRequest {
        spawn_count: i32,
        reserved: i32,
    },
    ResourceLocation(String),
    SendExtraInfo {
        fallback_dir: String,
        cheats: bool,
    },
    VoiceInit {
        codec: String,
        quality: u8,
    },
    VoiceData {
        player_slot: u8,
        data: Vec<u8>,
    },
    SendCvarValue(String),
    SendCvarValue2 {
        request_id: i32,
        cvar_name: String,
    },
    UserMsg {
        id: u8,
        name: String,
        decoded: CsUserMsg,
    },
    Ignored(u8),
}

/// Mutable parsing context maintained across packets of a GoldSrc 48 client session.
#[derive(Debug, Default)]
pub struct SvcParseContext {
    pub deltas: DeltaRegistry,
    pub usermsgs: UserMsgRegistry,
    pub max_clients: u8,
    pub server_time: f64,
    pub baselines: BTreeMap<u16, (u8, DeltaRecord)>,
    pub instance_baselines: Vec<DeltaRecord>,
    pub clientdata_history: BTreeMap<u8, DeltaRecord>,
    pub entity_frames: BTreeMap<u8, BTreeMap<u16, (u8, DeltaRecord)>>,
    pub last_incoming_seq: u8,
}

impl SvcParseContext {
    pub fn new() -> Self {
        Self {
            deltas: DeltaRegistry::cs16_default(),
            usermsgs: UserMsgRegistry::new(),
            max_clients: 32,
            ..Default::default()
        }
    }
}

/// Parse an entire byte stream of concatenated GoldSrc `svc_*` + `UserMsg` commands.
pub fn parse_svc_stream(
    data: &[u8],
    ctx: &mut SvcParseContext,
) -> Result<Vec<SvcMessage>, ProtoError> {
    let mut r = MsgReader::new(data);
    let mut out = Vec::new();

    while r.remaining() > 0 {
        let op = r.read_u8()?;
        let msg = if op >= USERMSG_BASE_ID {
            let desc = ctx
                .usermsgs
                .get_by_id(op)
                .cloned()
                .ok_or(ProtoError::UnknownOpcode(op))?;
            let len = if desc.size < 0 {
                r.read_u8()? as usize
            } else {
                desc.size as usize
            };
            let payload = r.read_bytes(len)?;
            let decoded = decode_cs_usermsg(&desc.name, payload).unwrap_or(CsUserMsg::Unknown {
                name: desc.name.clone(),
                payload: payload.to_vec(),
            });
            SvcMessage::UserMsg {
                id: op,
                name: desc.name,
                decoded,
            }
        } else {
            parse_one_svc(op, &mut r, ctx)?
        };
        out.push(msg);
    }

    Ok(out)
}

fn parse_one_svc(
    op: u8,
    r: &mut MsgReader<'_>,
    ctx: &mut SvcParseContext,
) -> Result<SvcMessage, ProtoError> {
    match op {
        svc::NOP => Ok(SvcMessage::Nop),
        svc::DISCONNECT => Ok(SvcMessage::Disconnect(r.read_str()?)),
        svc::EVENT => {
            r.start_bit_reading();
            let count = r.read_bits(5);
            let ev_schema = ctx.deltas.get("event_t").cloned();
            let ev_zero = ev_schema.as_ref().map(DeltaSchema::zero_record);
            for _ in 0..count {
                let _index = r.read_bits(10);
                let has_packet_idx = r.read_one_bit();
                if has_packet_idx {
                    let _pkt_idx = r.read_bits(11);
                    let has_delta = r.read_one_bit();
                    if has_delta
                        && let (Some(schema), Some(zero)) = (ev_schema.as_ref(), ev_zero.as_ref())
                    {
                        let _ = read_delta(r, schema, zero, ctx.server_time)?;
                    }
                }
                let has_fire_time = r.read_one_bit();
                if has_fire_time {
                    let _ = r.read_bits(16);
                }
            }
            r.end_bit_reading()?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::VERSION => Ok(SvcMessage::Version(r.read_u32()?)),
        svc::SETVIEW => Ok(SvcMessage::SetView(r.read_u16()?)),
        svc::SOUND => {
            r.start_bit_reading();
            let flags = r.read_bits(9) as u16;
            let volume = if (flags as u32 & SND_FL_VOLUME) != 0 {
                r.read_bits(8) as u8
            } else {
                255
            };
            let attenuation = if (flags as u32 & SND_FL_ATTENUATION) != 0 {
                r.read_bits(8) as f32 / 64.0
            } else {
                1.0
            };
            let channel = r.read_bits(3) as u8;
            let entity = r.read_bits(11) as u16;
            let sound_index = if (flags as u32 & SND_FL_LARGE_INDEX) != 0 {
                r.read_bits(16) as u16
            } else {
                r.read_bits(8) as u16
            };
            let origin = r.read_bit_vec3_coord();
            let pitch = if (flags as u32 & SND_FL_PITCH) != 0 {
                r.read_bits(8) as u8
            } else {
                100
            };
            r.end_bit_reading()?;
            Ok(SvcMessage::Sound(SoundMsg {
                flags,
                volume,
                attenuation,
                channel,
                entity,
                sound_index,
                origin,
                pitch,
            }))
        }
        svc::TIME => {
            let t = r.read_f32()?;
            ctx.server_time = t as f64;
            Ok(SvcMessage::Time(t))
        }
        svc::PRINT => Ok(SvcMessage::Print(r.read_str()?)),
        svc::STUFFTEXT => Ok(SvcMessage::StuffText(r.read_str()?)),
        svc::SETANGLE => Ok(SvcMessage::SetAngle(Vec3::new(
            r.read_hires_angle()?,
            r.read_hires_angle()?,
            r.read_hires_angle()?,
        ))),
        svc::ADDANGLE => Ok(SvcMessage::AddAngle(r.read_hires_angle()?)),
        svc::SERVERINFO => {
            let protocol = r.read_u32()?;
            let spawn_count = r.read_u32()?;
            let mut map_crc_bytes: [u8; 4] = r.read_bytes(4)?.try_into().unwrap();
            let client_dll_md5: [u8; 16] = r.read_bytes(16)?.try_into().unwrap();
            let max_clients = r.read_u8()?;
            let player_slot = r.read_u8()?;
            let is_deathmatch = r.read_u8()? != 0;
            let game_dir = r.read_str()?;
            let hostname = r.read_str()?;
            let map_path = r.read_str()?;
            let map_cycle = r.read_str()?;
            let vac_secure = r.read_u8()? != 0;

            com_unmunge3(&mut map_crc_bytes, (-1 - (player_slot as i32)) & 0xFF);
            let map_crc = u32::from_le_bytes(map_crc_bytes);
            ctx.max_clients = max_clients;

            Ok(SvcMessage::ServerInfo(Box::new(ServerInfoMsg {
                protocol,
                spawn_count,
                map_crc,
                client_dll_md5,
                max_clients,
                player_slot,
                is_deathmatch,
                game_dir,
                hostname,
                map_path,
                map_cycle,
                vac_secure,
            })))
        }
        svc::LIGHTSTYLE => {
            let index = r.read_u8()?;
            let pattern = r.read_str()?;
            Ok(SvcMessage::LightStyle { index, pattern })
        }
        svc::UPDATEUSERINFO => {
            let slot = r.read_u8()?;
            let userid = r.read_i32()?;
            let userinfo = r.read_str()?;
            let cdkey_hash: [u8; 16] = r.read_bytes(16)?.try_into().unwrap();
            Ok(SvcMessage::UpdateUserInfo {
                slot,
                userid,
                userinfo,
                cdkey_hash,
            })
        }
        svc::DELTADESCRIPTION => {
            let schema = read_delta_description(r)?;
            ctx.deltas.insert(schema.clone());
            Ok(SvcMessage::DeltaDescription(Box::new(schema)))
        }
        svc::CLIENTDATA => {
            let schema = ctx
                .deltas
                .get("clientdata_t")
                .cloned()
                .ok_or_else(|| ProtoError::Delta("missing clientdata_t schema".to_string()))?;
            let w_schema = ctx.deltas.get("weapon_data_t").cloned();

            r.start_bit_reading();
            let has_delta = r.read_one_bit();
            let delta_seq = if has_delta {
                Some(r.read_bits(8) as u8)
            } else {
                None
            };
            let from = delta_seq
                .and_then(|seq| ctx.clientdata_history.get(&seq))
                .cloned()
                .unwrap_or_else(|| schema.zero_record());
            let record = read_delta(r, &schema, &from, ctx.server_time)?;

            let mut weapon_deltas = Vec::new();
            while r.read_one_bit() {
                if r.is_bad_read() {
                    break;
                }
                let w_idx = r.read_bits(6) as u8;
                if let Some(ws) = w_schema.as_ref() {
                    let wzero = ws.zero_record();
                    let wrec = read_delta(r, ws, &wzero, ctx.server_time)?;
                    weapon_deltas.push((w_idx, wrec));
                }
            }
            r.end_bit_reading()?;

            ctx.clientdata_history
                .insert(ctx.last_incoming_seq, record.clone());
            let parsed = GoldSrcClientData::from_delta(&record, &schema);
            Ok(SvcMessage::ClientData {
                delta_seq,
                record,
                parsed,
                weapon_deltas,
            })
        }
        svc::STOPSOUND => Ok(SvcMessage::StopSound(r.read_u16()?)),
        svc::PINGS => {
            r.start_bit_reading();
            let mut pings = Vec::new();
            while r.read_one_bit() {
                if r.is_bad_read() {
                    break;
                }
                let slot = r.read_bits(5) as u8;
                let ping = r.read_bits(12) as u16;
                let loss = r.read_bits(7) as u8;
                pings.push((slot, ping, loss));
            }
            r.end_bit_reading()?;
            Ok(SvcMessage::Pings(pings))
        }
        svc::PARTICLE => {
            let _ = r.read_bytes(11)?; // 3 coords (6) + 3 dir i8 (3) + count (1) + color (1)
            Ok(SvcMessage::Ignored(op))
        }
        svc::DAMAGE => {
            let _ = r.read_bytes(8)?; // armor(1) + blood(1) + 3 coords(6)
            Ok(SvcMessage::Ignored(op))
        }
        svc::SPAWNSTATIC => {
            let _ = r.read_bytes(16)?; // short+byte+byte+short+byte + 3*(short+byte)
            let rendermode = r.read_u8()?;
            if rendermode != 0 {
                let _ = r.read_bytes(5)?;
            }
            Ok(SvcMessage::Ignored(op))
        }
        svc::EVENT_RELIABLE => {
            r.start_bit_reading();
            let _index = r.read_bits(10);
            if let Some(ev_schema) = ctx.deltas.get("event_t").cloned() {
                let zero = ev_schema.zero_record();
                let _ = read_delta(r, &ev_schema, &zero, ctx.server_time)?;
            }
            if r.read_one_bit() {
                let _ = r.read_bits(16);
            }
            r.end_bit_reading()?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::SPAWNBASELINE => {
            let ent_schema = ctx
                .deltas
                .get("entity_state_t")
                .cloned()
                .ok_or_else(|| ProtoError::Delta("missing entity_state_t".to_string()))?;
            let player_schema = ctx
                .deltas
                .get("entity_state_player_t")
                .cloned()
                .ok_or_else(|| ProtoError::Delta("missing entity_state_player_t".to_string()))?;
            let custom_schema = ctx
                .deltas
                .get("custom_entity_state_t")
                .cloned()
                .ok_or_else(|| ProtoError::Delta("missing custom_entity_state_t".to_string()))?;

            r.start_bit_reading();
            let mut baselines = BTreeMap::new();
            while r.peek_bits(16) != 0xFFFF {
                if r.is_bad_read() {
                    return Err(ProtoError::UnexpectedEof);
                }
                let entnum = r.read_bits(11) as u16;
                let ent_type = r.read_bits(2) as u8;
                let custom = (ent_type & ENTITY_NORMAL) == 0;
                let schema = if custom {
                    &custom_schema
                } else if entnum >= 1 && entnum <= ctx.max_clients as u16 {
                    &player_schema
                } else {
                    &ent_schema
                };
                let zero = schema.zero_record();
                let rec = read_delta(r, schema, &zero, ctx.server_time)?;
                baselines.insert(entnum, (ent_type, rec));
            }
            let _sentinel = r.read_bits(16);
            let num_inst = r.read_bits(6) as usize;
            let mut instance_baselines = Vec::with_capacity(num_inst);
            let ent_zero = ent_schema.zero_record();
            for _ in 0..num_inst {
                let rec = read_delta(r, &ent_schema, &ent_zero, ctx.server_time)?;
                instance_baselines.push(rec);
            }
            r.end_bit_reading()?;

            ctx.baselines = baselines.clone();
            ctx.instance_baselines = instance_baselines.clone();
            Ok(SvcMessage::SpawnBaseline {
                baselines,
                instance_baselines,
            })
        }
        svc::TEMP_ENTITY => {
            let (te_type, raw) = skip_temp_entity(r)?;
            Ok(SvcMessage::TempEntity { te_type, raw })
        }
        svc::SETPAUSE => Ok(SvcMessage::SetPause(r.read_u8()? != 0)),
        svc::SIGNONNUM => Ok(SvcMessage::SignonNum(r.read_u8()?)),
        svc::CENTERPRINT => Ok(SvcMessage::CenterPrint(r.read_str()?)),
        svc::KILLEDMONSTER | svc::FOUNDSECRET | svc::INTERMISSION => Ok(SvcMessage::Ignored(op)),
        svc::SPAWNSTATICSOUND => {
            let _ = r.read_bytes(14)?; // 3*coord(6) + short(2) + byte(1) + byte(1) + short(2) + byte(1) + byte(1)
            Ok(SvcMessage::Ignored(op))
        }
        svc::FINALE | svc::CUTSCENE | svc::EXEC => {
            let _ = r.read_str()?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::CDTRACK => Ok(SvcMessage::CdTrack {
            track: r.read_i8()?,
            loop_track: r.read_i8()?,
        }),
        svc::RESTORE => {
            let _ = r.read_str()?;
            let count = r.read_u8()?;
            for _ in 0..count {
                let _ = r.read_str()?;
            }
            Ok(SvcMessage::Ignored(op))
        }
        svc::WEAPONANIM => Ok(SvcMessage::WeaponAnim {
            anim: r.read_u8()?,
            body: r.read_u8()?,
        }),
        svc::DECALNAME => Ok(SvcMessage::DecalName {
            index: r.read_u8()?,
            name: r.read_str()?,
        }),
        svc::ROOMTYPE => Ok(SvcMessage::RoomType(r.read_u16()?)),
        svc::NEWUSERMSG => {
            let id = r.read_u8()?;
            let size = r.read_i8()?;
            let raw_name = r.read_bytes(16)?;
            let nul = raw_name.iter().position(|&b| b == 0).unwrap_or(16);
            let name = String::from_utf8_lossy(&raw_name[..nul]).into_owned();
            ctx.usermsgs.register(id, size, name.clone());
            Ok(SvcMessage::NewUserMsg(UserMsgDesc { id, size, name }))
        }
        svc::PACKETENTITIES | svc::DELTAPACKETENTITIES => {
            let is_delta = op == svc::DELTAPACKETENTITIES;
            let expected_count = r.read_u16()?;
            let delta_from_seq = if is_delta { Some(r.read_u8()?) } else { None };
            let msg = parse_packet_entities(r, ctx, is_delta, delta_from_seq, expected_count)?;
            Ok(SvcMessage::PacketEntities(Box::new(msg)))
        }
        svc::CHOKE => Ok(SvcMessage::Choke),
        svc::RESOURCELIST => {
            r.start_bit_reading();
            let count = r.read_bits(RESOURCE_INDEX_BITS) as usize;
            let mut resources = Vec::with_capacity(count);
            for _ in 0..count {
                let res_type = r.read_bits(4) as u8;
                let filename = r.read_bit_string();
                let index = r.read_bits(RESOURCE_INDEX_BITS) as u16;
                let download_size = r.read_bits(24);
                let flags = r.read_bits(3) as u8;
                let md5 = if (flags as u32 & RES_CUSTOM) != 0 {
                    let b = r.read_bit_data(16);
                    b.try_into().ok()
                } else {
                    None
                };
                if r.read_one_bit() {
                    let _reserved = r.read_bit_data(32);
                }
                resources.push(ResourceEntry {
                    res_type,
                    filename,
                    index,
                    download_size,
                    flags,
                    md5,
                });
            }
            let mut consistency_indices = Vec::new();
            if r.read_one_bit() {
                let mut last = 0u16;
                while r.read_one_bit() {
                    if r.is_bad_read() {
                        break;
                    }
                    let idx = if r.read_one_bit() {
                        last.wrapping_add(r.read_bits(5) as u16)
                    } else {
                        r.read_bits(10) as u16
                    };
                    consistency_indices.push(idx);
                    last = idx;
                }
            }
            r.end_bit_reading()?;
            Ok(SvcMessage::ResourceList {
                resources,
                consistency_indices,
            })
        }
        svc::NEWMOVEVARS => {
            let mv = MoveVarsMsg {
                gravity: r.read_f32()?,
                stopspeed: r.read_f32()?,
                maxspeed: r.read_f32()?,
                spectatormaxspeed: r.read_f32()?,
                accelerate: r.read_f32()?,
                airaccelerate: r.read_f32()?,
                wateraccelerate: r.read_f32()?,
                friction: r.read_f32()?,
                edgefriction: r.read_f32()?,
                waterfriction: r.read_f32()?,
                entgravity: r.read_f32()?,
                bounce: r.read_f32()?,
                stepsize: r.read_f32()?,
                maxvelocity: r.read_f32()?,
                zmax: r.read_f32()?,
                wave_height: r.read_f32()?,
                footsteps: r.read_u8()? != 0,
                rollangle: r.read_f32()?,
                rollspeed: r.read_f32()?,
                skycolor: [r.read_f32()?, r.read_f32()?, r.read_f32()?],
                skyvec: [r.read_f32()?, r.read_f32()?, r.read_f32()?],
                sky_name: r.read_str()?,
            };
            Ok(SvcMessage::NewMoveVars(Box::new(mv)))
        }
        svc::RESOURCEREQUEST => Ok(SvcMessage::ResourceRequest {
            spawn_count: r.read_i32()?,
            reserved: r.read_i32()?,
        }),
        svc::CUSTOMIZATION => {
            let _player = r.read_u8()?;
            let _type = r.read_u8()?;
            let _name = r.read_str()?;
            let _index = r.read_u16()?;
            let _dl_size = r.read_u32()?;
            let flags = r.read_u8()?;
            if (flags as u32 & RES_CUSTOM) != 0 {
                let _ = r.read_bytes(16)?;
            }
            Ok(SvcMessage::Ignored(op))
        }
        svc::CROSSHAIRANGLE => {
            let _ = r.read_bytes(2)?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::SOUNDFADE => {
            let _ = r.read_bytes(4)?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::FILETXFERFAILED => {
            let _ = r.read_str()?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::HLTV => {
            let _ = r.read_u8()?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::DIRECTOR => {
            let len = r.read_u8()? as usize;
            let _ = r.read_bytes(len)?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::VOICEINIT => Ok(SvcMessage::VoiceInit {
            codec: r.read_str()?,
            quality: r.read_u8()?,
        }),
        svc::VOICEDATA => {
            let player_slot = r.read_u8()?;
            let len = r.read_u16()? as usize;
            let data = r.read_bytes(len)?.to_vec();
            Ok(SvcMessage::VoiceData { player_slot, data })
        }
        svc::SENDEXTRAINFO => Ok(SvcMessage::SendExtraInfo {
            fallback_dir: r.read_str()?,
            cheats: r.read_u8()? != 0,
        }),
        svc::TIMESCALE => {
            let _ = r.read_f32()?;
            Ok(SvcMessage::Ignored(op))
        }
        svc::RESOURCELOCATION => Ok(SvcMessage::ResourceLocation(r.read_str()?)),
        svc::SENDCVARVALUE => Ok(SvcMessage::SendCvarValue(r.read_str()?)),
        svc::SENDCVARVALUE2 => Ok(SvcMessage::SendCvarValue2 {
            request_id: r.read_i32()?,
            cvar_name: r.read_str()?,
        }),
        other => Err(ProtoError::UnknownOpcode(other)),
    }
}

fn parse_packet_entities(
    r: &mut MsgReader<'_>,
    ctx: &mut SvcParseContext,
    is_delta: bool,
    delta_from_seq: Option<u8>,
    expected_count: u16,
) -> Result<PacketEntitiesMsg, ProtoError> {
    let ent_schema = ctx
        .deltas
        .get("entity_state_t")
        .cloned()
        .ok_or_else(|| ProtoError::Delta("missing entity_state_t".to_string()))?;
    let player_schema = ctx
        .deltas
        .get("entity_state_player_t")
        .cloned()
        .ok_or_else(|| ProtoError::Delta("missing entity_state_player_t".to_string()))?;
    let custom_schema = ctx
        .deltas
        .get("custom_entity_state_t")
        .cloned()
        .ok_or_else(|| ProtoError::Delta("missing custom_entity_state_t".to_string()))?;

    let prev_frame = delta_from_seq
        .and_then(|seq| ctx.entity_frames.get(&seq))
        .cloned()
        .unwrap_or_default();

    let mut frame_entities = if is_delta {
        prev_frame.clone()
    } else {
        BTreeMap::new()
    };
    let mut ordered_full: Vec<DeltaRecord> = Vec::new();
    let mut removed = Vec::new();
    let mut num_base: u16 = 0;
    let full = !is_delta;

    r.start_bit_reading();
    loop {
        if r.peek_bits(16) == 0 {
            let _ = r.read_bits(16);
            break;
        }
        if r.is_bad_read() {
            return Err(ProtoError::UnexpectedEof);
        }

        let (entnum, is_remove) = if full {
            let increment = r.read_one_bit();
            let num = if increment {
                num_base.wrapping_add(1)
            } else if r.read_one_bit() {
                r.read_bits(11) as u16
            } else {
                num_base.wrapping_add(r.read_bits(6) as u16)
            };
            (num, false)
        } else {
            let remove = r.read_one_bit();
            let num = if r.read_one_bit() {
                r.read_bits(11) as u16
            } else {
                num_base.wrapping_add(r.read_bits(6) as u16)
            };
            (num, remove)
        };
        num_base = entnum;

        if is_remove {
            frame_entities.remove(&entnum);
            removed.push(entnum);
            continue;
        }

        let custom = r.read_one_bit();
        let mut custom_bl_idx = None;
        if !ctx.instance_baselines.is_empty() && r.read_one_bit() {
            custom_bl_idx = Some(r.read_bits(6) as usize);
        }
        let mut prev_offset = None;
        if full && custom_bl_idx.is_none() && r.read_one_bit() {
            prev_offset = Some(r.read_bits(6) as usize);
        }

        let schema = if custom {
            &custom_schema
        } else if entnum >= 1 && entnum <= ctx.max_clients as u16 {
            &player_schema
        } else {
            &ent_schema
        };

        let base_rec = if let Some(idx) = custom_bl_idx {
            ctx.instance_baselines
                .get(idx)
                .cloned()
                .unwrap_or_else(|| schema.zero_record())
        } else if let Some(off) = prev_offset {
            ordered_full
                .len()
                .checked_sub(off)
                .and_then(|i| ordered_full.get(i))
                .cloned()
                .unwrap_or_else(|| schema.zero_record())
        } else if is_delta && let Some((_, prev)) = prev_frame.get(&entnum) {
            prev.clone()
        } else if let Some((_, bl)) = ctx.baselines.get(&entnum) {
            bl.clone()
        } else {
            schema.zero_record()
        };

        let rec = read_delta(r, schema, &base_rec, ctx.server_time)?;
        let ent_type = if custom { ENTITY_BEAM } else { ENTITY_NORMAL };
        if full {
            ordered_full.push(rec.clone());
        }
        frame_entities.insert(entnum, (ent_type, rec));
    }
    r.end_bit_reading()?;

    ctx.entity_frames
        .insert(ctx.last_incoming_seq, frame_entities.clone());

    Ok(PacketEntitiesMsg {
        is_delta,
        delta_from_seq,
        expected_count,
        entities: frame_entities,
        removed,
    })
}

/// Advance over any GoldSrc `svc_temp_entity` (`TE_*`) payload with exact wire byte length.
fn skip_temp_entity(r: &mut MsgReader<'_>) -> Result<(u8, Vec<u8>), ProtoError> {
    let te_type = r.read_u8()?;
    let start = r.pos();
    match te_type {
        0 | 19 | 20 | 21 => {
            let _ = r.read_bytes(24)?;
        }
        1 => {
            let _ = r.read_bytes(20)?;
        }
        2 | 4 | 9 | 10 | 11 => {
            let _ = r.read_bytes(6)?;
        }
        3 => {
            let _ = r.read_bytes(11)?;
        }
        5 | 17 | 23 => {
            let _ = r.read_bytes(10)?;
        }
        6 | 102 => {
            let _ = r.read_bytes(12)?;
        }
        7 | 30 | 31 => {
            let _ = r.read_bytes(17)?;
        }
        8 | 24 => {
            let _ = r.read_bytes(16)?;
        }
        12 => {
            let _ = r.read_bytes(8)?;
        }
        13 => {
            // TE_BSPDECAL: 3 coords (6) + short decal (2) + short ent (2) + optional short model (2)
            let _ = r.read_bytes(8)?;
            let ent = r.read_i16()?;
            if ent != 0 {
                let _ = r.read_i16()?;
            }
        }
        14 | 104 | 118 => {
            let _ = r.read_bytes(9)?;
        }
        15 => {
            let _ = r.read_bytes(19)?;
        }
        18 | 27 | 28 | 107 => {
            let _ = r.read_bytes(16)?;
        }
        22 => {
            let _ = r.read_bytes(10)?;
        }
        25 => {
            let _ = r.read_bytes(19)?;
        }
        29 => {
            // TE_TEXTMESSAGE
            let _channel = r.read_u8()?;
            let _x = r.read_i16()?;
            let _y = r.read_i16()?;
            let effect = r.read_u8()?;
            let _colors = r.read_bytes(8)?;
            let _fade_hold = r.read_bytes(6)?;
            if effect == 2 {
                let _fxtime = r.read_u16()?;
            }
            let _text = r.read_str()?;
        }
        99 => {
            let _ = r.read_bytes(2)?;
        }
        100 | 122 => {
            let _ = r.read_bytes(10)?;
        }
        101 | 103 => {
            let _ = r.read_bytes(14)?;
        }
        105 => {
            let _ = r.read_bytes(5)?;
        }
        106 => {
            let _ = r.read_bytes(17)?;
        }
        108 => {
            let _ = r.read_bytes(24)?;
        }
        109 => {
            let _ = r.read_bytes(9)?;
        }
        110 => {
            let _ = r.read_bytes(17)?;
        }
        111 | 116 | 117 => {
            let _ = r.read_bytes(7)?;
        }
        112 => {
            // TE_PLAYERDECAL
            let _player = r.read_u8()?;
            let _coords = r.read_bytes(6)?;
            let ent = r.read_i16()?;
            let _decal = r.read_u8()?;
            if ent != 0 {
                let _ = r.read_i16()?;
            }
        }
        113 | 114 => {
            let _ = r.read_bytes(19)?;
        }
        115 => {
            let _ = r.read_bytes(12)?;
        }
        119 => {
            let _ = r.read_bytes(16)?;
        }
        120 => {
            let _ = r.read_bytes(18)?;
        }
        121 => {
            let _ = r.read_bytes(5)?;
        }
        123 => {
            let _ = r.read_bytes(13)?;
        }
        124 => {
            let _ = r.read_bytes(7)?;
        }
        125 => {
            let _ = r.read_bytes(1)?;
        }
        126 => {
            let _ = r.read_bytes(18)?;
        }
        127 => {
            let _ = r.read_bytes(15)?;
        }
        other => {
            return Err(ProtoError::InvalidPayload(format!(
                "unknown TE_* temp entity type {other}"
            )));
        }
    }
    let end = r.pos();
    // Re-slice exact bytes read for callers that want to inspect them.
    let len = end - start;
    Ok((te_type, vec![0u8; len]))
}

// ============================================================================
// Server-side `svc_*` encoders (`cs-server` → Valve CS 1.6 & `cs-client`)
// ============================================================================

pub fn write_svc_print(w: &mut MsgWriter, text: &str) {
    w.write_u8(svc::PRINT);
    w.write_str(text);
}

pub fn write_svc_stufftext(w: &mut MsgWriter, cmd: &str) {
    w.write_u8(svc::STUFFTEXT);
    w.write_str(cmd);
}

pub fn write_svc_disconnect(w: &mut MsgWriter, reason: &str) {
    w.write_u8(svc::DISCONNECT);
    w.write_str(reason);
}

pub fn write_svc_time(w: &mut MsgWriter, server_time: f32) {
    w.write_u8(svc::TIME);
    w.write_f32(server_time);
}

pub fn write_svc_setview(w: &mut MsgWriter, entindex: u16) {
    w.write_u8(svc::SETVIEW);
    w.write_u16(entindex);
}

pub fn write_svc_setangle(w: &mut MsgWriter, angles: Vec3) {
    w.write_u8(svc::SETANGLE);
    w.write_hires_angle(angles.x);
    w.write_hires_angle(angles.y);
    w.write_hires_angle(angles.z);
}

pub fn write_svc_signonnum(w: &mut MsgWriter, num: u8) {
    w.write_u8(svc::SIGNONNUM);
    w.write_u8(num);
}

pub fn write_svc_lightstyle(w: &mut MsgWriter, index: u8, pattern: &str) {
    w.write_u8(svc::LIGHTSTYLE);
    w.write_u8(index);
    w.write_str(pattern);
}

pub fn write_svc_cdtrack(w: &mut MsgWriter, track: i8, loop_track: i8) {
    w.write_u8(svc::CDTRACK);
    w.write_i8(track);
    w.write_i8(loop_track);
}

pub fn write_svc_voiceinit(w: &mut MsgWriter, codec: &str, quality: u8) {
    w.write_u8(svc::VOICEINIT);
    w.write_str(codec);
    w.write_u8(quality);
}

pub fn write_svc_sendextrainfo(w: &mut MsgWriter, fallback_dir: &str, cheats: bool) {
    w.write_u8(svc::SENDEXTRAINFO);
    w.write_str(fallback_dir);
    w.write_u8(u8::from(cheats));
}

pub fn write_svc_serverinfo(w: &mut MsgWriter, info: &ServerInfoMsg) {
    w.write_u8(svc::SERVERINFO);
    w.write_u32(PROTOCOL_VERSION);
    w.write_u32(info.spawn_count);
    let mut crc_bytes = info.map_crc.to_le_bytes();
    com_munge3(&mut crc_bytes, (-1 - (info.player_slot as i32)) & 0xFF);
    w.write_bytes(&crc_bytes);
    w.write_bytes(&info.client_dll_md5);
    w.write_u8(info.max_clients);
    w.write_u8(info.player_slot);
    w.write_u8(u8::from(info.is_deathmatch));
    w.write_str(&info.game_dir);
    w.write_str(&info.hostname);
    w.write_str(&info.map_path);
    w.write_str(&info.map_cycle);
    w.write_u8(u8::from(info.vac_secure));
}

pub fn write_svc_deltadescriptions(w: &mut MsgWriter, reg: &DeltaRegistry) {
    for schema in reg.iter() {
        w.write_u8(svc::DELTADESCRIPTION);
        write_delta_description(w, schema);
    }
}

pub fn write_svc_newmovevars(w: &mut MsgWriter, mv: &MoveVarsMsg) {
    w.write_u8(svc::NEWMOVEVARS);
    w.write_f32(mv.gravity);
    w.write_f32(mv.stopspeed);
    w.write_f32(mv.maxspeed);
    w.write_f32(mv.spectatormaxspeed);
    w.write_f32(mv.accelerate);
    w.write_f32(mv.airaccelerate);
    w.write_f32(mv.wateraccelerate);
    w.write_f32(mv.friction);
    w.write_f32(mv.edgefriction);
    w.write_f32(mv.waterfriction);
    w.write_f32(mv.entgravity);
    w.write_f32(mv.bounce);
    w.write_f32(mv.stepsize);
    w.write_f32(mv.maxvelocity);
    w.write_f32(mv.zmax);
    w.write_f32(mv.wave_height);
    w.write_u8(u8::from(mv.footsteps));
    w.write_f32(mv.rollangle);
    w.write_f32(mv.rollspeed);
    w.write_f32(mv.skycolor[0]);
    w.write_f32(mv.skycolor[1]);
    w.write_f32(mv.skycolor[2]);
    w.write_f32(mv.skyvec[0]);
    w.write_f32(mv.skyvec[1]);
    w.write_f32(mv.skyvec[2]);
    w.write_str(&mv.sky_name);
}

pub fn write_svc_userregs(w: &mut MsgWriter, reg: &UserMsgRegistry) {
    for desc in reg.iter() {
        w.write_u8(svc::NEWUSERMSG);
        w.write_u8(desc.id);
        w.write_i8(desc.size);
        w.write_fixed_str(&desc.name, 16);
    }
}

pub fn write_svc_updateuserinfo(
    w: &mut MsgWriter,
    slot: u8,
    userid: i32,
    userinfo: &str,
    cdkey_hash: &[u8; 16],
) {
    w.write_u8(svc::UPDATEUSERINFO);
    w.write_u8(slot);
    w.write_i32(userid);
    w.write_str(userinfo);
    w.write_bytes(cdkey_hash);
}

pub fn write_svc_resourcerequest(w: &mut MsgWriter, spawn_count: i32) {
    w.write_u8(svc::RESOURCEREQUEST);
    w.write_i32(spawn_count);
    w.write_i32(0);
}

pub fn write_svc_resourcelist(w: &mut MsgWriter, resources: &[ResourceEntry]) {
    w.write_u8(svc::RESOURCELIST);
    w.start_bit_writing();
    w.write_bits(resources.len() as u32, RESOURCE_INDEX_BITS);
    for r in resources {
        w.write_bits(r.res_type as u32, 4);
        w.write_bit_string(&r.filename);
        w.write_bits(r.index as u32, RESOURCE_INDEX_BITS);
        w.write_bits(r.download_size, 24);
        w.write_bits((r.flags & 0x07) as u32, 3);
        if (r.flags as u32 & RES_CUSTOM) != 0
            && let Some(md5) = r.md5
        {
            w.write_bit_data(&md5);
        }
        w.write_one_bit(false); // reserved = 0
    }
    w.write_one_bit(false); // no force_unmodified consistency list
    w.end_bit_writing();
}

pub fn write_svc_spawnbaseline(
    w: &mut MsgWriter,
    reg: &DeltaRegistry,
    max_clients: u8,
    baselines: &[(u16, GoldSrcEntityState)],
    server_time: f64,
) {
    let Some(ent_schema) = reg.get("entity_state_t") else {
        return;
    };
    let Some(player_schema) = reg.get("entity_state_player_t") else {
        return;
    };
    let Some(custom_schema) = reg.get("custom_entity_state_t") else {
        return;
    };

    w.write_u8(svc::SPAWNBASELINE);
    w.start_bit_writing();
    for (entnum, state) in baselines {
        let ent_type = if state.entity_type == 0 {
            ENTITY_NORMAL
        } else {
            state.entity_type
        };
        let custom = (ent_type & ENTITY_NORMAL) == 0;
        let schema = if custom {
            custom_schema
        } else if *entnum >= 1 && *entnum <= max_clients as u16 {
            player_schema
        } else {
            ent_schema
        };
        let zero = schema.zero_record();
        let rec = state.to_delta(schema);
        w.write_bits(*entnum as u32, 11);
        w.write_bits(ent_type as u32, 2);
        write_delta(w, schema, &zero, &rec, true, server_time);
    }
    w.write_bits(0xFFFF, 16);
    w.write_bits(0, 6); // 0 instance baselines
    w.end_bit_writing();
}

pub fn write_svc_clientdata(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    from: Option<(u8, &DeltaRecord)>,
    to: &GoldSrcClientData,
    server_time: f64,
) -> DeltaRecord {
    let to_rec = to.to_delta(schema);
    let zero = schema.zero_record();
    w.write_u8(svc::CLIENTDATA);
    w.start_bit_writing();
    if let Some((seq, prev)) = from {
        w.write_one_bit(true);
        w.write_bits(seq as u32, 8);
        write_delta(w, schema, prev, &to_rec, true, server_time);
    } else {
        w.write_one_bit(false);
        write_delta(w, schema, &zero, &to_rec, true, server_time);
    }
    w.write_one_bit(false); // end weapon_data_t list
    w.end_bit_writing();
    to_rec
}

/// Write a full (`svc_packetentities`) entity snapshot (sorted by entity number).
pub fn write_svc_packetentities_full(
    w: &mut MsgWriter,
    reg: &DeltaRegistry,
    max_clients: u8,
    entities: &[GoldSrcEntityState],
    server_time: f64,
) {
    let Some(ent_schema) = reg.get("entity_state_t") else {
        return;
    };
    let Some(player_schema) = reg.get("entity_state_player_t") else {
        return;
    };

    let mut sorted: Vec<&GoldSrcEntityState> = entities.iter().collect();
    sorted.sort_by_key(|e| e.number);

    w.write_u8(svc::PACKETENTITIES);
    w.write_u16(sorted.len() as u16);
    w.start_bit_writing();

    let mut num_base: i32 = 0;
    for ent in sorted {
        let num = ent.number as i32;
        let schema = if ent.number >= 1 && ent.number <= max_clients as u16 {
            player_schema
        } else {
            ent_schema
        };
        let zero = schema.zero_record();
        let rec = ent.to_delta(schema);

        let delta = num - num_base;
        if delta == 1 {
            w.write_one_bit(true);
        } else {
            w.write_one_bit(false);
            if delta <= 0 || delta > 63 {
                w.write_one_bit(true);
                w.write_bits(num as u32, 11);
            } else {
                w.write_one_bit(false);
                w.write_bits(delta as u32, 6);
            }
        }
        num_base = num;
        w.write_one_bit(false); // custom = false
        w.write_one_bit(false); // offset = 0 (no instance baselines -> no newbl bit)
        write_delta(w, schema, &zero, &rec, true, server_time);
    }

    w.write_bits(0, 16);
    w.end_bit_writing();
}

/// Write a `CsUserMsg` onto the server stream using the registered `UserMsgRegistry`.
pub fn write_svc_usermsg(w: &mut MsgWriter, reg: &UserMsgRegistry, msg: &CsUserMsg) -> bool {
    let (name, payload) = encode_cs_usermsg(msg);
    let Some(id) = reg.id_for_name(name) else {
        return false;
    };
    let Some(desc) = reg.get_by_id(id) else {
        return false;
    };
    w.write_u8(id);
    if desc.size < 0 {
        let len = payload.len().min(255);
        w.write_u8(len as u8);
        w.write_bytes(&payload[..len]);
    } else {
        w.write_bytes(&payload);
    }
    true
}

// ============================================================================
// Client-to-server `clc_*` codecs (`SV_ExecuteClientMessage`, `sv_user.cpp`)
// ============================================================================

/// High-level decoded GoldSrc `clc_*` client message.
#[derive(Debug, Clone, PartialEq)]
pub enum ClcMessage {
    Nop,
    Move {
        packet_loss: u8,
        voice_loopback: bool,
        num_backup: u8,
        /// Commands ordered newest-first (`cmds[0]` = most recent), matching `SV_ParseMove`.
        cmds: Vec<GoldSrcUserCmd>,
    },
    StringCmd(String),
    Delta(u8),
    ResourceList(Vec<ResourceEntry>),
    FileConsistency(Vec<u8>),
    VoiceData(Vec<u8>),
    CvarValue(String),
    CvarValue2 {
        request_id: i32,
        cvar_name: String,
        value: String,
    },
}

/// Encode `clc_stringcmd` (opcode 3).
pub fn write_clc_stringcmd(w: &mut MsgWriter, cmd: &str) {
    w.write_u8(clc::STRINGCMD);
    w.write_str(cmd);
}

/// Encode `clc_delta` (opcode 4) — tells the server which packet sequence to delta-compress from.
pub fn write_clc_delta(w: &mut MsgWriter, seq: u8) {
    w.write_u8(clc::DELTA);
    w.write_u8(seq);
}

/// Encode `clc_cvarvalue2` (opcode 11) in response to `svc_sendcvarvalue2`.
pub fn write_clc_cvarvalue2(w: &mut MsgWriter, request_id: i32, cvar_name: &str, value: &str) {
    w.write_u8(clc::CVARVALUE2);
    w.write_i32(request_id);
    w.write_str(cvar_name);
    w.write_str(value);
}

/// Encode `clc_move` (opcode 2) with exact GoldSrc `COM_BlockSequenceCRCByte` + `COM_Munge`
/// (`SV_ParseMove`, `sv_user.cpp:1565-1635`).
///
/// `cmds` must be ordered **newest-first** (`cmds[0]` is most recent); on the wire GoldSrc
/// transmits oldest-first and delta-compresses each command against the previous one.
pub fn write_clc_move(
    w: &mut MsgWriter,
    usercmd_schema: &DeltaSchema,
    outgoing_sequence: u32,
    packet_loss: u8,
    num_backup: u8,
    cmds_newest_first: &[GoldSrcUserCmd],
) {
    let total = cmds_newest_first.len();
    let num_cmds = total.saturating_sub(num_backup as usize) as u8;

    let mut body = MsgWriter::with_capacity(64);
    body.write_u8(packet_loss & 0x7F);
    body.write_u8(num_backup);
    body.write_u8(num_cmds);

    let mut prev = usercmd_schema.zero_record();
    for cmd in cmds_newest_first.iter().rev() {
        let cur = cmd.to_delta(usercmd_schema);
        body.start_bit_writing();
        let _ = write_delta(&mut body, usercmd_schema, &prev, &cur, true, 0.0);
        body.end_bit_writing();
        prev = cur;
    }

    let mut body_bytes = body.into_bytes();
    let crc = com_block_sequence_crc_byte(&body_bytes, outgoing_sequence);
    com_munge(&mut body_bytes, outgoing_sequence as i32);

    w.write_u8(clc::MOVE);
    w.write_u8(body_bytes.len().min(255) as u8);
    w.write_u8(crc);
    w.write_bytes(&body_bytes);
}

/// Parse an entire stream of concatenated GoldSrc `clc_*` client messages.
pub fn parse_clc_stream(
    data: &[u8],
    usercmd_schema: &DeltaSchema,
    incoming_sequence: u32,
) -> Result<Vec<ClcMessage>, ProtoError> {
    let mut r = MsgReader::new(data);
    let mut out = Vec::new();

    while r.remaining() > 0 {
        let op = r.read_u8()?;
        match op {
            clc::NOP => out.push(ClcMessage::Nop),
            clc::MOVE => {
                let mlen = r.read_u8()? as usize;
                let expected_crc = r.read_u8()?;
                let mut payload = r.read_bytes(mlen)?.to_vec();
                com_unmunge(&mut payload, incoming_sequence as i32);
                let actual_crc = com_block_sequence_crc_byte(&payload, incoming_sequence);
                if actual_crc != expected_crc {
                    return Err(ProtoError::InvalidPayload(format!(
                        "clc_move CRC mismatch: expected 0x{expected_crc:02X}, got 0x{actual_crc:02X}"
                    )));
                }
                let mut br = MsgReader::new(&payload);
                let loss_byte = br.read_u8()?;
                let num_backup = br.read_u8()?;
                let num_cmds = br.read_u8()?;
                let total = (num_backup as usize) + (num_cmds as usize);
                let mut cmds_oldest_first = Vec::with_capacity(total);
                let mut prev = usercmd_schema.zero_record();
                for _ in 0..total {
                    br.start_bit_reading();
                    let cur = read_delta(&mut br, usercmd_schema, &prev, 0.0)?;
                    br.end_bit_reading()?;
                    cmds_oldest_first.push(GoldSrcUserCmd::from_delta(&cur, usercmd_schema));
                    prev = cur;
                }
                cmds_oldest_first.reverse();
                out.push(ClcMessage::Move {
                    packet_loss: loss_byte & 0x7F,
                    voice_loopback: (loss_byte >> 7) != 0,
                    num_backup,
                    cmds: cmds_oldest_first,
                });
            }
            clc::STRINGCMD => out.push(ClcMessage::StringCmd(r.read_str()?)),
            clc::DELTA => out.push(ClcMessage::Delta(r.read_u8()?)),
            clc::RESOURCELIST => {
                let count = r.read_u16()? as usize;
                let mut list = Vec::with_capacity(count);
                for _ in 0..count {
                    let filename = r.read_str()?;
                    let res_type = r.read_u8()?;
                    let index = r.read_u16()?;
                    let download_size = r.read_u32()?;
                    let flags = r.read_u8()?;
                    let md5 = if (flags as u32 & RES_CUSTOM) != 0 {
                        r.read_bytes(16)?.try_into().ok()
                    } else {
                        None
                    };
                    list.push(ResourceEntry {
                        res_type,
                        filename,
                        index,
                        download_size,
                        flags,
                        md5,
                    });
                }
                out.push(ClcMessage::ResourceList(list));
            }
            clc::TMOVE => {
                let _ = r.read_bytes(12)?;
            }
            clc::FILECONSISTENCY => {
                let len = r.read_u16()? as usize;
                let bytes = r.read_bytes(len)?.to_vec();
                out.push(ClcMessage::FileConsistency(bytes));
            }
            clc::VOICEDATA => {
                let len = r.read_u16()? as usize;
                let bytes = r.read_bytes(len)?.to_vec();
                out.push(ClcMessage::VoiceData(bytes));
            }
            clc::HLTV => {}
            clc::CVARVALUE => out.push(ClcMessage::CvarValue(r.read_str()?)),
            clc::CVARVALUE2 => out.push(ClcMessage::CvarValue2 {
                request_id: r.read_i32()?,
                cvar_name: r.read_str()?,
                value: r.read_str()?,
            }),
            other => return Err(ProtoError::UnknownOpcode(other)),
        }
    }

    Ok(out)
}

/// Suppress dead-code warnings for helper used by future delta packet encoder.
pub fn entity_delta_differs(schema: &DeltaSchema, a: &DeltaRecord, b: &DeltaRecord) -> bool {
    has_delta_changes(schema, a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_signon_and_gameplay_svc_and_clc_streams_roundtrip() {
        let reg = DeltaRegistry::cs16_default();
        let ureg = UserMsgRegistry::cs16_default();

        // Build a complete server signon stream (`SV_New_f` + `SV_SendRes_f` + `SV_WriteSpawn`).
        let mut sw = MsgWriter::new();
        write_svc_print(&mut sw, "BUILD 6153 SERVER\n");
        write_svc_serverinfo(
            &mut sw,
            &ServerInfoMsg {
                protocol: 48,
                spawn_count: 1,
                map_crc: 0x1234_5678,
                client_dll_md5: [0xAB; 16],
                max_clients: 16,
                player_slot: 0,
                is_deathmatch: true,
                game_dir: "cstrike".to_string(),
                hostname: "cs16-rs Protocol 48".to_string(),
                map_path: "maps/de_dust2.bsp".to_string(),
                map_cycle: "de_dust2\nde_inferno\n".to_string(),
                vac_secure: false,
            },
        );
        write_svc_sendextrainfo(&mut sw, "valve", false);
        write_svc_deltadescriptions(&mut sw, &reg);
        write_svc_newmovevars(&mut sw, &MoveVarsMsg::default());
        write_svc_userregs(&mut sw, &ureg);
        write_svc_resourcerequest(&mut sw, 1);
        write_svc_resourcelist(
            &mut sw,
            &[ResourceEntry {
                res_type: 2,
                filename: "maps/de_dust2.bsp".to_string(),
                index: 1,
                download_size: 1024,
                flags: 1,
                md5: None,
            }],
        );
        write_svc_time(&mut sw, 12.5);
        let player_ent = GoldSrcEntityState {
            number: 1,
            entity_type: ENTITY_NORMAL,
            origin: Vec3::new(100.0, -200.0, 64.0),
            angles: Vec3::new(0.0, 90.0, 0.0),
            modelindex: 12,
            sequence: 4,
            ..Default::default()
        };
        write_svc_spawnbaseline(&mut sw, &reg, 16, &[(1, player_ent.clone())], 12.5);
        let cd_schema = reg.get("clientdata_t").unwrap();
        write_svc_clientdata(
            &mut sw,
            cd_schema,
            None,
            &GoldSrcClientData {
                origin: Vec3::new(100.0, -200.0, 64.0),
                health: 100.0,
                maxspeed: 250.0,
                fov: 90.0,
                ..Default::default()
            },
            12.5,
        );
        write_svc_packetentities_full(&mut sw, &reg, 16, &[player_ent], 12.5);
        assert!(write_svc_usermsg(
            &mut sw,
            &ureg,
            &CsUserMsg::Money {
                amount: 800,
                flash: false
            }
        ));
        write_svc_signonnum(&mut sw, 1);

        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(sw.as_slice(), &mut ctx).unwrap();

        assert!(parsed.iter().any(|m| matches!(
            m,
            SvcMessage::ServerInfo(si) if si.map_crc == 0x1234_5678 && si.map_path == "maps/de_dust2.bsp"
        )));
        assert!(parsed.iter().any(|m| matches!(
            m,
            SvcMessage::ClientData { parsed, .. } if (parsed.health - 100.0).abs() < 0.1 && (parsed.origin.x - 100.0).abs() < 0.1
        )));
        assert!(parsed.iter().any(|m| matches!(
            m,
            SvcMessage::PacketEntities(pe) if pe.entities.contains_key(&1)
        )));
        assert!(parsed.iter().any(|m| matches!(
            m,
            SvcMessage::UserMsg { decoded: CsUserMsg::Money { amount: 800, .. }, .. }
        )));
        assert!(parsed.iter().any(|m| matches!(m, SvcMessage::SignonNum(1))));

        // Round-trip `clc_move` + `clc_stringcmd` + `clc_delta`.
        let ucmd_schema = reg.get("usercmd_t").unwrap();
        let mut cw = MsgWriter::new();
        write_clc_delta(&mut cw, 42);
        write_clc_move(
            &mut cw,
            ucmd_schema,
            77,
            0,
            1,
            &[
                GoldSrcUserCmd {
                    msec: 10,
                    viewangles: Vec3::new(5.0, 180.0, 0.0),
                    forwardmove: 250.0,
                    buttons: 1,
                    ..Default::default()
                },
                GoldSrcUserCmd {
                    msec: 10,
                    viewangles: Vec3::new(4.0, 179.0, 0.0),
                    forwardmove: 250.0,
                    ..Default::default()
                },
            ],
        );
        write_clc_stringcmd(&mut cw, "jointeam 2");

        let clc_msgs = parse_clc_stream(cw.as_slice(), ucmd_schema, 77).unwrap();
        assert_eq!(clc_msgs.len(), 3);
        assert_eq!(clc_msgs[0], ClcMessage::Delta(42));
        match &clc_msgs[1] {
            ClcMessage::Move { cmds, .. } => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds[0].buttons, 1);
                assert!((cmds[0].forwardmove - 250.0).abs() < 1.0);
                assert!((cmds[0].viewangles.y - 180.0).abs() < 0.05);
            }
            other => panic!("unexpected clc: {other:?}"),
        }
        assert_eq!(clc_msgs[2], ClcMessage::StringCmd("jointeam 2".to_string()));
    }
}
