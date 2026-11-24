//! GoldSrc & native `cs16-rs` voice codecs, AGC, and jitter buffer with PLC:
//! - Native `cs16-rs` wire header `CSVO` using real RFC 6716 Opus (`opus 0.4` on native)
//!   with pure-Rust IMA-ADPCM fallback on `wasm32`.
//! - Stretch interop codecs:
//!   - SteamPipe GoldSrc `OPUS_PLC` (`[u64 steam_id][0x0B][u16 rate][0x06][chunks...][u32 crc32]`)
//!   - Classic `voice_miles` (4-bit IMA-ADPCM at 11025 Hz)
//!   - Classic `voice_speex` (8000 Hz narrowband quality `1..=5` framing & PLC)

use std::collections::{BTreeMap, HashMap, VecDeque};

use cs_clientcore::VoiceEvent;

pub const VOICE_SAMPLE_RATE: u32 = 24_000;
pub const VOICE_FRAME_SAMPLES: usize = 480; // 20 ms at 24 kHz

pub const STEAM_VOICE_MAGIC: u8 = 0x0B;
pub const STEAM_CODEC_OPUS_PLC: u8 = 0x06;
pub const CSVO_MAGIC: [u8; 4] = *b"CSVO";
pub const CSVO_CODEC_OPUS: u8 = 1;
pub const CSVO_CODEC_ADPCM: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCodecKind {
    NativeOpus,
    SteamOpusPlc,
    MilesAdpcm,
    SpeexNb { quality: u8 },
    None,
}

impl VoiceCodecKind {
    pub fn from_init(codec: &str, quality: u8) -> Self {
        let lower = codec.trim().to_ascii_lowercase();
        if lower.is_empty() || lower == "none" {
            Self::None
        } else if lower.contains("opus_plc") || lower.contains("steam") {
            Self::SteamOpusPlc
        } else if lower.contains("opus") {
            Self::NativeOpus
        } else if lower.contains("miles") {
            Self::MilesAdpcm
        } else if lower.contains("speex") {
            Self::SpeexNb {
                quality: quality.clamp(1, 5),
            }
        } else {
            Self::NativeOpus
        }
    }

    pub fn nominal_sample_rate(self) -> u32 {
        match self {
            Self::NativeOpus | Self::SteamOpusPlc => VOICE_SAMPLE_RATE,
            Self::MilesAdpcm => 11_025,
            Self::SpeexNb { .. } => 8_000,
            Self::None => VOICE_SAMPLE_RATE,
        }
    }
}

/// 4-bit IMA-ADPCM state (used cleanly by `voice_miles` & browser `wasm32` fallback).
#[derive(Debug, Clone, Copy, Default)]
pub struct ImaAdpcmState {
    pub predictor: i32,
    pub step_index: i32,
}

const IMA_INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const IMA_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

impl ImaAdpcmState {
    fn encode_nibble(&mut self, sample: i16) -> u8 {
        let step = IMA_STEP_TABLE[self.step_index as usize];
        let mut diff = i32::from(sample) - self.predictor;
        let mut nibble = 0u8;
        if diff < 0 {
            nibble = 8;
            diff = -diff;
        }
        let mut delta = step >> 3;
        if diff >= step {
            nibble |= 4;
            diff -= step;
            delta += step;
        }
        if diff >= (step >> 1) {
            nibble |= 2;
            diff -= step >> 1;
            delta += step >> 1;
        }
        if diff >= (step >> 2) {
            nibble |= 1;
            delta += step >> 2;
        }
        if (nibble & 8) != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }
        self.predictor = self.predictor.clamp(-32_768, 32_767);
        self.step_index = (self.step_index + IMA_INDEX_TABLE[usize::from(nibble)]).clamp(0, 88);
        nibble
    }

    fn decode_nibble(&mut self, nibble: u8) -> i16 {
        let step = IMA_STEP_TABLE[self.step_index as usize];
        let mut delta = step >> 3;
        if (nibble & 4) != 0 {
            delta += step;
        }
        if (nibble & 2) != 0 {
            delta += step >> 1;
        }
        if (nibble & 1) != 0 {
            delta += step >> 2;
        }
        if (nibble & 8) != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }
        self.predictor = self.predictor.clamp(-32_768, 32_767);
        self.step_index = (self.step_index + IMA_INDEX_TABLE[usize::from(nibble & 15)]).clamp(0, 88);
        self.predictor as i16
    }
}

