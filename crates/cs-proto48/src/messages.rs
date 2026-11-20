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

/// `SoundMsg::flags` bit: the volume byte follows (otherwise 255).
pub const SND_FL_VOLUME: u32 = 1 << 0;
/// `SoundMsg::flags` bit: the attenuation byte follows (otherwise 1.0).
pub const SND_FL_ATTENUATION: u32 = 1 << 1;
/// `SoundMsg::flags` bit: the sound index takes 16 bits. The writer sets it by itself.
pub const SND_FL_LARGE_INDEX: u32 = 1 << 2;
/// `SoundMsg::flags` bit: the pitch byte follows (otherwise 100).
pub const SND_FL_PITCH: u32 = 1 << 3;

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

impl SvcMessage {
    /// The wire opcode the message is sent with; for a user message, its registered id.
    pub fn opcode(&self) -> u8 {
        match self {
            SvcMessage::Nop => svc::NOP,
            SvcMessage::Disconnect(_) => svc::DISCONNECT,
            SvcMessage::Version(_) => svc::VERSION,
            SvcMessage::SetView(_) => svc::SETVIEW,
            SvcMessage::Sound(_) => svc::SOUND,
            SvcMessage::Time(_) => svc::TIME,
            SvcMessage::Print(_) => svc::PRINT,
            SvcMessage::StuffText(_) => svc::STUFFTEXT,
            SvcMessage::SetAngle(_) => svc::SETANGLE,
            SvcMessage::AddAngle(_) => svc::ADDANGLE,
            SvcMessage::ServerInfo(_) => svc::SERVERINFO,
            SvcMessage::LightStyle { .. } => svc::LIGHTSTYLE,
            SvcMessage::UpdateUserInfo { .. } => svc::UPDATEUSERINFO,
            SvcMessage::DeltaDescription(_) => svc::DELTADESCRIPTION,
            SvcMessage::ClientData { .. } => svc::CLIENTDATA,
            SvcMessage::StopSound(_) => svc::STOPSOUND,
            SvcMessage::Pings(_) => svc::PINGS,
            SvcMessage::SpawnBaseline { .. } => svc::SPAWNBASELINE,
            SvcMessage::TempEntity { .. } => svc::TEMP_ENTITY,
            SvcMessage::SetPause(_) => svc::SETPAUSE,
            SvcMessage::SignonNum(_) => svc::SIGNONNUM,
            SvcMessage::CenterPrint(_) => svc::CENTERPRINT,
            SvcMessage::CdTrack { .. } => svc::CDTRACK,
            SvcMessage::WeaponAnim { .. } => svc::WEAPONANIM,
            SvcMessage::DecalName { .. } => svc::DECALNAME,
            SvcMessage::RoomType(_) => svc::ROOMTYPE,
            SvcMessage::NewUserMsg(_) => svc::NEWUSERMSG,
            SvcMessage::PacketEntities(update) if update.is_delta => svc::DELTAPACKETENTITIES,
            SvcMessage::PacketEntities(_) => svc::PACKETENTITIES,
            SvcMessage::Choke => svc::CHOKE,
            SvcMessage::ResourceList { .. } => svc::RESOURCELIST,
            SvcMessage::NewMoveVars(_) => svc::NEWMOVEVARS,
            SvcMessage::ResourceRequest { .. } => svc::RESOURCEREQUEST,
            SvcMessage::ResourceLocation(_) => svc::RESOURCELOCATION,
            SvcMessage::SendExtraInfo { .. } => svc::SENDEXTRAINFO,
            SvcMessage::VoiceInit { .. } => svc::VOICEINIT,
            SvcMessage::VoiceData { .. } => svc::VOICEDATA,
            SvcMessage::SendCvarValue(_) => svc::SENDCVARVALUE,
            SvcMessage::SendCvarValue2 { .. } => svc::SENDCVARVALUE2,
            SvcMessage::UserMsg { id, .. } => *id,
            SvcMessage::Ignored(op) => *op,
        }
    }
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
                let schema = EntitySchemas {
                    normal: &ent_schema,
                    player: &player_schema,
                    custom: &custom_schema,
                }
                .for_entity(entnum, ent_type, ctx.max_clients);
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

        let ent_type = if custom { ENTITY_BEAM } else { ENTITY_NORMAL };
        let schema = EntitySchemas {
            normal: &ent_schema,
            player: &player_schema,
            custom: &custom_schema,
        }
        .for_entity(entnum, ent_type, ctx.max_clients);

        // A record built for another schema (an entity whose kind changed, or malformed data)
        // is never used as a base; the entry then falls through to the next candidate.
        let fits = |rec: &&DeltaRecord| rec.values.len() == schema.fields.len();
        let base_rec = if let Some(idx) = custom_bl_idx {
            ctx.instance_baselines
                .get(idx)
                .filter(fits)
                .cloned()
                .unwrap_or_else(|| schema.zero_record())
        } else if let Some(off) = prev_offset {
            ordered_full
                .len()
                .checked_sub(off)
                .and_then(|i| ordered_full.get(i))
                .filter(fits)
                .cloned()
                .unwrap_or_else(|| schema.zero_record())
        } else if is_delta
            && let Some(prev) = matching_base(parsed_entry(prev_frame.get(&entnum)), custom, schema)
        {
            prev.clone()
        } else if let Some(bl) =
            matching_base(parsed_entry(ctx.baselines.get(&entnum)), custom, schema)
        {
            bl.clone()
        } else {
            schema.zero_record()
        };

