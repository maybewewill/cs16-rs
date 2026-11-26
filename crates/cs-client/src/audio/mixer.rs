//! Software GoldSrc stereo audio mixer:
//! - 128 dynamic + 128 static channels + 32 network voice streams
//! - GoldSrc `CHAN_*` replacement, priority eviction, `SND_STOP`, `SND_CHANGE_VOL`/`PITCH`
//! - `ATTN_*` linear distance falloff, listener-relative stereo panning, wall occlusion LPF
//! - Fractional pitch resampling (`50..=255` around `PITCH_NORM = 100`)
//! - Seamless `cue ` / `smpl` WAV loop points and sequential `VoxQueue` sentences
//! - `svc_soundfade` global fade envelope + 29-preset `RoomDsp` + underwater muffling

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cs_clientcore::SoundCommand;
use cs_clientcore::sound::{PITCH_NORM, SoundStart, chan};
use cs_vfs::Vfs;
use glam::Vec3;

use crate::audio::dsp::RoomDsp;
use crate::audio::sentences::{SentenceTable, VoxWord};
use crate::audio::wav::{DecodedWav, decode_wav};

pub const MIX_SAMPLE_RATE: u32 = 44_100;
pub const MAX_DYNAMIC_CHANNELS: usize = 128;
pub const MAX_STATIC_CHANNELS: usize = 128;
pub const MAX_VOICE_SLOTS: usize = 32;
pub const SOUND_NOMINAL_CLIP_DIST: f32 = 1000.0;

pub const SND_FL_STOP: u16 = 1 << 5;
pub const SND_FL_CHANGE_VOL: u16 = 1 << 6;
pub const SND_FL_CHANGE_PITCH: u16 = 1 << 7;

#[derive(Debug, Clone, Copy)]
pub struct ListenerState {
    pub origin: Vec3,
    pub right: Vec3,
    pub local_entity: Option<u16>,
    pub waterlevel: u8,
}

impl Default for ListenerState {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            right: Vec3::Y,
            local_entity: None,
            waterlevel: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SoundFadeState {
    target_percent: f32,
    hold_sec: f32,
    fade_out_sec: f32,
    fade_in_sec: f32,
    elapsed: f32,
    active: bool,
}

impl SoundFadeState {
    pub fn trigger(&mut self, percent: u8, hold_time: u8, fade_out: u8, fade_in: u8) {
        self.target_percent = (f32::from(percent.min(100))) / 100.0;
        self.hold_sec = f32::from(hold_time);
        self.fade_out_sec = f32::from(fade_out);
        self.fade_in_sec = f32::from(fade_in);
        self.elapsed = 0.0;
        self.active = true;
    }

    pub fn step(&mut self, dt: f32) {
        if !self.active {
            return;
        }
        self.elapsed += dt.max(0.0);
        let total = self.fade_out_sec + self.hold_sec + self.fade_in_sec;
        if self.elapsed >= total {
            self.active = false;
        }
    }