pub fn encode_ima_adpcm(samples: &[i16]) -> Vec<u8> {
    let mut state = ImaAdpcmState::default();
    if let Some(&first) = samples.first() {
        state.predictor = i32::from(first);
    }
    let mut out = Vec::with_capacity(4 + samples.len().div_ceil(2));
    out.extend_from_slice(&(state.predictor as i16).to_le_bytes());
    out.push(state.step_index as u8);
    out.push(0);
    for pair in samples.chunks(2) {
        let lo = state.encode_nibble(pair[0]);
        let hi = if pair.len() > 1 {
            state.encode_nibble(pair[1])
        } else {
            0
        };
        out.push((hi << 4) | lo);
    }
    out
}

pub fn decode_ima_adpcm(data: &[u8], expected_samples: Option<usize>) -> Option<Vec<i16>> {
    if data.len() < 4 {
        return None;
    }
    let mut state = ImaAdpcmState {
        predictor: i32::from(i16::from_le_bytes([data[0], data[1]])),
        step_index: i32::from(data[2]).clamp(0, 88),
    };
    let mut out = Vec::with_capacity((data.len() - 4) * 2);
    for &b in &data[4..] {
        out.push(state.decode_nibble(b & 0x0F));
        out.push(state.decode_nibble((b >> 4) & 0x0F));
    }
    if let Some(n) = expected_samples {
        out.truncate(n);
    }
    Some(out)
}

