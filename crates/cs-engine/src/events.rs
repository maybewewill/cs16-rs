//! Events: the engine side of `PRECACHE_EVENT` and `PLAYBACK_EVENT_FULL`. The game precaches
//! the event scripts (`events/ak47.sc`) while the map loads and plays one by index; the engine
//! queues it per client, and the next update of that client carries it in a `svc_event` after
//! the packet entities (or, for `FEV_RELIABLE`, in a `svc_event_reliable` on the reliable
//! stream at once). The client plays the script (muzzle flash, shell, sound) from the arguments.
//!
//! Sources, all read 2026-11-22: ReHLDS `rehlds/engine/pr_cmds.cpp` (`EV_Precache_internal`,
//! `EV_PlayReliableEvent_internal`, `EV_Playback`, `EV_SV_Playback`) and
//! `rehlds/engine/sv_main.cpp` (`SV_EmitEvents_internal`, `SV_ValidClientMulticast`).
//!
//! Differences from the source:
//! - The `groupinfo` filter of `EV_Playback` is not modelled (the engine has no `groupinfo`).
//! - No fake clients exist in this engine, so the `fakeclient` skips never apply.
//! - `host_client`, which `FEV_NOTHOST` compares with, is modelled as the client of the
//!   invoker (an assumption): the game plays weapon events from the shooter's own command.
//! - `EV_Precache_internal` loads the event script and stops the server when it is missing;
//!   this engine does not look at the file.
//! - A reliable event for a client whose reliable backlog is past `MAX_RELIABLE_BACKLOG` is
//!   dropped (ReHLDS fragments it); a server without the `event_t` schema sends none.
//! - A negative `delay` is refused (the source's `(int)` and `unsigned` conversions of it are
//!   undefined or wrap).
//! - `MAX_EVENT_QUEUE` (64) is from memory of the SDK's `event_state_t` (assumption); the
//!   message carries at most 31 events anyway.

use cs_proto48::event::{EventArgs, FEVENT_ANGLES, FEVENT_ORIGIN, WireEvent, write_svc_event_reliable};
use glam::Vec3;

use crate::client::ClientState;
use crate::edict::EdictId;
use crate::game::EngineApi;
use crate::precache::MAX_EVENTS;

/// `FEV_*` of `playback_event` (`rehlds/common/event_flags.h`).
pub mod event_flags {
    /// Skip the invoker's own client when it predicts events (`cl_lw 1`).
    pub const FEV_NOTHOST: i32 = 1;
    /// Send the event reliably (`svc_event_reliable`, at once).
    pub const FEV_RELIABLE: i32 = 2;
    /// Send to every client, not only those that can hear the invoker.
    pub const FEV_GLOBAL: i32 = 4;
    /// Replace the invoker's queued event of the same index instead of adding one.
    pub const FEV_UPDATE: i32 = 8;
    /// Send to the invoker's client only.
    pub const FEV_HOSTONLY: i32 = 16;
    /// Server-only event: sent as any other.
    pub const FEV_SERVER: i32 = 32;
    /// Client-only event: the server does not send it.
    pub const FEV_CLIENT: i32 = 64;
}
use event_flags::*;

/// Events one client may have queued (`MAX_EVENT_QUEUE`); further ones are dropped.
pub const MAX_EVENT_QUEUE: usize = 64;
/// `FL_DUCKING` in `EntVars::flags`.
const FL_DUCKING: u32 = 1 << 14;

/// One queued event (`event_info_t`); `index` 0 marks a free entry.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EventInfo {
    pub(crate) index: u16,
    pub(crate) packet_index: i32,
    /// The invoker's entity number, or -1.
    pub(crate) entity_index: i32,
    pub(crate) args: EventArgs,
    /// The delay in seconds.
    pub(crate) fire_time: f32,
}

impl EventInfo {
    fn set(&mut self, index: u16, invoker: i32, args: EventArgs, delay: f32) {
        self.entity_index = invoker;
        self.index = index;
        self.packet_index = -1;
        self.args = args;
        self.fire_time = delay;
    }
}

/// The events queued for one client (`event_state_t`).
#[derive(Clone, Debug)]
pub(crate) struct EventQueue {
    pub(crate) ei: [EventInfo; MAX_EVENT_QUEUE],
}

