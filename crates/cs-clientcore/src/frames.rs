//! The frame ring, the client clock and the dispatch of the messages that feed the entity
//! store: `svc_time`, `svc_clientdata`, `svc_packetentities` / `svc_deltapacketentities`,
//! `svc_spawnbaseline` (and `svc_serverinfo`, `svc_newmovevars`, `svc_setpause`, which carry
//! values the store needs).
//!
//! Ported from Xash3D FWGS (GPL-3.0-or-later; read 2026-11-23): `engine/client/cl_frame.c`
//! (`CL_ValidateDeltaPacket`, `CL_ParsePacketEntities`, `CL_EmitEntities`),
//! `engine/client/cl_parse_gs.c` (`CL_ParsePacketEntitiesGS`, `CL_FlushEntityPacketGS`: the
//! GoldSrc wire path), `engine/client/cl_parse.c` (`CL_ParseServerTime`, `CL_ParseClientData`'s
//! frame bookkeeping) and `engine/client/cl_main.c` (`CL_AdjustClock`, `CL_ReadPackets`).
//!
//! The bit-level delta decoding of an entity packet is done by `cs_proto48::messages` when it
//! parses the message: what arrives here is the complete entity set of the new frame. This
//! module owns what Xash3D keeps around it: the 64-slot ring of frames, the check that a delta
//! is based on a frame the client really holds, which entities are new in a frame (their
//! interpolation starts over), and the clock the interpolation reads.

use std::collections::BTreeMap;

use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcWeaponData};
use cs_proto48::messages::{PacketEntitiesMsg, SvcMessage, SvcParseContext, entity_schema_name};

use crate::cvars::ClientCvars;
use crate::entities::{EntityStore, ProcessContext};
use crate::interp::{EmitInputs, EmitResult, InterpSettings};
use crate::precache::PrecacheTables;

/// `MULTIPLAYER_BACKUP` (`netchan.h`): the number of frames the client keeps. Xash3D uses 16
/// in a single-player game; this client only plays multiplayer protocol.
pub const UPDATE_BACKUP: usize = 64;
/// `CL_UPDATE_MASK`.
pub const UPDATE_MASK: u32 = UPDATE_BACKUP as u32 - 1;
/// `cl_clockreset`: the gap between the client clock and the server time that resets it.
pub const CLOCK_RESET: f64 = 0.1;
/// `cl_fixtimerate`: how many milliseconds a frame may move the client clock towards the
/// server time.
pub const FIX_TIME_RATE: f64 = 7.5;

/// One entity of a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameEntity {
    pub state: GoldSrcEntityState,
    /// The entity was not in the frame the update was based on (all of them in a full
    /// update): its interpolation starts over (`SETVISBIT( frame->flags, pack )`).
    pub is_new: bool,
}

/// `frame_t`: what one server packet carried.
#[derive(Debug, Clone, Default)]
pub struct Frame {
    /// The netchan sequence of the packet; 0 for a slot never used.
    pub sequence: u32,
    /// The frame holds a usable entity set (`frame->valid`).
    pub valid: bool,
    /// `cl.mtime[0]` when the packet was parsed (`frame->time`).
    pub time: f64,
    /// The client's clock when it was parsed (`frame->receivedtime`).
    pub received_time: f64,
    /// The entities, by entity number.
    pub entities: Vec<FrameEntity>,
    pub clientdata: Option<GoldSrcClientData>,
    pub weapon_data: BTreeMap<u8, GoldSrcWeaponData>,
}

impl Frame {
    fn empty(sequence: u32) -> Self {
        Self {
            sequence,
            ..Self::default()
        }
    }

    pub fn entity(&self, number: u16) -> Option<&FrameEntity> {
        self.entities
            .binary_search_by_key(&number, |e| e.state.number)
            .ok()
            .map(|i| &self.entities[i])
    }

    pub fn contains(&self, number: u16) -> bool {
        self.entity(number).is_some()
    }
}

/// Why a delta update was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaRefusal {
    /// The update is based on the frame it arrived in or on one that is `UPDATE_MASK` or more
    /// packets old (`CL_ValidateDeltaPacket`: "update too old", "delta frame is too old").
    TooOld,
    /// The update is based on a frame the client does not hold: never received, lost, or
    /// itself refused. Xash3D does not check this; the original engine's "Delta from invalid
    /// frame" is the same test (assumption: remembered, not read).
    UnknownFrame,
}

