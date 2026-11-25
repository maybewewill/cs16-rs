//! The game-facing message calls of [`EngineApi`]: raw sends to a destination, typed helpers
//! for user messages, prints, sounds, temp entities and light styles. Every call refuses
//! quietly on bad game-supplied data (a slot out of range or not in the required state, a stale
//! entity, non-finite numbers, oversized messages): the message is dropped, nothing panics.

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::messages::{
    SND_FL_ATTENUATION, SND_FL_PITCH, SND_FL_SENTENCE, SND_FL_SPAWNING, SND_FL_STOP,
    SND_FL_VOLUME, SoundMsg, svc, write_svc_centerprint, write_svc_lightstyle, write_svc_print,
    write_svc_sound, write_svc_stufftext, write_svc_temp_entity, write_svc_usermsg,
};
use cs_proto48::netchan::MAX_MSGLEN;
use cs_proto48::tempent::TempEntity;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::client::ClientState;
use crate::edict::EdictId;
use crate::game::EngineApi;
use crate::signon::{INIT_BUFFER_BUDGET, MAX_LIGHTSTYLES};
use crate::unlag::userinfo_lw;

/// Bytes of text `client_print`, `print_all`, `center_print` and `stufftext` send at most.
pub const MAX_MESSAGE_TEXT: usize = 1024;
/// Bytes one `send` may carry: the netchan's `MAX_MSGLEN`, above which queued reliable data
/// goes out as a fragment stream; a longer message is dropped.
const MAX_SEND_BYTES: usize = MAX_MSGLEN;
/// Bytes of a light style pattern the engine keeps. ReHLDS `PF_lightstyle_I`
/// (`rehlds/engine/pr_cmds.cpp`, read 2026-11-21) copies the pattern with `Q_strlcpy` into a
/// buffer of `MAX_LIGHTSTYLE_SIZE` (64, `rehlds/common/qlimits.h`) bytes, which holds the
/// terminating NUL, so at most 63 bytes of pattern remain.
pub const MAX_LIGHT_STYLE_BYTES: usize = 63;
/// Highest entity index the `svc_sound` entity field (11 bits) can carry.
const MAX_SOUND_ENTITY: u16 = (1 << 11) - 1;
/// Whether the `svc_sound` entity field (11 bits) can carry entity `index`.
fn sound_entity_fits(index: u16) -> bool {
    index <= MAX_SOUND_ENTITY
}

/// Highest sound channel (3 bits on the wire).
const MAX_SOUND_CHANNEL: u8 = 7;
/// `CHAN_NETWORKVOICE_BASE`, which `SV_BuildSoundMsg` makes of a channel above 7.
const CHAN_NETWORKVOICE_BASE: u8 = 4;
/// The `svc_sound` flag bits that fit the wire (9).
const SND_FIELD_MASK: u32 = 0x1FF;
/// `CHAN_STATIC`: a sound on this channel is broadcast, not limited to the PAS.
const CHAN_STATIC: u8 = 6;
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
    /// The reliable stream of every client in the game (past `spawn`) at the end of the frame
    /// (`MSG_ALL`, `sv.reliable_datagram`): what the game sends to single clients in the same
    /// frame comes first.
    All,
    /// The unreliable queue of every non-fake `Active` client. Appended directly at emission time
    /// so chronological ordering relative to [`MsgDest::OneUnreliable`] is preserved (`sv.datagram`
    /// delivery order).
    Broadcast,
    /// The unreliable queue of every `Active` client in the potentially visible set of the
    /// point, as `SV_Multicast` with `MSG_FL_PVS` (ReHLDS `sv_main.cpp`, read 2026-11-21): a
    /// client gets it when the leaf of its edict's `origin` (not the eye) is in the PVS of the
    /// leaf of the point. A non-finite point reaches no one; a point in solid or outside the world, a map without visibility
    /// data and a single-client server reach every `Active` client.
    Pvs(Vec3),
    /// The unreliable queue of every `Active` client in the potentially audible set of the
    /// point, as `SV_Multicast` with `MSG_FL_PAS` (ReHLDS `sv_main.cpp`, read 2026-11-21): a
    /// client hears it when the leaf of its edict's `origin` (not the eye) is in the PAS row of
    /// the leaf of the point. A non-finite point reaches no one; a point in solid or outside the world, and a map without PAS
    /// (no visibility data, a single-client server, a map over the PAS caps), reach every
    /// `Active` client. The PAS reaches beyond the PVS.
    Pas(Vec3),
    /// The init buffer: stored, and replayed to every client that enters the server (in its
    /// `spawn` stage, after the light styles). Only while the map is being activated (in
    /// `spawn_entity` and `server_activate`) and while it fits
    /// [`INIT_BUFFER_BUDGET`](crate::INIT_BUFFER_BUDGET); otherwise
    /// the same as `All`.
    Init,
}