impl Default for EventQueue {
    fn default() -> Self {
        Self {
            ei: [EventInfo::default(); MAX_EVENT_QUEUE],
        }
    }
}

impl EventQueue {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// `SV_EmitEvents_internal` without the writing (`rehlds/engine/sv_main.cpp`, read
    /// 2026-11-22): empties the queue and returns what the update carries, in queue order, the
    /// first 31 only (the rest are discarded unsent). `entities` are the numbers of the packet
    /// entities of the update, in order.
    ///
    /// Quirks kept: an invoker found among the packet entities has `ducking` cleared (the
    /// client derives it from the entity) and origin and angles zeroed unless the game gave
    /// them; one not found keeps its arguments and gets the packet index one past the list and
    /// `entindex` set to its number (-1 for no invoker). The arguments are sent as a delta
    /// only when they differ from all zeros, `flags` included.
    pub(crate) fn take_wire_events(&mut self, entities: &[u16]) -> Vec<WireEvent> {
        let mut ev_count = self.ei.iter().filter(|e| e.index != 0).count().min(31);
        if ev_count == 0 {
            return Vec::new();
        }
        for info in self.ei.iter_mut().filter(|e| e.index != 0) {
            let etofind = info.entity_index;
            let found = entities
                .iter()
                .position(|&number| i32::from(number) == etofind);
            match found {
                Some(i) => {
                    info.packet_index = i as i32;
                    info.args.ducking = 0;
                    if info.args.flags & FEVENT_ORIGIN == 0 {
                        info.args.origin = Vec3::ZERO;
                    }
                    if info.args.flags & FEVENT_ANGLES == 0 {
                        info.args.angles = Vec3::ZERO;
                    }
                }
                None => {
                    info.args.entindex = etofind;
                    info.packet_index = entities.len() as i32;
                }
            }
        }
        let mut out = Vec::with_capacity(ev_count);
        for info in &mut self.ei {
            if info.index == 0 {
                info.packet_index = -1;
                info.entity_index = -1;
                continue;
            }
            if ev_count > 0 {
                ev_count -= 1;
                out.push(WireEvent {
                    index: info.index,
                    // Lossless: at most 256 packet entities plus one.
                    packet_index: Some(info.packet_index as u16),
                    args: (info.args != EventArgs::default()).then_some(info.args),
                    fire_time: (info.fire_time != 0.0).then(|| fire_time_wire(info.fire_time)),
                });
            }
            info.index = 0;
            info.packet_index = -1;
            info.entity_index = -1;
        }
        out
    }
}

/// `MSG_WriteBits(fire_time * 100.0f, 16)`: truncated, held to 16 bits by the writer.
fn fire_time_wire(seconds: f32) -> u16 {
    (seconds * 100.0).clamp(0.0, f32::from(u16::MAX)) as u16
}

/// `(int)(delay * 100.0f)` of `EV_PlayReliableEvent_internal`, written with `MSG_WriteBits(.., 16)`:
/// a negative product becomes a huge unsigned value, which the writer holds to 65535.
fn reliable_delay_wire(delay: f32) -> u16 {
    let hundredths = (delay * 100.0) as i32;
    u32::try_from(hundredths).map_or(u16::MAX, |v| v.min(u32::from(u16::MAX)) as u16)
}

