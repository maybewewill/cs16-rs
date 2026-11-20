//! The game-facing message calls of [`EngineApi`]: raw sends to a destination, typed helpers
//! for user messages, prints, sounds, temp entities and light styles. Every call refuses
//! quietly on bad game-supplied data (a slot out of range or not in the required state, a stale
//! entity, non-finite numbers, oversized messages): the message is dropped, nothing panics.

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::messages::{
    SND_FL_ATTENUATION, SND_FL_PITCH, SND_FL_VOLUME, SoundMsg, write_svc_centerprint,
    write_svc_lightstyle, write_svc_print, write_svc_sound, write_svc_stufftext,
    write_svc_temp_entity, write_svc_usermsg,
};
use cs_proto48::netchan::MAX_MSGLEN;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::client::ClientState;
use crate::game::EngineApi;
use crate::signon::MAX_LIGHTSTYLES;

/// Bytes the init buffer holds; a message that would take it past this is sent as
/// [`MsgDest::All`] instead.
pub const MAX_INIT_BUFFER: usize = 64 * 1024;
/// Bytes of text `client_print`, `print_all`, `center_print` and `stufftext` send at most.
pub const MAX_MESSAGE_TEXT: usize = 1024;
/// Bytes one `send` may carry: the netchan's `MAX_MSGLEN`, above which queued reliable data
/// goes out as a fragment stream; a longer message is dropped.
const MAX_SEND_BYTES: usize = MAX_MSGLEN;
/// Bytes of a light style pattern.
const MAX_LIGHT_STYLE_BYTES: usize = 64;
/// Highest sound channel (3 bits on the wire).
const MAX_SOUND_CHANNEL: u8 = 7;
/// Highest sound attenuation (the byte on the wire is `attenuation * 64`).
const MAX_ATTENUATION: f32 = 3.98;
/// Volume byte of a sound at full volume, the default on the wire.
const FULL_VOLUME: u8 = 255;
/// Pitch of a sound that is not shifted, the default on the wire.
const NORMAL_PITCH: u8 = 100;

/// Who a message goes to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MsgDest {
    /// The reliable stream of one slot, whatever stage its sign-on is in.
    One(u8),
    /// The unreliable queue of one slot, which must be `Active`.
    OneUnreliable(u8),
    /// The reliable stream of every client that has passed the `new` stage.
    All,
    /// The unreliable queue of every `Active` client.
    Broadcast,
    /// The unreliable queue of every `Active` client whose eye (its edict's `origin +
    /// view_ofs`) can potentially see the box of half-size 1 around the point.
    Pvs(Vec3),
    /// The init buffer: stored, and replayed to every client that enters the server (in its
    /// `spawn` stage, after the light styles). Only while the map is being activated (in
    /// `spawn_entity` and `server_activate`) and while it fits [`MAX_INIT_BUFFER`]; otherwise
    /// the same as `All`.
    Init,
}

