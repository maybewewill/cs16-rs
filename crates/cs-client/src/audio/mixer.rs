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
    wav: Option<DecodedWav>,
    volume: f32,
    pitch: u8,
    end_frame: usize,
    pause_samples: usize,
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
    wav: Option<DecodedWav>,
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
    wav_cache: HashMap<String, Option<DecodedWav>>,
    sentences: SentenceTable,
    dynamic: Vec<ChannelSlot>,
    static_ch: Vec<ChannelSlot>,
    voice_streams: Vec<VoiceStream>,
    dsp: RoomDsp,
    fade: SoundFadeState,
    listener: ListenerState,
    master_volume: f32,
    suit_volume: f32,
    occlusion_factor: f32,
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
            occlusion_factor: 0.25,
            next_age: 1,
            fade_sample_acc: 0,
        }
    }

    pub fn set_sentences(&mut self, table: SentenceTable) {
        self.sentences = table;
    }

    pub fn sentences(&self) -> &SentenceTable {
        &self.sentences
    }

    pub fn insert_cached_wav(&mut self, path: &str, bytes: &[u8]) {
        let key = normalize_sound_path(path);
        let decoded = decode_wav(bytes).ok();
        self.wav_cache.insert(key, decoded);
    }

    pub fn load_wav_from_vfs(&mut self, vfs: Option<&Vfs>, path: &str) -> Option<DecodedWav> {
        let key = normalize_sound_path(path);
        if let Some(cached) = self.wav_cache.get(&key) {
            return cached.clone();
        }
        let decoded = vfs
            .and_then(|v| v.read_bytes(&key).ok())
            .and_then(|b| decode_wav(&b).ok());
        self.wav_cache.insert(key, decoded.clone());
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
            if !ch.is_2d && ch.entity > 0 && let Some(&pos) = entity_positions.get(&ch.entity) {
                ch.origin = pos;
            }
            let (l, r, occ, alive) = Self::spatialize(
                &self.listener,
                ch.origin,
                ch.master_vol,
                ch.attenuation,
                ch.is_2d,
                ch.is_static && ch.wav.as_ref().is_some_and(|w| w.loop_start.is_some()),
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

    #[allow(clippy::too_many_arguments)]
    fn spatialize(
        listener: &ListenerState,
        origin: Vec3,
        vol: f32,
        attenuation: f32,
        is_2d: bool,
        keep_when_far: bool,
        occ_factor: f32,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) -> (f32, f32, bool, bool) {
        if is_2d || attenuation <= 0.0 {
            let v = vol.clamp(0.0, 2.0);
            return (v, v, false, true);
        }
        let delta = origin - listener.origin;
        let dist = delta.length();
        if !dist.is_finite() {
            return (0.0, 0.0, false, false);
        }
        let dist_mult = attenuation / SOUND_NOMINAL_CLIP_DIST;
        let scale = (1.0 - dist * dist_mult).max(0.0);
        if scale <= 0.0 {
            return (0.0, 0.0, false, keep_when_far);
        }
        let dir = if dist > 1e-3 { delta / dist } else { Vec3::ZERO };
        let rdot = listener.right.dot(dir).clamp(-1.0, 1.0);
        // Exact full stereo separation when orthogonal on X-axis (`|rdot| == 1` -> opposite side 0).
        let right_pan = (1.0 + rdot) * scale;
        let left_pan = (1.0 - rdot) * scale;
        let occluded = if dist > 48.0 {
            occlusion_fn.is_some_and(|f| f(origin))
        } else {
            false
        };
        let occ_mul = if occluded {
            (1.0 - occ_factor.clamp(0.0, 0.9)).max(0.1)
        } else {
            1.0
        };
        let left = (left_pan * vol * occ_mul).clamp(0.0, 2.0);
        let right = (right_pan * vol * occ_mul).clamp(0.0, 2.0);
        (left, right, occluded, true)
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
                    ch.master_vol = s.volume.clamp(0.0, 2.0);
                }
                if (s.flags & SND_FL_CHANGE_PITCH) != 0 {
                    ch.pitch = s.pitch.max(2);
                }
                let (l, r, occ, _) = Self::spatialize(
                    &self.listener,
                    ch.origin,
                    ch.master_vol,
                    ch.attenuation,
                    ch.is_2d,
                    is_static,
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
        let Some(wav) = self.load_wav_from_vfs(vfs, name) else {
            return;
        };
        self.start_decoded_wav(wav, s, is_static, occlusion_fn);
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
        self.start_decoded_wav(wav, &s, false, None);
    }

    fn start_decoded_wav(
        &mut self,
        wav: DecodedWav,
        s: &SoundStart,
        is_static: bool,
        occlusion_fn: Option<&dyn Fn(Vec3) -> bool>,
    ) {
        let is_2d = self.listener.local_entity == Some(s.entity)
            || s.attenuation <= 0.0
            || !s.origin.is_finite();
        let (l, r, occ, alive) = Self::spatialize(
            &self.listener,
            s.origin,
            s.volume.clamp(0.0, 2.0),
            s.attenuation,
            is_2d,
            is_static && wav.loop_start.is_some(),
            self.occlusion_factor,
            occlusion_fn,
        );
        if !alive {
            return;
        }
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);
        let idx = self.pick_slot(is_static, s.entity, s.channel);
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
            master_vol: s.volume.clamp(0.0, 2.0),
            attenuation: s.attenuation,
            pitch: s.pitch.max(2),
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
        let mut queue = VecDeque::new();
        for w in words {
            let wav = self.load_wav_from_vfs(vfs, &w.path);
            let pause_samples = (w.pause_sec.max(0.0) * (MIX_SAMPLE_RATE as f32)).round() as usize;
            let total_frames = wav.as_ref().map_or(0, DecodedWav::frame_count);
            let start_f = (total_frames * usize::from(w.start_pct)) / 100;
            let end_f = ((total_frames * usize::from(w.end_pct)) / 100).max(start_f);
            let mut item_wav = wav;
            if let Some(ref mut d) = item_wav {
                let clamped_end = end_f.min(total_frames).max(start_f);
                if start_f > 0 || clamped_end < total_frames {
                    let ch = usize::from(d.channels.max(1));
                    let slice: Vec<f32> = d.samples[start_f * ch..clamped_end * ch].to_vec();
                    d.samples = Arc::from(slice);
                    d.loop_start = None;
                    d.loop_end = None;
                }
            }
            queue.push_back(VoxItem {
                wav: item_wav,
                volume: (s.volume * w.volume).clamp(0.0, 2.0),
                pitch: w.pitch.max(50),
                end_frame: end_f.saturating_sub(start_f),
                pause_samples,
            });
        }
        let Some(first) = queue.pop_front() else {
            return;
        };
        let is_2d = self.listener.local_entity == Some(s.entity)
            || s.attenuation <= 0.0
            || !s.origin.is_finite();
        let (l, r, occ, alive) = Self::spatialize(
            &self.listener,
            s.origin,
            first.volume,
            s.attenuation,
            is_2d,
            false,
            self.occlusion_factor,
            occlusion_fn,
        );
        if !alive {
            return;
        }
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1);
        let idx = self.pick_slot(is_static, s.entity, s.channel);
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

    fn pick_slot(&self, is_static: bool, entity: u16, channel: u8) -> usize {
        let pool = if is_static {
            &self.static_ch
        } else {
            &self.dynamic
        };
        if channel != chan::AUTO
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
            let protected = local_ent == Some(ch.entity)
                && matches!(ch.channel, chan::WEAPON | chan::VOICE | chan::STREAM);
            let prio = if protected { 100.0 } else { 0.0 };
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

            for ch in self.dynamic.iter_mut().chain(self.static_ch.iter_mut()) {
                if !ch.active {
                    continue;
                }
                let (sl, sr) = sample_channel(ch);
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

fn sample_channel(ch: &mut ChannelSlot) -> (f32, f32) {
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

    let frac = (ch.phase - (i0 as f64)) as f32;
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

    let pitch_ratio = f64::from(ch.pitch.max(2)) / f64::from(PITCH_NORM);
    let step = (f64::from(wav.sample_rate) / f64::from(MIX_SAMPLE_RATE)) * pitch_ratio;
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

pub fn normalize_sound_path(rel: &str) -> String {
    let lower = rel.trim().replace('\\', "/").to_ascii_lowercase();
    let trimmed = lower.trim_start_matches(['/', '*']);
    if trimmed.starts_with("sound/") {
        trimmed.to_string()
    } else {
        format!("sound/{trimmed}")
    }
}
