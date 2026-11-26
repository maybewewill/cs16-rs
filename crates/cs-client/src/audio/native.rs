//! Native `rodio 0.23` + `cpal 0.18` audio backend wrapping `GoldSrcMixer`,
//! `MusicController`, `VoiceManager`, and live microphone capture.
//!
//! `GoldSrcMixer` lives exclusively inside [`SharedMixerSource`] on the OS real-time
//! audio thread. The main thread pre-decodes `.wav` files into `Arc<DecodedWav>` inside
//! `AudioEngine::wav_cache` and delivers commands over a bounded non-blocking
//! `std::sync::mpsc::sync_channel` (`try_send` / `try_iter`) — zero locks and zero VFS/disk
//! I/O on the real-time audio callback thread.

use std::collections::HashMap;
use std::io::Cursor;
use std::num::NonZero;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cs_clientcore::{PrecacheTables, SoundCommand, SoundStart, VoiceEvent};
use cs_vfs::Vfs;
use glam::Vec3;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use tracing::{info, warn};

use crate::audio::impacts;
use crate::audio::mixer::{
    GoldSrcMixer, ListenerState, MIX_SAMPLE_RATE, PredecodedVoxWord, SND_FL_CHANGE_PITCH,
    SND_FL_CHANGE_VOL, normalize_sound_path, trim_wav_percent,
};
use crate::audio::music::{MusicCommand, MusicController};
use crate::audio::sentences::{SentenceTable, VoxWord};
use crate::audio::voice::VoiceManager;
use crate::audio::wav::{DecodedWav, decode_wav};

/// Bounded capacity of the lock-free render-thread -> audio-thread command queue.
pub const AUDIO_CMD_QUEUE_CAPACITY: usize = 1024;

/// Maximum individual `.wav` size (bytes) eagerly decoded during signon precache warmup.
/// Larger ambient tracks decode lazily on first play to keep signon snappy and bounded.
const MAX_WARMUP_WAV_BYTES: usize = 512 * 1024;

/// Common client-side UI, footstep, reload, and bullet-ricochet WAVs warmed up on signon
/// alongside the server's `PrecacheTables` sound list so round 1 has zero decode hitches.
const COMMON_CLIENT_WARMUP_WAVS: &[&str] = &[
    "ui/buttonclick.wav",
    "ui/buttonclickrelease.wav",
    "ui/buttonrollover.wav",
    "player/pl_step1.wav",
    "player/pl_step2.wav",
    "player/pl_step3.wav",
    "player/pl_step4.wav",
    "weapons/generic_reload.wav",
    "weapons/generic_shot_reload.wav",
    "weapons/ric1.wav",
    "weapons/ric2.wav",
    "weapons/ric3.wav",
    "weapons/ric4.wav",
    "weapons/ric5.wav",
];