    pub fn gain(&self) -> f32 {
        if !self.active {
            return 1.0;
        }
        let min_gain = (1.0 - self.target_percent).clamp(0.0, 1.0);
        if self.elapsed < self.fade_out_sec && self.fade_out_sec > 0.0 {
            let t = self.elapsed / self.fade_out_sec;
            return 1.0 + (min_gain - 1.0) * t;
        }
        let t_hold = self.elapsed - self.fade_out_sec;
        if t_hold < self.hold_sec {
            return min_gain;
        }
        let t_in = t_hold - self.hold_sec;
        if t_in < self.fade_in_sec && self.fade_in_sec > 0.0 {
            let t = t_in / self.fade_in_sec;
            return min_gain + (1.0 - min_gain) * t;
        }
        1.0
    }
}

#[derive(Debug, Clone)]
struct VoxItem {
    wav: Option<Arc<DecodedWav>>,
    volume: f32,
    pitch: u8,
    end_frame: usize,
    pause_samples: usize,
}

/// Pre-decoded VOX word ready for lock-free playback on the audio thread without touching VFS.
#[derive(Debug, Clone)]
pub struct PredecodedVoxWord {
    pub wav: Option<Arc<DecodedWav>>,
    pub volume: f32,
    pub pitch: u8,
    pub pause_samples: usize,
}

/// Trims a decoded WAV to `[start_pct%, end_pct%]` of its frames, sharing the original [`Arc`]
/// with zero allocations when `start_pct == 0 && end_pct >= 100`.
pub fn trim_wav_percent(wav: Arc<DecodedWav>, start_pct: u8, end_pct: u8) -> Arc<DecodedWav> {
    let total_frames = wav.frame_count();
    let start_f = (total_frames * usize::from(start_pct.min(100))) / 100;
    let end_f = ((total_frames * usize::from(end_pct.min(100))) / 100)
        .min(total_frames)
        .max(start_f);
    if start_f == 0 && end_f == total_frames {
        return wav;
    }
    let ch = usize::from(wav.channels.max(1));
    let slice: Vec<f32> = wav.samples[start_f * ch..end_f * ch].to_vec();
    Arc::new(DecodedWav {
        sample_rate: wav.sample_rate,
        channels: wav.channels,
        samples: Arc::from(slice),
        loop_start: None,
        loop_end: None,
    })
}

/// Brush-model sound origin (`hw.dll:FUN_101fd050` / `S_StartDynamicSound`):
/// GoldSrc uses the entity's bounding-box center `origin + (mins + maxs) * 0.5`.
#[inline]
pub fn brush_model_sound_origin(origin: Vec3, mins: Vec3, maxs: Vec3) -> Vec3 {
    origin + (mins + maxs) * 0.5
}

#[derive(Debug, Clone)]
struct ChannelSlot {
    active: bool,
    entity: u16,
    channel: u8,
    is_static: bool,
    is_2d: bool,
    origin: Vec3,
    master_vol: f32,
    attenuation: f32,
    pitch: u8,
    sample_name: String,
    wav: Option<Arc<DecodedWav>>,
    phase: f64,
    left_gain: f32,
    right_gain: f32,
    occluded: bool,
    occ_lp: [f32; 2],
    vox_queue: VecDeque<VoxItem>,
    pause_remaining: usize,
    age: u64,
}

impl Default for ChannelSlot {
    fn default() -> Self {
        Self {
            active: false,
            entity: 0,
            channel: 0,
            is_static: false,
            is_2d: true,
            origin: Vec3::ZERO,
            master_vol: 1.0,
            attenuation: 0.8,
            pitch: PITCH_NORM,
            sample_name: String::new(),
            wav: None,
            phase: 0.0,
            left_gain: 0.0,
            right_gain: 0.0,
            occluded: false,
            occ_lp: [0.0; 2],
            vox_queue: VecDeque::new(),
            pause_remaining: 0,
            age: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct VoiceStream {
    sample_rate: u32,
    queue: VecDeque<f32>,
    phase: f64,
    gain: f32,
}

pub struct GoldSrcMixer {
    wav_cache: HashMap<String, Option<Arc<DecodedWav>>>,
    sentences: SentenceTable,
    dynamic: Vec<ChannelSlot>,
    static_ch: Vec<ChannelSlot>,
    voice_streams: Vec<VoiceStream>,
    dsp: RoomDsp,
    fade: SoundFadeState,
    listener: ListenerState,
    master_volume: f32,
    suit_volume: f32,
    snd_occlusion: bool,
    occlusion_factor: f32,
    lerping: bool,
    hisound: bool,
    timescale: f32,
    next_age: u64,
    fade_sample_acc: usize,
}

impl Default for GoldSrcMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl GoldSrcMixer {
    pub fn new() -> Self {
        Self {
            wav_cache: HashMap::new(),
            sentences: SentenceTable::default(),
            dynamic: vec![ChannelSlot::default(); MAX_DYNAMIC_CHANNELS],
            static_ch: vec![ChannelSlot::default(); MAX_STATIC_CHANNELS],
            voice_streams: vec![VoiceStream::default(); MAX_VOICE_SLOTS],
            dsp: RoomDsp::new(MIX_SAMPLE_RATE),
            fade: SoundFadeState::default(),
            listener: ListenerState::default(),
            master_volume: 0.8,
            suit_volume: 0.25,
            snd_occlusion: false,
            occlusion_factor: 0.0,
            lerping: true,
            hisound: true,
            timescale: 1.0,
            next_age: 1,
            fade_sample_acc: 0,
        }
    }

    pub fn snd_occlusion(&self) -> bool {
        self.snd_occlusion
    }

    pub fn set_snd_occlusion(&mut self, enabled: bool) {
        self.snd_occlusion = enabled;
    }

    pub fn occlusion_factor(&self) -> f32 {
        self.occlusion_factor
    }

    pub fn room_off(&self) -> bool {
        self.dsp.room_off()
    }

    pub fn set_resampling(&mut self, lerping: bool, hisound: bool) {
        self.lerping = lerping;
        self.hisound = hisound;
    }

    pub fn lerping(&self) -> bool {
        self.lerping
    }

    pub fn hisound(&self) -> bool {
        self.hisound
    }

    pub fn format_sound_list(&self) -> String {
        let mut out = String::new();
        let mut count = 0usize;
        for ch in self.dynamic.iter().chain(self.static_ch.iter()) {
            if !ch.active {
                continue;
            }
            count += 1;
            let kind = if ch.is_static { "static" } else { "dyn" };
            out.push_str(&format!(
                "  [{kind}] ent={} ch={} l={:.2} r={:.2} {}\n",
                ch.entity, ch.channel, ch.left_gain, ch.right_gain, ch.sample_name
            ));
        }
        format!("Active mixer channels: {count}\n{out}")
    }

    pub fn set_sentences(&mut self, table: SentenceTable) {
        self.sentences = table;
    }

    pub fn sentences(&self) -> &SentenceTable {
        &self.sentences
    }

    pub fn insert_cached_wav(&mut self, path: &str, bytes: &[u8]) {
        let key = normalize_sound_path(path);
        let decoded = decode_wav(bytes).ok().map(Arc::new);
        self.wav_cache.insert(key, decoded);
    }

    pub fn load_wav_from_vfs(&mut self, vfs: Option<&Vfs>, path: &str) -> Option<DecodedWav> {
        self.load_wav_arc_from_vfs(vfs, path).map(|w| (*w).clone())
    }

    pub fn load_wav_arc_from_vfs(
        &mut self,
        vfs: Option<&Vfs>,
        path: &str,
    ) -> Option<Arc<DecodedWav>> {
        let key = normalize_sound_path(path);
        if let Some(cached) = self.wav_cache.get(&key) {
            return cached.clone();
        }
        let decoded = vfs
            .and_then(|v| v.read_bytes(&key).ok())
            .and_then(|b| decode_wav(&b).ok())
            .map(Arc::new);
        if vfs.is_some() {
            self.wav_cache.insert(key, decoded.clone());
        }
        decoded
    }

    pub fn set_volumes(&mut self, master: f32, suit: f32) {
        self.master_volume = if master.is_finite() {
            master.clamp(0.0, 2.0)
        } else {
            0.0
        };
        self.suit_volume = if suit.is_finite() {
            suit.clamp(0.0, 2.0)
        } else {
            0.0
        };
    }

    pub fn set_room_type(&mut self, rt: u16) {
        self.dsp.set_room_type(rt);
    }

    pub fn set_room_off(&mut self, off: bool) {
        self.dsp.set_room_off(off);
    }

    pub fn set_occlusion_factor(&mut self, factor: f32) {
        if factor.is_finite() {
            self.occlusion_factor = factor.clamp(0.0, 0.9);
        }
    }

    /// Set `host_timescale` for `(timescale + 1.0) * 0.5` sound pitch scaling (`hw.dll:FUN_10202bf0`).
    pub fn set_timescale(&mut self, timescale: f32) {
        self.timescale = if timescale.is_finite() && timescale > 0.0 {
            timescale
        } else {
            1.0
        };
    }

    /// Retail GoldSrc plays the exact `pitch` requested by `S_StartDynamicSound` /
    /// `PM_PlayStepSound` with zero random duplicate-shot pitch drift (`1.0`).
    pub fn channel_rate_jitter(&self, entity: u16, channel: u8) -> Option<f64> {
        self.dynamic
            .iter()
            .chain(self.static_ch.iter())
            .find(|c| c.active && c.entity == entity && c.channel == channel)
            .map(|_| 1.0)
    }

    pub fn channel_pitch(&self, entity: u16, channel: u8) -> Option<u8> {
        self.dynamic
            .iter()
            .chain(self.static_ch.iter())
            .find(|c| c.active && c.entity == entity && c.channel == channel)
            .map(|c| c.pitch)
    }

    pub fn is_channel_occluded(&self, entity: u16, channel: u8) -> Option<bool> {
        self.dynamic
            .iter()
            .chain(self.static_ch.iter())
            .find(|c| c.active && c.entity == entity && c.channel == channel)
            .map(|c| c.occluded)
    }

    pub fn room_type(&self) -> usize {
        self.dsp.room_type()
    }

    pub fn trigger_fade(&mut self, percent: u8, hold_time: u8, fade_out: u8, fade_in: u8) {
        self.fade.trigger(percent, hold_time, fade_out, fade_in);
    }

    pub fn active_channel_counts(&self) -> (usize, usize) {
        let d = self.dynamic.iter().filter(|c| c.active).count();
        let s = self.static_ch.iter().filter(|c| c.active).count();
        (d, s)
    }

    pub fn channel_gains(&self, entity: u16, channel: u8) -> Option<(f32, f32)> {
        self.dynamic
            .iter()
            .chain(self.static_ch.iter())
            .find(|c| c.active && c.entity == entity && c.channel == channel)
            .map(|c| (c.left_gain, c.right_gain))
    }

    pub fn stop_channel(&mut self, entity: u16, channel: u8) {
        for ch in self.dynamic.iter_mut().chain(self.static_ch.iter_mut()) {
            if ch.active && ch.entity == entity && ch.channel == channel {
                ch.active = false;
                ch.vox_queue.clear();
            }
        }
    }

    pub fn stop_all(&mut self) {
        for ch in self.dynamic.iter_mut().chain(self.static_ch.iter_mut()) {
            ch.active = false;
            ch.vox_queue.clear();
        }
        for vs in &mut self.voice_streams {
            vs.queue.clear();
        }
    }

    pub fn update_listener_and_entities(
        &mut self,
        listener: ListenerState,
        entity_positions: &HashMap<u16, Vec3>,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        self.listener = listener;
        self.dsp.set_underwater(listener.waterlevel >= 3);

        for ch in self.dynamic.iter_mut().chain(self.static_ch.iter_mut()) {
            if !ch.active {
                continue;
            }
            if !ch.is_2d
                && self.listener.local_entity != Some(ch.entity)
                && ch.entity > 0
                && let Some(&pos) = entity_positions.get(&ch.entity)
            {
                ch.origin = pos;
            }
            let (l, r, occ, alive) = Self::spatialize(
                &self.listener,
                ch.entity,
                ch.origin,
                ch.master_vol,
                ch.attenuation,
                ch.is_2d,
                ch.is_static && ch.wav.as_ref().is_some_and(|w| w.loop_start.is_some()),
                self.snd_occlusion,
                self.occlusion_factor,
                occlusion_fn,
            );
            if !alive {
                ch.active = false;
                ch.vox_queue.clear();
            } else {
                ch.left_gain = l;
                ch.right_gain = r;
                ch.occluded = occ;
            }
        }
    }

    /// Lock-free audio-thread listener update using sorted `(entity_id, origin)` pairs
    /// (`O(log N)` lookup with zero hashing or allocations).
    pub fn update_listener_and_sorted_entities(
        &mut self,
        listener: ListenerState,
        sorted_positions: &[(u16, Vec3)],
    ) {
        self.listener = listener;
        self.dsp.set_underwater(listener.waterlevel >= 3);

        for ch in self.dynamic.iter_mut().chain(self.static_ch.iter_mut()) {
            if !ch.active {
                continue;
            }
            if !ch.is_2d
                && self.listener.local_entity != Some(ch.entity)
                && ch.entity > 0
                && let Ok(idx) = sorted_positions.binary_search_by_key(&ch.entity, |&(id, _)| id)
            {
                ch.origin = sorted_positions[idx].1;
            }
            let (l, r, occ, alive) = Self::spatialize(
                &self.listener,
                ch.entity,
                ch.origin,
                ch.master_vol,
                ch.attenuation,
                ch.is_2d,
                ch.is_static && ch.wav.as_ref().is_some_and(|w| w.loop_start.is_some()),
                self.snd_occlusion,
                self.occlusion_factor,
                None,
            );
            if !alive {
                ch.active = false;
                ch.vox_queue.clear();
            } else {
                ch.left_gain = l;
                ch.right_gain = r;
                ch.occluded = occ;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spatialize(
        listener: &ListenerState,
        entity: u16,
        origin: Vec3,
        vol: f32,
        attenuation: f32,
        is_2d: bool,
        keep_when_far: bool,
        snd_occlusion: bool,
        occ_factor: f32,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) -> (f32, f32, bool, bool) {
        let clamped_vol = if vol.is_finite() {
            vol.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if is_2d
            || listener.local_entity == Some(entity)
            || attenuation <= 0.0
            || !origin.is_finite()
        {
            return (clamped_vol, clamped_vol, false, true);
        }
        let delta = origin - listener.origin;
        let dist = delta.length();
        if !dist.is_finite() {
            return (0.0, 0.0, false, false);
        }
        let dist_mult = attenuation / SOUND_NOMINAL_CLIP_DIST;
        let scale = (1.0 - dist * dist_mult).clamp(0.0, 1.0);
        if scale <= 0.0 {
            return (0.0, 0.0, false, keep_when_far);
        }
        let dir = if dist > 1e-4 {
            delta / dist
        } else {
            Vec3::ZERO
        };
        let right = listener.right.normalize_or_zero();
        let dot = right.dot(dir).clamp(-1.0, 1.0);
        // Xash3D FWGS `engine/client/sound/s_main.c` `S_SpatializeChannel`:
        // `rvol = bound(0, master_vol * (1 - dist * dist_mult) * (1 + dot), 255)`
        // `lvol = bound(0, master_vol * (1 - dist * dist_mult) * (1 - dot), 255)`
        let right_gain = (clamped_vol * scale * (1.0 + dot)).clamp(0.0, 1.0);
        let left_gain = (clamped_vol * scale * (1.0 - dot)).clamp(0.0, 1.0);
        // Retail GoldSrc (`hw.dll`) has ZERO wall occlusion LPF; only apply optional LPF when
        // `snd_occlusion` is explicitly enabled alongside a positive `s_occfactor`.
        let occluded = snd_occlusion && occ_factor > 0.0 && occlusion_fn.is_some_and(|f| f(origin));
        (left_gain, right_gain, occluded, true)
    }

    pub fn handle_sound_command(
        &mut self,
        vfs: Option<&Vfs>,
        cmd: &SoundCommand,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        match cmd {
            SoundCommand::Stop { entity, channel } => self.stop_channel(*entity, *channel),
            SoundCommand::Fade {
                percent,
                hold_time,
                fade_out,
                fade_in,
            } => self.trigger_fade(*percent, *hold_time, *fade_out, *fade_in),
            SoundCommand::RoomType(rt) => self.set_room_type(*rt),
            SoundCommand::CdTrack { .. } => {}
            SoundCommand::Start(s) => self.start_sound(vfs, s, false, occlusion_fn),
            SoundCommand::Ambient(s) => self.start_sound(vfs, s, true, occlusion_fn),
        }
    }

    pub fn start_sound(
        &mut self,
        vfs: Option<&Vfs>,
        s: &SoundStart,
        is_static: bool,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        if s.is_stop() {
            self.stop_channel(s.entity, s.channel);
            return;
        }
        let is_alter = (s.flags & (SND_FL_CHANGE_VOL | SND_FL_CHANGE_PITCH)) != 0;
        if is_alter {
            let pool = if is_static {
                &mut self.static_ch
            } else {
                &mut self.dynamic
            };
            if let Some(ch) = pool
                .iter_mut()
                .find(|c| c.active && c.entity == s.entity && c.channel == s.channel)
            {
                if (s.flags & SND_FL_CHANGE_VOL) != 0 {
                    ch.master_vol = s.volume.clamp(0.0, 1.0);
                }
                if (s.flags & SND_FL_CHANGE_PITCH) != 0 {
                    ch.pitch = normalize_pitch(s.pitch);
                }
                let (l, r, occ, _) = Self::spatialize(
                    &self.listener,
                    ch.entity,
                    ch.origin,
                    ch.master_vol,
                    ch.attenuation,
                    ch.is_2d,
                    is_static,
                    self.snd_occlusion,
                    self.occlusion_factor,
                    occlusion_fn,
                );
                ch.left_gain = l;
                ch.right_gain = r;
                ch.occluded = occ;
                return;
            }
        }

        let Some(name) = s.name.as_deref() else {
            return;
        };
        if s.is_sentence() || name.starts_with(['!', '#', '%']) {
            if let Some(words) = self.sentences.resolve(name) {
                self.start_vox_words(vfs, &words, s, is_static, occlusion_fn);
            }
            return;
        }
        let Some(wav) = self.load_wav_arc_from_vfs(vfs, name) else {
            return;
        };
        self.start_decoded_wav(wav, s, is_static, occlusion_fn);
    }

    /// Starts, alters, or stops a sound using a main-thread pre-decoded `Arc<DecodedWav>` without
    /// touching disk or VFS on the real-time audio thread.
    pub fn start_predecoded_sound(
        &mut self,
        wav: Option<Arc<DecodedWav>>,
        s: &SoundStart,
        is_static: bool,
    ) {
        if s.is_stop() {
            self.stop_channel(s.entity, s.channel);
            return;
        }
        let is_alter = (s.flags & (SND_FL_CHANGE_VOL | SND_FL_CHANGE_PITCH)) != 0;
        if is_alter {
            let pool = if is_static {
                &mut self.static_ch
            } else {
                &mut self.dynamic
            };
            if let Some(ch) = pool
                .iter_mut()
                .find(|c| c.active && c.entity == s.entity && c.channel == s.channel)
            {
                if (s.flags & SND_FL_CHANGE_VOL) != 0 {
                    ch.master_vol = s.volume.clamp(0.0, 1.0);
                }
                if (s.flags & SND_FL_CHANGE_PITCH) != 0 {
                    ch.pitch = normalize_pitch(s.pitch);
                }
                let (l, r, occ, _) = Self::spatialize(
                    &self.listener,
                    ch.entity,
                    ch.origin,
                    ch.master_vol,
                    ch.attenuation,
                    ch.is_2d,
                    is_static,
                    self.snd_occlusion,
                    self.occlusion_factor,
                    None,
                );
                ch.left_gain = l;
                ch.right_gain = r;
                ch.occluded = occ;
                return;
            }
        }
        let Some(wav) = wav else {
            return;
        };
        self.start_decoded_wav(wav, s, is_static, None);
    }

    pub fn start_2d_decoded(
        &mut self,
        entity: u16,
        channel: u8,
        wav: DecodedWav,
        volume: f32,
        pitch: u8,
    ) {
        let s = SoundStart::by_name("", entity, channel, Vec3::ZERO, volume, 0.0, pitch);
        self.start_decoded_wav(Arc::new(wav), &s, false, None);
    }

    fn start_decoded_wav(
        &mut self,
        wav: Arc<DecodedWav>,
        s: &SoundStart,
        is_static: bool,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        let is_2d = s.attenuation <= 0.0 || !s.origin.is_finite();
        let master_vol = if s.volume.is_finite() {
            s.volume.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (l, r, occ, alive) = Self::spatialize(
            &self.listener,
            s.entity,
            s.origin,
            master_vol,
            s.attenuation,
            is_2d,
            is_static && wav.loop_start.is_some(),
            self.snd_occlusion,
            self.occlusion_factor,
            occlusion_fn,
        );
        if !alive {
            return;
        }
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);
        let raw_name = s.name.as_deref().unwrap_or("");
        let sample_name = if raw_name.is_empty() {
            String::new()
        } else {
            normalize_sound_path(raw_name)
        };
        let idx = self.pick_slot(is_static, s.entity, s.channel, s.origin, &sample_name);
        let slot = if is_static {
            &mut self.static_ch[idx]
        } else {
            &mut self.dynamic[idx]
        };
        *slot = ChannelSlot {
            active: true,
            entity: s.entity,
            channel: s.channel,
            is_static,
            is_2d,
            origin: s.origin,
            master_vol,
            attenuation: s.attenuation,
            pitch: normalize_pitch(s.pitch),
            sample_name,
            wav: Some(wav),
            phase: 0.0,
            left_gain: l,
            right_gain: r,
            occluded: occ,
            occ_lp: [0.0; 2],
            vox_queue: VecDeque::new(),
            pause_remaining: 0,
            age,
        };
    }

    pub fn start_vox_words(
        &mut self,
        vfs: Option<&Vfs>,
        words: &[VoxWord],
        s: &SoundStart,
        is_static: bool,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        let mut queue = VecDeque::with_capacity(words.len());
        for w in words {
            let wav = self
                .load_wav_arc_from_vfs(vfs, &w.path)
                .map(|d| trim_wav_percent(d, w.start_pct, w.end_pct));
            let pause_samples = (w.pause_sec.max(0.0) * (MIX_SAMPLE_RATE as f32)).round() as usize;
            let end_frame = wav.as_ref().map_or(0, |d| d.frame_count());
            queue.push_back(VoxItem {
                wav,
                volume: (s.volume * w.volume).clamp(0.0, 2.0),
                pitch: w.pitch.max(50),
                end_frame,
                pause_samples,
            });
        }
        self.enqueue_vox_queue(queue, s, is_static, occlusion_fn);
    }

    /// Starts a pre-decoded VOX sentence queue on the audio thread without touching VFS.
    pub fn start_predecoded_vox(
        &mut self,
        words: Vec<PredecodedVoxWord>,
        s: &SoundStart,
        is_static: bool,
    ) {
        let mut queue = VecDeque::with_capacity(words.len());
        for w in words {
            let end_frame = w.wav.as_ref().map_or(0, |d| d.frame_count());
            queue.push_back(VoxItem {
                wav: w.wav,
                volume: (s.volume * w.volume).clamp(0.0, 2.0),
                pitch: w.pitch.max(50),
                end_frame,
                pause_samples: w.pause_samples,
            });
        }
        self.enqueue_vox_queue(queue, s, is_static, None);
    }

    fn enqueue_vox_queue(
        &mut self,
        mut queue: VecDeque<VoxItem>,
        s: &SoundStart,
        is_static: bool,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        let Some(first) = queue.pop_front() else {
            return;
        };
        let is_2d = s.attenuation <= 0.0 || !s.origin.is_finite();
        let (l, r, occ, alive) = Self::spatialize(
            &self.listener,
            s.entity,
            s.origin,
            first.volume,
            s.attenuation,
            is_2d,
            false,
            self.snd_occlusion,
            self.occlusion_factor,
            occlusion_fn,
        );
        if !alive {
            return;
        }
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);
        let idx = self.pick_slot(is_static, s.entity, s.channel, s.origin, "");
        let slot = if is_static {
            &mut self.static_ch[idx]
        } else {
            &mut self.dynamic[idx]
        };
        *slot = ChannelSlot {
            active: true,
            entity: s.entity,
            channel: s.channel,
            is_static,
            is_2d,
            origin: s.origin,
            master_vol: first.volume,
            attenuation: s.attenuation,
            pitch: first.pitch,
            sample_name: String::new(),
            wav: first.wav,
            phase: 0.0,
            left_gain: l,
            right_gain: r,
            occluded: occ,
            occ_lp: [0.0; 2],
            vox_queue: queue,
            pause_remaining: first.pause_samples,
            age,
        };
    }

    fn pick_slot(
        &self,
        is_static: bool,
        entity: u16,
        channel: u8,
        origin: Vec3,
        sample_name: &str,
    ) -> usize {
        let pool = if is_static {
            &self.static_ch
        } else {
            &self.dynamic
        };
        if is_static {
            // `SND_PickStaticChannel` (`Xash3D FWGS s_main.c`): static sounds only replace an
            // identical `(entity, origin, sample)` slot so world ambient sounds layer cleanly.
            if !sample_name.is_empty()
                && let Some(pos) = pool.iter().position(|c| {
                    c.active
                        && c.entity == entity
                        && c.origin == origin
                        && c.sample_name == sample_name
                })
            {
                return pos;
            }
        } else if channel == chan::ITEM {
            // Layer distinct `CHAN_ITEM` reload/keyframe sounds (`clipout`, `clipin`, `boltpull`)
            // without cutting each other off; re-triggering the same sample replaces cleanly.
            if !sample_name.is_empty()
                && let Some(pos) = pool.iter().position(|c| {
                    c.active
                        && c.entity == entity
                        && c.channel == chan::ITEM
                        && c.sample_name == sample_name
                })
            {
                return pos;
            }
        } else if channel != chan::AUTO
            && let Some(pos) = pool
                .iter()
                .position(|c| c.active && c.entity == entity && c.channel == channel)
        {
            return pos;
        }
        if let Some(pos) = pool.iter().position(|c| !c.active) {
            return pos;
        }
        let local_ent = self.listener.local_entity;
        let mut best_idx = 0usize;
        let mut best_score = f32::INFINITY;
        for (i, ch) in pool.iter().enumerate() {
            let protected_stream = ch.channel == chan::STREAM;
            let protected_local = local_ent == Some(ch.entity)
                && matches!(
                    ch.channel,
                    chan::WEAPON | chan::VOICE | chan::STREAM | chan::BODY | chan::ITEM
                );
            let prio = if protected_stream {
                200.0
            } else if protected_local {
                100.0
            } else {
                0.0
            };
            let score = prio + ch.left_gain + ch.right_gain;
            if score < best_score
                || ((score - best_score).abs() < 1e-6 && ch.age < pool[best_idx].age)
            {
                best_score = score;
                best_idx = i;
            }
        }
        best_idx
    }

    pub fn push_voice_pcm(&mut self, slot: u8, sample_rate: u32, pcm_mono: &[f32], gain: f32) {
        let Some(vs) = self.voice_streams.get_mut(usize::from(slot)) else {
            return;
        };
        vs.sample_rate = sample_rate.max(8000);
        vs.gain = gain.clamp(0.0, 4.0);
        let max_len = (vs.sample_rate as usize) * 2;
        for &s in pcm_mono {
            if vs.queue.len() >= max_len {
                vs.queue.pop_front();
            }
            vs.queue.push_back(if s.is_finite() { s } else { 0.0 });
        }
    }

    /// Mix `out_stereo_interleaved.len() / 2` stereo frames (`MIX_SAMPLE_RATE = 44100 Hz`).
    pub fn mix_frames(&mut self, out_stereo_interleaved: &mut [f32]) {
        let frames = out_stereo_interleaved.len() / 2;
        for frame_idx in 0..frames {
            self.fade_sample_acc += 1;
            if self.fade_sample_acc >= 441 {
                self.fade.step(0.01);
                self.fade_sample_acc = 0;
            }
            let fade_gain = self.fade.gain();
            let mut sum_l = 0.0f32;
            let mut sum_r = 0.0f32;

            let lerping = self.lerping;
            let hisound = self.hisound;
            for ch in self.dynamic.iter_mut().chain(self.static_ch.iter_mut()) {
                if !ch.active {
                    continue;
                }
                let (sl, sr) = sample_channel(ch, self.timescale, lerping, hisound);
                sum_l += sl * ch.left_gain;
                sum_r += sr * ch.right_gain;
            }

            let mut voice_l = 0.0f32;
            let mut voice_r = 0.0f32;
            for vs in &mut self.voice_streams {
                if vs.queue.is_empty() || vs.sample_rate == 0 {
                    continue;
                }
                let step = f64::from(vs.sample_rate) / f64::from(MIX_SAMPLE_RATE);
                let idx0 = vs.phase as usize;
                let frac = (vs.phase - idx0 as f64) as f32;
                let s0 = vs.queue.front().copied().unwrap_or(0.0);
                let s1 = vs.queue.get(1).copied().unwrap_or(s0);
                let val = (s0 + (s1 - s0) * frac) * vs.gain;
                voice_l += val;
                voice_r += val;
                vs.phase += step;
                while vs.phase >= 1.0 {
                    vs.phase -= 1.0;
                    let _ = vs.queue.pop_front();
                }
            }

            let master = self.master_volume * fade_gain;
            let (dsp_l, dsp_r) = self.dsp.process_frame(sum_l * master, sum_r * master);
            let out_l = soft_clip(dsp_l + voice_l);
            let out_r = soft_clip(dsp_r + voice_r);
            out_stereo_interleaved[frame_idx * 2] = out_l;
            out_stereo_interleaved[frame_idx * 2 + 1] = out_r;
        }
    }
}

fn soft_clip(x: f32) -> f32 {
    if !x.is_finite() {
        0.0
    } else if x.abs() <= 0.95 {
        x
    } else {
        x.tanh()
    }
}

/// Samples one output frame from `ch`.
///
/// Resampling honors `s_lerping` (`S_MixChannelToBuffer` / `S_MixAudio` in Xash3D FWGS
/// `engine/client/sound/s_mix.c`, read 2026-11-25) and `hisound` (`11025 Hz` step quantization
/// when `hisound 0`).
fn sample_channel(
    ch: &mut ChannelSlot,
    timescale: f32,
    lerping: bool,
    hisound: bool,
) -> (f32, f32) {
    let Some(wav) = ch.wav.as_ref() else {
        advance_vox_or_stop(ch);
        return (0.0, 0.0);
    };
    let nframes = wav.frame_count();
    if nframes == 0 {
        advance_vox_or_stop(ch);
        return (0.0, 0.0);
    }

    let i0 = ch.phase.floor() as usize;
    if i0 >= nframes {
        if advance_vox_or_pause(ch) {
            return (0.0, 0.0);
        }
        return (0.0, 0.0);
    }

    let frac = if lerping {
        (ch.phase - (i0 as f64)) as f32
    } else {
        0.0
    };
    let channels = usize::from(wav.channels.max(1));
    let loop_end = wav.loop_end.unwrap_or(nframes).min(nframes);
    let i1 = if i0 + 1 < nframes {
        i0 + 1
    } else if let Some(ls) = wav.loop_start
        && i0 < loop_end
    {
        ls.min(nframes - 1)
    } else {
        i0
    };

    let (mut l, mut r) = if channels == 1 {
        let s0 = wav.samples[i0];
        let s1 = wav.samples[i1];
        let v = s0 + (s1 - s0) * frac;
        (v, v)
    } else {
        let s0_l = wav.samples[i0 * 2];
        let s0_r = wav.samples[i0 * 2 + 1];
        let s1_l = wav.samples[i1 * 2];
        let s1_r = wav.samples[i1 * 2 + 1];
        (s0_l + (s1_l - s0_l) * frac, s0_r + (s1_r - s0_r) * frac)
    };

    if ch.occluded {
        // 1-pole ~1200 Hz low-pass filter when obstructed by world brush geometry.
        let alpha = 0.157;
        ch.occ_lp[0] += (l - ch.occ_lp[0]) * alpha;
        ch.occ_lp[1] += (r - ch.occ_lp[1]) * alpha;
        l = ch.occ_lp[0];
        r = ch.occ_lp[1];
    }

    let pitch_ratio = f64::from(normalize_pitch(ch.pitch)) / f64::from(PITCH_NORM);
    // Retail `hw.dll:FUN_10202bf0`: `(timescale + 1.0) * 0.5` pitch scaling (exact `1.0` at normal
    // timescale, zero random duplicate pitch drift).
    let timescale_pitch = (f64::from(timescale) + 1.0) * 0.5;
    let effective_src_rate = if hisound {
        f64::from(wav.sample_rate)
    } else {
        f64::from(wav.sample_rate.min(11025))
    };
    let step = (effective_src_rate / f64::from(MIX_SAMPLE_RATE)) * pitch_ratio * timescale_pitch;
    ch.phase += step;

    if let (Some(ls), Some(le)) = (wav.loop_start, wav.loop_end)
        && le > ls
        && ch.phase >= (le as f64)
    {
        let span = (le - ls) as f64;
        ch.phase = (ls as f64) + ((ch.phase - (le as f64)) % span);
    } else if ch.phase >= (nframes as f64) {
        let _ = advance_vox_or_pause(ch);
    }

    (l, r)
}

fn advance_vox_or_pause(ch: &mut ChannelSlot) -> bool {
    if ch.pause_remaining > 0 {
        ch.pause_remaining -= 1;
        ch.wav = None;
        return true;
    }
    advance_vox_or_stop(ch)
}

fn advance_vox_or_stop(ch: &mut ChannelSlot) -> bool {
    if ch.pause_remaining > 0 {
        ch.pause_remaining -= 1;
        return true;
    }
    while let Some(next) = ch.vox_queue.pop_front() {
        ch.master_vol = next.volume;
        ch.pitch = next.pitch;
        ch.pause_remaining = next.pause_samples;
        ch.phase = 0.0;
        let _ = next.end_frame;
        if let Some(wav) = next.wav {
            ch.wav = Some(wav);
            return true;
        }
    }
    ch.active = false;
    ch.wav = None;
    false
}

/// Exact GoldSrc / Xash3D FWGS `SND_Spatialize` (`engine/client/sound/s_main.c`):
/// - `entity == local_entity || attenuation <= 0.0` returns pure centered `(v, v)` with zero pan
///   and zero distance falloff.
/// - 3D sources use `SOUND_NOMINAL_CLIP_DIST = 1000.0`, `dist_mult = attenuation / 1000.0`, linear
///   distance falloff `(1.0 - dist * dist_mult).clamp(0.0, 1.0)`, and GoldSrc stereo dot-product
///   panning (`rscale = 1.0 + dot`, `lscale = 1.0 - dot`) bounded to `[0.0, 1.0]`.
pub fn snd_spatialize(
    listener: &ListenerState,
    entity: u16,
    origin: Vec3,
    master_vol: f32,
    attenuation: f32,
) -> (f32, f32) {
    let (l, r, _, _) = GoldSrcMixer::spatialize(
        listener,
        entity,
        origin,
        master_vol,
        attenuation,
        false,
        true,
        false,
        0.0,
        None,
    );
    (l, r)
}

#[inline]
fn normalize_pitch(pitch: u8) -> u8 {
    if pitch <= 1 { PITCH_NORM } else { pitch }
}

pub fn normalize_sound_path(rel: &str) -> String {
    let lower = rel.trim().replace('\\', "/").to_ascii_lowercase();
    let trimmed = lower.trim_start_matches(['/', '*']);
    if trimmed.starts_with("sound/") {
        trimmed.to_string()
    } else {
        format!("sound/{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::wav::encode_test_wav_pcm16;

    fn sine_wav(sample_rate: u32, freq_hz: f32, frames: usize, amp: f32) -> Vec<u8> {
        let pcm: Vec<i16> = (0..frames)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                let s = (2.0 * std::f32::consts::PI * freq_hz * t).sin() * amp;
                (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16
            })
            .collect();
        encode_test_wav_pcm16(sample_rate, 1, &pcm, None, None)
    }

    #[test]
    fn defaults_are_unmuffled_and_dry_waveform() {
        let wav = sine_wav(22_050, 4_000.0, 2_205, 0.6);

        let mut mixer_wall = GoldSrcMixer::new();
        let mut mixer_open = GoldSrcMixer::new();
        assert!(
            !mixer_wall.snd_occlusion(),
            "snd_occlusion must default to false (0)"
        );
        assert_eq!(mixer_wall.occlusion_factor(), 0.0);
        assert_eq!(
            mixer_wall.room_type(),
            0,
            "room_type must default to 0 (dry)"
        );
        assert!(!mixer_wall.room_off());

        let listener = ListenerState {
            origin: Vec3::ZERO,
            right: Vec3::Y,
            local_entity: Some(1),
            waterlevel: 0,
        };
        for m in [&mut mixer_wall, &mut mixer_open] {
            m.set_volumes(1.0, 0.25);
            m.insert_cached_wav("weapons/ak47-1.wav", &wav);
            m.update_listener_and_entities(listener, &HashMap::new(), None);
        }

        let shot = SoundStart::by_name(
            "weapons/ak47-1.wav",
            7,
            chan::WEAPON,
            Vec3::new(300.0, 0.0, 0.0),
            0.8,
            0.8,
            PITCH_NORM,
        );
        // Even when world trace reports a wall in between, default retail GoldSrc applies ZERO LPF.
        mixer_wall.start_sound(None, &shot, false, Some(&|_| true));
        mixer_open.start_sound(None, &shot, false, Some(&|_| false));
        assert_eq!(mixer_wall.is_channel_occluded(7, chan::WEAPON), Some(false));

        // Mix past the 100 ms sample to also prove room_type = 0 leaves zero reverb tail.
        let mut out_wall = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 5) * 2];
        let mut out_open = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 5) * 2];
        mixer_wall.mix_frames(&mut out_wall);
        mixer_open.mix_frames(&mut out_open);

        let max_diff = out_wall
            .iter()
            .zip(out_open.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert_eq!(
            max_diff, 0.0,
            "default snd_occlusion=0 must render bit-exact unmuffled PCM through walls"
        );
        let tail_start = (MIX_SAMPLE_RATE as usize * 3 / 20) * 2;
        let tail_peak = out_open[tail_start..]
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        assert_eq!(
            tail_peak, 0.0,
            "default room_type=0 must be 100% dry with zero tail"
        );
    }

    #[test]
    fn snd_spatialize_local_centered_and_remote_linear_1000u_pan() {
        let listener = ListenerState {
            origin: Vec3::new(100.0, 200.0, 64.0),
            right: Vec3::new(0.0, -1.0, 0.0), // GoldSrc yaw=0 facing +X -> right is -Y
            local_entity: Some(1),
            waterlevel: 0,
        };

        // 1. Local entity: exact centered `left == right == vol` regardless of offset or angle.
        for origin in [
            Vec3::new(900.0, -900.0, 500.0),
            Vec3::new(-5000.0, 4000.0, -200.0),
            Vec3::new(100.0, 150.0, 64.0),
        ] {
            let (l, r) = snd_spatialize(&listener, 1, origin, 0.75, 0.8);
            assert_eq!(
                (l, r),
                (0.75, 0.75),
                "local entity must be centered at {origin:?}"
            );
        }

        // 2. Non-attenuated (`attenuation <= 0.0`) remote entity: pure centered 2D.
        let (l2d, r2d) = snd_spatialize(&listener, 5, Vec3::new(100.0, -500.0, 64.0), 0.6, 0.0);
        assert_eq!((l2d, r2d), (0.6, 0.6));

        // 3. Remote entity straight ahead (`dot == 0.0`): exact linear `1000.0` nominal clip dist.
        for (dist, attn, expected) in [
            (0.0f32, 1.0f32, 1.0f32),
            (250.0, 1.0, 0.75),
            (500.0, 1.0, 0.50),
            (1000.0, 1.0, 0.0),
            (1200.0, 1.0, 0.0),
            (500.0, 0.8, 0.60),
        ] {
            let pos = listener.origin + Vec3::new(dist, 0.0, 0.0);
            let (l, r) = snd_spatialize(&listener, 2, pos, 1.0, attn);
            assert!(
                (l - expected).abs() < 1e-5 && (r - expected).abs() < 1e-5,
                "dist={dist} attn={attn}: got ({l}, {r}), expected {expected}"
            );
        }

        // 4. Remote entity directly to listener's right (-Y) vs left (+Y) at 500u, attn=1.0, vol=0.5.
        let right_pos = listener.origin + Vec3::new(0.0, -500.0, 0.0);
        let (lr, rr) = snd_spatialize(&listener, 3, right_pos, 0.5, 1.0);
        assert!(
            (lr - 0.0).abs() < 1e-5 && (rr - 0.5).abs() < 1e-5,
            "right ear: ({lr}, {rr})"
        );

        let left_pos = listener.origin + Vec3::new(0.0, 500.0, 0.0);
        let (ll, rl) = snd_spatialize(&listener, 4, left_pos, 0.5, 1.0);
        assert!(
            (ll - 0.5).abs() < 1e-5 && (rl - 0.0).abs() < 1e-5,
            "left ear: ({ll}, {rl})"
        );

        // Close panned source at vol=1.0 must clamp cleanly to 1.0 (matches Xash3D `bound(0, v, 255)`).
        let near_right = listener.origin + Vec3::new(0.0, -100.0, 0.0);
        let (ln, rn) = snd_spatialize(&listener, 6, near_right, 1.0, 0.8);
        assert_eq!(ln, 0.0);
        assert_eq!(rn, 1.0);
    }

    #[test]
    fn zero_pitch_drift_across_20_consecutive_pitch_norm_shots() {
        let wav = sine_wav(22_050, 660.0, 2_205, 0.5);
        let mut mixer = GoldSrcMixer::new();
        mixer.set_volumes(1.0, 0.25);
        mixer.insert_cached_wav("weapons/ak47-1.wav", &wav);

        // Fire 20 overlapping dynamic shots on CHAN_AUTO: every single slot must keep exact 1.0x pitch.
        for i in 0..20u16 {
            let s = SoundStart::by_name(
                "weapons/ak47-1.wav",
                10 + i,
                chan::AUTO,
                Vec3::ZERO,
                0.8,
                0.0,
                PITCH_NORM,
            );
            mixer.start_sound(None, &s, false, None);
            assert_eq!(mixer.channel_rate_jitter(10 + i, chan::AUTO), Some(1.0));
            assert_eq!(mixer.channel_pitch(10 + i, chan::AUTO), Some(PITCH_NORM));
        }
        assert_eq!(mixer.active_channel_counts().0, 20);

        // Compare rendered waveform of shot #1 vs shot #20 played consecutively: bit-exact match.
        mixer.stop_all();
        let mut first_pcm = vec![0.0f32; 1024 * 2];
        let mut last_pcm = vec![0.0f32; 1024 * 2];
        for i in 0..20 {
            let s = SoundStart::by_name(
                "weapons/ak47-1.wav",
                1,
                chan::WEAPON,
                Vec3::ZERO,
                0.8,
                0.0,
                PITCH_NORM,
            );
            mixer.start_sound(None, &s, false, None);
            let mut tmp = vec![0.0f32; 1024 * 2];
            mixer.mix_frames(&mut tmp);
            if i == 0 {
                first_pcm = tmp;
            } else if i == 19 {
                last_pcm = tmp;
            }
        }
        assert_eq!(
            first_pcm, last_pcm,
            "20 consecutive PITCH_NORM shots must have zero pitch drift"
        );
    }

    #[test]
    fn goldsrc_channel_replacement_and_reload_layering() {
        let wav = sine_wav(22_050, 440.0, 11_025, 0.5);
        let mut mixer = GoldSrcMixer::new();
        mixer.set_volumes(1.0, 0.25);
        for name in [
            "weapons/ak47-1.wav",
            "weapons/ak47-2.wav",
            "weapons/ak47_clipout.wav",
            "weapons/ak47_clipin.wav",
            "weapons/ak47_boltpull.wav",
            "ambience/wind1.wav",
            "ambience/wind2.wav",
        ] {
            mixer.insert_cached_wav(name, &wav);
        }

        // Firing on (ent=1, CHAN_WEAPON) replaces previous (ent=1, CHAN_WEAPON).
        mixer.start_sound(
            None,
            &SoundStart::by_name(
                "weapons/ak47-1.wav",
                1,
                chan::WEAPON,
                Vec3::ZERO,
                0.9,
                0.0,
                100,
            ),
            false,
            None,
        );
        mixer.start_sound(
            None,
            &SoundStart::by_name(
                "weapons/ak47-2.wav",
                1,
                chan::WEAPON,
                Vec3::ZERO,
                0.7,
                0.0,
                100,
            ),
            false,
            None,
        );
        assert_eq!(mixer.active_channel_counts(), (1, 0));
        assert_eq!(mixer.channel_gains(1, chan::WEAPON), Some((0.7, 0.7)));

        // Reload keyframe sounds on CHAN_ITEM layer without cutting off each other or CHAN_WEAPON.
        for reload in [
            "weapons/ak47_clipout.wav",
            "weapons/ak47_clipin.wav",
            "weapons/ak47_boltpull.wav",
        ] {
            mixer.start_sound(
                None,
                &SoundStart::by_name(reload, 1, chan::ITEM, Vec3::ZERO, 0.8, 0.0, 100),
                false,
                None,
            );
        }
        assert_eq!(mixer.active_channel_counts(), (4, 0));

        // CHAN_AUTO layers freely without replacing anything.
        mixer.start_sound(
            None,
            &SoundStart::by_name(
                "weapons/ak47_clipout.wav",
                1,
                chan::AUTO,
                Vec3::ZERO,
                0.8,
                0.0,
                100,
            ),
            false,
            None,
        );
        mixer.start_sound(
            None,
            &SoundStart::by_name(
                "weapons/ak47_clipin.wav",
                1,
                chan::AUTO,
                Vec3::ZERO,
                0.8,
                0.0,
                100,
            ),
            false,
            None,
        );
        assert_eq!(mixer.active_channel_counts(), (6, 0));

        // Static world ambient sounds on entity 0 at distinct origins layer cleanly.
        mixer.start_sound(
            None,
            &SoundStart::by_name(
                "ambience/wind1.wav",
                0,
                chan::STATIC,
                Vec3::new(100.0, 0.0, 0.0),
                0.5,
                0.8,
                100,
            ),
            true,
            None,
        );
        mixer.start_sound(
            None,
            &SoundStart::by_name(
                "ambience/wind2.wav",
                0,
                chan::STATIC,
                Vec3::new(-100.0, 0.0, 0.0),
                0.5,
                0.8,
                100,
            ),
            true,
            None,
        );
        assert_eq!(mixer.active_channel_counts(), (6, 2));
    }
}