/// What [`ClientFrames::handle`] did with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Not a message the frames care about, or one that changes no frame.
    Ignored,
    /// An entity update became the current frame.
    Applied,
    /// An entity update was read and thrown away. The caller must stop acknowledging frames
    /// (`cl.validsequence = 0`, so no `clc_delta` goes out) and the server then sends a full
    /// update.
    Refused(DeltaRefusal),
}

/// `CL_ValidateDeltaPacket` for the ring `frames`: may an update that arrived in packet
/// `incoming_sequence` be based on packet `old_packet` (the low byte of its sequence)?
pub fn validate_delta_packet(
    frames: &FrameRing,
    incoming_sequence: u32,
    old_packet: u8,
) -> Result<(), DeltaRefusal> {
    let subtracted = incoming_sequence.wrapping_sub(u32::from(old_packet)) & 0xFF;
    if subtracted == 0 || subtracted >= UPDATE_MASK {
        return Err(DeltaRefusal::TooOld);
    }
    let old = frames.slot(u32::from(old_packet));
    if !old.valid || (old.sequence & 0xFF) != u32::from(old_packet) {
        return Err(DeltaRefusal::UnknownFrame);
    }
    Ok(())
}

/// `cl.frames`.
#[derive(Debug, Clone)]
pub struct FrameRing {
    frames: Vec<Frame>,
}

impl Default for FrameRing {
    fn default() -> Self {
        Self {
            frames: vec![Frame::default(); UPDATE_BACKUP],
        }
    }
}

impl FrameRing {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The slot `sequence` maps to, whatever it holds.
    fn slot(&self, sequence: u32) -> &Frame {
        &self.frames[(sequence & UPDATE_MASK) as usize]
    }

    /// The frame of packet `sequence`, if the ring still holds it.
    pub fn get(&self, sequence: u32) -> Option<&Frame> {
        let f = self.slot(sequence);
        (f.sequence == sequence && sequence != 0).then_some(f)
    }

    /// The slot of `sequence` for writing; a slot that held another packet starts empty
    /// (`cl.frames[...].valid = false` at the start of every packet).
    fn begin(&mut self, sequence: u32) -> &mut Frame {
        let f = &mut self.frames[(sequence & UPDATE_MASK) as usize];
        if f.sequence != sequence {
            *f = Frame::empty(sequence);
        }
        f
    }
}

/// The client's clock: `cl.time`, `cl.oldtime`, `cl.mtime[]` and `cl.timedelta`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientClock {
    pub time: f64,
    pub oldtime: f64,
    /// The server times of the last two `svc_time` messages, newest first.
    pub mtime: [f64; 2],
    pub timedelta: f64,
}

impl Default for ClientClock {
    fn default() -> Self {
        // "because level starts from 1.0f second" (`CL_ParseServerData`).
        Self {
            time: 0.0,
            oldtime: 0.0,
            mtime: [1.0, 1.0],
            timedelta: 0.0,
        }
    }
}

impl ClientClock {
    /// `CL_ParseServerTime`.
    pub fn on_server_time(&mut self, server_time: f32, max_clients: u8) {
        self.mtime[1] = self.mtime[0];
        self.mtime[0] = f64::from(server_time);
        if max_clients == 1 {
            self.time = self.mtime[0];
        }
        let dt = self.time - self.mtime[0];
        if dt.abs() > CLOCK_RESET {
            self.time = self.mtime[0];
            self.timedelta = 0.0;
        } else if dt != 0.0 {
            self.timedelta = dt;
        }
        if self.oldtime > self.time {
            self.oldtime = self.time;
        }
    }

    /// `CL_ReadPackets`: the clock runs by the frame time, unless the game is paused.
    pub fn advance(&mut self, frametime: f64, paused: bool) {
        self.oldtime = self.time;
        if !paused {
            self.time += frametime;
        }
    }

    /// `CL_AdjustClock`: moves the clock a little towards the server's, so lag does not
    /// stretch it.
    pub fn adjust(&mut self) {
        if self.timedelta == 0.0 {
            return;
        }
        if self.timedelta.abs() >= 0.001 {
            let msec = self.timedelta * 1000.0;
            let sign = if msec < 0.0 { 1.0 } else { -1.0 };
            let msec = FIX_TIME_RATE.min(msec.abs());
            let adjust = sign * (msec / 1000.0);
            if adjust.abs() < self.timedelta.abs() {
                self.timedelta += adjust;
                self.time += adjust;
            }
            if self.oldtime > self.time {
                self.oldtime = self.time;
            }
        }
    }
}