impl EngineApi<'_> {
    /// `PF_precache_event` / `EV_Precache_internal` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read
    /// 2026-11-22): the index of the event script `name` (`events/ak47.sc`), 1-based, in the
    /// event resource list the client is sent at sign-on. A name already there gives its
    /// index. 0, nothing added, when the name is empty or `server_activate` has passed and the
    /// name is new (the source stops the server with an error), or when all 255 indices are
    /// taken.
    pub fn precache_event(&mut self, name: &str) -> u16 {
        match self.precache.precache_event(name) {
            Ok(index) => index,
            Err(reason) => {
                tracing::warn!(name, ?reason, "precache_event refused");
                0
            }
        }
    }

    /// `PF_playbackevent` / `EV_Playback` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read
    /// 2026-11-22): queues event `event_index` for the clients that should see it. See the
    /// module documentation for what differs from the source.
    ///
    /// - `FEV_CLIENT` events are not sent. An index outside `1..256` or without a precached
    ///   event, a stale `invoker` and non-finite numbers are refused quietly.
    /// - A zero `origin` or `angles` means "not given": the invoker's own are used (which, for
    ///   an invoker, the client derives from the entity, so they are not sent). `ducking` is
    ///   set from `FL_DUCKING` for a client invoker.
    /// - Audience: every `Active` client. With an invoker and no `FEV_GLOBAL`, those whose
    ///   edict origin is in the PAS of the invoker's origin (`SV_ValidClientMulticast`, 4);
    ///   without an invoker everyone. `FEV_NOTHOST` leaves out the invoker's own client when
    ///   its `cl_lw` is set (that client plays the event itself); `FEV_HOSTONLY` keeps only
    ///   that client.
    /// - `FEV_RELIABLE`: written to the client's reliable stream at once. Otherwise the event
    ///   joins the client's queue (64 entries, a full queue drops it) with `delay` seconds,
    ///   to be sent in its next update; with `FEV_UPDATE` it replaces the invoker's queued
    ///   event of the same index.
    #[allow(clippy::too_many_arguments)]
    pub fn playback_event(
        &mut self,
        flags: i32,
        invoker: Option<EdictId>,
        event_index: u16,
        delay: f32,
        origin: Vec3,
        angles: Vec3,
        fparam1: f32,
        fparam2: f32,
        iparam1: i32,
        iparam2: i32,
        bparam1: bool,
        bparam2: bool,
    ) {
        if flags & FEV_CLIENT != 0 {
            return;
        }
        if !origin.is_finite()
            || !angles.is_finite()
            || !fparam1.is_finite()
            || !fparam2.is_finite()
            || !delay.is_finite()
            || delay < 0.0
        {
            return;
        }
        let mut eargs = EventArgs::default();
        if origin != Vec3::ZERO {
            eargs.origin = origin;
            eargs.flags |= FEVENT_ORIGIN;
        }
        if angles != Vec3::ZERO {
            eargs.angles = angles;
            eargs.flags |= FEVENT_ANGLES;
        }
        eargs.fparam1 = fparam1;
        eargs.fparam2 = fparam2;
        eargs.iparam1 = iparam1;
        eargs.iparam2 = iparam2;
        eargs.bparam1 = i32::from(bparam1);
        eargs.bparam2 = i32::from(bparam2);

        if event_index < 1
            || usize::from(event_index) >= MAX_EVENTS
            || !self.precache.event_exists(event_index)
        {
            return;
        }

        let max_clients = self.clients.len();
        let mut invoker_num = -1;
        let event_origin;
        if let Some(id) = invoker {
            let Some(vars) = self.edicts.get(id).map(|e| &e.vars) else {
                return;
            };
            event_origin = vars.origin;
            invoker_num = i32::from(id.index);
            if (1..=max_clients).contains(&usize::from(id.index)) && vars.flags & FL_DUCKING != 0 {
                eargs.ducking = 1;
            }
            if eargs.flags & FEVENT_ORIGIN == 0 {
                eargs.origin = vars.origin;
            }
            if eargs.flags & FEVENT_ANGLES == 0 {
                eargs.angles = vars.angles;
            }
        } else {
            event_origin = eargs.origin;
        }
        if !event_origin.is_finite() {
            return;
        }
        // The client of the invoker: the `host_client` of `FEV_NOTHOST` (assumption, see above).
        let host_slot = invoker
            .filter(|id| (1..=max_clients).contains(&usize::from(id.index)))
            .map(|id| usize::from(id.index) - 1);

        let row = self.map.pas_row(event_origin);
        for slot in 0..max_clients {
            if self.clients[slot].state != ClientState::Active {
                continue;
            }
            let client_edict = self.edicts.client(slot as u8);
            if invoker.is_some() && flags & FEV_GLOBAL == 0 {
                let Some(listener) = self.edicts.get(client_edict).map(|e| e.vars.origin) else {
                    continue;
                };
                if !row.is_none_or(|row| self.map.pas_row_contains(row, listener)) {
                    continue;
                }
            }
            if flags & FEV_NOTHOST != 0
                && Some(slot) == host_slot
                && crate::api_messages::lw_of(&self.clients[slot])
            {
                continue;
            }
            if flags & FEV_HOSTONLY != 0 && Some(client_edict) != invoker {
                continue;
            }
            if flags & FEV_RELIABLE != 0 {
                self.send_reliable_event(slot, invoker.map_or(0, |id| i32::from(id.index)), event_index, delay, eargs);
                continue;
            }
            let queue = &mut self.outbox.events[slot];
            if flags & FEV_UPDATE != 0
                && let Some(ei) = queue
                    .ei
                    .iter_mut()
                    .find(|ei| ei.index == event_index && invoker.is_some() && ei.entity_index == invoker_num)
            {
                ei.set(event_index, invoker_num, eargs, delay);
                continue;
            }
            if let Some(ei) = queue.ei.iter_mut().find(|ei| ei.index == 0) {
                ei.set(event_index, invoker_num, eargs, delay);
            }
        }
    }

    /// `EV_PlayReliableEvent_internal`: `svc_event_reliable` to the reliable stream of `slot`.
    fn send_reliable_event(
        &mut self,
        slot: usize,
        entindex: i32,
        event_index: u16,
        delay: f32,
        mut args: EventArgs,
    ) {
        let Some(schema) = self.outbox.event_schema.as_ref() else {
            return;
        };
        args.entindex = entindex;
        let mut w = cs_proto48::bitbuf::MsgWriter::new();
        write_svc_event_reliable(
            &mut w,
            schema,
            event_index,
            &args,
            (delay != 0.0).then(|| reliable_delay_wire(delay)),
            self.sv_time,
        );
        self.outbox.push_reliable(slot, w.as_slice());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(index: u16, entity: i32) -> EventInfo {
        EventInfo {
            index,
            packet_index: -1,
            entity_index: entity,
            args: EventArgs {
                flags: 0,
                ducking: 1,
                origin: Vec3::new(1.0, 2.0, 3.0),
                angles: Vec3::new(4.0, 5.0, 6.0),
                fparam1: 0.5,
                ..EventArgs::default()
            },
            fire_time: 0.0,
        }
    }

    #[test]
    fn invoker_in_the_packet_loses_ducking_origin_and_angles() {
        let mut q = EventQueue::default();
        q.ei[0] = queued(7, 3);
        let events = q.take_wire_events(&[1, 3, 9]);
        assert_eq!(events.len(), 1);
        let ev = events[0];
        assert_eq!((ev.index, ev.packet_index, ev.fire_time), (7, Some(1), None));
        let args = ev.args.unwrap();
        assert_eq!(args.ducking, 0);
        assert_eq!((args.origin, args.angles), (Vec3::ZERO, Vec3::ZERO));
        assert_eq!(args.fparam1, 0.5);
        assert!(q.ei.iter().all(|e| e.index == 0));
    }

    #[test]
    fn invoker_outside_the_packet_keeps_its_arguments() {
        let mut q = EventQueue::default();
        q.ei[2] = queued(7, 40);
        let ev = q.take_wire_events(&[1, 3])[0];
        assert_eq!(ev.packet_index, Some(2));
        let args = ev.args.unwrap();
        assert_eq!((args.entindex, args.ducking), (40, 1));
        assert_eq!(args.origin, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn all_zero_arguments_send_no_delta_and_at_most_31_events_go() {
        let mut q = EventQueue::default();
        for (i, e) in q.ei.iter_mut().enumerate().take(40) {
            *e = EventInfo {
                index: 1 + i as u16,
                packet_index: -1,
                entity_index: -1,
                args: EventArgs::default(),
                fire_time: 0.25,
            };
        }
        let events = q.take_wire_events(&[]);
        assert_eq!(events.len(), 31);
        assert_eq!(events[0].index, 1);
        // No invoker: `entindex` -1 differs from zero, so the delta is sent.
        assert_eq!(events[0].args.map(|a| a.entindex), Some(-1));
        assert_eq!(events[0].fire_time, Some(25));
        assert!(q.ei.iter().all(|e| e.index == 0));
    }

    #[test]
    fn delay_conversions_clamp() {
        assert_eq!(fire_time_wire(0.5), 50);
        assert_eq!(fire_time_wire(-1.0), 0);
        assert_eq!(fire_time_wire(1e9), u16::MAX);
        assert_eq!(reliable_delay_wire(0.5), 50);
        assert_eq!(reliable_delay_wire(-1.0), u16::MAX);
    }
}