        let rec = read_delta(r, schema, &base_rec, ctx.server_time)?;
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
    let rest = r.remaining_slice();
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
    let len = r.pos() - start;
    let raw = rest.get(..len).ok_or(ProtoError::UnexpectedEof)?;
    Ok((te_type, raw.to_vec()))
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

/// `svc_centerprint`: `text` shown in the middle of the client's screen.
pub fn write_svc_centerprint(w: &mut MsgWriter, text: &str) {
    w.write_u8(svc::CENTERPRINT);
    w.write_str(text);
}

/// `svc_temp_entity`: the `te_type` byte and then `payload` as is; the caller encodes the
/// payload for the effect type.
pub fn write_svc_temp_entity(w: &mut MsgWriter, te_type: u8, payload: &[u8]) {
    w.write_u8(svc::TEMP_ENTITY);
    w.write_u8(te_type);
    w.write_bytes(payload);
}

pub fn write_svc_disconnect(w: &mut MsgWriter, reason: &str) {
    w.write_u8(svc::DISCONNECT);
    w.write_str(reason);
}

pub fn write_svc_time(w: &mut MsgWriter, server_time: f32) {
    w.write_u8(svc::TIME);
    w.write_f32(server_time);
}

/// `svc_choke`: tells the client the server held back an update to keep within its rate.
pub fn write_svc_choke(w: &mut MsgWriter) {
    w.write_u8(svc::CHOKE);
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
    let Some(schemas) = EntitySchemas::from_registry(reg) else {
        return;
    };

    w.write_u8(svc::SPAWNBASELINE);
    w.start_bit_writing();
    for (entnum, state) in baselines {
        let ent_type = wire_entity_type(state.entity_type);
        let schema = schemas.for_entity(*entnum, ent_type, max_clients);
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

/// Map a caller-side entity type byte (`GoldSrcEntityState::entity_type`, or the type stored next
/// to a record in a `from` frame or `baselines` map handed to a writer) to the wire type the
/// writers send: `0` (the `Default`) and any value with the `ENTITY_NORMAL` bit set become
/// `ENTITY_NORMAL`; any other non-zero value (such as `ENTITY_BEAM`) becomes `ENTITY_BEAM`.
pub fn wire_entity_type(entity_type: u8) -> u8 {
    if entity_type == 0 || (entity_type & ENTITY_NORMAL) != 0 {
        ENTITY_NORMAL
    } else {
        ENTITY_BEAM
    }
}

/// Whether a wire entity type byte (`ENTITY_NORMAL` / `ENTITY_BEAM`) marks a custom entity,
/// as `svc_spawnbaseline` and `svc_packetentities` decide it.
fn is_custom_entity_type(ent_type: u8) -> bool {
    (ent_type & ENTITY_NORMAL) == 0
}

/// `candidate`'s record when it can serve as the delta base of an entity entry: its wire type is
/// of the same kind (custom or normal) as the entry and it holds one value per field of
/// `schema`. Writers and parser both pick base records through this, so they agree on every
/// entry; writers first map caller-side type bytes with [`caller_entry`].
fn matching_base<'a>(
    candidate: Option<(u8, &'a DeltaRecord)>,
    custom: bool,
    schema: &DeltaSchema,
) -> Option<&'a DeltaRecord> {
    candidate
        .filter(|(ent_type, rec)| {
            is_custom_entity_type(*ent_type) == custom && rec.values.len() == schema.fields.len()
        })
        .map(|(_, rec)| rec)
}

/// A parser-stored `(wire type, record)` entry as a [`matching_base`] candidate.
fn parsed_entry(entry: Option<&(u8, DeltaRecord)>) -> Option<(u8, &DeltaRecord)> {
    entry.map(|(ent_type, rec)| (*ent_type, rec))
}

/// A caller-supplied `(type, record)` entry as a [`matching_base`] candidate, its type byte
/// mapped through [`wire_entity_type`].
fn caller_entry(entry: Option<&(u8, DeltaRecord)>) -> Option<(u8, &DeltaRecord)> {
    entry.map(|(ent_type, rec)| (wire_entity_type(*ent_type), rec))
}

const ENTITY_SCHEMA: &str = "entity_state_t";
const PLAYER_ENTITY_SCHEMA: &str = "entity_state_player_t";
const CUSTOM_ENTITY_SCHEMA: &str = "custom_entity_state_t";

/// Name of the delta schema an entity is encoded with in `svc_spawnbaseline` and
/// `svc_packetentities` / `svc_deltapacketentities`: `custom_entity_state_t` for a custom wire
/// type (`ENTITY_NORMAL` bit clear), `entity_state_player_t` for indices `1..=max_clients`,
/// `entity_state_t` otherwise. `wire_type` is a wire type byte; map caller-side type bytes with
/// [`wire_entity_type`] first.
pub fn entity_schema_name(entnum: u16, wire_type: u8, max_clients: u8) -> &'static str {
    if is_custom_entity_type(wire_type) {
        CUSTOM_ENTITY_SCHEMA
    } else if (1..=max_clients as u16).contains(&entnum) {
        PLAYER_ENTITY_SCHEMA
    } else {
        ENTITY_SCHEMA
    }
}

/// The three `entity_state_t` delta schemas an entity message chooses between.
struct EntitySchemas<'a> {
    normal: &'a DeltaSchema,
    player: &'a DeltaSchema,
    custom: &'a DeltaSchema,
}

