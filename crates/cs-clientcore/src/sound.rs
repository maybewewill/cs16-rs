//! Sound commands: what `svc_sound`, `svc_stopsound`, `svc_spawnstaticsound` and `svc_soundfade`
//! ask the client's mixer to do, as data for the shell, plus the helpers the other effect
//! modules (events, temp entities) start sounds with.
//!
//! Ported from Xash3D FWGS (GPL-3.0-or-later), read 2026-11-23:
//! - `engine/client/cl_parse_gs.c` `CL_ParseSoundPacketGS` (what a sound packet means: the
//!   sentence name `!N`, `CHAN_STATIC` going to the ambient path, the sound being dropped while
//!   the audio is not prepared), `CL_ParseSpawnStaticSound`, and the `svc_goldsrc_stopsound`
//!   case of `CL_ParseGoldSrcServerMessage` (the word is `entity << 3 | channel`);
//! - `engine/client/cl_parse.c` `CL_ParseSoundFade`;
//! - `engine/client/s_main.c` `S_StartSound` and `S_AmbientSound`: a pitch of 1 or less plays at
//!   100 (`PITCH_NORM`), `SND_STOP`/`SND_CHANGE_VOL`/`SND_CHANGE_PITCH` alter a playing sound.
//!
//! The wire layout is read by `cs-proto48` (`SoundMsg`, `SpawnStaticSoundMsg`); this module
//! turns it into [`SoundCommand`]s. Mixing, spatialisation and the sound files are the shell's
//! business: the core only emits "play this".

use cs_proto48::messages::{
    SND_FL_CHANGE_PITCH, SND_FL_CHANGE_VOL, SND_FL_SENTENCE, SND_FL_STOP, SvcMessage,
};
use glam::Vec3;

use crate::precache::PrecacheTables;

/// Sound channels (`CHAN_*`, `common/const.h` numbering as the wire carries it).
pub mod chan {
    pub const AUTO: u8 = 0;
    pub const WEAPON: u8 = 1;
    pub const VOICE: u8 = 2;
    pub const ITEM: u8 = 3;
    pub const BODY: u8 = 4;
    pub const STREAM: u8 = 5;
    pub const STATIC: u8 = 6;
    pub const NETWORKVOICE_BASE: u8 = 7;
}

/// `PITCH_NORM`.
pub const PITCH_NORM: u8 = 100;
/// `ATTN_NORM`.
pub const ATTN_NORM: f32 = 0.8;
/// The commands kept when the shell does not drain them (oldest are dropped).
pub const MAX_PENDING_SOUNDS: usize = 512;

/// One sound to start.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundStart {
    /// The file name relative to `sound/` as the precache table has it, `!N` for sentence `N`,
    /// or a name chosen by the client itself. `None` when the server named an index that was
    /// never precached.
    pub name: Option<String>,
    /// The precache index the server sent; 0 for a sound the client picked by name.
    pub index: u16,
    pub entity: u16,
    pub channel: u8,
    pub origin: Vec3,
    /// 0.0 to 1.0.
    pub volume: f32,
    /// As the mixer wants it (`byte / 64` on the wire).
    pub attenuation: f32,
    /// `PITCH_NORM` (100) is unchanged; never below 2.
    pub pitch: u8,
    /// `SND_FL_*` bits as sent, minus the large-index bit, which only tells the reader.
    pub flags: u16,
}

impl SoundStart {
    pub fn is_sentence(&self) -> bool {
        u32::from(self.flags) & SND_FL_SENTENCE != 0
    }

    /// `SND_STOP`: stop the sound already playing on this entity and channel instead of
    /// starting one.
    pub fn is_stop(&self) -> bool {
        u32::from(self.flags) & SND_FL_STOP != 0
    }

    /// `SND_CHANGE_VOL` or `SND_CHANGE_PITCH`: alter a sound that is playing; when it is not,
    /// start it.
    pub fn is_alter(&self) -> bool {
        u32::from(self.flags) & (SND_FL_CHANGE_VOL | SND_FL_CHANGE_PITCH) != 0
    }

    /// A sound the client starts itself, by file name.
    pub fn by_name(
        name: &str,
        entity: u16,
        channel: u8,
        origin: Vec3,
        volume: f32,
        attenuation: f32,
        pitch: u8,
    ) -> Self {
        Self {
            name: Some(name.to_string()),
            index: 0,
            entity,
            channel,
            origin,
            volume,
            attenuation,
            pitch: norm_pitch(pitch),
            flags: 0,
        }
    }
}