/// Everything the client keeps between server packets for the world it sees.
#[derive(Debug, Clone)]
pub struct ClientFrames {
    pub ring: FrameRing,
    pub clock: ClientClock,
    pub entities: EntityStore,
    pub settings: InterpSettings,
    /// `cl.validsequence`: the sequence of the last good entity update, 0 when the client
    /// cannot render a frame and must not acknowledge one.
    pub valid_sequence: u32,
    /// `cl.parsecount`: the sequence of the newest packet any message came from.
    pub parsecount: u32,
    /// `cl.paused` (`svc_setpause`).
    pub paused: bool,
}

impl Default for ClientFrames {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientFrames {
    pub fn new() -> Self {
        Self {
            ring: FrameRing::default(),
            clock: ClientClock::default(),
            entities: EntityStore::default(),
            settings: InterpSettings::default(),
            valid_sequence: 0,
            parsecount: 0,
            paused: false,
        }
    }

    /// Forgets the map: frames, entities and the clock (the settings stay).
    pub fn reset(&mut self) {
        let settings = self.settings.clone();
        *self = Self::new();
        self.settings = settings;
    }

    /// Reads `ex_interp`, `cl_nointerp` and `cl_updaterate`.
    pub fn apply_cvars(&mut self, cvars: &ClientCvars) {
        self.settings.apply_cvars(cvars);
    }

    /// The frame of the newest packet (`cl.frames[cl.parsecountmod]`), if it is usable.
    pub fn current_frame(&self) -> Option<&Frame> {
        self.ring.get(self.parsecount).filter(|f| f.valid)
    }

    /// The client's frame step: advances the clock by `frametime` seconds and eases it
    /// towards the server's (`CL_ReadPackets`, `CL_AdjustClock`).
    pub fn advance(&mut self, frametime: f64) {
        self.clock.advance(frametime, self.paused);
        self.clock.adjust();
    }

    /// Takes one parsed server message of the packet with the netchan sequence
    /// `incoming_sequence`; `now` is the caller's clock (`host.realtime`).
    pub fn handle(
        &mut self,
        msg: &SvcMessage,
        ctx: &SvcParseContext,
        models: &PrecacheTables,
        incoming_sequence: u32,
        now: f64,
    ) -> FrameOutcome {
        // Every packet starts a frame (`cl.parsecount = incoming_sequence`).
        self.parsecount = incoming_sequence;
        {
            let frame = self.ring.begin(incoming_sequence);
            if frame.received_time == 0.0 {
                frame.received_time = now;
            }
            frame.time = self.clock.mtime[0];
        }

        match msg {
            SvcMessage::ServerInfo(info) => {
                self.reset();
                self.parsecount = incoming_sequence;
                self.entities.max_clients = info.max_clients;
                self.entities.playernum = info.player_slot;
                FrameOutcome::Ignored
            }
            SvcMessage::Time(t) => {
                self.clock.on_server_time(*t, self.entities.max_clients);
                self.ring.begin(incoming_sequence).time = self.clock.mtime[0];
                FrameOutcome::Ignored
            }
            SvcMessage::SetPause(paused) => {
                self.paused = *paused;
                FrameOutcome::Ignored
            }
            SvcMessage::NewMoveVars(mv) => {
                self.entities.max_velocity = mv.maxvelocity;
                FrameOutcome::Ignored
            }
            SvcMessage::SpawnBaseline { baselines, .. } => {
                for (&number, (ent_type, rec)) in baselines {
                    let name = entity_schema_name(number, *ent_type, ctx.max_clients);
                    if let Some(schema) = ctx.deltas.get(name) {
                        self.entities.set_baseline(GoldSrcEntityState::from_delta(
                            number, *ent_type, rec, schema,
                        ));
                    }
                }
                FrameOutcome::Ignored
            }
            SvcMessage::ClientData { parsed, .. } => {
                let weapon_data = ctx
                    .deltas
                    .get("weapon_data_t")
                    .map(|schema| {
                        ctx.weapondata_history
                            .get(&ctx.last_incoming_seq)
                            .into_iter()
                            .flatten()
                            .map(|(&i, rec)| (i, GoldSrcWeaponData::from_delta(rec, schema)))
                            .collect()
                    })
                    .unwrap_or_default();
                let frame = self.ring.begin(incoming_sequence);
                frame.clientdata = Some(parsed.clone());
                frame.weapon_data = weapon_data;
                FrameOutcome::Ignored
            }
            SvcMessage::PacketEntities(update) => {
                self.on_packet_entities(update, ctx, models, incoming_sequence)
            }
            _ => FrameOutcome::Ignored,
        }
    }