impl<'a> EntitySchemas<'a> {
    fn from_registry(reg: &'a DeltaRegistry) -> Option<Self> {
        Some(Self {
            normal: reg.get(ENTITY_SCHEMA)?,
            player: reg.get(PLAYER_ENTITY_SCHEMA)?,
            custom: reg.get(CUSTOM_ENTITY_SCHEMA)?,
        })
    }

    /// The schema [`entity_schema_name`] names for this entity.
    fn for_entity(&self, entnum: u16, wire_type: u8, max_clients: u8) -> &'a DeltaSchema {
        match entity_schema_name(entnum, wire_type, max_clients) {
            CUSTOM_ENTITY_SCHEMA => self.custom,
            PLAYER_ENTITY_SCHEMA => self.player,
            _ => self.normal,
        }
    }

    /// `(custom, schema)` for `ent`, chosen the way the parser chooses for its entry.
    fn for_state(&self, ent: &GoldSrcEntityState, max_clients: u8) -> (bool, &'a DeltaSchema) {
        let wire_type = wire_entity_type(ent.entity_type);
        (
            is_custom_entity_type(wire_type),
            self.for_entity(ent.number, wire_type, max_clients),
        )
    }
}

/// Write a full (`svc_packetentities`) entity snapshot (sorted by entity number).
///
/// Each entity is delta-encoded against its entry in `baselines` (the records sent with
/// `svc_spawnbaseline`, keyed by entity number, as the parser stores them in
/// `SvcParseContext::baselines`) when that entry is of the same kind, and against the zero record
/// otherwise — the same record the parser picks. Assumes no instance baselines were sent (the
/// parser then reads no instance-baseline bit).
pub fn write_svc_packetentities_full(
    w: &mut MsgWriter,
    reg: &DeltaRegistry,
    max_clients: u8,
    baselines: &BTreeMap<u16, (u8, DeltaRecord)>,
    entities: &[GoldSrcEntityState],
    server_time: f64,
) {
    let Some(schemas) = EntitySchemas::from_registry(reg) else {
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
        let (custom, schema) = schemas.for_state(ent, max_clients);
        let rec = ent.to_delta(schema);
        let zero = schema.zero_record();
        let base = matching_base(caller_entry(baselines.get(&ent.number)), custom, schema)
            .unwrap_or(&zero);

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
        w.write_one_bit(custom);
        w.write_one_bit(false); // no offset into this packet's earlier entities
        write_delta(w, schema, base, &rec, true, server_time);
    }

    w.write_bits(0, 16);
    w.end_bit_writing();
}

/// Write the bit-packed entity number of a `svc_deltapacketentities` entry (remove flag, then
/// an 11-bit absolute number or a 6-bit offset from the previous entry), as the parser reads it.
fn write_delta_entity_header(w: &mut MsgWriter, num_base: &mut u16, num: u16, remove: bool) {
    w.write_one_bit(remove);
    let offset = num as i32 - *num_base as i32;
    if offset <= 0 || offset > 63 {
        w.write_one_bit(true);
        w.write_bits(num as u32, 11);
    } else {
        w.write_one_bit(false);
        w.write_bits(offset as u32, 6);
    }
    *num_base = num;
}

/// Write a `svc_deltapacketentities` update from the frame `from` (the snapshot the client
/// acknowledged as `delta_seq`) to the snapshot `to`.
///
/// - An entity missing from `to` is sent as a removal.
/// - An entity in both frames with the same kind (custom or normal) is delta-encoded against its
///   `from` record, or omitted when no wire field changed (the client keeps it from `delta_seq`).
/// - An entity new in `to` is delta-encoded against its `baselines` entry when that entry is of
///   the same kind, else against the zero record — the record the parser picks.
/// - An entity whose kind changed is sent as a removal followed by a new entity.
///
/// Assumes no instance baselines were sent, like [`write_svc_packetentities_full`].
#[allow(clippy::too_many_arguments)]
pub fn write_svc_packetentities_delta(
    w: &mut MsgWriter,
    reg: &DeltaRegistry,
    max_clients: u8,
    delta_seq: u8,
    baselines: &BTreeMap<u16, (u8, DeltaRecord)>,
    from: &BTreeMap<u16, (u8, DeltaRecord)>,
    to: &[GoldSrcEntityState],
    server_time: f64,
) {
    let Some(schemas) = EntitySchemas::from_registry(reg) else {
        return;
    };

    let to_by_num: BTreeMap<u16, &GoldSrcEntityState> = to.iter().map(|e| (e.number, e)).collect();
    let mut numbers: Vec<u16> = from.keys().chain(to_by_num.keys()).copied().collect();
    numbers.sort_unstable();
    numbers.dedup();

    w.write_u8(svc::DELTAPACKETENTITIES);
    w.write_u16(to_by_num.len() as u16);
    w.write_u8(delta_seq);
    w.start_bit_writing();

    let mut num_base: u16 = 0;
    for num in numbers {
        let Some(ent) = to_by_num.get(&num) else {
            write_delta_entity_header(w, &mut num_base, num, true);
            continue;
        };
        let (custom, schema) = schemas.for_state(ent, max_clients);
        let rec = ent.to_delta(schema);

        let prev = from.get(&num);
        let zero = schema.zero_record();
        let base = match matching_base(caller_entry(prev), custom, schema) {
            Some(prev_rec) => {
                if !has_delta_changes(schema, prev_rec, &rec) {
                    continue;
                }
                prev_rec
            }
            None => {
                if prev.is_some() {
                    // Kind changed: drop the old entity, then send this one as new.
                    write_delta_entity_header(w, &mut num_base, num, true);
                }
                matching_base(caller_entry(baselines.get(&num)), custom, schema).unwrap_or(&zero)
            }
        };
        write_delta_entity_header(w, &mut num_base, num, false);
        w.write_one_bit(custom);
        write_delta(w, schema, base, &rec, true, server_time);
    }

    w.write_bits(0, 16);
    w.end_bit_writing();
}