/// `text` cut at its first NUL byte and then to [`MAX_MESSAGE_TEXT`] bytes on a character
/// boundary.
fn clip_text(text: &str) -> &str {
    let text = text.split('\0').next().unwrap_or_default();
    if text.len() <= MAX_MESSAGE_TEXT {
        return text;
    }
    let mut end = MAX_MESSAGE_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

impl EngineApi<'_> {
    /// Sends `bytes`, complete `svc_*` messages the game wrote, to `dest`. The engine does not
    /// look inside them. Nothing is sent for empty `bytes`, for more than `MAX_MSGLEN` bytes
    /// at once, or for a destination with no one in it (a slot out of range, free or not in
    /// the stage the destination needs).
    pub fn send(&mut self, dest: MsgDest, bytes: &[u8]) {
        if bytes.is_empty() || bytes.len() > MAX_SEND_BYTES {
            return;
        }
        match dest {
            MsgDest::One(slot) => {
                let slot = usize::from(slot);
                if self.clients.get(slot).is_some_and(|c| c.state != ClientState::Free) {
                    self.outbox.reliable[slot].write_bytes(bytes);
                }
            }
            MsgDest::OneUnreliable(slot) => {
                let slot = usize::from(slot);
                if self.clients.get(slot).is_some_and(|c| c.state == ClientState::Active) {
                    self.outbox.unreliable[slot].write_bytes(bytes);
                }
            }
            MsgDest::All => {
                for (view, out) in self.clients.iter().zip(&mut self.outbox.reliable) {
                    if view.state.is_past_new() {
                        out.write_bytes(bytes);
                    }
                }
            }
            MsgDest::Broadcast => {
                for (view, out) in self.clients.iter().zip(&mut self.outbox.unreliable) {
                    if view.state == ClientState::Active {
                        out.write_bytes(bytes);
                    }
                }
            }
            MsgDest::Pvs(point) => {
                let (mins, maxs) = (point - Vec3::ONE, point + Vec3::ONE);
                for slot in 0..self.clients.len() {
                    if self.clients[slot].state != ClientState::Active {
                        continue;
                    }
                    // Active clients have an edict; one without is not sent to.
                    let eye = self
                        .edicts
                        .get(self.edicts.client(slot as u8))
                        .map(|e| e.vars.origin + e.vars.view_ofs);
                    if eye.is_some_and(|eye| self.can_see(eye, mins, maxs)) {
                        self.outbox.unreliable[slot].write_bytes(bytes);
                    }
                }
            }
            MsgDest::Init => {
                if self.outbox.init_open && self.outbox.init.len() + bytes.len() <= MAX_INIT_BUFFER
                {
                    self.outbox.init.extend_from_slice(bytes);
                } else {
                    self.send(MsgDest::All, bytes);
                }
            }
        }
    }

    /// Encodes `msg` with the id the game registered for its name and sends it to `dest`.
    /// Returns `false`, sending nothing, when the game did not register that name; `true`
    /// otherwise, even if `dest` has no one in it.
    pub fn send_usermsg(&mut self, dest: MsgDest, msg: &CsUserMsg) -> bool {
        let mut w = MsgWriter::new();
        if !write_svc_usermsg(&mut w, self.usermsgs, msg) {
            return false;
        }
        self.send(dest, w.as_slice());
        true
    }

    /// Prints `text` in the console of `slot` (`svc_print`). The text is cut at its first NUL
    /// byte and to [`MAX_MESSAGE_TEXT`] bytes; a slot that is out of range or free gets
    /// nothing.
    pub fn client_print(&mut self, slot: u8, text: &str) {
        let mut w = MsgWriter::new();
        write_svc_print(&mut w, clip_text(text));
        self.send(MsgDest::One(slot), w.as_slice());
    }

    /// Prints `text` in the console of every client past the `new` stage; cut like
    /// [`client_print`](Self::client_print).
    pub fn print_all(&mut self, text: &str) {
        let mut w = MsgWriter::new();
        write_svc_print(&mut w, clip_text(text));
        self.send(MsgDest::All, w.as_slice());
    }

    /// Shows `text` in the middle of the screen of `slot` (`svc_centerprint`); cut and
    /// dropped like [`client_print`](Self::client_print).
    pub fn center_print(&mut self, slot: u8, text: &str) {
        let mut w = MsgWriter::new();
        write_svc_centerprint(&mut w, clip_text(text));
        self.send(MsgDest::One(slot), w.as_slice());
    }

    /// Makes the client of `slot` run `text` as console commands (`svc_stufftext`); cut and
    /// dropped like [`client_print`](Self::client_print).
    pub fn stufftext(&mut self, slot: u8, text: &str) {
        let mut w = MsgWriter::new();
        write_svc_stufftext(&mut w, clip_text(text));
        self.send(MsgDest::One(slot), w.as_slice());
    }

    /// Plays the precached sound `sample` on `channel` (0 to 7) from `ent`'s origin to every
    /// client that can see that point (`MsgDest::Pvs`), with `volume` clamped to 0.0..=1.0,
    /// `attenuation` to 0.0..=3.98 and `pitch` as is (100 is normal). Returns `false`, sending
    /// nothing, for a stale entity, a sample that is not in the sound precache (or is empty),
    /// a channel above 7 or a non-finite volume or attenuation; `true` once the message is
    /// queued, even if no client can hear it.
    pub fn emit_sound(
        &mut self,
        ent: crate::edict::EdictId,
        channel: u8,
        sample: &str,
        volume: f32,
        attenuation: f32,
        pitch: u8,
    ) -> bool {
        if channel > MAX_SOUND_CHANNEL || !volume.is_finite() || !attenuation.is_finite() {
            return false;
        }
        let Some(origin) = self.edicts.get(ent).map(|e| e.vars.origin) else {
            return false;
        };
        let Some(sound_index) = self.precache.sound_index(sample).filter(|&i| i != 0) else {
            return false;
        };
        let volume = (volume.clamp(0.0, 1.0) * f32::from(FULL_VOLUME)).round() as u8;
        let attenuation = attenuation.clamp(0.0, MAX_ATTENUATION);
        let mut flags = 0u32;
        if volume != FULL_VOLUME {
            flags |= SND_FL_VOLUME;
        }
        if attenuation != 1.0 {
            flags |= SND_FL_ATTENUATION;
        }
        if pitch != NORMAL_PITCH {
            flags |= SND_FL_PITCH;
        }
        let mut w = MsgWriter::new();
        write_svc_sound(
            &mut w,
            &SoundMsg {
                flags: flags as u16,
                volume,
                attenuation,
                channel,
                entity: ent.index,
                sound_index,
                origin,
                pitch,
            },
        );
        self.send(MsgDest::Pvs(origin), w.as_slice());
        true
    }

    /// Sends a temp entity effect to `dest` (`svc_temp_entity`): the `te_type` byte, then
    /// `payload` as is, which the caller encodes for that effect. A message over `MAX_MSGLEN`
    /// bytes is dropped.
    pub fn temp_entity(&mut self, dest: MsgDest, te_type: u8, payload: &[u8]) {
        let mut w = MsgWriter::new();
        write_svc_temp_entity(&mut w, te_type, payload);
        self.send(dest, w.as_slice());
    }

    /// Sets light style `index` to `pattern` (`a` dark to `z` bright, one letter per tenth of a
    /// second) and sends the change to every client past the `new` stage; clients that enter
    /// later are sent the stored styles. Ignored for an `index` of 64 or more and for a pattern
    /// over 64 bytes or with a NUL byte in it. An empty pattern is allowed. The styles go
    /// back to their defaults (style 0 is `m`) when the map changes.
    pub fn light_style(&mut self, index: u8, pattern: &str) {
        if index >= MAX_LIGHTSTYLES
            || pattern.len() > MAX_LIGHT_STYLE_BYTES
            || pattern.contains('\0')
        {
            return;
        }
        self.outbox.light_styles[usize::from(index)] = pattern.to_string();
        let mut w = MsgWriter::new();
        write_svc_lightstyle(&mut w, index, pattern);
        self.send(MsgDest::All, w.as_slice());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_at_nul_and_on_a_character_boundary() {
        assert_eq!(clip_text("abc"), "abc");
        assert_eq!(clip_text("ab\0cd"), "ab");
        assert_eq!(clip_text("\0ab"), "");
        let long = "x".repeat(MAX_MESSAGE_TEXT + 10);
        assert_eq!(clip_text(&long).len(), MAX_MESSAGE_TEXT);
        // 1200 bytes of 3-byte characters: 341 of them fit.
        let wide = "\u{20ac}".repeat(400);
        assert_eq!(clip_text(&wide).len(), 1023);
        assert!(wide.starts_with(clip_text(&wide)));
        // A NUL after the limit does not matter; the limit does.
        let both = format!("{long}\0tail");
        assert_eq!(clip_text(&both).len(), MAX_MESSAGE_TEXT);
    }
}