/// What the mixer is asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundCommand {
    /// `S_StartSound`: a dynamic sound on an entity channel.
    Start(SoundStart),
    /// `S_AmbientSound`: a sound for the static channels (`CHAN_STATIC` of `svc_sound`, and
    /// `svc_spawnstaticsound`).
    Ambient(SoundStart),
    /// `S_StopSound`: whatever plays on the entity's channel.
    Stop { entity: u16, channel: u8 },
    /// `S_SoundFade`: fade the whole mix out and back in.
    Fade {
        percent: u8,
        hold_time: u8,
        fade_out: u8,
        fade_in: u8,
    },
    /// `svc_roomtype`: switch the client DSP room reverb preset (`room_type`).
    RoomType(u16),
    /// `svc_cdtrack`: start or loop a background music track (`-1`/`0` stops).
    CdTrack { track: i8, loop_track: i8 },
}

/// Incoming voice configuration or payload pushed from `svc_voiceinit` / `svc_voicedata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    Init { codec: String, quality: u8 },
    Data { player_slot: u8, data: Vec<u8> },
}

/// Maximum queued voice events before oldest are dropped.
pub const MAX_PENDING_VOICE: usize = 256;

/// Bounded queue of incoming voice events for the shell's voice decoder.
#[derive(Debug, Clone, Default)]
pub struct VoiceQueue {
    pending: Vec<VoiceEvent>,
}

impl VoiceQueue {
    pub fn push(&mut self, event: VoiceEvent) {
        if self.pending.len() >= MAX_PENDING_VOICE {
            self.pending.remove(0);
        }
        self.pending.push(event);
    }

    pub fn drain(&mut self) -> Vec<VoiceEvent> {
        std::mem::take(&mut self.pending)
    }