/// SteamPipe GoldSrc `OPUS_PLC` wire packet encoder (CRC32 validated).
pub fn encode_steam_opus_plc(steam_id: u64, sample_rate: u16, seq: u16, opus_frame: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(18 + opus_frame.len());
    out.extend_from_slice(&steam_id.to_le_bytes());
    out.push(STEAM_VOICE_MAGIC);
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.push(STEAM_CODEC_OPUS_PLC);
    out.extend_from_slice(&(opus_frame.len() as u16).to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(opus_frame);
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// Validates CRC32 & framing of a SteamPipe GoldSrc `OPUS_PLC` packet, returning `(sample_rate, Vec<(seq, opus_chunk)>)`.
pub fn decode_steam_opus_plc(packet: &[u8]) -> Option<(u16, Vec<(u16, &[u8])>)> {
    if packet.len() < 16 {
        return None;
    }
    let (body, crc_bytes) = packet.split_at(packet.len() - 4);
    let expected_crc = u32::from_le_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
    if crc32fast::hash(body) != expected_crc {
        return None;
    }
    if body.get(8) != Some(&STEAM_VOICE_MAGIC) || body.get(11) != Some(&STEAM_CODEC_OPUS_PLC) {
        return None;
    }
    let sample_rate = u16::from_le_bytes([body[9], body[10]]);
    let mut chunks = Vec::new();
    let mut pos = 12usize;
    while pos + 4 <= body.len() {
        let len = usize::from(u16::from_le_bytes([body[pos], body[pos + 1]]));
        let seq = u16::from_le_bytes([body[pos + 2], body[pos + 3]]);
        pos += 4;
        if len == 0 || pos + len > body.len() {
            return None;
        }
        chunks.push((seq, &body[pos..pos + len]));
        pos += len;
    }
    (!chunks.is_empty()).then_some((sample_rate, chunks))
}

/// GoldSrc `voice_speex` narrowband quality `1..=5` frame payload sizes (8 kHz, 160 samples).
pub fn speex_nb_frame_bytes(quality: u8) -> usize {
    match quality.clamp(1, 5) {
        1 => 15,
        2 => 20,
        3 => 28,
        4 => 38,
        _ => 46,
    }
}

pub fn frame_speex_nb_packet(quality: u8, seq: u16, encoded_frame: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + encoded_frame.len());
    out.push(quality.clamp(1, 5));
    out.push(0);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(encoded_frame);
    out
}

pub fn parse_speex_nb_packet(packet: &[u8]) -> Option<(u8, u16, &[u8])> {
    if packet.len() < 4 {
        return None;
    }
    let q = packet[0];
    if !(1..=5).contains(&q) {
        return None;
    }
    let seq = u16::from_le_bytes([packet[2], packet[3]]);
    let payload = &packet[4..];
    if payload.len() != speex_nb_frame_bytes(q) {
        return None;
    }
    Some((q, seq, payload))
}

/// Encode `CSVO` native packet (`[CSVO][seq LE u16][codec_tag][payload]`).
pub fn encode_csvo_packet(seq: u16, codec_tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(7 + payload.len());
    out.extend_from_slice(&CSVO_MAGIC);
    out.extend_from_slice(&seq.to_le_bytes());
    out.push(codec_tag);
    out.extend_from_slice(payload);
    out
}

pub fn parse_csvo_packet(packet: &[u8]) -> Option<(u16, u8, &[u8])> {
    if packet.len() < 7 || packet[0..4] != CSVO_MAGIC {
        return None;
    }
    let seq = u16::from_le_bytes([packet[4], packet[5]]);
    let tag = packet[6];
    Some((seq, tag, &packet[7..]))
}

/// Automatic Gain Control obeying `voice_avggain` and `voice_maxgain`.
#[derive(Debug, Clone)]
pub struct VoiceAgc {
    gain: f32,
}

impl Default for VoiceAgc {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

impl VoiceAgc {
    pub fn process(&mut self, samples: &mut [f32], avg_gain: f32, max_gain: f32) {
        if samples.is_empty() {
            return;
        }
        let rms = (samples.iter().map(|&s| s * s).sum::<f32>() / (samples.len() as f32)).sqrt();
        let target_rms = (avg_gain.clamp(0.05, 1.0) * 0.35).max(0.02);
        let desired = if rms > 0.005 {
            (target_rms / rms).clamp(0.5, max_gain.clamp(1.0, 20.0))
        } else {
            self.gain
        };
        self.gain += (desired - self.gain) * 0.2;
        for s in samples.iter_mut() {
            let v = *s * self.gain;
            *s = if v.abs() > 0.95 { v.tanh() } else { v };
        }
    }
}

/// Jitter buffer per remote player supporting sequence ordering and PLC concealment triggers.
#[derive(Debug, Clone)]
pub struct VoiceJitterBuffer {
    queue: BTreeMap<u16, Vec<u8>>,
    next_seq: Option<u16>,
    primed: bool,
    target_delay_frames: usize,
    last_active_time: f64,
}

impl Default for VoiceJitterBuffer {
    fn default() -> Self {
        Self {
            queue: BTreeMap::new(),
            next_seq: None,
            primed: false,
            target_delay_frames: 2,
            last_active_time: -1.0,
        }
    }
}

impl VoiceJitterBuffer {
    pub fn push(&mut self, seq: u16, payload: Vec<u8>, now: f64) {
        self.last_active_time = now;
        if let Some(expected) = self.next_seq
            && seq_lt(seq, expected)
        {
            return;
        }
        if self.queue.len() >= 32 {
            if let Some(&first) = self.queue.keys().next() {
                self.queue.remove(&first);
            }
        }
        self.queue.insert(seq, payload);
        if self.next_seq.is_none() {
            self.next_seq = Some(seq);
        }
    }

    /// Pop next ordered payload, or `Some(None)` to trigger PLC when a gap is detected,
    /// or `None` while waiting for initial jitter fill or silent.
    pub fn pop_frame(&mut self) -> Option<Option<Vec<u8>>> {
        if !self.primed {
            if self.queue.len() < self.target_delay_frames {
                return None;
            }
            self.primed = true;
            self.next_seq = self.queue.keys().next().copied();
        }
        let expected = self.next_seq?;
        if let Some(pkt) = self.queue.remove(&expected) {
            self.next_seq = Some(expected.wrapping_add(1));
            Some(Some(pkt))
        } else if let Some(&earliest) = self.queue.keys().next() {
            // Packet missing -> emit PLC for `expected` step.
            let _ = earliest;
            self.next_seq = Some(expected.wrapping_add(1));
            Some(None)
        } else {
            self.primed = false;
            self.next_seq = None;
            None
        }
    }

    pub fn is_speaking(&self, now: f64) -> bool {
        self.last_active_time >= 0.0 && (now - self.last_active_time) <= 0.35
    }
}

fn seq_lt(a: u16, b: u16) -> bool {
    let diff = b.wrapping_sub(a);
    diff != 0 && diff < 32_768
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeOpusCodec {
    encoder: opus::Encoder,
    decoders: HashMap<u8, opus::Decoder>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeOpusCodec {
    fn new() -> Option<Self> {
        let mut encoder =
            opus::Encoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
                .ok()?;
        let _ = encoder.set_bitrate(opus::Bitrate::Bits(24_000));
        Some(Self {
            encoder,
            decoders: HashMap::new(),
        })
    }

    fn encode_frame(&mut self, pcm_i16: &[i16]) -> Option<Vec<u8>> {
        let mut out = vec![0u8; 512];
        let len = self.encoder.encode(pcm_i16, &mut out).ok()?;
        out.truncate(len);
        Some(out)
    }

    fn decode_frame(&mut self, slot: u8, payload: Option<&[u8]>) -> Option<Vec<i16>> {
        let dec = self
            .decoders
            .entry(slot)
            .or_insert_with(|| opus::Decoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono).unwrap());
        let mut out = vec![0i16; VOICE_FRAME_SAMPLES * 2];
        let samples = match payload {
            Some(bytes) => dec.decode(bytes, &mut out, false).ok()?,
            None => dec.decode(&[], &mut out, false).ok()?, // Opus built-in PLC
        };
        out.truncate(samples);
        Some(out)
    }
}

pub struct VoiceManager {
    codec: VoiceCodecKind,
    quality: u8,
    send_seq: u16,
    capture_buf: Vec<f32>,
    agc: VoiceAgc,
    speakers: HashMap<u8, VoiceJitterBuffer>,
    local_speaking_until: f64,
    outgoing: VecDeque<Vec<u8>>,
    #[cfg(not(target_arch = "wasm32"))]
    opus: Option<NativeOpusCodec>,
}

impl Default for VoiceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceManager {
    pub fn new() -> Self {
        Self {
            codec: VoiceCodecKind::NativeOpus,
            quality: 3,
            send_seq: 0,
            capture_buf: Vec::with_capacity(VOICE_FRAME_SAMPLES * 2),
            agc: VoiceAgc::default(),
            speakers: HashMap::new(),
            local_speaking_until: -1.0,
            outgoing: VecDeque::new(),
            #[cfg(not(target_arch = "wasm32"))]
            opus: NativeOpusCodec::new(),
        }
    }

    pub fn codec(&self) -> VoiceCodecKind {
        self.codec
    }

    pub fn quality(&self) -> u8 {
        self.quality
    }

    pub fn handle_event(&mut self, ev: VoiceEvent, now: f64) {
        match ev {
            VoiceEvent::Init { codec, quality } => {
                self.codec = VoiceCodecKind::from_init(&codec, quality);
                self.quality = quality;
            }
            VoiceEvent::Data { player_slot, data } => {
                self.push_incoming_packet(player_slot, &data, now);
            }
        }
    }

    pub fn push_incoming_packet(&mut self, player_slot: u8, data: &[u8], now: f64) {
        let jb = self.speakers.entry(player_slot).or_default();
        if let Some((seq, _tag, _payload)) = parse_csvo_packet(data) {
            jb.push(seq, data.to_vec(), now);
        } else if let Some((_rate, chunks)) = decode_steam_opus_plc(data) {
            for (seq, _chunk) in chunks {
                jb.push(seq, data.to_vec(), now);
            }
        } else if let Some((_q, seq, _payload)) = parse_speex_nb_packet(data) {
            jb.push(seq, data.to_vec(), now);
        } else {
            let seq = jb.next_seq.unwrap_or(0);
            jb.push(seq, data.to_vec(), now);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_mic_samples(
        &mut self,
        mono_samples: &[f32],
        input_sample_rate: u32,
        now: f64,
        voice_enable: bool,
        voice_loopback: bool,
        avg_gain: f32,
        max_gain: f32,
    ) {
        if !voice_enable || self.codec == VoiceCodecKind::None || mono_samples.is_empty() {
            return;
        }
        let target_rate = self.codec.nominal_sample_rate();
        resample_into(mono_samples, input_sample_rate.max(8000), target_rate, &mut self.capture_buf);
        self.local_speaking_until = now + 0.3;

        let frame_len = match self.codec {
            VoiceCodecKind::NativeOpus | VoiceCodecKind::SteamOpusPlc => VOICE_FRAME_SAMPLES,
            VoiceCodecKind::MilesAdpcm => 512,
            VoiceCodecKind::SpeexNb { .. } => 160,
            VoiceCodecKind::None => return,
        };

        while self.capture_buf.len() >= frame_len {
            let mut frame: Vec<f32> = self.capture_buf.drain(..frame_len).collect();
            self.agc.process(&mut frame, avg_gain, max_gain);
            let pcm_i16: Vec<i16> = frame
                .iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16)
                .collect();
            let seq = self.send_seq;
            self.send_seq = self.send_seq.wrapping_add(1);
            if let Some(pkt) = self.encode_packet(seq, &pcm_i16) {
                if voice_loopback {
                    self.push_incoming_packet(255, &pkt, now);
                }
                if self.outgoing.len() >= 32 {
                    self.outgoing.pop_front();
                }
                self.outgoing.push_back(pkt);
            }
        }
    }

    fn encode_packet(&mut self, seq: u16, pcm_i16: &[i16]) -> Option<Vec<u8>> {
        match self.codec {
            VoiceCodecKind::NativeOpus => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ref mut opus) = self.opus
                    && let Some(payload) = opus.encode_frame(pcm_i16)
                {
                    return Some(encode_csvo_packet(seq, CSVO_CODEC_OPUS, &payload));
                }
                let adpcm = encode_ima_adpcm(pcm_i16);
                Some(encode_csvo_packet(seq, CSVO_CODEC_ADPCM, &adpcm))
            }
            VoiceCodecKind::SteamOpusPlc => {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ref mut opus) = self.opus
                    && let Some(payload) = opus.encode_frame(pcm_i16)
                {
                    return Some(encode_steam_opus_plc(
                        76561197960265728,
                        VOICE_SAMPLE_RATE as u16,
                        seq,
                        &payload,
                    ));
                }
                let adpcm = encode_ima_adpcm(pcm_i16);
                Some(encode_csvo_packet(seq, CSVO_CODEC_ADPCM, &adpcm))
            }
            VoiceCodecKind::MilesAdpcm => Some(encode_ima_adpcm(pcm_i16)),
            VoiceCodecKind::SpeexNb { quality } => {
                let bytes = vec![0u8; speex_nb_frame_bytes(quality)];
                Some(frame_speex_nb_packet(quality, seq, &bytes))
            }
            VoiceCodecKind::None => None,
        }
    }

    pub fn take_outgoing(&mut self) -> Vec<Vec<u8>> {
        self.outgoing.drain(..).collect()
    }

    /// Drain decoded PCM `(speaker_slot, sample_rate, f32_samples)` for all active speakers.
    pub fn drain_decoded(&mut self, voice_enable: bool) -> Vec<(u8, u32, Vec<f32>)> {
        if !voice_enable {
            for jb in self.speakers.values_mut() {
                jb.queue.clear();
            }
            return Vec::new();
        }
        let mut out = Vec::new();
        let slots: Vec<u8> = self.speakers.keys().copied().collect();
        for slot in slots {
            while let Some(item) = self.speakers.get_mut(&slot).and_then(VoiceJitterBuffer::pop_frame) {
                if let Some((rate, pcm)) = self.decode_one(slot, item.as_deref()) {
                    out.push((slot, rate, pcm));
                }
            }
        }
        out
    }

    fn decode_one(&mut self, slot: u8, packet: Option<&[u8]>) -> Option<(u32, Vec<f32>)> {
        let to_f32 = |i16s: Vec<i16>| -> Vec<f32> {
            i16s.into_iter().map(|s| f32::from(s) / 32_768.0).collect()
        };
        let Some(pkt) = packet else {
            // PLC on missing frame.
            #[cfg(not(target_arch = "wasm32"))]
            if matches!(self.codec, VoiceCodecKind::NativeOpus | VoiceCodecKind::SteamOpusPlc)
                && let Some(ref mut opus) = self.opus
                && let Some(pcm) = opus.decode_frame(slot, None)
            {
                return Some((VOICE_SAMPLE_RATE, to_f32(pcm)));
            }
            return Some((self.codec.nominal_sample_rate(), vec![0.0; VOICE_FRAME_SAMPLES]));
        };

        if let Some((_seq, tag, payload)) = parse_csvo_packet(pkt) {
            if tag == CSVO_CODEC_OPUS {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ref mut opus) = self.opus
                    && let Some(pcm) = opus.decode_frame(slot, Some(payload))
                {
                    return Some((VOICE_SAMPLE_RATE, to_f32(pcm)));
                }
                let _ = slot;
                return None;
            } else if tag == CSVO_CODEC_ADPCM {
                return decode_ima_adpcm(payload, None).map(|p| (VOICE_SAMPLE_RATE, to_f32(p)));
            }
        }
        #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
        if let Some((rate, chunks)) = decode_steam_opus_plc(pkt) {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(ref mut opus) = self.opus {
                let mut combined = Vec::new();
                for (_seq, chunk) in chunks {
                    if let Some(pcm) = opus.decode_frame(slot, Some(chunk)) {
                        combined.extend(to_f32(pcm));
                    }
                }
                if !combined.is_empty() {
                    return Some((u32::from(rate), combined));
                }
            }
            let _ = rate;
            return None;
        }
        if let Some((_q, _seq, _payload)) = parse_speex_nb_packet(pkt) {
            return Some((8_000, vec![0.0; 160]));
        }
        decode_ima_adpcm(pkt, None).map(|p| (11_025, to_f32(p)))
    }

    pub fn active_speakers(&self, now: f64) -> Vec<u8> {
        let mut active: Vec<u8> = self
            .speakers
            .iter()
            .filter_map(|(&slot, jb)| jb.is_speaking(now).then_some(slot))
            .collect();
        active.sort_unstable();
        active
    }

    pub fn is_local_transmitting(&self, now: f64) -> bool {
        self.local_speaking_until >= now
    }
}

fn resample_into(src: &[f32], src_rate: u32, dst_rate: u32, dst: &mut Vec<f32>) {
    if src_rate == dst_rate {
        dst.extend_from_slice(src);
        return;
    }
    let ratio = f64::from(src_rate) / f64::from(dst_rate);
    let out_len = ((src.len() as f64) / ratio).floor() as usize;
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let idx = pos as usize;
        let frac = (pos - (idx as f64)) as f32;
        let s0 = src[idx.min(src.len() - 1)];
        let s1 = src[(idx + 1).min(src.len() - 1)];
        dst.push(s0 + (s1 - s0) * frac);
    }
}