/// Write `svc_sound` (`SV_StartSound`, `sv_main.cpp`). The optional fields follow `s.flags`;
/// `SND_FL_LARGE_INDEX` is added when `sound_index` does not fit in a byte.
pub fn write_svc_sound(w: &mut MsgWriter, s: &SoundMsg) {
    let mut flags = s.flags as u32;
    if s.sound_index > 0xFF {
        flags |= SND_FL_LARGE_INDEX;
    }
    w.write_u8(svc::SOUND);
    w.start_bit_writing();
    w.write_bits(flags, 9);
    if (flags & SND_FL_VOLUME) != 0 {
        w.write_bits(s.volume as u32, 8);
    }
    if (flags & SND_FL_ATTENUATION) != 0 {
        w.write_bits((s.attenuation * 64.0) as u32, 8);
    }
    w.write_bits(s.channel as u32, 3);
    w.write_bits(s.entity as u32, 11);
    if (flags & SND_FL_LARGE_INDEX) != 0 {
        w.write_bits(s.sound_index as u32, 16);
    } else {
        w.write_bits(s.sound_index as u32, 8);
    }
    w.write_bit_vec3_coord(s.origin);
    if (flags & SND_FL_PITCH) != 0 {
        w.write_bits(s.pitch as u32, 8);
    }
    w.end_bit_writing();
}

/// Largest ping, in milliseconds, `svc_pings` can carry (12 bits).
pub const MAX_WIRE_PING: u16 = 4095;
/// Largest loss, in percent, `svc_pings` can carry (7 bits).
pub const MAX_WIRE_LOSS: u8 = 127;

