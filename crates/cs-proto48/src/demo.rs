//! GoldSrc **HLDEMO Protocol 5** (`.dem`) parser and writer.
//!
//! Ported from GPL-3 `hldemo` spec and Xash3D FWGS `engine/client/cl_demo.c` structures
//! (`header = 544 B`, `directory_entry = 92 B`, `netmsg demo_info_t = 436 B`,
//! `usercmd_t = 52 B`), using `#![forbid(unsafe_code)]` with bounded allocation guards.

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::delta::GoldSrcUserCmd;
use crate::messages::MoveVarsMsg;
use glam::Vec3;

const MAX_SOUND_SAMPLE_BYTES: usize = 4096;

pub const DEMO_MAGIC: [u8; 8] = *b"HLDEMO\0\0";
pub const DEMO_PROTOCOL: i32 = 5;
pub const DEMO_HEADER_BYTES: usize = 544;
pub const DEMO_DIR_ENTRY_BYTES: usize = 92;
pub const DEMO_NETMSG_INFO_BYTES: usize = 436;
pub const DEMO_USERCMD_BYTES: usize = 52;
pub const MAX_DIRECTORY_ENTRIES: usize = 1024;
pub const MAX_DEMO_MSG_BYTES: usize = 65_536;
pub const MAX_DEMO_FRAMES_PER_SECTION: usize = 2_000_000;

/// Top-level GoldSrc protocol 5 demo file.
#[derive(Debug, Clone, PartialEq)]
pub struct DemoFile {
    pub header: DemoHeader,
    pub directory: Vec<DemoDirectoryEntry>,
}

/// 544-byte `demoheader_t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoHeader {
    pub demo_protocol: i32,
    pub net_protocol: i32,
    pub map_name: String,
    pub game_dir: String,
    pub map_crc: u32,
    pub directory_offset: i32,
}