enum AudioCommand {
    SetParams {
        master_volume: f32,
        suit_volume: f32,
        room_off: bool,
        occlusion_factor: f32,
        lerping: bool,
        hisound: bool,
    },
    SetTimescale(f32),
    UpdateListener {
        listener: ListenerState,
        /// Sorted `(entity_index, origin)` pairs for `O(log N)` lookup on the audio thread.
        entity_positions: Vec<(u16, Vec3)>,
    },
    StartSound {
        start: SoundStart,
        is_static: bool,
        wav: Option<Arc<DecodedWav>>,
    },
    StartVox {
        start: SoundStart,
        is_static: bool,
        words: Vec<PredecodedVoxWord>,
    },
    StopChannel {
        entity: u16,
        channel: u8,
    },
    StopAll,
    TriggerFade {
        percent: u8,
        hold_time: u8,
        fade_out: u8,
        fade_in: u8,
    },
    SetRoomType(u16),
    PushVoicePcm {
        slot: u8,
        sample_rate: u32,
        samples: Vec<f32>,
        gain: f32,
    },
    FormatSoundList(mpsc::SyncSender<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MixerParamsKey {
    master_bits: u32,
    suit_bits: u32,
    room_off: bool,
    occ_bits: u32,
    lerping: bool,
    hisound: bool,
}

impl MixerParamsKey {
    fn new(
        master_volume: f32,
        suit_volume: f32,
        room_off: bool,
        occlusion_factor: f32,
        lerping: bool,
        hisound: bool,
    ) -> Self {
        Self {
            master_bits: master_volume.to_bits(),
            suit_bits: suit_volume.to_bits(),
            room_off,
            occ_bits: occlusion_factor.to_bits(),
            lerping,
            hisound,
        }
    }
}

struct SharedMixerSource {
    mixer: GoldSrcMixer,
    cmd_rx: mpsc::Receiver<AudioCommand>,
    buf: [f32; 512],
    pos: usize,
}

impl SharedMixerSource {
    fn new(mixer: GoldSrcMixer, cmd_rx: mpsc::Receiver<AudioCommand>) -> Self {
        Self {
            mixer,
            cmd_rx,
            buf: [0.0; 512],
            pos: 512,
        }
    }

    /// Drains all queued commands non-blockingly (`try_iter`) on the audio thread.
    fn drain_commands(&mut self) -> usize {
        let mut count = 0usize;
        for cmd in self.cmd_rx.try_iter() {
            Self::apply_command(&mut self.mixer, cmd);
            count += 1;
        }
        count
    }

    fn apply_command(mixer: &mut GoldSrcMixer, cmd: AudioCommand) {
        match cmd {
            AudioCommand::SetParams {
                master_volume,
                suit_volume,
                room_off,
                occlusion_factor,
                lerping,
                hisound,
            } => {
                mixer.set_volumes(master_volume, suit_volume);
                mixer.set_room_off(room_off);
                mixer.set_occlusion_factor(occlusion_factor);
                mixer.set_resampling(lerping, hisound);
            }
            AudioCommand::SetTimescale(ts) => mixer.set_timescale(ts),
            AudioCommand::UpdateListener {
                listener,
                entity_positions,
            } => mixer.update_listener_and_sorted_entities(listener, &entity_positions),
            AudioCommand::StartSound {
                start,
                is_static,
                wav,
            } => mixer.start_predecoded_sound(wav, &start, is_static),
            AudioCommand::StartVox {
                start,
                is_static,
                words,
            } => mixer.start_predecoded_vox(words, &start, is_static),
            AudioCommand::StopChannel { entity, channel } => {
                mixer.stop_channel(entity, channel);
            }
            AudioCommand::StopAll => mixer.stop_all(),
            AudioCommand::TriggerFade {
                percent,
                hold_time,
                fade_out,
                fade_in,
            } => mixer.trigger_fade(percent, hold_time, fade_out, fade_in),
            AudioCommand::SetRoomType(rt) => mixer.set_room_type(rt),
            AudioCommand::PushVoicePcm {
                slot,
                sample_rate,
                samples,
                gain,
            } => mixer.push_voice_pcm(slot, sample_rate, &samples, gain),
            AudioCommand::FormatSoundList(reply_tx) => {
                let _ = reply_tx.try_send(mixer.format_sound_list());
            }
        }
    }
}

impl Iterator for SharedMixerSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.buf.len() {
            self.drain_commands();
            self.mixer.mix_frames(&mut self.buf);
            self.pos = 0;
        }
        let sample = self.buf[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

impl Source for SharedMixerSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(2).unwrap()
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(MIX_SAMPLE_RATE).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Detached audio-thread consumer returned by [`AudioEngine::new_offline`], driving the exact
/// same lock-free [`SharedMixerSource`] that `rodio::Player` pulls on the OS audio thread.
pub struct OfflineAudioMixer {
    source: SharedMixerSource,
}

impl OfflineAudioMixer {
    /// Drains all currently queued commands without blocking.
    pub fn drain_commands(&mut self) -> usize {
        self.source.drain_commands()
    }

    /// Pulls `out_stereo_interleaved.len()` interleaved stereo samples via `SharedMixerSource`.
    pub fn mix_samples(&mut self, out_stereo_interleaved: &mut [f32]) {
        for sample in out_stereo_interleaved.iter_mut() {
            *sample = self.source.next().unwrap_or(0.0);
        }
    }

    pub fn active_channel_counts(&self) -> (usize, usize) {
        self.source.mixer.active_channel_counts()
    }

    pub fn room_type(&self) -> usize {
        self.source.mixer.room_type()
    }

    pub fn format_sound_list(&self) -> String {
        self.source.mixer.format_sound_list()
    }
}

struct MicCapture {
    _stream: cpal::Stream,
    sample_rate: u32,
    rx: mpsc::Receiver<Vec<f32>>,
}

impl MicCapture {
    fn try_open() -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_input_device()?;
        let config = device.default_input_config().ok()?;
        let sample_rate = config.sample_rate();
        let channels = usize::from(config.channels().max(1));
        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(64);
        let stream_cfg: cpal::StreamConfig = config.into();
        let stream = device
            .build_input_stream(
                stream_cfg,
                move |data: &[f32], _| {
                    let mut mono_buf = Vec::with_capacity(data.len() / channels);
                    for frame in data.chunks(channels) {
                        let mono = frame.iter().sum::<f32>() / (frame.len() as f32);
                        mono_buf.push(mono);
                    }
                    if !mono_buf.is_empty() {
                        let _ = tx.try_send(mono_buf);
                    }
                },
                |err| warn!(%err, "voice mic input stream error"),
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(Self {
            _stream: stream,
            sample_rate,
            rx,
        })
    }

    fn drain(&self) -> Vec<f32> {
        let max_samples = (self.sample_rate as usize) * 2;
        let mut out = Vec::new();
        for chunk in self.rx.try_iter() {
            if out.len().saturating_add(chunk.len()) > max_samples {
                out.clear();
            }
            out.extend_from_slice(&chunk);
        }
        out
    }
}

pub struct AudioEngine {
    sink: Option<MixerDeviceSink>,
    _mixer_player: Option<Player>,
    cmd_tx: mpsc::SyncSender<AudioCommand>,
    wav_cache: HashMap<String, Option<Arc<DecodedWav>>>,
    sentences: SentenceTable,
    last_mixer_params: Option<MixerParamsKey>,
    music_player: Option<Player>,
    music: MusicController,
    voice: VoiceManager,
    mic: Option<MicCapture>,
    mic_attempted: bool,
    recording: bool,
    sentences_loaded: bool,
    dropped_commands: u64,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<AudioCommand>(AUDIO_CMD_QUEUE_CAPACITY);
        let source = SharedMixerSource::new(GoldSrcMixer::new(), cmd_rx);
        let (sink, mixer_player) = match DeviceSinkBuilder::from_default_device()
            .and_then(|b| b.open_stream())
        {
            Ok(mut s) => {
                s.log_on_drop(false);
                let player = Player::connect_new(s.mixer());
                player.append(source);
                info!("Initialized lock-free GoldSrc audio mixer stream");
                (Some(s), Some(player))
            }
            Err(err) => {
                drop(source);
                warn!(%err, "Audio output unavailable; running muted");
                (None, None)
            }
        };
        Self {
            sink,
            _mixer_player: mixer_player,
            cmd_tx,
            wav_cache: HashMap::new(),
            sentences: SentenceTable::default(),
            last_mixer_params: None,
            music_player: None,
            music: MusicController::new(),
            voice: VoiceManager::new(),
            mic: None,
            mic_attempted: false,
            recording: false,
            sentences_loaded: false,
            dropped_commands: 0,
        }
    }

    /// Creates a deterministic `(AudioEngine, OfflineAudioMixer)` pair with no OS audio device
    /// for headless verification and unit/integration testing.
    pub fn new_offline() -> (Self, OfflineAudioMixer) {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<AudioCommand>(AUDIO_CMD_QUEUE_CAPACITY);
        let offline = OfflineAudioMixer {
            source: SharedMixerSource::new(GoldSrcMixer::new(), cmd_rx),
        };
        let engine = Self {
            sink: None,
            _mixer_player: None,
            cmd_tx,
            wav_cache: HashMap::new(),
            sentences: SentenceTable::default(),
            last_mixer_params: None,
            music_player: None,
            music: MusicController::new(),
            voice: VoiceManager::new(),
            mic: None,
            mic_attempted: false,
            recording: false,
            sentences_loaded: false,
            dropped_commands: 0,
        };
        (engine, offline)
    }

    fn try_send(&mut self, cmd: AudioCommand) -> bool {
        match self.cmd_tx.try_send(cmd) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.dropped_commands = self.dropped_commands.saturating_add(1);
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }

    /// Number of commands dropped because the bounded lock-free audio queue was full.
    pub fn dropped_commands(&self) -> u64 {
        self.dropped_commands
    }

    pub fn load_sentences(&mut self, vfs: &Vfs) {
        if self.sentences_loaded {
            return;
        }
        if let Ok(text) = vfs.read_string_lossy("sound/sentences.txt") {
            self.sentences = SentenceTable::parse(&text);
            self.sentences_loaded = true;
        }
    }

    pub fn set_sentences(&mut self, table: SentenceTable) {
        self.sentences = table;
        self.sentences_loaded = true;
    }

    /// Pre-decodes raw WAV bytes into `wav_cache` on the main thread, returning `true` on success.
    pub fn insert_cached_wav(&mut self, path: &str, bytes: &[u8]) -> bool {
        let key = normalize_sound_path(path);
        let decoded = decode_wav(bytes).ok().map(Arc::new);
        let ok = decoded.is_some();
        self.wav_cache.insert(key, decoded);
        ok
    }

    /// Returns a cached `Arc<DecodedWav>` or decodes it from `vfs` on the main thread.
    pub fn load_or_decode_wav(&mut self, vfs: Option<&Vfs>, path: &str) -> Option<Arc<DecodedWav>> {
        let key = normalize_sound_path(path);
        if let Some(cached) = self.wav_cache.get(&key) {
            return cached.clone();
        }
        let v = vfs?;
        let decoded = v
            .read_bytes(&key)
            .ok()
            .and_then(|b| decode_wav(&b).ok())
            .map(Arc::new);
        self.wav_cache.insert(key, decoded.clone());
        decoded
    }

    pub fn is_wav_cached(&self, path: &str) -> bool {
        self.wav_cache
            .get(&normalize_sound_path(path))
            .is_some_and(Option::is_some)
    }

    pub fn cached_wav_count(&self) -> usize {
        self.wav_cache.values().filter(|v| v.is_some()).count()
    }

    /// Eagerly decodes common weapon, footstep, player, item, debris, radio, and UI WAVs
    /// from `precache.sounds` (plus core client-side UI/footstep/reload WAVs) into `wav_cache`
    /// on signon so Round 1's first shot has 0.00 ms decode hitch.
    ///
    /// Returns the number of newly decoded WAVs inserted into `wav_cache`.
    pub fn warmup_precache(&mut self, vfs: &Vfs, precache: &PrecacheTables) -> usize {
        let mut warmed = 0usize;
        let candidates = precache
            .sounds
            .values()
            .map(|e| e.name.as_str())
            .chain(COMMON_CLIENT_WARMUP_WAVS.iter().copied());

        for raw in candidates {
            if !is_warmup_candidate(raw) {
                continue;
            }
            let key = normalize_sound_path(raw);
            if self.wav_cache.contains_key(&key) {
                continue;
            }
            let Ok(bytes) = vfs.read_bytes(&key) else {
                continue;
            };
            if bytes.len() > MAX_WARMUP_WAV_BYTES {
                continue;
            }
            if let Ok(decoded) = decode_wav(&bytes) {
                self.wav_cache.insert(key, Some(Arc::new(decoded)));
                warmed += 1;
            }
        }
        warmed
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_params(
        &mut self,
        master_volume: f32,
        suit_volume: f32,
        room_off: bool,
        occlusion_factor: f32,
        lerping: bool,
        hisound: bool,
        mp3_volume: f32,
        bgm_enabled: bool,
        mp3_fade_time: f32,
    ) {
        let key = MixerParamsKey::new(
            master_volume,
            suit_volume,
            room_off,
            occlusion_factor,
            lerping,
            hisound,
        );
        if self.last_mixer_params != Some(key)
            && self.try_send(AudioCommand::SetParams {
                master_volume,
                suit_volume,
                room_off,
                occlusion_factor,
                lerping,
                hisound,
            })
        {
            self.last_mixer_params = Some(key);
        }
        self.music.set_params(mp3_volume, bgm_enabled, mp3_fade_time);
    }

    pub fn set_timescale(&mut self, timescale: f32) {
        let _ = self.try_send(AudioCommand::SetTimescale(timescale));
    }

    pub fn format_sound_list(&self) -> String {
        if self.sink.is_none() {
            return format!(
                "Audio output unavailable (muted); cached WAVs: {}\n",
                self.cached_wav_count()
            );
        }
        let (reply_tx, reply_rx) = mpsc::sync_channel::<String>(1);
        if self
            .cmd_tx
            .try_send(AudioCommand::FormatSoundList(reply_tx))
            .is_err()
        {
            return "Audio mixer queue busy\n".to_string();
        }
        reply_rx
            .recv_timeout(Duration::from_millis(50))
            .unwrap_or_else(|_| "Audio mixer busy\n".to_string())
    }

    pub fn play_ui_click(&mut self, vfs: Option<&Vfs>, master_volume: f32) {
        self.play_2d(vfs, "UI/buttonclick.wav", 0.55 * master_volume);
    }

    pub fn play_ui_sound(&mut self, vfs: Option<&Vfs>, path: &str) {
        self.play_2d(vfs, path, 0.7);
    }

    pub fn sync_menu_music(
        &mut self,
        vfs: Option<&Vfs>,
        should_play: bool,
        mp3_vol: f32,
        bgm_enabled: bool,
        fade_time: f32,
        dt: f32,
    ) {
        self.music.set_params(mp3_vol, bgm_enabled, fade_time);
        let menu_path = vfs.and_then(|v| {
            ["media/gamestartup.mp3", "media/Half-Life01.mp3"]
                .into_iter()
                .find(|p| v.exists(p))
        });
        self.music.sync_menu_music(should_play, menu_path);
        self.step_music(vfs, dt);
    }

    pub fn handle_mp3_command(&mut self, args: &[String]) {
        self.music.handle_mp3_command(args);
    }

    pub fn handle_cd_command(&mut self, args: &[String]) {
        self.music.handle_cd_command(args);
    }

    pub fn step_music(&mut self, vfs: Option<&Vfs>, dt: f32) {
        for cmd in self.music.step(dt) {
            match cmd {
                MusicCommand::Play {
                    path,
                    looping,
                    volume,
                } => {
                    if let Some(p) = self.music_player.take() {
                        p.stop();
                    }
                    let (Some(sink), Some(v)) = (&self.sink, vfs) else {
                        continue;
                    };
                    if let Ok(bytes) = v.read_bytes(&path)
                        && let Ok(decoder) = Decoder::new(Cursor::new(SharedBytes(Arc::from(bytes))))
                    {
                        let player = Player::connect_new(sink.mixer());
                        player.set_volume(volume.clamp(0.0, 1.0));
                        if looping {
                            player.append(decoder.repeat_infinite());
                        } else {
                            player.append(decoder);
                        }
                        self.music_player = Some(player);
                    }
                }
                MusicCommand::SetVolume(vol) => {
                    if let Some(ref p) = self.music_player {
                        p.set_volume(vol.clamp(0.0, 1.0));
                    }
                }
                MusicCommand::Stop => {
                    if let Some(p) = self.music_player.take() {
                        p.stop();
                    }
                }
            }
        }
    }

    pub fn update_listener(
        &mut self,
        listener: ListenerState,
        entity_positions: &HashMap<u16, Vec3>,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        let _ = occlusion_fn;
        let mut sorted: Vec<(u16, Vec3)> = entity_positions
            .iter()
            .map(|(&id, &pos)| (id, pos))
            .collect();
        sorted.sort_unstable_by_key(|&(id, _)| id);
        let _ = self.try_send(AudioCommand::UpdateListener {
            listener,
            entity_positions: sorted,
        });
    }

    /// Lock-free listener update reading from a flat `entity_index -> Option<Vec3>` table
    /// (already strictly sorted by entity index, zero hashing and zero sorting).
    pub fn update_listener_indexed(
        &mut self,
        listener: ListenerState,
        entity_positions: &[Option<Vec3>],
    ) {
        let mut sorted = Vec::new();
        for (idx, pos) in entity_positions.iter().enumerate() {
            if let (Some(origin), Ok(id)) = (*pos, u16::try_from(idx)) {
                sorted.push((id, origin));
            }
        }
        let _ = self.try_send(AudioCommand::UpdateListener {
            listener,
            entity_positions: sorted,
        });
    }

    fn predecode_vox_words(&mut self, vfs: Option<&Vfs>, words: &[VoxWord]) -> Vec<PredecodedVoxWord> {
        let mut out = Vec::with_capacity(words.len());
        for w in words {
            let wav = self
                .load_or_decode_wav(vfs, &w.path)
                .map(|d| trim_wav_percent(d, w.start_pct, w.end_pct));
            let pause_samples = (w.pause_sec.max(0.0) * (MIX_SAMPLE_RATE as f32)).round() as usize;
            out.push(PredecodedVoxWord {
                wav,
                volume: w.volume,
                pitch: w.pitch,
                pause_samples,
            });
        }
        out
    }

    fn dispatch_sound_start(&mut self, vfs: Option<&Vfs>, s: &SoundStart, is_static: bool) {
        if s.is_stop() {
            let _ = self.try_send(AudioCommand::StopChannel {
                entity: s.entity,
                channel: s.channel,
            });
            return;
        }
        let is_alter = (s.flags & (SND_FL_CHANGE_VOL | SND_FL_CHANGE_PITCH)) != 0;
        let Some(name) = s.name.as_deref() else {
            if is_alter {
                let _ = self.try_send(AudioCommand::StartSound {
                    start: s.clone(),
                    is_static,
                    wav: None,
                });
            }
            return;
        };
        if s.is_sentence() || name.starts_with(['!', '#', '%']) {
            if is_alter {
                let _ = self.try_send(AudioCommand::StartSound {
                    start: s.clone(),
                    is_static,
                    wav: None,
                });
                return;
            }
            if let Some(words) = self.sentences.resolve(name) {
                let predecoded = self.predecode_vox_words(vfs, &words);
                let _ = self.try_send(AudioCommand::StartVox {
                    start: s.clone(),
                    is_static,
                    words: predecoded,
                });
            }
            return;
        }
        let wav = self.load_or_decode_wav(vfs, name);
        if wav.is_some() || is_alter {
            let _ = self.try_send(AudioCommand::StartSound {
                start: s.clone(),
                is_static,
                wav,
            });
        }
    }

    pub fn handle_sound_command(
        &mut self,
        vfs: Option<&Vfs>,
        cmd: &SoundCommand,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        let _ = occlusion_fn;
        match cmd {
            SoundCommand::CdTrack { track, loop_track } => {
                self.music.handle_svc_cdtrack(*track, *loop_track);
            }
            SoundCommand::Stop { entity, channel } => {
                let _ = self.try_send(AudioCommand::StopChannel {
                    entity: *entity,
                    channel: *channel,
                });
            }
            SoundCommand::Fade {
                percent,
                hold_time,
                fade_out,
                fade_in,
            } => {
                let _ = self.try_send(AudioCommand::TriggerFade {
                    percent: *percent,
                    hold_time: *hold_time,
                    fade_out: *fade_out,
                    fade_in: *fade_in,
                });
            }
            SoundCommand::RoomType(rt) => {
                let _ = self.try_send(AudioCommand::SetRoomType(*rt));
            }
            SoundCommand::Start(s) => self.dispatch_sound_start(vfs, s, false),
            SoundCommand::Ambient(s) => self.dispatch_sound_start(vfs, s, true),
        }
    }

    pub fn play_sentence(&mut self, vfs: Option<&Vfs>, spec: &str, volume: f32) {
        self.play_send_audio(vfs, spec, volume, 100);
    }

    /// Dispatch a GoldSrc `SendAudio` message (`client.dll` `FUN_10064bc0`): `"%!"` plays a
    /// sentence at `pitch`, otherwise plays a WAV sample directly via `start_sound`.
    pub fn play_send_audio(&mut self, vfs: Option<&Vfs>, raw_spec: &str, volume: f32, pitch: u8) {
        let spec = if raw_spec.trim().is_empty() {
            "misc/talk.wav"
        } else {
            raw_spec.trim()
        };
        let pitch = pitch.max(2);
        if let Some(rest) = spec.strip_prefix("%!") {
            let tok: String = rest
                .chars()
                .take_while(|c| (' '..='z').contains(c))
                .take(31)
                .collect();
            if tok.is_empty() {
                return;
            }
            let s = SoundStart::by_name(&tok, 0, 2, Vec3::ZERO, volume, 0.0, pitch);
            if let Some(words) = self.sentences.resolve(&tok) {
                let predecoded = self.predecode_vox_words(vfs, &words);
                let _ = self.try_send(AudioCommand::StartVox {
                    start: s,
                    is_static: false,
                    words: predecoded,
                });
            }
        } else {
            let s = SoundStart::by_name(spec, 0, 2, Vec3::ZERO, volume, 0.0, pitch);
            self.dispatch_sound_start(vfs, &s, false);
        }
    }

    /// Dispatch `SendAudio` (`client.dll` `FUN_10064bc0`) and arm `radio_flashes[sender]`
    /// for 22 half-cycles of `0.15s` (`3.3s`) when `sender > 0`.
    #[allow(clippy::too_many_arguments)]
    pub fn play_send_audio_with_radar(
        &mut self,
        vfs: Option<&Vfs>,
        raw_spec: &str,
        volume: f32,
        pitch: u8,
        sender: u8,
        now: f64,
        radio_flashes: &mut HashMap<u8, f64>,
    ) {
        self.play_send_audio(vfs, raw_spec, volume, pitch);
        if sender > 0 {
            radio_flashes.insert(sender, now + cs_clientcore::hud::RADIO_FLASH_DURATION_SECS);
        }
    }

    pub fn play_reload_sound(&mut self, vfs: Option<&Vfs>, volume: u8, kind: u8) {
        let vol = f32::from(volume) / 255.0;
        let path = impacts::reload_sound_path(kind);
        self.play_2d(vfs, path, vol);
    }

    pub fn stop_channel(&mut self, entity: u16, channel: u8) {
        let _ = self.try_send(AudioCommand::StopChannel { entity, channel });
    }

    pub fn stop_all(&mut self) {
        let _ = self.try_send(AudioCommand::StopAll);
    }

    pub fn play_2d(&mut self, vfs: Option<&Vfs>, sound_path: &str, volume: f32) {
        self.play_2d_tracked(vfs, sound_path, volume, None);
    }

    pub fn play_2d_tracked(
        &mut self,
        vfs: Option<&Vfs>,
        sound_path: &str,
        volume: f32,
        target: Option<(u16, u8)>,
    ) {
        let (entity, channel) = target.unwrap_or((0, 0));
        let s = SoundStart::by_name(sound_path, entity, channel, Vec3::ZERO, volume, 0.0, 100);
        self.dispatch_sound_start(vfs, &s, false);
    }

    pub fn play_3d_tracked(
        &mut self,
        vfs: Option<&Vfs>,
        sound_path: &str,
        pos: Vec3,
        _listener_pos: Vec3,
        volume: f32,
        target: Option<(u16, u8)>,
    ) {
        let (entity, channel) = target.unwrap_or((0, 0));
        let s = SoundStart::by_name(sound_path, entity, channel, pos, volume, 0.8, 100);
        self.dispatch_sound_start(vfs, &s, false);
    }

    pub fn set_voice_steam_compat(&mut self, enabled: bool) {
        self.voice.set_steam_compat(enabled);
    }

    pub fn set_voice_recording(&mut self, recording: bool, force_mic_record: bool) {
        self.recording = recording;
        self.voice.set_recording(recording);
        if recording && self.mic.is_none() && (!self.mic_attempted || force_mic_record) {
            self.mic_attempted = true;
            self.mic = MicCapture::try_open();
        }
    }

    pub fn push_voice_event(&mut self, ev: VoiceEvent, now: f64) {
        self.voice.handle_event(ev, now);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn step_voice(
        &mut self,
        now: f64,
        voice_enable: bool,
        voice_scale: f32,
        voice_loopback: bool,
        avg_gain: f32,
        max_gain: f32,
    ) -> Vec<Vec<u8>> {
        self.step_voice_with_filters(
            now,
            voice_enable,
            voice_scale,
            voice_loopback,
            avg_gain,
            max_gain,
            0,
            &[1.0; 32],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn step_voice_with_filters(
        &mut self,
        now: f64,
        voice_enable: bool,
        voice_scale: f32,
        voice_loopback: bool,
        avg_gain: f32,
        max_gain: f32,
        muted_mask: u32,
        player_scales: &[f32; 32],
    ) -> Vec<Vec<u8>> {
        if let Some(ref mic) = self.mic {
            let pcm = mic.drain();
            if self.recording && !pcm.is_empty() {
                self.voice.push_mic_samples(
                    &pcm,
                    mic.sample_rate,
                    now,
                    voice_enable,
                    voice_loopback,
                    avg_gain,
                    max_gain,
                );
            }
        }
        let decoded = self.voice.drain_decoded(voice_enable);
        for (slot, rate, samples) in decoded {
            let idx = usize::from(slot);
            if idx < 32 && (muted_mask & (1u32 << idx)) != 0 {
                continue;
            }
            let p_scale = player_scales.get(idx).copied().unwrap_or(1.0).clamp(0.0, 2.0);
            let gain = voice_scale * p_scale;
            if gain > 0.0 && !samples.is_empty() {
                let _ = self.try_send(AudioCommand::PushVoicePcm {
                    slot,
                    sample_rate: rate,
                    samples,
                    gain,
                });
            }
        }
        self.voice.take_outgoing()
    }

    pub fn active_speakers(&self, now: f64) -> Vec<u8> {
        self.voice.active_speakers(now)
    }

    pub fn is_local_transmitting(&self, now: f64) -> bool {
        self.voice.is_local_transmitting(now)
    }
}

fn is_warmup_candidate(raw_name: &str) -> bool {
    let trimmed = raw_name.trim();
    if trimmed.is_empty() || trimmed.starts_with(['!', '#', '%']) {
        return false;
    }
    let norm = normalize_sound_path(trimmed);
    if !norm.ends_with(".wav") {
        return false;
    }
    const PREFIXES: &[&str] = &[
        "sound/weapons/",
        "sound/player/",
        "sound/ui/",
        "sound/items/",
        "sound/common/",
        "sound/debris/",
        "sound/radio/",
    ];
    PREFIXES.iter().any(|p| norm.starts_with(p))
}

struct SharedBytes(Arc<[u8]>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use cs_clientcore::PrecacheEntry;
    use cs_vfs::{VfsBackend, VfsError};

    use super::*;
    use crate::audio::wav::encode_test_wav_pcm16;

    fn sine_wav(freq_hz: f32, frames: usize) -> Vec<u8> {
        let pcm: Vec<i16> = (0..frames)
            .map(|i| {
                let t = i as f32 / 22_050.0;
                ((2.0 * std::f32::consts::PI * freq_hz * t).sin() * 0.75 * 32_767.0).round() as i16
            })
            .collect();
        encode_test_wav_pcm16(22_050, 1, &pcm, None, None)
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    }

    #[derive(Debug)]
    struct CountingVfsBackend {
        files: HashMap<String, Vec<u8>>,
        reads: Arc<AtomicUsize>,
    }

    impl VfsBackend for CountingVfsBackend {
        fn list_paths(&self) -> Vec<String> {
            self.files.keys().cloned().collect()
        }

        fn contains(&self, normalized_path: &str) -> bool {
            self.files.contains_key(normalized_path)
        }

        fn read_bytes(&self, normalized_path: &str) -> Result<Vec<u8>, VfsError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.files
                .get(normalized_path)
                .cloned()
                .ok_or_else(|| VfsError::NotFound(normalized_path.to_string()))
        }
    }

    #[test]
    fn send_audio_with_radar_arms_3_3s_flash_for_positive_sender() {
        let (mut engine, _offline) = AudioEngine::new_offline();
        let mut flashes = HashMap::new();
        engine.play_send_audio_with_radar(None, "%!MRAD_GO", 1.0, 100, 4, 5.0, &mut flashes);
        assert_eq!(flashes.get(&4), Some(&(5.0 + 3.3)));

        engine.play_send_audio_with_radar(None, "%!MRAD_ctwin", 1.0, 100, 0, 6.0, &mut flashes);
        assert!(!flashes.contains_key(&0));
    }

    #[test]
    fn lock_free_channel_delivers_commands_and_never_blocks_when_full() {
        let (mut engine, mut offline) = AudioEngine::new_offline();
        let wav = sine_wav(440.0, 4_000);
        assert!(engine.insert_cached_wav("weapons/ak47-1.wav", &wav));

        engine.set_params(1.0, 0.25, true, 0.0, true, true, 0.5, true, 2.0);
        engine.handle_sound_command(None, &SoundCommand::RoomType(14), None);
        engine.update_listener_indexed(
            ListenerState {
                origin: Vec3::ZERO,
                right: Vec3::Y,
                local_entity: Some(1),
                waterlevel: 0,
            },
            &[None, Some(Vec3::ZERO), Some(Vec3::new(0.0, 120.0, 0.0))],
        );
        engine.play_3d_tracked(
            None,
            "weapons/ak47-1.wav",
            Vec3::new(0.0, 120.0, 0.0),
            Vec3::ZERO,
            1.0,
            Some((2, 1)),
        );

        let mut buf = vec![0.0f32; 2048];
        offline.mix_samples(&mut buf);
        assert_eq!(offline.room_type(), 14);
        assert_eq!(offline.active_channel_counts(), (1, 0));
        assert!(rms(&buf) > 0.1, "audio thread must mix pre-decoded WAV PCM");

        // Flood beyond bounded queue capacity without draining: must never block.
        for _ in 0..(AUDIO_CMD_QUEUE_CAPACITY + 64) {
            engine.stop_channel(99, 1);
        }
        assert!(
            engine.dropped_commands() >= 64,
            "overflowing bounded lock-free queue must drop cleanly without blocking"
        );
        assert!(offline.drain_commands() >= AUDIO_CMD_QUEUE_CAPACITY);
    }

    #[test]
    fn predecoded_wav_cache_reuses_arc_with_zero_repeated_vfs_reads() {
        let reads = Arc::new(AtomicUsize::new(0));
        let mut files = HashMap::new();
        files.insert("sound/weapons/ak47-1.wav".to_string(), sine_wav(520.0, 2_000));
        let vfs = Vfs::from_backend(Arc::new(CountingVfsBackend {
            files,
            reads: Arc::clone(&reads),
        }));

        let (mut engine, mut offline) = AudioEngine::new_offline();
        engine.set_params(1.0, 0.25, true, 0.0, true, true, 0.5, true, 2.0);

        let first = engine
            .load_or_decode_wav(Some(&vfs), "weapons/ak47-1.wav")
            .expect("first decode");
        assert_eq!(reads.load(Ordering::Relaxed), 1);

        let second = engine
            .load_or_decode_wav(Some(&vfs), "WEAPONS\\AK47-1.WAV")
            .expect("cache hit");
        assert_eq!(reads.load(Ordering::Relaxed), 1, "cache hit must not touch VFS");
        assert!(Arc::ptr_eq(&first, &second), "cache hit must share identical Arc<DecodedWav>");

        engine.play_2d(Some(&vfs), "weapons/ak47-1.wav", 1.0);
        assert_eq!(reads.load(Ordering::Relaxed), 1, "playing cached WAV must not touch VFS");

        let mut buf = vec![0.0f32; 1024];
        offline.mix_samples(&mut buf);
        assert!(rms(&buf) > 0.1);
    }

    #[test]
    fn signon_precache_warmup_decodes_common_wavs_before_round_one() {
        let reads = Arc::new(AtomicUsize::new(0));
        let mut files = HashMap::new();
        files.insert("sound/weapons/ak47-1.wav".to_string(), sine_wav(440.0, 2_000));
        files.insert("sound/player/pl_step1.wav".to_string(), sine_wav(220.0, 1_500));
        files.insert("sound/ui/buttonclick.wav".to_string(), sine_wav(880.0, 1_000));
        files.insert("sound/ambience/wind_long.wav".to_string(), sine_wav(110.0, 2_000));
        let vfs = Vfs::from_backend(Arc::new(CountingVfsBackend {
            files,
            reads: Arc::clone(&reads),
        }));

        let mut precache = PrecacheTables::default();
        for (idx, name) in [
            (1u16, "weapons/ak47-1.wav"),
            (2, "player/pl_step1.wav"),
            (3, "ambience/wind_long.wav"),
            (4, "!MRAD_LOCKNLOAD"),
        ] {
            precache.sounds.insert(
                idx,
                PrecacheEntry {
                    name: name.to_string(),
                    download_size: 0,
                    flags: 0,
                },
            );
        }

        let (mut engine, mut offline) = AudioEngine::new_offline();
        let warmed = engine.warmup_precache(&vfs, &precache);
        assert_eq!(warmed, 3, "expected ak47, pl_step1, and ui/buttonclick warmed");
        assert!(engine.is_wav_cached("weapons/ak47-1.wav"));
        assert!(engine.is_wav_cached("player/pl_step1.wav"));
        assert!(engine.is_wav_cached("UI/buttonclick.wav"));
        assert!(
            !engine.is_wav_cached("ambience/wind_long.wav"),
            "ambient loops are decoded lazily, not eagerly warmed"
        );

        let reads_after_warmup = reads.load(Ordering::Relaxed);
        // Round 1 first shot + first footstep + UI click perform ZERO VFS reads.
        engine.set_params(1.0, 0.25, true, 0.0, true, true, 0.5, true, 2.0);
        engine.play_2d(Some(&vfs), "weapons/ak47-1.wav", 1.0);
        engine.play_2d(Some(&vfs), "player/pl_step1.wav", 0.8);
        engine.play_ui_click(Some(&vfs), 1.0);
        assert_eq!(
            reads.load(Ordering::Relaxed),
            reads_after_warmup,
            "warmed round-1 sounds must hit wav_cache with 0 VFS reads"
        );
        assert_eq!(
            engine.warmup_precache(&vfs, &precache),
            0,
            "second warmup call must be a no-op for already-cached entries"
        );

        let mut out = vec![0.0f32; 2048];
        offline.mix_samples(&mut out);
        assert!(rms(&out) > 0.1);
    }

    #[test]
    fn vox_sentences_predecode_on_main_thread_and_mix_on_audio_thread() {
        let mut files = HashMap::new();
        files.insert("sound/radio/locknload.wav".to_string(), sine_wav(380.0, 2_000));
        let vfs = Vfs::from_memory(files);

        let (mut engine, mut offline) = AudioEngine::new_offline();
        engine.set_sentences(SentenceTable::parse("MRAD_LOCKNLOAD radio/locknload(v100 s10 e90)"));
        engine.set_params(1.0, 1.0, true, 0.0, true, true, 0.5, true, 2.0);
        engine.play_send_audio(Some(&vfs), "%!MRAD_LOCKNLOAD", 1.0, 100);

        let mut out = vec![0.0f32; 2048];
        offline.mix_samples(&mut out);
        assert!(rms(&out) > 0.1, "pre-decoded VOX sentence must render on audio thread");
    }
}