/// Write `svc_pings` (`SV_WritePingsToClient`): `(slot, ping ms, loss %)` per player. Pings
/// above [`MAX_WIRE_PING`] and losses above [`MAX_WIRE_LOSS`] are written as those limits.
pub fn write_svc_pings(w: &mut MsgWriter, pings: &[(u8, u16, u8)]) {
    w.write_u8(svc::PINGS);
    w.start_bit_writing();
    for &(slot, ping, loss) in pings {
        w.write_one_bit(true);
        w.write_bits(slot as u32, 5);
        w.write_bits(u32::from(ping.min(MAX_WIRE_PING)), 12);
        w.write_bits(u32::from(loss.min(MAX_WIRE_LOSS)), 7);
    }
    w.write_one_bit(false);
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

/// Encode `clc_resourcelist` (opcode 5): the client's custom resources (`SV_ParseResourceList`).
/// An entry flagged `RES_CUSTOM` always carries 16 MD5 bytes (zeros when `md5` is `None`).
pub fn write_clc_resourcelist(w: &mut MsgWriter, resources: &[ResourceEntry]) {
    let count = resources.len().min(u16::MAX as usize);
    w.write_u8(clc::RESOURCELIST);
    w.write_u16(count as u16);
    for r in &resources[..count] {
        w.write_str(&r.filename);
        w.write_u8(r.res_type);
        w.write_u16(r.index);
        w.write_u32(r.download_size);
        w.write_u8(r.flags);
        if (r.flags as u32 & RES_CUSTOM) != 0 {
            w.write_bytes(&r.md5.unwrap_or([0; 16]));
        }
    }
}

/// Encode `clc_fileconsistency` (opcode 7): a `u16` length, then the consistency payload
/// (truncated to `u16::MAX` bytes).
pub fn write_clc_fileconsistency(w: &mut MsgWriter, payload: &[u8]) {
    let len = payload.len().min(u16::MAX as usize);
    w.write_u8(clc::FILECONSISTENCY);
    w.write_u16(len as u16);
    w.write_bytes(&payload[..len]);
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

/// Most messages one payload may yield in [`parse_clc_stream`]; `clc_nop` does not count.
pub const MAX_CLC_MESSAGES_PER_PAYLOAD: usize = 8192;

/// Parse an entire stream of concatenated GoldSrc `clc_*` client messages. `clc_nop` yields no
/// message; a stream that would yield more than [`MAX_CLC_MESSAGES_PER_PAYLOAD`] messages is an
/// error.
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
            clc::NOP => {}
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
        if out.len() > MAX_CLC_MESSAGES_PER_PAYLOAD {
            return Err(ProtoError::InvalidPayload(format!(
                "more than {MAX_CLC_MESSAGES_PER_PAYLOAD} clc messages in one payload"
            )));
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
        let player_baselines = BTreeMap::from([(
            1u16,
            (
                ENTITY_NORMAL,
                player_ent.to_delta(reg.get("entity_state_player_t").unwrap()),
            ),
        )]);
        write_svc_packetentities_full(
            &mut sw,
            &reg,
            16,
            &player_baselines,
            std::slice::from_ref(&player_ent),
            12.5,
        );
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
        let pe = parsed
            .iter()
            .find_map(|m| match m {
                SvcMessage::PacketEntities(pe) => Some(pe),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            decode_entities(&reg, 16, &pe.entities),
            vec![player_ent.clone()]
        );
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

    /// Entity records keyed by number, as the parser keeps frames and baselines.
    type EntityRecords = BTreeMap<u16, (u8, DeltaRecord)>;

    fn entity_schema(reg: &DeltaRegistry, max_clients: u8, num: u16, ent_type: u8) -> &DeltaSchema {
        let name = if wire_entity_type(ent_type) == ENTITY_BEAM {
            "custom_entity_state_t"
        } else if num >= 1 && num <= max_clients as u16 {
            "entity_state_player_t"
        } else {
            "entity_state_t"
        };
        reg.get(name).unwrap()
    }

    /// Wire-level `(type, record)` map for `states`, as the parser stores frames and baselines.
    fn entity_records(
        reg: &DeltaRegistry,
        max_clients: u8,
        states: &[GoldSrcEntityState],
    ) -> BTreeMap<u16, (u8, DeltaRecord)> {
        states
            .iter()
            .map(|e| {
                let schema = entity_schema(reg, max_clients, e.number, e.entity_type);
                (e.number, (e.entity_type, e.to_delta(schema)))
            })
            .collect()
    }

    fn decode_entities(
        reg: &DeltaRegistry,
        max_clients: u8,
        entities: &BTreeMap<u16, (u8, DeltaRecord)>,
    ) -> Vec<GoldSrcEntityState> {
        entities
            .iter()
            .map(|(&num, (ent_type, rec))| {
                GoldSrcEntityState::from_delta(
                    num,
                    *ent_type,
                    rec,
                    entity_schema(reg, max_clients, num, *ent_type),
                )
            })
            .collect()
    }

    /// Parse a single packet-entities message with the given baselines and previous frame.
    fn parse_entities(
        stream: &[u8],
        max_clients: u8,
        baselines: &BTreeMap<u16, (u8, DeltaRecord)>,
        frame: Option<(u8, EntityRecords)>,
    ) -> PacketEntitiesMsg {
        let mut ctx = SvcParseContext::new();
        ctx.max_clients = max_clients;
        ctx.server_time = 12.5;
        ctx.baselines = baselines.clone();
        if let Some((seq, from)) = frame {
            ctx.entity_frames.insert(seq, from);
            ctx.last_incoming_seq = seq.wrapping_add(1);
        }
        let mut parsed = parse_svc_stream(stream, &mut ctx).unwrap();
        assert_eq!(parsed.len(), 1);
        match parsed.pop() {
            Some(SvcMessage::PacketEntities(pe)) => *pe,
            other => panic!("expected packet entities, got {other:?}"),
        }
    }

    fn normal_ent(number: u16, x: f32, modelindex: u16) -> GoldSrcEntityState {
        GoldSrcEntityState {
            number,
            entity_type: ENTITY_NORMAL,
            origin: Vec3::new(x, -200.25, 64.0),
            angles: Vec3::new(0.0, 90.0, 0.0),
            modelindex,
            sequence: 4,
            ..Default::default()
        }
    }

    /// A custom (beam) entity using only fields `custom_entity_state_t` carries.
    fn beam_ent(number: u16, x: f32, modelindex: u16) -> GoldSrcEntityState {
        GoldSrcEntityState {
            number,
            entity_type: ENTITY_BEAM,
            origin: Vec3::new(x, 20.0, 30.0),
            modelindex,
            sequence: 3,
            body: 1,
            renderamt: 100,
            ..Default::default()
        }
    }

    #[test]
    fn delta_packetentities_roundtrip() {
        let reg = DeltaRegistry::cs16_default();
        let max_clients = 16;
        let seq = 7;

        let from = entity_records(
            &reg,
            max_clients,
            &[
                normal_ent(1, 100.0, 12),
                normal_ent(5, 300.0, 20),
                normal_ent(9, 500.0, 30),
            ],
        );
        // 1 moves, 5 is removed, 9 is unchanged, 12 is new.
        let to = vec![
            normal_ent(1, 132.5, 12),
            normal_ent(9, 500.0, 30),
            normal_ent(12, -40.125, 44),
        ];

        let mut w = MsgWriter::new();
        write_svc_packetentities_delta(
            &mut w,
            &reg,
            max_clients,
            seq,
            &BTreeMap::new(),
            &from,
            &to,
            12.5,
        );
        let pe = parse_entities(
            w.as_slice(),
            max_clients,
            &BTreeMap::new(),
            Some((seq, from)),
        );

        assert!(pe.is_delta);
        assert_eq!(pe.delta_from_seq, Some(seq));
        assert_eq!(pe.expected_count, 3);
        assert_eq!(pe.removed, vec![5]);
        assert_eq!(decode_entities(&reg, max_clients, &pe.entities), to);
    }

    #[test]
    fn full_packetentities_encode_against_baselines() {
        let reg = DeltaRegistry::cs16_default();
        let max_clients = 16;

        // Entity 1 differs from its baseline only in `body`, which it has at zero: encoding it
        // against the zero record would leave the parser holding the baseline's body.
        let with_baseline = normal_ent(1, 100.0, 12);
        let baseline_state = GoldSrcEntityState {
            body: 2,
            ..with_baseline.clone()
        };
        // Entity 20 has no baseline; entity 21 equals its baseline in every field.
        let without_baseline = normal_ent(20, -64.5, 33);
        let equal_to_baseline = GoldSrcEntityState {
            skin: 3,
            renderamt: 200,
            ..normal_ent(21, 8.0, 40)
        };
        let baselines = entity_records(
            &reg,
            max_clients,
            &[baseline_state, equal_to_baseline.clone()],
        );
        let entities = vec![with_baseline, without_baseline, equal_to_baseline];

        let mut w = MsgWriter::new();
        write_svc_packetentities_full(&mut w, &reg, max_clients, &baselines, &entities, 12.5);
        let pe = parse_entities(w.as_slice(), max_clients, &baselines, None);

        assert!(!pe.is_delta);
        assert_eq!(decode_entities(&reg, max_clients, &pe.entities), entities);
    }

    #[test]
    fn delta_packetentities_new_entity_uses_its_baseline() {
        let reg = DeltaRegistry::cs16_default();
        let max_clients = 16;
        let seq = 200;

        let from = entity_records(&reg, max_clients, &[normal_ent(9, 500.0, 30)]);
        // The baseline has non-zero `renderamt` and `skin` where the new entity has zero.
        let new_ent = normal_ent(30, 16.0, 50);
        let baselines = entity_records(
            &reg,
            max_clients,
            &[GoldSrcEntityState {
                renderamt: 255,
                skin: 2,
                ..new_ent.clone()
            }],
        );
        let to = vec![normal_ent(9, 500.0, 30), new_ent];

        let mut w = MsgWriter::new();
        write_svc_packetentities_delta(
            &mut w,
            &reg,
            max_clients,
            seq,
            &baselines,
            &from,
            &to,
            12.5,
        );
        let pe = parse_entities(w.as_slice(), max_clients, &baselines, Some((seq, from)));

        assert!(pe.removed.is_empty());
        assert_eq!(decode_entities(&reg, max_clients, &pe.entities), to);
    }

    #[test]
    fn delta_packetentities_entity_type_change_is_remove_plus_new() {
        let reg = DeltaRegistry::cs16_default();
        let max_clients = 16;
        let seq = 3;

        // 40 turns from a normal entity into a beam, 41 from a beam into a normal entity.
        let from = entity_records(
            &reg,
            max_clients,
            &[normal_ent(40, 10.0, 5), beam_ent(41, 70.0, 6)],
        );
        let to = vec![beam_ent(40, 10.0, 5), normal_ent(41, 70.0, 6)];

        let mut w = MsgWriter::new();
        write_svc_packetentities_delta(
            &mut w,
            &reg,
            max_clients,
            seq,
            &BTreeMap::new(),
            &from,
            &to,
            12.5,
        );
        let pe = parse_entities(
            w.as_slice(),
            max_clients,
            &BTreeMap::new(),
            Some((seq, from)),
        );

        assert_eq!(pe.removed, vec![40, 41]);
        assert_eq!(decode_entities(&reg, max_clients, &pe.entities), to);
    }

    #[test]
    fn centerprint_and_temp_entity_roundtrip() {
        // TE_EXPLOSION (3): 11 payload bytes.
        let payload: Vec<u8> = (20..31).collect();
        let mut w = MsgWriter::new();
        write_svc_centerprint(&mut w, "Round start");
        write_svc_temp_entity(&mut w, 3, &payload);
        write_svc_centerprint(&mut w, "");
        let parsed = parse_svc_stream(w.as_slice(), &mut SvcParseContext::new()).unwrap();
        assert_eq!(
            parsed,
            vec![
                SvcMessage::CenterPrint("Round start".to_string()),
                SvcMessage::TempEntity {
                    te_type: 3,
                    raw: payload
                },
                SvcMessage::CenterPrint(String::new()),
            ]
        );
        assert_eq!(&w.as_slice()[..2], &[svc::CENTERPRINT, b'R']);
    }

    #[test]
    fn temp_entity_raw_holds_the_payload_bytes() {
        // TE_EXPLOSION (3): 11 payload bytes.
        let payload: Vec<u8> = (1..=11).collect();
        let mut stream = vec![svc::TEMP_ENTITY, 3];
        stream.extend_from_slice(&payload);
        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(&stream, &mut ctx).unwrap();
        assert_eq!(
            parsed,
            vec![SvcMessage::TempEntity {
                te_type: 3,
                raw: payload
            }]
        );
    }

    /// A normal entity whose `entity_type` is left at the `Default` 0.
    fn default_type_ent(number: u16, x: f32, modelindex: u16) -> GoldSrcEntityState {
        GoldSrcEntityState {
            entity_type: 0,
            ..normal_ent(number, x, modelindex)
        }
    }

    /// Default-type `states` as the parser hands them back, with the wire type `ENTITY_NORMAL`.
    fn as_wire(states: &[GoldSrcEntityState]) -> Vec<GoldSrcEntityState> {
        states
            .iter()
            .map(|e| GoldSrcEntityState {
                entity_type: ENTITY_NORMAL,
                ..e.clone()
            })
            .collect()
    }

    fn numbered(states: &[GoldSrcEntityState]) -> Vec<(u16, GoldSrcEntityState)> {
        states.iter().map(|e| (e.number, e.clone())).collect()
    }

    #[test]
    fn spawnbaseline_then_full_update_with_default_entity_type() {
        let reg = DeltaRegistry::cs16_default();
        let max_clients = 16;

        // 1 differs from its baseline in `body`; 25 equals its baseline; 26 has none.
        let player = default_type_ent(1, 100.0, 12);
        let equal_to_baseline = GoldSrcEntityState {
            skin: 3,
            renderamt: 200,
            ..default_type_ent(25, 8.0, 40)
        };
        let baseline_states = [
            GoldSrcEntityState {
                body: 2,
                ..player.clone()
            },
            equal_to_baseline.clone(),
        ];
        let entities = vec![player, equal_to_baseline, default_type_ent(26, -64.5, 33)];

        let mut w = MsgWriter::new();
        write_svc_spawnbaseline(&mut w, &reg, max_clients, &numbered(&baseline_states), 12.5);
        let baselines = entity_records(&reg, max_clients, &baseline_states);
        write_svc_packetentities_full(&mut w, &reg, max_clients, &baselines, &entities, 12.5);

        let mut ctx = SvcParseContext::new();
        ctx.max_clients = max_clients;
        ctx.server_time = 12.5;
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        let [
            SvcMessage::SpawnBaseline { .. },
            SvcMessage::PacketEntities(pe),
        ] = parsed.as_slice()
        else {
            panic!("expected baseline + packet entities, got {parsed:?}");
        };
        assert!(pe.removed.is_empty());
        assert_eq!(
            decode_entities(&reg, max_clients, &pe.entities),
            as_wire(&entities)
        );
    }

    #[test]
    fn delta_update_with_default_entity_type() {
        let reg = DeltaRegistry::cs16_default();
        let max_clients = 16;
        let seq = 10;

        // 30 stays unchanged, 31 changes `sequence`, 32 is new; all three have baselines whose
        // `renderamt`/`skin` are non-zero where the entities have zero.
        let unchanged = default_type_ent(30, 16.0, 50);
        let changing = default_type_ent(31, 48.0, 51);
        let new_ent = default_type_ent(32, -24.0, 52);
        let baseline_states: Vec<GoldSrcEntityState> = [&unchanged, &changing, &new_ent]
            .into_iter()
            .map(|e| GoldSrcEntityState {
                renderamt: 255,
                skin: 2,
                ..e.clone()
            })
            .collect();
        let baselines = entity_records(&reg, max_clients, &baseline_states);
        let frame = vec![unchanged.clone(), changing.clone()];

        // Signon and the full update the client acknowledges as `seq`.
        let mut w = MsgWriter::new();
        write_svc_spawnbaseline(&mut w, &reg, max_clients, &numbered(&baseline_states), 12.5);
        write_svc_packetentities_full(&mut w, &reg, max_clients, &baselines, &frame, 12.5);
        let mut ctx = SvcParseContext::new();
        ctx.max_clients = max_clients;
        ctx.server_time = 12.5;
        ctx.last_incoming_seq = seq;
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        let [
            SvcMessage::SpawnBaseline { .. },
            SvcMessage::PacketEntities(full),
        ] = parsed.as_slice()
        else {
            panic!("expected baseline + packet entities, got {parsed:?}");
        };
        assert_eq!(
            decode_entities(&reg, max_clients, &full.entities),
            as_wire(&frame)
        );

        // The server's own record of that frame, built from the same default-type states.
        let from = entity_records(&reg, max_clients, &frame);
        let to = vec![
            unchanged,
            GoldSrcEntityState {
                sequence: 9,
                ..changing
            },
            new_ent,
        ];
        let mut w = MsgWriter::new();
        write_svc_packetentities_delta(
            &mut w,
            &reg,
            max_clients,
            seq,
            &baselines,
            &from,
            &to,
            12.5,
        );
        ctx.last_incoming_seq = seq.wrapping_add(1);
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        let [SvcMessage::PacketEntities(pe)] = parsed.as_slice() else {
            panic!("expected packet entities, got {parsed:?}");
        };
        assert_eq!(pe.delta_from_seq, Some(seq));
        assert!(pe.removed.is_empty());
        assert_eq!(
            decode_entities(&reg, max_clients, &pe.entities),
            as_wire(&to)
        );
    }

    #[test]
    fn pings_are_clamped_to_their_bit_widths() {
        let mut w = MsgWriter::new();
        write_svc_pings(&mut w, &[(1, 4096, 128), (2, u16::MAX, u8::MAX), (3, 4095, 127)]);
        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        assert_eq!(
            parsed,
            vec![SvcMessage::Pings(vec![
                (1, MAX_WIRE_PING, MAX_WIRE_LOSS),
                (2, MAX_WIRE_PING, MAX_WIRE_LOSS),
                (3, 4095, 127),
            ])]
        );
    }

    #[test]
    fn sound_pings_and_clc_resourcelist_roundtrip() {
        let sound = SoundMsg {
            flags: (SND_FL_VOLUME | SND_FL_ATTENUATION | SND_FL_LARGE_INDEX | SND_FL_PITCH) as u16,
            volume: 200,
            attenuation: 0.8125,
            channel: 2,
            entity: 17,
            sound_index: 300,
            origin: Vec3::new(128.0, -256.0, 32.0),
            pitch: 110,
        };
        let pings = vec![(0u8, 35u16, 0u8), (3, 120, 5), (31, 999, 100)];

        let mut sw = MsgWriter::new();
        write_svc_sound(&mut sw, &sound);
        write_svc_pings(&mut sw, &pings);
        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(sw.as_slice(), &mut ctx).unwrap();
        assert_eq!(
            parsed,
            vec![SvcMessage::Sound(sound), SvcMessage::Pings(pings)]
        );

        let resources = vec![
            ResourceEntry {
                res_type: 3,
                filename: "models/player/gign/gign.mdl".to_string(),
                index: 7,
                download_size: 123_456,
                flags: 0,
                md5: None,
            },
            ResourceEntry {
                res_type: 5,
                filename: "tempdecal.wad".to_string(),
                index: 0,
                download_size: 4096,
                flags: RES_CUSTOM as u8,
                md5: Some([0x5A; 16]),
            },
        ];
        let consistency = [1u8, 2, 3, 0xFF, 0];

        let mut cw = MsgWriter::new();
        write_clc_resourcelist(&mut cw, &resources);
        write_clc_fileconsistency(&mut cw, &consistency);
        let ucmd_schema = DeltaRegistry::cs16_default()
            .get("usercmd_t")
            .unwrap()
            .clone();
        let clc_msgs = parse_clc_stream(cw.as_slice(), &ucmd_schema, 1).unwrap();
        assert_eq!(
            clc_msgs,
            vec![
                ClcMessage::ResourceList(resources),
                ClcMessage::FileConsistency(consistency.to_vec()),
            ]
        );
    }

    fn usercmd_schema() -> DeltaSchema {
        DeltaRegistry::cs16_default()
            .get("usercmd_t")
            .unwrap()
            .clone()
    }

    #[test]
    fn clc_nops_are_skipped() {
        let nops = vec![clc::NOP; 10_000];
        assert_eq!(parse_clc_stream(&nops, &usercmd_schema(), 1).unwrap(), vec![]);
        // Nops between messages leave the messages alone.
        let mut w = MsgWriter::new();
        w.write_u8(clc::NOP);
        w.write_u8(clc::DELTA);
        w.write_u8(7);
        w.write_u8(clc::NOP);
        assert_eq!(
            parse_clc_stream(w.as_slice(), &usercmd_schema(), 1).unwrap(),
            vec![ClcMessage::Delta(7)]
        );
    }

    #[test]
    fn clc_message_count_is_capped() {
        let mut w = MsgWriter::new();
        for _ in 0..MAX_CLC_MESSAGES_PER_PAYLOAD {
            w.write_u8(clc::DELTA);
            w.write_u8(1);
        }
        let at_cap = parse_clc_stream(w.as_slice(), &usercmd_schema(), 1).unwrap();
        assert_eq!(at_cap.len(), MAX_CLC_MESSAGES_PER_PAYLOAD);
        w.write_u8(clc::DELTA);
        w.write_u8(1);
        assert!(matches!(
            parse_clc_stream(w.as_slice(), &usercmd_schema(), 1),
            Err(ProtoError::InvalidPayload(_))
        ));
    }

    #[test]
    fn svc_choke_roundtrips() {
        let mut w = MsgWriter::new();
        write_svc_choke(&mut w);
        assert_eq!(w.as_slice(), &[svc::CHOKE]);
        let mut ctx = SvcParseContext::new();
        assert_eq!(
            parse_svc_stream(w.as_slice(), &mut ctx).unwrap(),
            vec![SvcMessage::Choke]
        );
    }

    #[test]
    fn unknown_temp_entity_type_is_an_error() {
        // 250 is not a TE_* type (they end at TE_USERTRACER = 127).
        let stream = [svc::TEMP_ENTITY, 250, 0, 0, 0, 0];
        let mut ctx = SvcParseContext::new();
        assert!(matches!(
            parse_svc_stream(&stream, &mut ctx),
            Err(ProtoError::InvalidPayload(_))
        ));
    }
}