/// `client_t::lw` of a client view in use: the one `cl_lw` reader, [`crate::unlag::userinfo_lw`].
pub(crate) fn lw_of(view: &crate::game::ClientView) -> bool {
    view.in_use && crate::unlag::userinfo_lw(&view.userinfo)
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
    /// the stage the destination needs). A target whose buffer for this call already holds more
    /// than [`MAX_RELIABLE_BACKLOG`](crate::MAX_RELIABLE_BACKLOG) bytes (a game looping sends
    /// within one call) is skipped; the message is dropped for it and counted in
    /// `NetStats::outbox_dropped`.
    pub fn send(&mut self, dest: MsgDest, bytes: &[u8]) {
        self.send_except(dest, bytes, None);
    }

    /// [`send`](Self::send), skipping the client `except` where the destination is the
    /// unreliable queue of many (`SV_Multicast` with `MSG_FL_ONE` skips `host_client`, ReHLDS
    /// `rehlds/engine/sv_main.cpp`, read 2026-11-22). Only `Broadcast` and `Pas` honour it; the
    /// other destinations are not used with it.
    fn send_except(&mut self, dest: MsgDest, bytes: &[u8], except: Option<usize>) {
        if bytes.is_empty() || bytes.len() > MAX_SEND_BYTES {
            return;
        }
        match dest {
            MsgDest::One(slot) => {
                let slot = usize::from(slot);
                if self.outbox.discard_direct == Some(slot) {
                    return;
                }
                if self.clients.get(slot).is_some_and(|c| c.state != ClientState::Free) {
                    self.outbox.push_reliable(slot, bytes);
                }
            }
            MsgDest::OneUnreliable(slot) => {
                let slot = usize::from(slot);
                if self.outbox.discard_direct == Some(slot) {
                    return;
                }
                if self.clients.get(slot).is_some_and(|c| c.state == ClientState::Active) {
                    self.outbox.push_unreliable(slot, bytes);
                }
            }
            // `MSG_ALL`: `sv.reliable_datagram`, which `SV_UpdateToReliableMessages` copies to
            // the clients in the game at the end of the frame (ReHLDS `sv_main.cpp`, read
            // 2026-11-22), after the messages sent to single clients during the frame.
            MsgDest::All => self.outbox.push_reliable_all(bytes),
            MsgDest::Broadcast => {
                for (slot, view) in self.clients.iter().enumerate() {
                    if view.state == ClientState::Active && !view.fake && Some(slot) != except {
                        self.outbox.push_unreliable(slot, bytes);
                    }
                }
            }
            MsgDest::Pvs(point) => self.send_in_pvs(point, bytes),
            MsgDest::Pas(point) => {
                // `SV_Multicast` with `MSG_FL_PAS` (`rehlds/engine/sv_main.cpp`, read
                // 2026-11-21): the PAS row of the leaf of the point against the leaf of each
                // client's origin (not its eye). A point in solid or outside the world and a
                // map without PAS (visibility data missing, one client, over the caps) reach
                // everyone; a client whose edict is gone is not sent to.
                if !point.is_finite() {
                    return;
                }
                let row = self.map.pas_row(point);
                for slot in 0..self.clients.len() {
                    if self.clients[slot].state != ClientState::Active || Some(slot) == except {
                        continue;
                    }
                    let origin = self
                        .edicts
                        .get(self.edicts.client(slot as u8))
                        .map(|e| e.vars.origin);
                    let hears = origin.is_some_and(|origin| {
                        row.is_none_or(|row| self.map.pas_row_contains(row, origin))
                    });
                    if hears {
                        self.outbox.push_unreliable(slot, bytes);
                    }
                }
            }
            MsgDest::Init => {
                if self.outbox.init_open
                    && self.outbox.init.len() + bytes.len() <= INIT_BUFFER_BUDGET
                {
                    self.outbox.init.extend_from_slice(bytes);
                } else {
                    self.send(MsgDest::All, bytes);
                }
            }
        }
    }

    /// `SV_Multicast` with `MSG_FL_PVS`: the PVS row of the leaf of `point` against the leaf of
    /// each `Active` client's edict origin; see [`MsgDest::Pvs`]. A client whose edict is gone
    /// is not sent to.
    fn send_in_pvs(&mut self, point: Vec3, bytes: &[u8]) {
        if !point.is_finite() {
            return;
        }
        let row = if self.clients.len() > 1 { self.map.pvs_row(point) } else { None };
        for slot in 0..self.clients.len() {
            if self.clients[slot].state != ClientState::Active {
                continue;
            }
            let origin = self
                .edicts
                .get(self.edicts.client(slot as u8))
                .map(|e| e.vars.origin);
            let hears = origin.is_some_and(|origin| {
                row.as_ref().is_none_or(|row| self.map.pvs_row_contains(row, origin))
            });
            if hears {
                self.outbox.push_unreliable(slot, bytes);
            }
        }
    }

    /// `SV_Multicast` with `MSG_FL_BROADCAST` and `reliable` set (`SV_StartSound_internal` for
    /// `CHAN_STATIC` and `SND_STOP` sounds in multiplayer): the reliable stream of every client
    /// with `client->active`, which `SV_WriteSpawn` sets (`sv_main.cpp`, read 2026-11-22): the
    /// same set as [`MsgDest::All`] reaches ([`ClientState::is_in_game`]).
    fn send_reliable_to_active(&mut self, bytes: &[u8], except: Option<usize>) {
        for slot in 0..self.clients.len() {
            if self.clients[slot].state.is_in_game() && Some(slot) != except {
                self.outbox.push_reliable(slot, bytes);
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

    /// Sends a pre-encoded custom user message by registered name.
    pub fn send_raw_usermsg(&mut self, dest: MsgDest, name: &str, payload: &[u8]) -> bool {
        let mut w = MsgWriter::new();
        if !cs_proto48::messages::write_svc_raw_usermsg(&mut w, self.usermsgs, name, payload) {
            return false;
        }
        self.send(dest, w.as_slice());
        true
    }

    /// Sends a custom mod user message ONLY to slots whose userinfo advertises mod support
    /// (`_cs16rs_mods = 1`); original CS 1.6 clients never receive extended mod traffic.
    pub fn send_mod_usermsg(&mut self, dest: MsgDest, name: &str, payload: &[u8]) -> bool {
        let mut w = MsgWriter::new();
        if !cs_proto48::messages::write_svc_raw_usermsg(&mut w, self.usermsgs, name, payload) {
            return false;
        }
        let bytes = w.as_slice();
        match dest {
            MsgDest::One(slot) => {
                if self.client_supports_mods(slot) {
                    self.send(MsgDest::One(slot), bytes);
                }
            }
            MsgDest::OneUnreliable(slot) => {
                if self.client_supports_mods(slot) {
                    self.send(MsgDest::OneUnreliable(slot), bytes);
                }
            }
            MsgDest::All => {
                for slot in 0..self.clients.len() {
                    if self.clients[slot].state.is_in_game()
                        && self.client_supports_mods(slot as u8)
                    {
                        self.outbox.push_reliable(slot, bytes);
                    }
                }
            }
            MsgDest::Broadcast | MsgDest::Pvs(_) | MsgDest::Pas(_) => {
                for slot in 0..self.clients.len() {
                    if self.clients[slot].state == ClientState::Active
                        && self.client_supports_mods(slot as u8)
                    {
                        self.outbox.push_unreliable(slot, bytes);
                    }
                }
            }
            MsgDest::Init => {
                // Never buffer extended mod usermsgs into `Init` because vanilla clients replay it.
                return false;
            }
        }
        true
    }

    /// `SET_VIEW` (`PF_setview_I`, ReHLDS `pr_cmds.cpp`, read 2026-11-22): the client of the
    /// edict `client` is told to look through `view` (`svc_setview` on its reliable stream; the
    /// original also keeps the entity for `SetupVisibility`, which the game here answers from its
    /// own state). Nothing for an edict that is not a client slot.
    pub fn set_view(&mut self, client: EdictId, view: EdictId) {
        let Some(slot) = client.index.checked_sub(1).filter(|s| usize::from(*s) < self.clients.len())
        else {
            return;
        };
        let mut w = MsgWriter::new();
        cs_proto48::messages::write_svc_setview(&mut w, view.index);
        self.send(MsgDest::One(slot as u8), w.as_slice());
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

    /// Plays the precached sound `sample` on `channel` (0 to 7) from the centre of `ent`'s
    /// bounds, `origin + (mins + maxs) * 0.5`, to every client that can hear that point
    /// (`MsgDest::Pas`), with `volume` clamped to 0.0..=1.0, `attenuation` to 0.0..=3.98 and
    /// `pitch` as is (100 is normal). The wire origin is that same point, so a brush entity
    /// (origin zero, bounds around its model) sounds from the middle of the brush.
    ///
    /// ReHLDS `SV_StartSound_internal` (`rehlds/engine/sv_main.cpp`, read 2026-11-21) computes
    /// `origin[i] = (maxs[i] + mins[i]) * 0.5 + entity origin[i]` the same way and multicasts
    /// the sound with `MSG_FL_PAS` (potentially audible set), as this does, with the audience
    /// rule of `SV_Multicast` (see [`MsgDest::Pas`]). A sound on `CHAN_STATIC` (6) goes to
    /// every `Active` client instead, on their reliable stream (unreliable on a single-client
    /// server), as `SV_StartSound_internal` does (see [`stop_sound`](Self::stop_sound)).
    ///
    /// Returns `false`, sending nothing, for a stale entity or one whose index does not fit the
    /// 11-bit wire field (2048 and up), a non-finite position, a sample that is not in the
    /// sound precache (or is empty), a channel above 7 or a non-finite volume or attenuation;
    /// `true` once the message is queued, even if no client can hear it.
    pub fn emit_sound(
        &mut self,
        ent: crate::edict::EdictId,
        channel: u8,
        sample: &str,
        volume: f32,
        attenuation: f32,
        pitch: u8,
    ) -> bool {
        if !volume.is_finite() {
            return false;
        }
        let volume = (volume.clamp(0.0, 1.0) * f32::from(FULL_VOLUME)).round() as u8;
        self.start_sound(ent, channel, sample, volume, attenuation, pitch, 0, None)
    }

    /// `PM_SV_PlaySound` (ReHLDS `rehlds/engine/sv_pmove.cpp`, read 2026-11-22), the sound the
    /// movement code of the game plays for `player`: `SV_StartSound(1, ...)`, so the sound goes
    /// out as [`emit_sound`](Self::emit_sound) does (from the middle of the player's box, to the
    /// clients that can hear that point) except that the player's own client does not get it
    /// (`MSG_FL_ONE`: `SV_Multicast` skips `host_client`; that client predicts the sound
    /// itself). The same on `CHAN_STATIC`, which goes to every active client but that one.
    /// `volume` is sent as `(int)(volume * 255.0)`, truncated, not rounded.
    ///
    /// The arguments are cut to what the wire holds as `SV_BuildSoundMsg` does (`sv_main.cpp`,
    /// read 2026-11-22): a channel below 0 is 0 (`CHAN_AUTO`) and one above 7 is 4
    /// (`CHAN_NETWORKVOICE_BASE`), the volume is held to 0..=255, the pitch to 0..=255, and the
    /// attenuation to what [`emit_sound`](Self::emit_sound) holds it to. `flags` are the
    /// `SND_FL_*` bits, ORed into the message (only the low 9 bits fit the wire). Refused like
    /// `emit_sound` (a stale entity, a sample not in the sound precache, a non-finite volume or
    /// attenuation); `player` need not be a client, then nobody is skipped.
    #[allow(clippy::too_many_arguments)]
    pub fn pm_play_sound(
        &mut self,
        player: crate::edict::EdictId,
        channel: i32,
        sample: &str,
        volume: f32,
        attenuation: f32,
        flags: i32,
        pitch: i32,
    ) -> bool {
        if !volume.is_finite() {
            return false;
        }
        // `(int)(volume * 255.0)`: the product is a double, cut toward zero.
        let volume = ((f64::from(volume) * 255.0) as i32).clamp(0, 255) as u8;
        let channel = match channel {
            ..=-1 => 0,
            0..=7 => channel as u8,
            _ => CHAN_NETWORKVOICE_BASE,
        };
        let pitch = pitch.clamp(0, 255) as u8;
        let flags = (flags as u32) & SND_FIELD_MASK;
        let except = (player.index >= 1 && usize::from(player.index) <= self.clients.len())
            .then(|| usize::from(player.index) - 1);
        self.start_sound(player, channel, sample, volume, attenuation, pitch, flags, except)
    }

    /// `PF_sound_I` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read 2026-11-22), the game's
    /// `EMIT_SOUND_DYN2`: `SV_StartSound(0, ...)`, so the sound goes out as
    /// [`emit_sound`](Self::emit_sound) does and reaches every client that can hear the point,
    /// the entity's own client included. `volume` is 0 to 1 and is sent as
    /// `(int)(volume * 255)`; `flags` are the `SND_FL_*` bits (`SND_FL_STOP`,
    /// `SND_FL_CHANGE_VOL`, `SND_FL_CHANGE_PITCH`), ORed into the message; the arguments are held
    /// to what the wire carries as `SV_BuildSoundMsg` does (see
    /// [`pm_play_sound`](Self::pm_play_sound)).
    ///
    /// Quirk kept: a `SND_FL_STOP` sound is never limited to the PAS. Where `PF_sound_I` stops
    /// the server with an error for an argument out of range (a volume over 255 as an int, a
    /// channel outside 0 to 7, a pitch outside 0 to 255) this holds it to the range instead.
    /// Assumption: a sentence sample (`!N`, `#N`) is refused like any sample that is not
    /// precached; the weapons do not use them.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_sound_flags(
        &mut self,
        ent: crate::edict::EdictId,
        channel: i32,
        sample: &str,
        volume: f32,
        attenuation: f32,
        flags: i32,
        pitch: i32,
    ) -> bool {
        if !volume.is_finite() {
            return false;
        }
        let volume = ((f64::from(volume) * 255.0) as i32).clamp(0, 255) as u8;
        let channel = match channel {
            ..=-1 => 0,
            0..=7 => channel as u8,
            _ => CHAN_NETWORKVOICE_BASE,
        };
        let pitch = pitch.clamp(0, 255) as u8;
        let flags = (flags as u32) & SND_FIELD_MASK;
        self.start_sound(ent, channel, sample, volume, attenuation, pitch, flags, None)
    }

    /// `PF_ambientsound_I` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read 2026-11-22): a sound that
    /// plays from the point `origin`, as `svc_spawnstaticsound`: `origin` (coords), the sound
    /// index (`short`), `volume * 255` and `attenuation * 64` (bytes), the entity number
    /// (`short`), the pitch and the flags (bytes). It goes to every `Active` client on the
    /// unreliable stream (`g_psv.datagram`), or, with [`SND_FL_SPAWNING`], into the init buffer
    /// that every client entering the server is replayed (`g_psv.signon`; see
    /// [`MsgDest::Init`]).
    ///
    /// A sample starting with `!` is a sentence: the flags get `SND_FL_SENTENCE`, the number
    /// after the `!` is the sound index, and a number of 1536 and up is refused
    /// (`CVOXFILESENTENCEMAX`). Any other sample must be in the sound precache.
    ///
    /// Returns `false`, sending nothing, for a stale entity, a non-finite origin, volume or
    /// attenuation, and a sample that is not valid as above (the original prints and returns).
    /// Assumptions: an empty sample is refused (the original would match slot 0 of its table);
    /// the volume and the attenuation bytes are held to 0 to 255 (the original's `MSG_WriteByte`
    /// does not check them in the build we read); the entity must be alive, which the original
    /// cannot ask.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_ambient_sound(
        &mut self,
        ent: crate::edict::EdictId,
        origin: Vec3,
        sample: &str,
        volume: f32,
        attenuation: f32,
        flags: i32,
        pitch: i32,
    ) -> bool {
        /// `CVOXFILESENTENCEMAX`: sentence numbers at and above this are not sentences.
        const SENTENCE_MAX: i64 = 1536;
        if !origin.is_finite()
            || !volume.is_finite()
            || !attenuation.is_finite()
            || !sound_entity_fits(ent.index)
            || self.edicts.get(ent).is_none()
        {
            return false;
        }
        let mut flags = flags as u32;
        let sound_index = if let Some(number) = sample.strip_prefix('!') {
            flags |= SND_FL_SENTENCE;
            // `Q_atoi`: the leading digits, 0 for none.
            let digits: String = number.chars().take_while(char::is_ascii_digit).collect();
            let n = digits.parse::<i64>().unwrap_or(if digits.is_empty() { 0 } else { i64::MAX });
            if n >= SENTENCE_MAX {
                return false;
            }
            n as u16
        } else {
            match self.precache.sound_index(sample).filter(|&i| i != 0) {
                Some(i) => i,
                None => return false,
            }
        };
        let mut w = MsgWriter::new();
        w.write_u8(svc::SPAWNSTATICSOUND);
        w.write_coord(origin.x);
        w.write_coord(origin.y);
        w.write_coord(origin.z);
        w.write_u16(sound_index);
        w.write_u8((f64::from(volume) * 255.0).clamp(0.0, 255.0) as u8);
        w.write_u8((f64::from(attenuation) * 64.0).clamp(0.0, 255.0) as u8);
        w.write_u16(ent.index);
        w.write_u8(pitch.clamp(0, 255) as u8);
        w.write_u8(flags as u8);
        let dest = if flags & SND_FL_SPAWNING != 0 {
            MsgDest::Init
        } else {
            MsgDest::Broadcast
        };
        self.send(dest, w.as_slice());
        true
    }

    /// `PF_DecalIndex` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read 2026-11-22): the index of the
    /// decal `name` in `decals.wad` as the server holds it (see
    /// [`Server::set_decals`](crate::Server::set_decals)), the first match without regard to
    /// case, or -1 for a name it does not know.
    pub fn decal_index(&self, name: &str) -> i32 {
        self.precache.decal_index(name)
    }

    /// Sends a temp entity built by one of the [`cs_proto48::tempent`] functions to `dest`;
    /// [`temp_entity`](Self::temp_entity) with the effect and its payload taken from `te`.
    pub fn send_temp_entity(&mut self, dest: MsgDest, te: &TempEntity) {
        self.temp_entity(dest, te.te_type, &te.payload);
    }

    /// Stops the sound `sample` on `channel` of `ent` (`svc_sound` with `SND_STOP`), for
    /// every `Active` client: such a message is never limited to the PAS
    /// (`SV_StartSound_internal` sends PAS only for `channel != CHAN_STATIC && !(flags &
    /// SND_STOP)`). Refused like [`emit_sound`](Self::emit_sound).
    pub fn stop_sound(&mut self, ent: crate::edict::EdictId, channel: u8, sample: &str) -> bool {
        self.start_sound(
            ent,
            channel,
            sample,
            FULL_VOLUME,
            1.0,
            NORMAL_PITCH,
            SND_FL_STOP,
            None,
        )
    }

    /// `SV_StartSound_internal`: builds the message and picks the audience; `extra_flags` is
    /// added to the `svc_sound` flags, `volume` is the byte on the wire, and `except` is the
    /// slot `recipients == 1` (`MSG_FL_ONE`) leaves out.
    #[allow(clippy::too_many_arguments)]
    fn start_sound(
        &mut self,
        ent: crate::edict::EdictId,
        channel: u8,
        sample: &str,
        volume: u8,
        attenuation: f32,
        pitch: u8,
        extra_flags: u32,
        except: Option<usize>,
    ) -> bool {
        if channel > MAX_SOUND_CHANNEL
            || !sound_entity_fits(ent.index)
            || !attenuation.is_finite()
        {
            return false;
        }
        let Some(origin) = self
            .edicts
            .get(ent)
            .map(|e| e.vars.origin + (e.vars.mins + e.vars.maxs) * 0.5)
            .filter(|origin| origin.is_finite())
        else {
            return false;
        };
        let Some(sound_index) = self.precache.sound_index(sample).filter(|&i| i != 0) else {
            return false;
        };
        let attenuation = attenuation.clamp(0.0, MAX_ATTENUATION);
        let mut flags = extra_flags;
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
        if channel != CHAN_STATIC && extra_flags & SND_FL_STOP == 0 {
            self.send_except(MsgDest::Pas(origin), w.as_slice(), except);
        } else if self.clients.len() > 1 {
            self.send_reliable_to_active(w.as_slice(), except);
        } else {
            self.send_except(MsgDest::Broadcast, w.as_slice(), except);
        }
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

    /// `svc_weaponanim` to the client of `slot`: the view model plays `sequence` with bodygroup
    /// `body`. Both are sent as bytes (`WRITE_BYTE`), so a wider value is cut to its low byte.
    /// Reliable, as the game's `MESSAGE_BEGIN(MSG_ONE, SVC_WEAPONANIM, ...)` is (ReGameDLL
    /// `CBasePlayerWeapon::SendWeaponAnim`, read 2026-11-22); the `skiplocal` rule (a client with
    /// `cl_lw` predicts its own animations and is not sent them) is the game's, see
    /// [`client_lw`](Self::client_lw). A slot that is out of range or free gets nothing.
    pub fn weapon_anim(&mut self, slot: u8, sequence: i32, body: i32) {
        self.send(
            MsgDest::One(slot),
            &[svc::WEAPONANIM, sequence as u8, body as u8],
        );
    }

    /// The client's `cl_lw` (local weapons): `host_client->lw` of ReHLDS, set by
    /// `SV_ExtractFromUserinfo` (`rehlds/engine/sv_main.cpp`, read 2026-11-22) as
    /// `atoi(cl_lw) != 0`, false when the key is missing or empty. `false` for a slot that is
    /// out of range. This is also what `ENGINE_CANSKIP` (`PF_CanSkipPlayer`, `rehlds/engine/
    /// pr_cmds.cpp`, read 2026-11-22) answers for a slot that is a client.
    pub fn client_lw(&self, slot: u8) -> bool {
        self.clients
            .get(usize::from(slot))
            .is_some_and(|c| userinfo_lw(&c.userinfo))
    }

    /// Sets light style `index` to `pattern` (`a` dark to `z` bright, one letter per tenth of a
    /// second) and sends the change to every client past the `new` stage; clients that enter
    /// later are sent the stored styles. Ignored for an `index` of 64 or more and for a pattern
    /// with a NUL byte in it. A pattern longer than [`MAX_LIGHT_STYLE_BYTES`] (63) is cut to
    /// that many bytes, on a character boundary, as ReHLDS keeps at most 63 (see the constant).
    /// An empty pattern is allowed. The styles go back to their defaults (style 0 is `m`) when
    /// the map changes.
    pub fn light_style(&mut self, index: u8, pattern: &str) {
        if index >= MAX_LIGHTSTYLES || pattern.contains('\0') {
            return;
        }
        let mut end = pattern.len().min(MAX_LIGHT_STYLE_BYTES);
        while !pattern.is_char_boundary(end) {
            end -= 1;
        }
        let pattern = &pattern[..end];
        self.outbox.light_styles[usize::from(index)] = pattern.to_string();
        let mut w = MsgWriter::new();
        write_svc_lightstyle(&mut w, index, pattern);
        self.send(MsgDest::All, w.as_slice());
    }
}

#[cfg(test)]
mod tests {
    use super::sound_entity_fits;

    #[test]
    fn sound_entity_guard_stops_at_the_11_bit_field() {
        assert!(sound_entity_fits(0));
        assert!(sound_entity_fits(2047));
        assert!(!sound_entity_fits(2048));
        assert!(!sound_entity_fits(u16::MAX));
    }

    use super::*;
    use crate::edict::{DEFAULT_MAX_EDICTS, EdictPool};
    use crate::game::{ClientView, Outbox};
    use crate::map::ServerMap;
    use crate::precache::PrecacheTables;
    use crate::world::WorldLinks;
    use cs_cvar::CVarRegistry;
    use cs_proto48::usermsg::UserMsgRegistry;

    /// `svc_spawnstaticsound` byte by byte (`PF_ambientsound_I`): coords `(1, -2.5, 100)` are
    /// 8, -20, 800; the sound is index 1; volume `0.5 * 255` is 127; attenuation
    /// `0.8125 * 64` is 52; entity 1; pitch 90; flags 0x10 (the sentence flag is not set for a
    /// named sample, so the 0x10 is the flag the caller passed).
    #[test]
    fn ambient_sound_bytes_follow_pf_ambientsound() {
        let map = ServerMap::stub("de_test");
        let mut world = WorldLinks::new(map.world_bounds());
        let mut edicts = EdictPool::new(1, DEFAULT_MAX_EDICTS);
        let mut precache = PrecacheTables::new(&map.path, map.submodel_count);
        let mut cvars = CVarRegistry::new_empty();
        let usermsgs = UserMsgRegistry::cs16_default();
        let mut outbox = Outbox::new(1);
        let mut clients = vec![ClientView::default()];
        clients[0].state = ClientState::Active;
        assert_eq!(precache.precache_sound("ambience/fan.wav"), Ok(1));
        let mut eng = EngineApi::new(
            &mut edicts,
            &mut precache,
            &mut cvars,
            &usermsgs,
            &map,
            &mut world,
            1.0,
            0.0,
            &mut outbox,
            &mut clients,
        );
        let ent = eng.client_edict(0);
        assert!(eng.emit_ambient_sound(
            ent,
            Vec3::new(1.0, -2.5, 100.0),
            "ambience/fan.wav",
            0.5,
            0.8125,
            0x10,
            90
        ));
        // A sentence: `!12` is sound 12 with `SND_FL_SENTENCE` (0x10) added.
        assert!(eng.emit_ambient_sound(ent, Vec3::ZERO, "!12", 1.0, 1.0, 0, 100));
        let want = [
            vec![
                svc::SPAWNSTATICSOUND,
                0x08, 0x00, 0xEC, 0xFF, 0x20, 0x03, // origin
                0x01, 0x00, // sound index
                127, 52, // volume, attenuation
                0x01, 0x00, // entity
                90, 0x10, // pitch, flags
            ],
            vec![
                svc::SPAWNSTATICSOUND,
                0, 0, 0, 0, 0, 0, //
                12, 0, //
                255, 64, //
                0x01, 0x00, //
                100, 0x10,
            ],
        ]
        .concat();
        assert_eq!(outbox.unreliable[0].as_slice(), &want[..]);
    }

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

    #[test]
    fn send_mod_usermsg_reaches_only_mod_capable_clients() {
        let mut edicts = crate::edict::EdictPool::new(2, 16);
        let mut precache = crate::precache::PrecacheTables::new("maps/de_dust2.bsp", 1);
        let mut cvars = cs_cvar::CVarRegistry::new_with_defaults();
        let mut usermsgs = UserMsgRegistry::cs16_default();
        let msg_id = usermsgs.register_next(-1, "CsModEvent").unwrap();
        let map = crate::map::ServerMap::stub("de_dust2");
        let mut world = crate::world::WorldLinks::new(map.world_bounds());
        let mut outbox = crate::game::Outbox::new(2);
        let mut clients = [
            crate::game::ClientView {
                in_use: true,
                active: true,
                state: ClientState::Active,
                ..Default::default()
            },
            crate::game::ClientView {
                in_use: true,
                active: true,
                state: ClientState::Active,
                ..Default::default()
            },
        ];
        clients[1]
            .userinfo
            .set(crate::game::MOD_CAPABILITY_KEY, "1");

        let mut eng = EngineApi::new(
            &mut edicts,
            &mut precache,
            &mut cvars,
            &usermsgs,
            &map,
            &mut world,
            1.0,
            0.01,
            &mut outbox,
            &mut clients,
        );
        assert!(eng.send_mod_usermsg(MsgDest::All, "CsModEvent", b"ok"));
        assert!(!eng.send_mod_usermsg(MsgDest::Init, "CsModEvent", b"ok"));
        assert!(outbox.reliable[0].as_slice().is_empty());
        assert_eq!(outbox.reliable[1].as_slice(), &[msg_id, 2, b'o', b'k']);
    }
}