/// 92-byte directory entry plus its decoded frames.
#[derive(Debug, Clone, PartialEq)]
pub struct DemoDirectoryEntry {
    pub entry_type: i32,
    pub description: String,
    pub flags: i32,
    pub cd_track: i32,
    pub track_time: f32,
    pub frame_count: i32,
    pub offset: i32,
    pub file_length: i32,
    pub frames: Vec<DemoFrame>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DemoFrame {
    /// Raw frame type byte (`0`/`1` = NetMsg, `2`..=`9` = typed demo frames).
    pub frame_type_raw: u8,
    pub time: f32,
    pub frame: i32,
    pub data: DemoFrameData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DemoFrameData {
    NetMsg(Box<DemoNetMsgFrame>),
    DemoStart,
    ConsoleCommand(String),
    ClientData(DemoClientData),
    NextSection,
    Event(DemoEvent),
    WeaponAnim {
        anim: i32,
        body: i32,
    },
    Sound {
        channel: i32,
        sample: Vec<u8>,
        attenuation: f32,
        volume: f32,
        flags: i32,
        pitch: i32,
    },
    DemoBuffer(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DemoNetMsgFrame {
    pub info: DemoNetMsgInfo,
    pub incoming_sequence: i32,
    pub incoming_acknowledged: i32,
    pub incoming_reliable_acknowledged: i32,
    pub incoming_reliable_sequence: i32,
    pub outgoing_sequence: i32,
    pub reliable_sequence: i32,
    pub last_reliable_sequence: i32,
    pub msg: Vec<u8>,
}

/// 436-byte `demo_info_t`.
#[derive(Debug, Clone, PartialEq)]
pub struct DemoNetMsgInfo {
    pub timestamp: f32,
    pub ref_params: DemoRefParams,
    pub cmd: DemoUserCmd,
    pub movevars: DemoMoveVars,
    pub view: Vec3,
    pub viewmodel: i32,
}

impl Default for DemoNetMsgInfo {
    fn default() -> Self {
        Self {
            timestamp: 0.0,
            ref_params: DemoRefParams::default(),
            cmd: DemoUserCmd::default(),
            movevars: DemoMoveVars::default(),
            view: Vec3::ZERO,
            viewmodel: 0,
        }
    }
}

/// 232-byte `ref_params_t`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DemoRefParams {
    pub vieworg: Vec3,
    pub viewangles: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub frametime: f32,
    pub time: f32,
    pub intermission: i32,
    pub paused: i32,
    pub spectator: i32,
    pub onground: i32,
    pub waterlevel: i32,
    pub simvel: Vec3,
    pub simorg: Vec3,
    pub viewheight: Vec3,
    pub idealpitch: f32,
    pub cl_viewangles: Vec3,
    pub health: i32,
    pub crosshairangle: Vec3,
    pub viewsize: f32,
    pub punchangle: Vec3,
    pub maxclients: i32,
    pub viewentity: i32,
    pub playernum: i32,
    pub max_entities: i32,
    pub demoplayback: i32,
    pub hardware: i32,
    pub smoothing: i32,
    pub ptr_cmd: i32,
    pub ptr_movevars: i32,
    pub viewport: [i32; 4],
    pub next_view: i32,
    pub only_client_draw: i32,
}

/// 52-byte C-aligned `usercmd_t` embedded inside `demo_info_t`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DemoUserCmd {
    pub lerp_msec: i16,
    pub msec: u8,
    pub viewangles: Vec3,
    pub forwardmove: f32,
    pub sidemove: f32,
    pub upmove: f32,
    pub lightlevel: i8,
    pub buttons: u16,
    pub impulse: u8,
    pub weaponselect: i8,
    pub impact_index: i32,
    pub impact_position: Vec3,
}

impl From<&GoldSrcUserCmd> for DemoUserCmd {
    fn from(cmd: &GoldSrcUserCmd) -> Self {
        Self {
            lerp_msec: cmd.lerp_msec,
            msec: cmd.msec,
            viewangles: cmd.viewangles,
            forwardmove: cmd.forwardmove,
            sidemove: cmd.sidemove,
            upmove: cmd.upmove,
            lightlevel: cmd.lightlevel as i8,
            buttons: cmd.buttons,
            impulse: cmd.impulse,
            weaponselect: cmd.weaponselect as i8,
            impact_index: cmd.impact_index,
            impact_position: cmd.impact_position,
        }
    }
}

/// 132-byte `movevars_t` embedded inside `demo_info_t`.
#[derive(Debug, Clone, PartialEq)]
pub struct DemoMoveVars {
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
    pub footsteps: i32,
    pub sky_name: String,
    pub rollangle: f32,
    pub rollspeed: f32,
    pub skycolor: [f32; 3],
    pub skyvec: [f32; 3],
}

impl Default for DemoMoveVars {
    fn default() -> Self {
        Self::from(&MoveVarsMsg::default())
    }
}

impl From<&MoveVarsMsg> for DemoMoveVars {
    fn from(m: &MoveVarsMsg) -> Self {
        Self {
            gravity: m.gravity,
            stopspeed: m.stopspeed,
            maxspeed: m.maxspeed,
            spectatormaxspeed: m.spectatormaxspeed,
            accelerate: m.accelerate,
            airaccelerate: m.airaccelerate,
            wateraccelerate: m.wateraccelerate,
            friction: m.friction,
            edgefriction: m.edgefriction,
            waterfriction: m.waterfriction,
            entgravity: m.entgravity,
            bounce: m.bounce,
            stepsize: m.stepsize,
            maxvelocity: m.maxvelocity,
            zmax: m.zmax,
            wave_height: m.wave_height,
            footsteps: i32::from(m.footsteps),
            sky_name: m.sky_name.clone(),
            rollangle: m.rollangle,
            rollspeed: m.rollspeed,
            skycolor: m.skycolor,
            skyvec: m.skyvec,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoClientData {
    pub origin: Vec3,
    pub viewangles: Vec3,
    pub weapon_bits: i32,
    pub fov: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DemoEvent {
    pub flags: i32,
    pub index: i32,
    pub delay: f32,
    pub args: DemoEventArgs,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DemoEventArgs {
    pub flags: i32,
    pub entity_index: i32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub velocity: Vec3,
    pub ducking: i32,
    pub fparam1: f32,
    pub fparam2: f32,
    pub iparam1: i32,
    pub iparam2: i32,
    pub bparam1: i32,
    pub bparam2: i32,
}

fn read_fixed_cstr(r: &mut MsgReader<'_>, len: usize) -> Result<String, ProtoError> {
    let raw = r.read_bytes(len)?;
    let nul = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Ok(String::from_utf8_lossy(&raw[..nul]).into_owned())
}

fn write_fixed_cstr(w: &mut MsgWriter, s: &str, len: usize) {
    let bytes = s.as_bytes();
    let copy_len = bytes.len().min(len.saturating_sub(1));
    w.write_bytes(&bytes[..copy_len]);
    for _ in copy_len..len {
        w.write_u8(0);
    }
}

fn read_vec3(r: &mut MsgReader<'_>) -> Result<Vec3, ProtoError> {
    Ok(Vec3::new(r.read_f32()?, r.read_f32()?, r.read_f32()?))
}

fn write_vec3(w: &mut MsgWriter, v: Vec3) {
    w.write_f32(v.x);
    w.write_f32(v.y);
    w.write_f32(v.z);
}

/// Parses a complete GoldSrc `HLDEMO` protocol 5 `.dem` buffer.
pub fn parse_demo(data: &[u8]) -> Result<DemoFile, ProtoError> {
    if data.len() < DEMO_HEADER_BYTES {
        return Err(ProtoError::UnexpectedEof);
    }
    let mut hr = MsgReader::new(&data[..DEMO_HEADER_BYTES]);
    let magic = hr.read_bytes(8)?;
    if &magic[..6] != b"HLDEMO" {
        return Err(ProtoError::InvalidPayload("invalid HLDEMO magic".into()));
    }
    let demo_protocol = hr.read_i32()?;
    if demo_protocol != DEMO_PROTOCOL {
        return Err(ProtoError::InvalidPayload(format!(
            "unsupported HLDEMO protocol {demo_protocol} (expected {DEMO_PROTOCOL})"
        )));
    }
    let net_protocol = hr.read_i32()?;
    let map_name = read_fixed_cstr(&mut hr, 260)?;
    let game_dir = read_fixed_cstr(&mut hr, 260)?;
    let map_crc = hr.read_u32()?;
    let directory_offset = hr.read_i32()?;

    let dir_off = usize::try_from(directory_offset)
        .map_err(|_| ProtoError::InvalidPayload("negative directory_offset".into()))?;
    if dir_off < DEMO_HEADER_BYTES || dir_off.saturating_add(4) > data.len() {
        return Err(ProtoError::InvalidPayload(format!(
            "directory_offset {dir_off} out of bounds (file len {})",
            data.len()
        )));
    }
    let mut dr = MsgReader::new(&data[dir_off..]);
    let count_i32 = dr.read_i32()?;
    let count = usize::try_from(count_i32)
        .ok()
        .filter(|&c| (1..=MAX_DIRECTORY_ENTRIES).contains(&c))
        .ok_or_else(|| {
            ProtoError::InvalidPayload(format!("invalid directory count {count_i32}"))
        })?;
    let needed = count.saturating_mul(DEMO_DIR_ENTRY_BYTES);
    if dr.remaining() < needed {
        return Err(ProtoError::UnexpectedEof);
    }

    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let entry_type = dr.read_i32()?;
        let description = read_fixed_cstr(&mut dr, 64)?;
        let flags = dr.read_i32()?;
        let cd_track = dr.read_i32()?;
        let track_time = dr.read_f32()?;
        let frame_count = dr.read_i32()?;
        let offset = dr.read_i32()?;
        let file_length = dr.read_i32()?;
        entries.push(DemoDirectoryEntry {
            entry_type,
            description,
            flags,
            cd_track,
            track_time,
            frame_count,
            offset,
            file_length,
            frames: Vec::new(),
        });
    }

    for entry in &mut entries {
        let off = usize::try_from(entry.offset)
            .map_err(|_| ProtoError::InvalidPayload("negative section offset".into()))?;
        let end = if entry.file_length > 0 {
            let len = usize::try_from(entry.file_length)
                .map_err(|_| ProtoError::InvalidPayload("invalid section file_length".into()))?;
            off.checked_add(len)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| ProtoError::InvalidPayload("section out of file bounds".into()))?
        } else {
            dir_off.min(data.len())
        };
        if off < DEMO_HEADER_BYTES || off > end {
            return Err(ProtoError::InvalidPayload(format!(
                "section offset {off}..{end} out of bounds"
            )));
        }
        entry.frames = parse_section_frames(&data[off..end])?;
    }

    Ok(DemoFile {
        header: DemoHeader {
            demo_protocol,
            net_protocol,
            map_name,
            game_dir,
            map_crc,
            directory_offset,
        },
        directory: entries,
    })
}

fn parse_section_frames(section: &[u8]) -> Result<Vec<DemoFrame>, ProtoError> {
    let mut r = MsgReader::new(section);
    let mut frames = Vec::new();
    while r.remaining() > 0 {
        if frames.len() >= MAX_DEMO_FRAMES_PER_SECTION {
            return Err(ProtoError::InvalidPayload("demo section frame limit exceeded".into()));
        }
        let frame_type_raw = r.read_u8()?;
        let time = r.read_f32()?;
        let frame = r.read_i32()?;
        let data = match frame_type_raw {
            0 | 1 => {
                let info = read_netmsg_info(&mut r)?;
                let incoming_sequence = r.read_i32()?;
                let incoming_acknowledged = r.read_i32()?;
                let incoming_reliable_acknowledged = r.read_i32()?;
                let incoming_reliable_sequence = r.read_i32()?;
                let outgoing_sequence = r.read_i32()?;
                let reliable_sequence = r.read_i32()?;
                let last_reliable_sequence = r.read_i32()?;
                let msg_len_i32 = r.read_i32()?;
                let msg_len = usize::try_from(msg_len_i32)
                    .ok()
                    .filter(|&n| n <= MAX_DEMO_MSG_BYTES)
                    .ok_or_else(|| {
                        ProtoError::InvalidPayload(format!("invalid demo msg length {msg_len_i32}"))
                    })?;
                let msg = r.read_bytes(msg_len)?.to_vec();
                DemoFrameData::NetMsg(Box::new(DemoNetMsgFrame {
                    info,
                    incoming_sequence,
                    incoming_acknowledged,
                    incoming_reliable_acknowledged,
                    incoming_reliable_sequence,
                    outgoing_sequence,
                    reliable_sequence,
                    last_reliable_sequence,
                    msg,
                }))
            }
            2 => DemoFrameData::DemoStart,
            3 => DemoFrameData::ConsoleCommand(read_fixed_cstr(&mut r, 64)?),
            4 => DemoFrameData::ClientData(DemoClientData {
                origin: read_vec3(&mut r)?,
                viewangles: read_vec3(&mut r)?,
                weapon_bits: r.read_i32()?,
                fov: r.read_f32()?,
            }),
            5 => DemoFrameData::NextSection,
            6 => DemoFrameData::Event(DemoEvent {
                flags: r.read_i32()?,
                index: r.read_i32()?,
                delay: r.read_f32()?,
                args: DemoEventArgs {
                    flags: r.read_i32()?,
                    entity_index: r.read_i32()?,
                    origin: read_vec3(&mut r)?,
                    angles: read_vec3(&mut r)?,
                    velocity: read_vec3(&mut r)?,
                    ducking: r.read_i32()?,
                    fparam1: r.read_f32()?,
                    fparam2: r.read_f32()?,
                    iparam1: r.read_i32()?,
                    iparam2: r.read_i32()?,
                    bparam1: r.read_i32()?,
                    bparam2: r.read_i32()?,
                },
            }),
            7 => DemoFrameData::WeaponAnim {
                anim: r.read_i32()?,
                body: r.read_i32()?,
            },
            8 => {
                let channel = r.read_i32()?;
                let sample_len_i32 = r.read_i32()?;
                let sample_len = usize::try_from(sample_len_i32)
                    .ok()
                    .filter(|&n| n <= MAX_SOUND_SAMPLE_BYTES)
                    .ok_or_else(|| {
                        ProtoError::InvalidPayload(format!("invalid demo sound sample len {sample_len_i32}"))
                    })?;
                let sample = r.read_bytes(sample_len)?.to_vec();
                let attenuation = r.read_f32()?;
                let volume = r.read_f32()?;
                let flags = r.read_i32()?;
                let pitch = r.read_i32()?;
                DemoFrameData::Sound {
                    channel,
                    sample,
                    attenuation,
                    volume,
                    flags,
                    pitch,
                }
            }
            9 => {
                let buf_len_i32 = r.read_i32()?;
                let buf_len = usize::try_from(buf_len_i32)
                    .ok()
                    .filter(|&n| n <= MAX_DEMO_MSG_BYTES)
                    .ok_or_else(|| {
                        ProtoError::InvalidPayload(format!("invalid demo buffer len {buf_len_i32}"))
                    })?;
                DemoFrameData::DemoBuffer(r.read_bytes(buf_len)?.to_vec())
            }
            other => {
                return Err(ProtoError::InvalidPayload(format!(
                    "unknown HLDEMO frame type {other}"
                )));
            }
        };
        let done = matches!(data, DemoFrameData::NextSection);
        frames.push(DemoFrame {
            frame_type_raw,
            time,
            frame,
            data,
        });
        if done {
            break;
        }
    }
    Ok(frames)
}

fn read_netmsg_info(r: &mut MsgReader<'_>) -> Result<DemoNetMsgInfo, ProtoError> {
    if r.remaining() < DEMO_NETMSG_INFO_BYTES {
        return Err(ProtoError::UnexpectedEof);
    }
    let timestamp = r.read_f32()?;
    let ref_params = DemoRefParams {
        vieworg: read_vec3(r)?,
        viewangles: read_vec3(r)?,
        forward: read_vec3(r)?,
        right: read_vec3(r)?,
        up: read_vec3(r)?,
        frametime: r.read_f32()?,
        time: r.read_f32()?,
        intermission: r.read_i32()?,
        paused: r.read_i32()?,
        spectator: r.read_i32()?,
        onground: r.read_i32()?,
        waterlevel: r.read_i32()?,
        simvel: read_vec3(r)?,
        simorg: read_vec3(r)?,
        viewheight: read_vec3(r)?,
        idealpitch: r.read_f32()?,
        cl_viewangles: read_vec3(r)?,
        health: r.read_i32()?,
        crosshairangle: read_vec3(r)?,
        viewsize: r.read_f32()?,
        punchangle: read_vec3(r)?,
        maxclients: r.read_i32()?,
        viewentity: r.read_i32()?,
        playernum: r.read_i32()?,
        max_entities: r.read_i32()?,
        demoplayback: r.read_i32()?,
        hardware: r.read_i32()?,
        smoothing: r.read_i32()?,
        ptr_cmd: r.read_i32()?,
        ptr_movevars: r.read_i32()?,
        viewport: [r.read_i32()?, r.read_i32()?, r.read_i32()?, r.read_i32()?],
        next_view: r.read_i32()?,
        only_client_draw: r.read_i32()?,
    };
    let lerp_msec = r.read_i16()?;
    let msec = r.read_u8()?;
    let _pad_cmd_0 = r.read_u8()?;
    let cmd_viewangles = read_vec3(r)?;
    let forwardmove = r.read_f32()?;
    let sidemove = r.read_f32()?;
    let upmove = r.read_f32()?;
    let lightlevel = r.read_i8()?;
    let _pad_cmd_1 = r.read_u8()?;
    let buttons = r.read_u16()?;
    let impulse = r.read_u8()?;
    let weaponselect = r.read_i8()?;
    let _pad_cmd_2 = r.read_bytes(2)?;
    let impact_index = r.read_i32()?;
    let impact_position = read_vec3(r)?;
    let cmd = DemoUserCmd {
        lerp_msec,
        msec,
        viewangles: cmd_viewangles,
        forwardmove,
        sidemove,
        upmove,
        lightlevel,
        buttons,
        impulse,
        weaponselect,
        impact_index,
        impact_position,
    };

    let movevars = DemoMoveVars {
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
        footsteps: r.read_i32()?,
        sky_name: read_fixed_cstr(r, 32)?,
        rollangle: r.read_f32()?,
        rollspeed: r.read_f32()?,
        skycolor: [r.read_f32()?, r.read_f32()?, r.read_f32()?],
        skyvec: [r.read_f32()?, r.read_f32()?, r.read_f32()?],
    };
    let view = read_vec3(r)?;
    let viewmodel = r.read_i32()?;
    Ok(DemoNetMsgInfo {
        timestamp,
        ref_params,
        cmd,
        movevars,
        view,
        viewmodel,
    })
}

fn write_netmsg_info(w: &mut MsgWriter, info: &DemoNetMsgInfo) {
    w.write_f32(info.timestamp);
    let rp = &info.ref_params;
    write_vec3(w, rp.vieworg);
    write_vec3(w, rp.viewangles);
    write_vec3(w, rp.forward);
    write_vec3(w, rp.right);
    write_vec3(w, rp.up);
    w.write_f32(rp.frametime);
    w.write_f32(rp.time);
    w.write_i32(rp.intermission);
    w.write_i32(rp.paused);
    w.write_i32(rp.spectator);
    w.write_i32(rp.onground);
    w.write_i32(rp.waterlevel);
    write_vec3(w, rp.simvel);
    write_vec3(w, rp.simorg);
    write_vec3(w, rp.viewheight);
    w.write_f32(rp.idealpitch);
    write_vec3(w, rp.cl_viewangles);
    w.write_i32(rp.health);
    write_vec3(w, rp.crosshairangle);
    w.write_f32(rp.viewsize);
    write_vec3(w, rp.punchangle);
    w.write_i32(rp.maxclients);
    w.write_i32(rp.viewentity);
    w.write_i32(rp.playernum);
    w.write_i32(rp.max_entities);
    w.write_i32(rp.demoplayback);
    w.write_i32(rp.hardware);
    w.write_i32(rp.smoothing);
    w.write_i32(rp.ptr_cmd);
    w.write_i32(rp.ptr_movevars);
    for v in rp.viewport {
        w.write_i32(v);
    }
    w.write_i32(rp.next_view);
    w.write_i32(rp.only_client_draw);

    let cmd = &info.cmd;
    w.write_i16(cmd.lerp_msec);
    w.write_u8(cmd.msec);
    w.write_u8(0); // alignment pad before vec3 viewangles
    write_vec3(w, cmd.viewangles);
    w.write_f32(cmd.forwardmove);
    w.write_f32(cmd.sidemove);
    w.write_f32(cmd.upmove);
    w.write_i8(cmd.lightlevel);
    w.write_u8(0); // alignment pad before u16 buttons
    w.write_u16(cmd.buttons);
    w.write_u8(cmd.impulse);
    w.write_i8(cmd.weaponselect);
    w.write_u16(0); // alignment pad before i32 impact_index
    w.write_i32(cmd.impact_index);
    write_vec3(w, cmd.impact_position);

    let mv = &info.movevars;
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
    w.write_i32(mv.footsteps);
    write_fixed_cstr(w, &mv.sky_name, 32);
    w.write_f32(mv.rollangle);
    w.write_f32(mv.rollspeed);
    for c in mv.skycolor {
        w.write_f32(c);
    }
    for v in mv.skyvec {
        w.write_f32(v);
    }
    write_vec3(w, info.view);
    w.write_i32(info.viewmodel);
}

/// Serializes a [`DemoFile`] to valid GoldSrc `HLDEMO` v5 bytes, recomputing section
/// offsets, byte lengths, and the directory offset automatically.
pub fn write_demo(demo: &DemoFile) -> Result<Vec<u8>, ProtoError> {
    if demo.directory.is_empty() || demo.directory.len() > MAX_DIRECTORY_ENTRIES {
        return Err(ProtoError::InvalidPayload(format!(
            "invalid directory count {}",
            demo.directory.len()
        )));
    }
    let mut section_blobs: Vec<Vec<u8>> = Vec::with_capacity(demo.directory.len());
    for entry in &demo.directory {
        let mut sw = MsgWriter::new();
        for f in &entry.frames {
            let type_byte = match &f.data {
                DemoFrameData::NetMsg(_) => {
                    if f.frame_type_raw <= 1 {
                        f.frame_type_raw
                    } else {
                        1
                    }
                }
                DemoFrameData::DemoStart => 2,
                DemoFrameData::ConsoleCommand(_) => 3,
                DemoFrameData::ClientData(_) => 4,
                DemoFrameData::NextSection => 5,
                DemoFrameData::Event(_) => 6,
                DemoFrameData::WeaponAnim { .. } => 7,
                DemoFrameData::Sound { .. } => 8,
                DemoFrameData::DemoBuffer(_) => 9,
            };
            sw.write_u8(type_byte);
            sw.write_f32(f.time);
            sw.write_i32(f.frame);
            match &f.data {
                DemoFrameData::NetMsg(nm) => {
                    if nm.msg.len() > MAX_DEMO_MSG_BYTES {
                        return Err(ProtoError::InvalidPayload("demo netmsg too large".into()));
                    }
                    write_netmsg_info(&mut sw, &nm.info);
                    sw.write_i32(nm.incoming_sequence);
                    sw.write_i32(nm.incoming_acknowledged);
                    sw.write_i32(nm.incoming_reliable_acknowledged);
                    sw.write_i32(nm.incoming_reliable_sequence);
                    sw.write_i32(nm.outgoing_sequence);
                    sw.write_i32(nm.reliable_sequence);
                    sw.write_i32(nm.last_reliable_sequence);
                    sw.write_i32(nm.msg.len() as i32);
                    sw.write_bytes(&nm.msg);
                }
                DemoFrameData::DemoStart | DemoFrameData::NextSection => {}
                DemoFrameData::ConsoleCommand(cmd) => write_fixed_cstr(&mut sw, cmd, 64),
                DemoFrameData::ClientData(cd) => {
                    write_vec3(&mut sw, cd.origin);
                    write_vec3(&mut sw, cd.viewangles);
                    sw.write_i32(cd.weapon_bits);
                    sw.write_f32(cd.fov);
                }
                DemoFrameData::Event(ev) => {
                    sw.write_i32(ev.flags);
                    sw.write_i32(ev.index);
                    sw.write_f32(ev.delay);
                    sw.write_i32(ev.args.flags);
                    sw.write_i32(ev.args.entity_index);
                    write_vec3(&mut sw, ev.args.origin);
                    write_vec3(&mut sw, ev.args.angles);
                    write_vec3(&mut sw, ev.args.velocity);
                    sw.write_i32(ev.args.ducking);
                    sw.write_f32(ev.args.fparam1);
                    sw.write_f32(ev.args.fparam2);
                    sw.write_i32(ev.args.iparam1);
                    sw.write_i32(ev.args.iparam2);
                    sw.write_i32(ev.args.bparam1);
                    sw.write_i32(ev.args.bparam2);
                }
                DemoFrameData::WeaponAnim { anim, body } => {
                    sw.write_i32(*anim);
                    sw.write_i32(*body);
                }
                DemoFrameData::Sound {
                    channel,
                    sample,
                    attenuation,
                    volume,
                    flags,
                    pitch,
                } => {
                    sw.write_i32(*channel);
                    sw.write_i32(sample.len() as i32);
                    sw.write_bytes(sample);
                    sw.write_f32(*attenuation);
                    sw.write_f32(*volume);
                    sw.write_i32(*flags);
                    sw.write_i32(*pitch);
                }
                DemoFrameData::DemoBuffer(buf) => {
                    sw.write_i32(buf.len() as i32);
                    sw.write_bytes(buf);
                }
            }
        }
        section_blobs.push(sw.into_bytes());
    }

    let mut offsets = Vec::with_capacity(section_blobs.len());
    let mut cursor = DEMO_HEADER_BYTES;
    for blob in &section_blobs {
        offsets.push(cursor);
        cursor = cursor
            .checked_add(blob.len())
            .ok_or_else(|| ProtoError::InvalidPayload("demo size overflow".into()))?;
    }
    let dir_offset = i32::try_from(cursor)
        .map_err(|_| ProtoError::InvalidPayload("directory offset overflow".into()))?;

    let mut out = MsgWriter::with_capacity(
        cursor + 4 + demo.directory.len() * DEMO_DIR_ENTRY_BYTES,
    );
    out.write_bytes(&DEMO_MAGIC);
    out.write_i32(DEMO_PROTOCOL);
    out.write_i32(demo.header.net_protocol);
    write_fixed_cstr(&mut out, &demo.header.map_name, 260);
    write_fixed_cstr(&mut out, &demo.header.game_dir, 260);
    out.write_u32(demo.header.map_crc);
    out.write_i32(dir_offset);
    debug_assert_eq!(out.len(), DEMO_HEADER_BYTES);

    for blob in &section_blobs {
        out.write_bytes(blob);
    }
    out.write_i32(demo.directory.len() as i32);
    for (idx, entry) in demo.directory.iter().enumerate() {
        out.write_i32(entry.entry_type);
        write_fixed_cstr(&mut out, &entry.description, 64);
        out.write_i32(entry.flags);
        out.write_i32(entry.cd_track);
        out.write_f32(entry.track_time);
        out.write_i32(entry.frames.len() as i32);
        out.write_i32(offsets[idx] as i32);
        out.write_i32(section_blobs[idx].len() as i32);
    }
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{SvcMessage, SvcParseContext, parse_svc_stream, write_svc_time};

    #[test]
    fn wire_struct_sizes_match_goldsrc_c_layout() {
        let mut info_w = MsgWriter::new();
        write_netmsg_info(&mut info_w, &DemoNetMsgInfo::default());
        assert_eq!(info_w.len(), DEMO_NETMSG_INFO_BYTES);

        let demo = DemoFile {
            header: DemoHeader {
                demo_protocol: DEMO_PROTOCOL,
                net_protocol: 48,
                map_name: "de_dust2".into(),
                game_dir: "cstrike".into(),
                map_crc: 0x1234_5678,
                directory_offset: 0,
            },
            directory: vec![DemoDirectoryEntry {
                entry_type: 1,
                description: "Playback".into(),
                flags: 0,
                cd_track: 0,
                track_time: 0.0,
                frame_count: 1,
                offset: 0,
                file_length: 0,
                frames: vec![DemoFrame {
                    frame_type_raw: 5,
                    time: 0.0,
                    frame: 0,
                    data: DemoFrameData::NextSection,
                }],
            }],
        };
        let bytes = write_demo(&demo).unwrap();
        let dir_off = i32::from_le_bytes(bytes[540..544].try_into().unwrap()) as usize;
        assert_eq!(DEMO_HEADER_BYTES, 544);
        // Header (544) + NextSection frame (1 + 4 + 4 = 9) -> dir_off = 553
        assert_eq!(dir_off, 553);
        // Directory = 4 + 1 * 92
        assert_eq!(bytes.len() - dir_off, 4 + DEMO_DIR_ENTRY_BYTES);
    }

    #[test]
    fn roundtrips_all_nine_frame_types_across_sections() {
        let mut svc_w = MsgWriter::new();
        write_svc_time(&mut svc_w, 12.5);
        let net_msg = DemoNetMsgFrame {
            info: DemoNetMsgInfo {
                timestamp: 12.5,
                viewmodel: 3,
                view: Vec3::new(0.0, 0.0, 28.0),
                cmd: DemoUserCmd {
                    lerp_msec: 100,
                    msec: 10,
                    viewangles: Vec3::new(5.0, 90.0, 0.0),
                    forwardmove: 250.0,
                    buttons: 1,
                    impulse: 201,
                    weaponselect: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
            incoming_sequence: 10,
            incoming_acknowledged: 9,
            incoming_reliable_acknowledged: 1,
            incoming_reliable_sequence: 1,
            outgoing_sequence: 11,
            reliable_sequence: 0,
            last_reliable_sequence: 9,
            msg: svc_w.into_bytes(),
        };

        let demo = DemoFile {
            header: DemoHeader {
                demo_protocol: DEMO_PROTOCOL,
                net_protocol: 48,
                map_name: "de_nuke".into(),
                game_dir: "cstrike".into(),
                map_crc: 0xDEAD_BEEF,
                directory_offset: 0,
            },
            directory: vec![
                DemoDirectoryEntry {
                    entry_type: 0,
                    description: "LOADING".into(),
                    flags: 0,
                    cd_track: 0,
                    track_time: 0.0,
                    frame_count: 0,
                    offset: 0,
                    file_length: 0,
                    frames: vec![
                        DemoFrame {
                            frame_type_raw: 0,
                            time: 0.0,
                            frame: 0,
                            data: DemoFrameData::NetMsg(Box::new(net_msg.clone())),
                        },
                        DemoFrame {
                            frame_type_raw: 5,
                            time: 0.0,
                            frame: 0,
                            data: DemoFrameData::NextSection,
                        },
                    ],
                },
                DemoDirectoryEntry {
                    entry_type: 1,
                    description: "Playback".into(),
                    flags: 0,
                    cd_track: 0,
                    track_time: 1.5,
                    frame_count: 0,
                    offset: 0,
                    file_length: 0,
                    frames: vec![
                        DemoFrame {
                            frame_type_raw: 2,
                            time: 0.1,
                            frame: 1,
                            data: DemoFrameData::DemoStart,
                        },
                        DemoFrame {
                            frame_type_raw: 3,
                            time: 0.2,
                            frame: 2,
                            data: DemoFrameData::ConsoleCommand("say glhf".into()),
                        },
                        DemoFrame {
                            frame_type_raw: 4,
                            time: 0.3,
                            frame: 3,
                            data: DemoFrameData::ClientData(DemoClientData {
                                origin: Vec3::new(100.0, 200.0, 64.0),
                                viewangles: Vec3::new(0.0, 45.0, 0.0),
                                weapon_bits: 0b101,
                                fov: 90.0,
                            }),
                        },
                        DemoFrame {
                            frame_type_raw: 6,
                            time: 0.4,
                            frame: 4,
                            data: DemoFrameData::Event(DemoEvent {
                                flags: 1,
                                index: 7,
                                delay: 0.0,
                                args: DemoEventArgs {
                                    entity_index: 1,
                                    origin: Vec3::new(1.0, 2.0, 3.0),
                                    ..Default::default()
                                },
                            }),
                        },
                        DemoFrame {
                            frame_type_raw: 7,
                            time: 0.5,
                            frame: 5,
                            data: DemoFrameData::WeaponAnim { anim: 2, body: 1 },
                        },
                        DemoFrame {
                            frame_type_raw: 8,
                            time: 0.6,
                            frame: 6,
                            data: DemoFrameData::Sound {
                                channel: 1,
                                sample: b"weapons/ak47-1.wav\0".to_vec(),
                                attenuation: 0.8,
                                volume: 1.0,
                                flags: 0,
                                pitch: 100,
                            },
                        },
                        DemoFrame {
                            frame_type_raw: 9,
                            time: 0.7,
                            frame: 7,
                            data: DemoFrameData::DemoBuffer(vec![1, 2, 3, 4]),
                        },
                        DemoFrame {
                            frame_type_raw: 1,
                            time: 1.5,
                            frame: 8,
                            data: DemoFrameData::NetMsg(Box::new(net_msg)),
                        },
                        DemoFrame {
                            frame_type_raw: 5,
                            time: 1.5,
                            frame: 9,
                            data: DemoFrameData::NextSection,
                        },
                    ],
                },
            ],
        };

        let bytes = write_demo(&demo).unwrap();
        let parsed = parse_demo(&bytes).unwrap();
        assert_eq!(parsed.header.map_name, "de_nuke");
        assert_eq!(parsed.header.game_dir, "cstrike");
        assert_eq!(parsed.header.map_crc, 0xDEAD_BEEF);
        assert_eq!(parsed.directory.len(), 2);
        assert_eq!(parsed.directory[0].frames.len(), 2);
        assert_eq!(parsed.directory[1].frames.len(), 9);
        if let DemoFrameData::NetMsg(nm) = &parsed.directory[1].frames[7].data {
            let mut ctx = SvcParseContext::default();
            let msgs = parse_svc_stream(&nm.msg, &mut ctx).unwrap();
            assert_eq!(msgs, vec![SvcMessage::Time(12.5)]);
        } else {
            panic!("expected netmsg");
        }
    }

    #[test]
    fn rejects_corrupt_or_out_of_bounds_demos() {
        assert!(parse_demo(b"short").is_err());
        let mut bad = vec![0u8; DEMO_HEADER_BYTES + 32];
        bad[..8].copy_from_slice(&DEMO_MAGIC);
        bad[8..12].copy_from_slice(&4i32.to_le_bytes()); // bad version
        assert!(parse_demo(&bad).is_err());
        bad[8..12].copy_from_slice(&5i32.to_le_bytes());
        bad[540..544].copy_from_slice(&999_999i32.to_le_bytes()); // OOB directory offset
        assert!(parse_demo(&bad).is_err());
    }
}