    pub fn pending(&self) -> &[VoiceEvent] {
        &self.pending
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

/// The sound commands of the connection, in the order they were asked for.
#[derive(Debug, Clone, Default)]
pub struct SoundQueue {
    pending: Vec<SoundCommand>,
    /// `svc_sound` messages that named a sound index the server never precached.
    pub unknown_sounds: u32,
}

impl SoundQueue {
    /// Takes every command asked for since the last call.
    pub fn drain(&mut self) -> Vec<SoundCommand> {
        std::mem::take(&mut self.pending)
    }

    /// The commands waiting, without taking them.
    pub fn pending(&self) -> &[SoundCommand] {
        &self.pending
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// Queues a command the client generated itself (an event or temp entity sound).
    pub fn push(&mut self, command: SoundCommand) {
        if self.pending.len() >= MAX_PENDING_SOUNDS {
            self.pending.remove(0);
        }
        self.pending.push(command);
    }

    /// Reacts to one parsed svc message; messages that are not about sound are ignored.
    ///
    /// `CL_ParseSoundPacketGS`: a sentence is named `!N`; any other sound is looked up in the
    /// sound precache; `CHAN_STATIC` goes to the ambient path. The origin is the one sent: the
    /// server writes the entity's origin for sounds that follow an entity, and the shell
    /// re-reads the entity's position itself.
    pub fn handle(&mut self, msg: &SvcMessage, precache: &PrecacheTables) {
        match msg {
            SvcMessage::Sound(s) => {
                let flags = s.flags & !(cs_proto48::messages::SND_FL_LARGE_INDEX as u16);
                let sentence = u32::from(flags) & SND_FL_SENTENCE != 0;
                let Some(name) = resolve(precache, s.sound_index, sentence) else {
                    self.unknown_sounds = self.unknown_sounds.saturating_add(1);
                    // A sound to stop is stopped whether or not its file is known: the
                    // entity and channel are all `S_StopSound` needs.
                    if u32::from(flags) & SND_FL_STOP != 0 {
                        self.push(SoundCommand::Stop {
                            entity: s.entity,
                            channel: s.channel,
                        });
                    }
                    return;
                };
                let start = SoundStart {
                    name: Some(name),
                    index: s.sound_index,
                    entity: s.entity,
                    channel: s.channel,
                    origin: s.origin,
                    volume: f32::from(s.volume) / 255.0,
                    attenuation: s.attenuation,
                    pitch: norm_pitch(s.pitch),
                    flags,
                };
                if s.channel == chan::STATIC {
                    self.push(SoundCommand::Ambient(start));
                } else {
                    self.push(SoundCommand::Start(start));
                }
            }
            SvcMessage::SpawnStaticSound(s) => {
                let sentence = u32::from(s.flags) & SND_FL_SENTENCE != 0;
                let Some(name) = resolve(precache, s.sound_index, sentence) else {
                    self.unknown_sounds = self.unknown_sounds.saturating_add(1);
                    return;
                };
                self.push(SoundCommand::Ambient(SoundStart {
                    name: Some(name),
                    index: s.sound_index,
                    entity: s.entity,
                    channel: chan::STATIC,
                    origin: s.origin,
                    volume: f32::from(s.volume) / 255.0,
                    attenuation: s.attenuation(),
                    pitch: norm_pitch(s.pitch),
                    flags: u16::from(s.flags),
                }));
            }
            // `svc_goldsrc_stopsound`: `S_StopSound( param1 >> 3, param1 & 7 )`.
            SvcMessage::StopSound(word) => self.push(SoundCommand::Stop {
                entity: word >> 3,
                channel: (word & 7) as u8,
            }),
            SvcMessage::SoundFade(f) => self.push(SoundCommand::Fade {
                percent: f.percent,
                hold_time: f.hold_time,
                fade_out: f.fade_out,
                fade_in: f.fade_in,
            }),
            SvcMessage::RoomType(rt) => self.push(SoundCommand::RoomType(*rt)),
            SvcMessage::CdTrack { track, loop_track } => self.push(SoundCommand::CdTrack {
                track: *track,
                loop_track: *loop_track,
            }),
            _ => {}
        }
    }
}

/// `S_StartSound`: `if( pitch <= 1 ) pitch = PITCH_NORM;`.
fn norm_pitch(pitch: u8) -> u8 {
    if pitch <= 1 { PITCH_NORM } else { pitch }
}

/// The name of the sound the server means: `!N` for a sentence, else the precache entry.
fn resolve(precache: &PrecacheTables, index: u16, sentence: bool) -> Option<String> {
    if sentence {
        Some(format!("!{index}"))
    } else {
        precache.sound(index).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::messages::{
        ResourceEntry, SND_FL_ATTENUATION, SND_FL_LARGE_INDEX, SND_FL_PITCH, SND_FL_VOLUME,
        SoundFadeMsg, SoundMsg, SpawnStaticSoundMsg,
    };

    fn tables() -> PrecacheTables {
        let mut t = PrecacheTables::default();
        t.add(&[ResourceEntry {
            res_type: crate::resources::res_type::SOUND,
            filename: "weapons/ak47-1.wav".into(),
            index: 7,
            download_size: 0,
            flags: 0,
            md5: None,
            reserved: None,
        }]);
        t
    }

    fn sound(flags: u32, channel: u8, index: u16) -> SvcMessage {
        SvcMessage::Sound(SoundMsg {
            flags: flags as u16,
            volume: 255,
            attenuation: 0.8,
            channel,
            entity: 3,
            sound_index: index,
            origin: Vec3::new(1.0, 2.0, 3.0),
            pitch: 100,
        })
    }

    #[test]
    fn a_sound_starts_by_its_precache_name() {
        let mut q = SoundQueue::default();
        q.handle(&sound(SND_FL_VOLUME | SND_FL_ATTENUATION | SND_FL_PITCH, chan::WEAPON, 7), &tables());
        let SoundCommand::Start(s) = &q.pending()[0] else {
            panic!("{:?}", q.pending());
        };
        assert_eq!(s.name.as_deref(), Some("weapons/ak47-1.wav"));
        assert_eq!((s.entity, s.channel, s.pitch), (3, chan::WEAPON, 100));
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.origin, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn static_channel_goes_to_the_ambient_path() {
        let mut q = SoundQueue::default();
        q.handle(&sound(0, chan::STATIC, 7), &tables());
        assert!(matches!(q.pending()[0], SoundCommand::Ambient(_)));
    }

    #[test]
    fn a_sentence_is_named_by_its_number() {
        let mut q = SoundQueue::default();
        q.handle(&sound(SND_FL_SENTENCE, chan::VOICE, 12), &tables());
        let SoundCommand::Start(s) = &q.pending()[0] else {
            panic!();
        };
        assert_eq!(s.name.as_deref(), Some("!12"));
        assert!(s.is_sentence());
    }

    #[test]
    fn the_large_index_bit_is_not_kept() {
        let mut q = SoundQueue::default();
        q.handle(&sound(SND_FL_LARGE_INDEX, chan::AUTO, 7), &tables());
        let SoundCommand::Start(s) = &q.pending()[0] else {
            panic!();
        };
        assert_eq!(s.flags, 0);
    }

    #[test]
    fn an_unprecached_stop_still_stops() {
        let mut q = SoundQueue::default();
        q.handle(&sound(SND_FL_STOP, chan::ITEM, 99), &tables());
        assert_eq!(
            q.pending(),
            &[SoundCommand::Stop {
                entity: 3,
                channel: chan::ITEM
            }]
        );
    }

    #[test]
    fn a_client_sound_of_pitch_one_plays_normal() {
        let s = SoundStart::by_name("a.wav", 0, chan::AUTO, Vec3::ZERO, 1.0, 1.0, 1);
        assert_eq!(s.pitch, PITCH_NORM);
    }

    #[test]
    fn an_unprecached_index_is_counted_not_played() {
        let mut q = SoundQueue::default();
        q.handle(&sound(0, chan::AUTO, 99), &tables());
        assert!(q.pending().is_empty());
        assert_eq!(q.unknown_sounds, 1);
    }

    #[test]
    fn a_pitch_of_one_or_less_plays_normal() {
        let mut q = SoundQueue::default();
        let SvcMessage::Sound(mut s) = sound(SND_FL_PITCH, chan::AUTO, 7) else {
            unreachable!()
        };
        s.pitch = 1;
        q.handle(&SvcMessage::Sound(s), &tables());
        let SoundCommand::Start(s) = &q.pending()[0] else {
            panic!();
        };
        assert_eq!(s.pitch, PITCH_NORM);
    }

    #[test]
    fn stop_sound_splits_entity_and_channel() {
        let mut q = SoundQueue::default();
        q.handle(&SvcMessage::StopSound((5 << 3) | 4), &tables());
        assert_eq!(
            q.drain(),
            vec![SoundCommand::Stop {
                entity: 5,
                channel: 4
            }]
        );
        assert!(q.pending().is_empty());
    }

    #[test]
    fn static_sound_and_fade_are_commands() {
        let mut q = SoundQueue::default();
        q.handle(
            &SvcMessage::SpawnStaticSound(SpawnStaticSoundMsg {
                origin: Vec3::new(4.0, 5.0, 6.0),
                sound_index: 7,
                volume: 128,
                attenuation: 64,
                entity: 0,
                pitch: 100,
                flags: 0,
            }),
            &tables(),
        );
        q.handle(
            &SvcMessage::SoundFade(SoundFadeMsg {
                percent: 100,
                hold_time: 2,
                fade_out: 3,
                fade_in: 4,
            }),
            &tables(),
        );
        let cmds = q.drain();
        let SoundCommand::Ambient(a) = &cmds[0] else {
            panic!();
        };
        assert_eq!(a.attenuation, 1.0);
        assert_eq!(a.channel, chan::STATIC);
        assert!((a.volume - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(
            cmds[1],
            SoundCommand::Fade {
                percent: 100,
                hold_time: 2,
                fade_out: 3,
                fade_in: 4
            }
        );
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut q = SoundQueue::default();
        for i in 0..MAX_PENDING_SOUNDS + 10 {
            q.push(SoundCommand::Stop {
                entity: i as u16,
                channel: 0,
            });
        }
        assert_eq!(q.pending().len(), MAX_PENDING_SOUNDS);
        assert_eq!(
            q.pending()[0],
            SoundCommand::Stop {
                entity: 10,
                channel: 0
            }
        );
    }

    #[test]
    fn roomtype_cdtrack_and_voice_queue_work() {
        let mut q = SoundQueue::default();
        q.handle(&SvcMessage::RoomType(17), &tables());
        q.handle(
            &SvcMessage::CdTrack {
                track: 4,
                loop_track: 4,
            },
            &tables(),
        );
        let cmds = q.drain();
        assert_eq!(
            cmds,
            vec![
                SoundCommand::RoomType(17),
                SoundCommand::CdTrack {
                    track: 4,
                    loop_track: 4,
                },
            ]
        );

        let mut vq = VoiceQueue::default();
        vq.push(VoiceEvent::Init {
            codec: "voice_opus".into(),
            quality: 3,
        });
        vq.push(VoiceEvent::Data {
            player_slot: 1,
            data: vec![1, 2, 3],
        });
        assert_eq!(vq.pending().len(), 2);
        assert_eq!(vq.drain().len(), 2);
        assert!(vq.pending().is_empty());
    }
}
