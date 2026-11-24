//! Native `rodio 0.23` + `cpal 0.18` audio backend wrapping `GoldSrcMixer`,
//! `MusicController`, `VoiceManager`, and live microphone capture.

use std::collections::HashMap;
use std::io::Cursor;
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cs_clientcore::{SoundCommand, SoundStart, VoiceEvent};
use cs_vfs::Vfs;
use glam::Vec3;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use tracing::{info, warn};

use crate::audio::impacts;
use crate::audio::mixer::{GoldSrcMixer, ListenerState, MIX_SAMPLE_RATE};
use crate::audio::music::{MusicCommand, MusicController};
use crate::audio::sentences::SentenceTable;
use crate::audio::voice::VoiceManager;

struct SharedMixerSource {
    mixer: Arc<Mutex<GoldSrcMixer>>,
    buf: [f32; 512],
    pos: usize,
}

impl SharedMixerSource {
    fn new(mixer: Arc<Mutex<GoldSrcMixer>>) -> Self {
        Self {
            mixer,
            buf: [0.0; 512],
            pos: 512,
        }
    }
}

impl Iterator for SharedMixerSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.buf.len() {
            if let Ok(mut m) = self.mixer.lock() {
                m.mix_frames(&mut self.buf);
            } else {
                self.buf.fill(0.0);
            }
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

struct MicCapture {
    _stream: cpal::Stream,
    sample_rate: u32,
    samples: Arc<Mutex<Vec<f32>>>,
}

impl MicCapture {
    fn try_open() -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_input_device()?;
        let config = device.default_input_config().ok()?;
        let sample_rate = config.sample_rate();
        let channels = usize::from(config.channels().max(1));
        let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let buf_ref = Arc::clone(&samples);
        let stream_cfg: cpal::StreamConfig = config.into();
        let stream = device
            .build_input_stream(
                stream_cfg,
                move |data: &[f32], _| {
                    if let Ok(mut guard) = buf_ref.lock() {
                        if guard.len() > (sample_rate as usize) * 2 {
                            guard.clear();
                        }
                        for frame in data.chunks(channels) {
                            let mono = frame.iter().sum::<f32>() / (frame.len() as f32);
                            guard.push(mono);
                        }
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
            samples,
        })
    }

    fn drain(&self) -> Vec<f32> {
        self.samples
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default()
    }
}

pub struct AudioEngine {
    sink: Option<MixerDeviceSink>,
    _mixer_player: Option<Player>,
    mixer: Arc<Mutex<GoldSrcMixer>>,
    music_player: Option<Player>,
    music: MusicController,
    voice: VoiceManager,
    mic: Option<MicCapture>,
    mic_attempted: bool,
    recording: bool,
    sentences_loaded: bool,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        let mixer = Arc::new(Mutex::new(GoldSrcMixer::new()));
        let (sink, mixer_player) = match DeviceSinkBuilder::from_default_device()
            .and_then(|b| b.open_stream())
        {
            Ok(mut s) => {
                s.log_on_drop(false);
                let player = Player::connect_new(s.mixer());
                player.append(SharedMixerSource::new(Arc::clone(&mixer)));
                info!("Initialized GoldSrc audio mixer stream");
                (Some(s), Some(player))
            }
            Err(err) => {
                warn!(%err, "Audio output unavailable; running muted");
                (None, None)
            }
        };
        Self {
            sink,
            _mixer_player: mixer_player,
            mixer,
            music_player: None,
            music: MusicController::new(),
            voice: VoiceManager::new(),
            mic: None,
            mic_attempted: false,
            recording: false,
            sentences_loaded: false,
        }
    }

    pub fn load_sentences(&mut self, vfs: &Vfs) {
        if self.sentences_loaded {
            return;
        }
        if let Ok(text) = vfs.read_string_lossy("sound/sentences.txt") {
            let table = SentenceTable::parse(&text);
            if let Ok(mut m) = self.mixer.lock() {
                m.set_sentences(table);
            }
            self.sentences_loaded = true;
        }
    }

    pub fn set_params(
        &mut self,
        master_volume: f32,
        suit_volume: f32,
        room_off: bool,
        occlusion_factor: f32,
        mp3_volume: f32,
        bgm_enabled: bool,
        mp3_fade_time: f32,
    ) {
        if let Ok(mut m) = self.mixer.lock() {
            m.set_volumes(master_volume, suit_volume);
            m.set_room_off(room_off);
            m.set_occlusion_factor(occlusion_factor);
        }
        self.music.set_params(mp3_volume, bgm_enabled, mp3_fade_time);
    }

    pub fn play_ui_click(&mut self, vfs: Option<&Vfs>, master_volume: f32) {
        self.play_2d(vfs, "UI/buttonclick.wav", 0.55 * master_volume);
    }

    pub fn sync_menu_music(&mut self, vfs: Option<&Vfs>, should_play: bool, mp3_vol: f32) {
        self.music
            .set_params(mp3_vol, true, self.music_fade_time_or_default());
        let menu_path = vfs.and_then(|v| {
            ["media/gamestartup.mp3", "media/Half-Life01.mp3"]
                .into_iter()
                .find(|p| v.exists(p))
        });
        self.music.sync_menu_music(should_play, menu_path);
        self.step_music(vfs, 0.016);
    }

    fn music_fade_time_or_default(&self) -> f32 {
        2.0
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
        if let Ok(mut m) = self.mixer.lock() {
            m.update_listener_and_entities(listener, entity_positions, occlusion_fn);
        }
    }

    pub fn handle_sound_command(
        &mut self,
        vfs: Option<&Vfs>,
        cmd: &SoundCommand,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        if let SoundCommand::CdTrack { track, loop_track } = cmd {
            self.music.handle_svc_cdtrack(*track, *loop_track);
            return;
        }
        if let Ok(mut m) = self.mixer.lock() {
            m.handle_sound_command(vfs, cmd, occlusion_fn);
        }
    }

    pub fn play_sentence(&mut self, vfs: Option<&Vfs>, spec: &str, volume: f32) {
        let s = SoundStart::by_name(spec, 0, 2, Vec3::ZERO, volume, 0.0, 100);
        if let Ok(mut m) = self.mixer.lock()
            && let Some(words) = m.sentences().resolve(spec)
        {
            m.start_vox_words(vfs, &words, &s, false, None);
        }
    }

    pub fn play_reload_sound(&mut self, vfs: Option<&Vfs>, volume: u8, kind: u8) {
        let vol = f32::from(volume) / 255.0;
        let path = impacts::reload_sound_path(kind);
        self.play_2d(vfs, path, vol);
    }

    pub fn stop_channel(&mut self, entity: u16, channel: u8) {
        if let Ok(mut m) = self.mixer.lock() {
            m.stop_channel(entity, channel);
        }
    }

    pub fn stop_all(&mut self) {
        if let Ok(mut m) = self.mixer.lock() {
            m.stop_all();
        }
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
        if let Ok(mut m) = self.mixer.lock() {
            m.start_sound(vfs, &s, false, None);
        }
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
        if let Ok(mut m) = self.mixer.lock() {
            m.start_sound(vfs, &s, false, None);
        }
    }

    pub fn set_voice_recording(&mut self, recording: bool, force_mic_record: bool) {
        self.recording = recording;
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
        if !decoded.is_empty()
            && let Ok(mut m) = self.mixer.lock()
        {
            for (slot, rate, samples) in decoded {
                m.push_voice_pcm(slot, rate, &samples, voice_scale);
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

struct SharedBytes(Arc<[u8]>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
