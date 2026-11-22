//! `svc_event` and `svc_event_reliable`: the `event_args_t` the game hands to
//! `PLAYBACK_EVENT_FULL` as the wire carries it (schema `event_t` of `delta.lst`), and the
//! writers of both messages.
//!
//! Sources, all read 2026-11-22: ReHLDS `rehlds/engine/sv_main.cpp` `SV_EmitEvents_internal`
//! (the `svc_event` layout) and `rehlds/engine/pr_cmds.cpp` `EV_PlayReliableEvent_internal`
//! (the `svc_event_reliable` layout).

use glam::Vec3;

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::delta::{DeltaRecord, DeltaSchema, read_delta, write_delta};
use crate::messages::svc;

/// `FEVENT_ORIGIN` (`rehlds/common/event_args.h`, read 2026-11-22): `event_args_t::origin` was
/// given by the game and is sent.
pub const FEVENT_ORIGIN: i32 = 1 << 0;
/// `FEVENT_ANGLES` (same file): `event_args_t::angles` was given by the game and is sent.
pub const FEVENT_ANGLES: i32 = 1 << 1;

/// Events one `svc_event` carries at most (5-bit count, capped at 31 by `SV_EmitEvents`).
pub const MAX_EVENTS_PER_MESSAGE: usize = 31;

/// `event_args_t` (`rehlds/common/event_args.h`). Every field is a C `int` or `float` of the
/// original. `flags` and the unused `velocity` are not on the wire: the decoder gives `flags`
/// 0, and [`EventArgs::default`] is the all-zero `nullargs` of `SV_EmitEvents`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EventArgs {
    /// `FEVENT_*` bits; the engine's, not sent.
    pub flags: i32,
    pub entindex: i32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub ducking: i32,
    pub fparam1: f32,
    pub fparam2: f32,
    pub iparam1: i32,
    pub iparam2: i32,
    pub bparam1: i32,
    pub bparam2: i32,
}

impl EventArgs {
    /// The fields as a record of the `event_t` schema. `entindex` is written as is: a negative
    /// value is clamped to the field's maximum by the writer, as `MSG_WriteBits` does.
    pub fn to_delta(&self, schema: &DeltaSchema) -> DeltaRecord {
        let mut rec = schema.zero_record();
        rec.set_i32(schema, "entindex", self.entindex);
        rec.set_i32(schema, "bparam1", self.bparam1);
        rec.set_i32(schema, "bparam2", self.bparam2);
        for (i, name) in ["origin[0]", "origin[1]", "origin[2]"].into_iter().enumerate() {
            rec.set_f32(schema, name, self.origin[i]);
        }
        rec.set_f32(schema, "fparam1", self.fparam1);
        rec.set_f32(schema, "fparam2", self.fparam2);
        rec.set_i32(schema, "iparam1", self.iparam1);
        rec.set_i32(schema, "iparam2", self.iparam2);
        for (i, name) in ["angles[0]", "angles[1]", "angles[2]"].into_iter().enumerate() {
            rec.set_f32(schema, name, self.angles[i]);
        }
        rec.set_i32(schema, "ducking", self.ducking);
        rec
    }

    /// The fields of a record of the `event_t` schema (`flags` is 0).
    pub fn from_delta(rec: &DeltaRecord, schema: &DeltaSchema) -> Self {
        let f = |name: &str| rec.get_f32(schema, name);
        let i = |name: &str| rec.get_i32(schema, name);
        Self {
            flags: 0,
            entindex: i("entindex"),
            origin: Vec3::new(f("origin[0]"), f("origin[1]"), f("origin[2]")),
            angles: Vec3::new(f("angles[0]"), f("angles[1]"), f("angles[2]")),
            ducking: i("ducking"),
            fparam1: f("fparam1"),
            fparam2: f("fparam2"),
            iparam1: i("iparam1"),
            iparam2: i("iparam2"),
            bparam1: i("bparam1"),
            bparam2: i("bparam2"),
        }
    }
}

/// One event of a `svc_event`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WireEvent {
    /// Index in the event resource list (10 bits).
    pub index: u16,
    /// Position of the invoker in the packet entities of the same update (11 bits); `None`
    /// for an event without one.
    pub packet_index: Option<u16>,
    /// The arguments, `None` when they equal `nullargs` (the client then uses zeros). Only
    /// sent together with a `packet_index`.
    pub args: Option<EventArgs>,
    /// Delay in hundredths of a second (16 bits); `None` for no delay.
    pub fire_time: Option<u16>,
}

/// `svc_event` as `SV_EmitEvents_internal` writes it: the opcode, then in a bit block the
/// count (5 bits, at most [`MAX_EVENTS_PER_MESSAGE`]; further events are not written), and per
/// event: index (10), a bit and the packet index (11) with a bit and the delta of the arguments
/// against `nullargs`, a bit and the delay (16). Nothing is written for an empty list.
pub fn write_svc_event(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    events: &[WireEvent],
    server_time: f64,
) {
    if events.is_empty() {
        return;
    }
    let events = &events[..events.len().min(MAX_EVENTS_PER_MESSAGE)];
    let zero = schema.zero_record();
    w.write_u8(svc::EVENT);
    w.start_bit_writing();
    w.write_bits(events.len() as u32, 5);
    for ev in events {
        w.write_bits(u32::from(ev.index), 10);
        match ev.packet_index {
            Some(packet) => {
                w.write_one_bit(true);
                w.write_bits(u32::from(packet), 11);
                match &ev.args {
                    Some(args) => {
                        w.write_one_bit(true);
                        write_delta(w, schema, &zero, &args.to_delta(schema), true, server_time);
                    }
                    None => w.write_one_bit(false),
                }
            }
            None => w.write_one_bit(false),
        }
        write_fire_time(w, ev.fire_time);
    }
    w.end_bit_writing();
}