    /// `CL_ParsePacketEntitiesGS` after the wire decoding.
    fn on_packet_entities(
        &mut self,
        update: &PacketEntitiesMsg,
        ctx: &SvcParseContext,
        models: &PrecacheTables,
        incoming_sequence: u32,
    ) -> FrameOutcome {
        // `delta` without a base frame number cannot be told from a corrupt message.
        let old_packet = if update.is_delta {
            match update.delta_from_seq {
                Some(seq) => Some(seq),
                None => return self.refuse(incoming_sequence, DeltaRefusal::UnknownFrame),
            }
        } else {
            None
        };
        if let Some(old_packet) = old_packet
            && let Err(why) = validate_delta_packet(&self.ring, incoming_sequence, old_packet)
        {
            return self.refuse(incoming_sequence, why);
        }

        // Which entities were in the frame this update is based on: those that are not are
        // new (the first update has no base, so all of them are).
        let old = old_packet.and_then(|seq| self.ring.get(u32::from(seq)));
        let mut entities = Vec::with_capacity(update.entities.len());
        for (&number, (ent_type, rec)) in &update.entities {
            let name = entity_schema_name(number, *ent_type, ctx.max_clients);
            let Some(schema) = ctx.deltas.get(name) else {
                continue;
            };
            entities.push(FrameEntity {
                state: GoldSrcEntityState::from_delta(number, *ent_type, rec, schema),
                is_new: old.is_none_or(|f| !f.contains(number)),
            });
        }

        let frame = self.ring.begin(incoming_sequence);
        frame.entities = entities;
        frame.valid = true;
        frame.time = self.clock.mtime[0];
        // "mark current delta state"
        self.valid_sequence = incoming_sequence;

        let cx = ProcessContext {
            parsecount: incoming_sequence,
            mtime: self.clock.mtime[0],
            time: self.clock.time,
            models,
            settings: &self.settings,
        };
        if let Some(frame) = self.ring.get(incoming_sequence) {
            self.entities.process_packet(frame, &cx);
        }
        FrameOutcome::Applied
    }

    /// `CL_FlushEntityPacketGS`: the update is dropped, the frame is invalid, and
    /// `cl.validsequence = 0` ("can't render a frame").
    fn refuse(&mut self, incoming_sequence: u32, why: DeltaRefusal) -> FrameOutcome {
        let frame = self.ring.begin(incoming_sequence);
        frame.valid = false;
        frame.entities.clear();
        self.valid_sequence = 0;
        FrameOutcome::Refused(why)
    }

    /// `CL_EmitEntities`: interpolates the newest frame and lists what the renderer draws.
    /// Empty while paused, before the first good update or when the newest packet had no
    /// entity update.
    pub fn emit(&mut self, inputs: &EmitInputs) -> EmitResult {
        if self.paused || self.valid_sequence == 0 {
            return EmitResult::default();
        }
        let Some(frame) = self.ring.get(self.parsecount).filter(|f| f.valid) else {
            return EmitResult::default();
        };
        crate::interp::emit_entities(
            &mut self.entities,
            frame,
            &self.clock,
            &self.settings,
            inputs,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_time_resets_a_clock_that_ran_off() {
        let mut c = ClientClock::default();
        c.on_server_time(10.0, 8);
        assert_eq!(c.time, 10.0);
        assert_eq!(c.timedelta, 0.0);
        // Small gap: the clock stays, the difference is remembered.
        c.time = 10.05;
        c.on_server_time(10.01, 8);
        assert_eq!(c.time, 10.05);
        assert!((c.timedelta - 0.04).abs() < 1e-6);
        // Large gap: reset.
        c.time = 12.0;
        c.on_server_time(10.1, 8);
        assert_eq!(c.time, f64::from(10.1f32));
        assert_eq!(c.timedelta, 0.0);
    }

    #[test]
    fn the_clock_is_eased_towards_the_server() {
        let mut c = ClientClock {
            time: 10.05,
            mtime: [10.0, 9.9],
            timedelta: 0.05,
            ..ClientClock::default()
        };
        c.adjust();
        // Moves back by at most 7.5 ms.
        assert!((c.time - 10.0425).abs() < 1e-9);
        assert!((c.timedelta - 0.0425).abs() < 1e-9);
    }

    #[test]
    fn a_paused_clock_stands_still() {
        let mut c = ClientClock {
            time: 5.0,
            ..ClientClock::default()
        };
        c.advance(0.01, true);
        assert_eq!(c.time, 5.0);
        c.advance(0.01, false);
        assert!((c.time - 5.01).abs() < 1e-12);
    }
}