/// `svc_event_reliable` as `EV_PlayReliableEvent_internal` writes it: the opcode, then in a bit
/// block the index (10), the delta of the arguments against zeros (always, forced), a bit and
/// the delay (16).
pub fn write_svc_event_reliable(
    w: &mut MsgWriter,
    schema: &DeltaSchema,
    index: u16,
    args: &EventArgs,
    fire_time: Option<u16>,
    server_time: f64,
) {
    w.write_u8(svc::EVENT_RELIABLE);
    w.start_bit_writing();
    w.write_bits(u32::from(index), 10);
    write_delta(
        w,
        schema,
        &schema.zero_record(),
        &args.to_delta(schema),
        true,
        server_time,
    );
    write_fire_time(w, fire_time);
    w.end_bit_writing();
}

fn write_fire_time(w: &mut MsgWriter, fire_time: Option<u16>) {
    match fire_time {
        Some(t) => {
            w.write_one_bit(true);
            w.write_bits(u32::from(t), 16);
        }
        None => w.write_one_bit(false),
    }
}

/// Reads the body of a `svc_event` (after the opcode) with an active bit reader. Without the
/// `event_t` schema an event without arguments still reads; one with a delta is an error
/// (the stream could not stay aligned).
pub(crate) fn read_svc_event(
    r: &mut MsgReader<'_>,
    schema: Option<&DeltaSchema>,
    server_time: f64,
) -> Result<Vec<WireEvent>, ProtoError> {
    let zero = schema.map(DeltaSchema::zero_record);
    let count = r.read_bits(5);
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let index = r.read_bits(10) as u16;
        let mut packet_index = None;
        let mut args = None;
        if r.read_one_bit() {
            packet_index = Some(r.read_bits(11) as u16);
            if r.read_one_bit() {
                let (Some(schema), Some(zero)) = (schema, zero.as_ref()) else {
                    return Err(ProtoError::Delta("missing event_t".to_string()));
                };
                let rec = read_delta(r, schema, zero, server_time)?;
                args = Some(EventArgs::from_delta(&rec, schema));
            }
        }
        let fire_time = r.read_one_bit().then(|| r.read_bits(16) as u16);
        if r.is_bad_read() {
            return Err(ProtoError::UnexpectedEof);
        }
        out.push(WireEvent {
            index,
            packet_index,
            args,
            fire_time,
        });
    }
    Ok(out)
}

/// Reads the body of a `svc_event_reliable` (after the opcode) with an active bit reader:
/// the index, the arguments (zeros without the schema) and the delay in hundredths of a
/// second.
pub(crate) fn read_svc_event_reliable(
    r: &mut MsgReader<'_>,
    schema: Option<&DeltaSchema>,
    server_time: f64,
) -> Result<(u16, EventArgs, Option<u16>), ProtoError> {
    let index = r.read_bits(10) as u16;
    let args = match schema {
        Some(schema) => {
            let rec = read_delta(r, schema, &schema.zero_record(), server_time)?;
            EventArgs::from_delta(&rec, schema)
        }
        None => EventArgs::default(),
    };
    let fire_time = r.read_one_bit().then(|| r.read_bits(16) as u16);
    if r.is_bad_read() {
        return Err(ProtoError::UnexpectedEof);
    }
    Ok((index, args, fire_time))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::DeltaRegistry;
    use crate::messages::{SvcMessage, SvcParseContext, parse_svc_stream};

    fn args() -> EventArgs {
        EventArgs {
            flags: FEVENT_ORIGIN,
            entindex: 5,
            origin: Vec3::new(1.5, -2.25, 3.0),
            angles: Vec3::new(0.0, 90.0, -45.0),
            ducking: 1,
            fparam1: 0.5,
            fparam2: -1.25,
            iparam1: -7,
            iparam2: 100_000,
            bparam1: 1,
            bparam2: 0,
        }
    }

    #[test]
    fn svc_event_round_trips() {
        let reg = DeltaRegistry::cs16_default();
        let schema = reg.get("event_t").unwrap();
        let events = [
            WireEvent {
                index: 3,
                packet_index: Some(2),
                args: Some(args()),
                fire_time: Some(25),
            },
            WireEvent {
                index: 255,
                packet_index: Some(300),
                args: None,
                fire_time: None,
            },
        ];
        let mut w = MsgWriter::new();
        write_svc_event(&mut w, schema, &events, 1.0);
        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        let [SvcMessage::Event(got)] = parsed.as_slice() else {
            panic!("{parsed:?}");
        };
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].index, got[0].packet_index, got[0].fire_time),
            (3, Some(2), Some(25))
        );
        // `flags` is not on the wire.
        assert_eq!(got[0].args, Some(EventArgs { flags: 0, ..args() }));
        assert_eq!(got[1], events[1]);
    }

    #[test]
    fn svc_event_reliable_round_trips_and_no_events_write_nothing() {
        let reg = DeltaRegistry::cs16_default();
        let schema = reg.get("event_t").unwrap();
        let mut w = MsgWriter::new();
        write_svc_event(&mut w, schema, &[], 1.0);
        assert!(w.is_empty());
        write_svc_event_reliable(&mut w, schema, 9, &args(), Some(1), 1.0);
        let mut ctx = SvcParseContext::new();
        let parsed = parse_svc_stream(w.as_slice(), &mut ctx).unwrap();
        assert_eq!(
            parsed,
            vec![SvcMessage::EventReliable {
                index: 9,
                args: EventArgs { flags: 0, ..args() },
                fire_time: Some(1),
            }]
        );
    }
}
