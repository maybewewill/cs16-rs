//! GoldSrc & native `cs16-rs` voice codecs, AGC, and jitter buffer with PLC:
//! - Native `cs16-rs` wire header `CSVO` using real RFC 6716 Opus (`opus 0.4` on native)
//!   with pure-Rust IMA-ADPCM fallback on `wasm32`.
//! - SteamPipe GoldSrc `OPUS_PLC` (`[u64 steam_id][0x0B sample_rate][0x06 outer_len][i16 frame_len][seq][opus]...[-1 reset][0x00 silence][u32 crc32]`).
//! - Classic `voice_miles` (4-bit IMA-ADPCM at 11025 Hz) and `voice_speex` (8000 Hz narrowband).

use std::collections::{BTreeMap, HashMap, VecDeque};

use cs_clientcore::VoiceEvent;

pub const VOICE_SAMPLE_RATE: u32 = 24_000;
pub const VOICE_FRAME_SAMPLES: usize = 480; // 20 ms at 24 kHz

pub const STEAM_VOICE_MAGIC: u8 = 0x0B;
pub const STEAM_CODEC_OPUS_PLC: u8 = 0x06;
pub const STEAM_VOICE_TAG_SILENCE: u8 = 0x00;
pub const STEAM_VOICE_TAG_OPUS_PLC: u8 = 0x06;
pub const STEAM_VOICE_TAG_SAMPLE_RATE: u8 = 0x0B;
pub const STEAM_VOICE_RESET_LEN: i16 = -1;
pub const MAX_STEAM_SILENCE_SAMPLES: usize = 48_000;
pub const DEFAULT_STEAM_VOICE_ID: u64 = 76_561_197_960_265_728;

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

/// Chunk decoded from a SteamPipe GoldSrc `VOICE_OPUS_PLC` (`0x06` / `0x00`) payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamVoiceChunk<'a> {
    OpusFrame { seq: u16, data: &'a [u8] },
    Reset,
    Silence { samples: u16 },
}

/// Parsed SteamPipe GoldSrc `VOICE_OPUS_PLC` wire packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamVoicePacket<'a> {
    pub steam_id: u64,
    pub sample_rate: u16,
    pub chunks: Vec<SteamVoiceChunk<'a>>,
}

/// Encodes a full SteamPipe GoldSrc `VOICE_OPUS_PLC` packet:
/// `[u64 steam_id][0x0B sample_rate][0x06 outer_len][i16 frame_len][seq][opus]...[-1 reset][0x00 silence][u32 crc32]`.
pub fn encode_steam_voice_packet(
    steam_id: u64,
    sample_rate: u16,
    chunks: &[SteamVoiceChunk<'_>],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + chunks.len() * 64);
    out.extend_from_slice(&steam_id.to_le_bytes());
    out.push(STEAM_VOICE_TAG_SAMPLE_RATE);
    out.extend_from_slice(&sample_rate.to_le_bytes());

    let mut opus_len_pos: Option<usize> = None;
    let flush_opus_block = |buf: &mut Vec<u8>, len_pos: &mut Option<usize>| {
        if let Some(pos) = len_pos.take() {
            let payload_len = buf.len().saturating_sub(pos + 2);
            let encoded_len = u16::try_from(payload_len).unwrap_or(u16::MAX).to_le_bytes();
            buf[pos] = encoded_len[0];
            buf[pos + 1] = encoded_len[1];
        }
    };

    for chunk in chunks {
        match *chunk {
            SteamVoiceChunk::OpusFrame { seq, data } => {
                if opus_len_pos.is_none() {
                    out.push(STEAM_VOICE_TAG_OPUS_PLC);
                    opus_len_pos = Some(out.len());
                    out.extend_from_slice(&0u16.to_le_bytes());
                }
                let frame_len = i16::try_from(data.len().min(i16::MAX as usize)).unwrap_or(i16::MAX);
                out.extend_from_slice(&frame_len.to_le_bytes());
                out.extend_from_slice(&seq.to_le_bytes());
                out.extend_from_slice(&data[..frame_len as usize]);
            }
            SteamVoiceChunk::Reset => {
                if opus_len_pos.is_none() {
                    out.push(STEAM_VOICE_TAG_OPUS_PLC);
                    opus_len_pos = Some(out.len());
                    out.extend_from_slice(&0u16.to_le_bytes());
                }
                out.extend_from_slice(&STEAM_VOICE_RESET_LEN.to_le_bytes());
            }
            SteamVoiceChunk::Silence { samples } => {
                flush_opus_block(&mut out, &mut opus_len_pos);
                out.push(STEAM_VOICE_TAG_SILENCE);
                out.extend_from_slice(&samples.to_le_bytes());
            }
        }
    }
    flush_opus_block(&mut out, &mut opus_len_pos);

    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// SteamPipe GoldSrc `OPUS_PLC` wire packet encoder (CRC32 validated).
pub fn encode_steam_opus_plc(steam_id: u64, sample_rate: u16, seq: u16, opus_frame: &[u8]) -> Vec<u8> {
    encode_steam_voice_packet(
        steam_id,
        sample_rate,
        &[SteamVoiceChunk::OpusFrame {
            seq,
            data: opus_frame,
        }],
    )
}

/// Encodes an end-of-spurt SteamPipe `VOICE_OPUS_PLC` reset (`frame_len = -1`) packet.
pub fn encode_steam_opus_plc_reset(steam_id: u64, sample_rate: u16) -> Vec<u8> {
    encode_steam_voice_packet(steam_id, sample_rate, &[SteamVoiceChunk::Reset])
}

/// Decoded SteamPipe GoldSrc `OPUS_PLC` payload: `(sample_rate, Vec<(seq, opus_chunk)>)`.
pub type SteamOpusPlcPacket<'a> = (u16, Vec<(u16, &'a [u8])>);

/// Validates CRC32 and parses the full SteamPipe `VOICE_OPUS_PLC` packet grammar
/// (`0x0B` sample rate, `0x06` outer-length framed Opus + `-1` resets, `0x00` silence).
pub fn parse_steam_voice_packet(packet: &[u8]) -> Option<SteamVoicePacket<'_>> {
    if packet.len() < 15 {
        return None;
    }
    let (body, crc_bytes) = packet.split_at(packet.len() - 4);
    let expected_crc = u32::from_le_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
    if crc32fast::hash(body) != expected_crc {
        return None;
    }
    let steam_id = u64::from_le_bytes([
        body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7],
    ]);
    let mut sample_rate = VOICE_SAMPLE_RATE as u16;
    let mut chunks = Vec::new();
    let mut pos = 8usize;
    while pos < body.len() {
        let tag = body[pos];
        pos += 1;
        match tag {
            STEAM_VOICE_TAG_SAMPLE_RATE => {
                if pos + 2 > body.len() {
                    return None;
                }
                sample_rate = u16::from_le_bytes([body[pos], body[pos + 1]]);
                pos += 2;
            }
            STEAM_VOICE_TAG_OPUS_PLC => {
                if pos + 2 > body.len() {
                    return None;
                }
                let outer_len = usize::from(u16::from_le_bytes([body[pos], body[pos + 1]]));
                pos += 2;
                if outer_len == 0 || pos + outer_len > body.len() {
                    return None;
                }
                let end = pos + outer_len;
                while pos < end {
                    if pos + 2 > end {
                        return None;
                    }
                    let frame_len = i16::from_le_bytes([body[pos], body[pos + 1]]);
                    pos += 2;
                    if frame_len == STEAM_VOICE_RESET_LEN {
                        chunks.push(SteamVoiceChunk::Reset);
                    } else if frame_len > 0 {
                        let len = frame_len as usize;
                        if pos + 2 + len > end {
                            return None;
                        }
                        let seq = u16::from_le_bytes([body[pos], body[pos + 1]]);
                        pos += 2;
                        chunks.push(SteamVoiceChunk::OpusFrame {
                            seq,
                            data: &body[pos..pos + len],
                        });
                        pos += len;
                    } else {
                        return None;
                    }
                }
            }
            STEAM_VOICE_TAG_SILENCE => {
                if pos + 2 > body.len() {
                    return None;
                }
                let samples = u16::from_le_bytes([body[pos], body[pos + 1]]);
                pos += 2;
                chunks.push(SteamVoiceChunk::Silence { samples });
            }
            _ => return None,
        }
    }
    (!chunks.is_empty()).then_some(SteamVoicePacket {
        steam_id,
        sample_rate,
        chunks,
    })
}

/// Validates CRC32 & framing of a SteamPipe GoldSrc `OPUS_PLC` packet, returning `(sample_rate, Vec<(seq, opus_chunk)>)`.
pub fn decode_steam_opus_plc(packet: &[u8]) -> Option<SteamOpusPlcPacket<'_>> {
    let parsed = parse_steam_voice_packet(packet)?;
    let frames: Vec<(u16, &[u8])> = parsed
        .chunks
        .into_iter()
        .filter_map(|chunk| match chunk {
            SteamVoiceChunk::OpusFrame { seq, data } => Some((seq, data)),
            SteamVoiceChunk::Reset | SteamVoiceChunk::Silence { .. } => None,
        })
        .collect();
    (!frames.is_empty()).then_some((parsed.sample_rate, frames))
}

/// Result of per-packet codec sniffing on incoming `svc_voicedata` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SniffedVoicePacket {
    CsvoOpus { seq: u16 },
    CsvoAdpcm { seq: u16 },
    SteamOpusPlc {
        first_seq: Option<u16>,
        sample_rate: u16,
        has_reset: bool,
        has_silence: bool,
    },
    SpeexNb { quality: u8, seq: u16 },
    MilesAdpcm,
}

/// Auto-detects the wire codec of a single voice packet regardless of `svc_voiceinit`.
pub fn sniff_voice_packet(data: &[u8]) -> SniffedVoicePacket {
    if let Some((seq, tag, _)) = parse_csvo_packet(data) {
        return match tag {
            CSVO_CODEC_OPUS => SniffedVoicePacket::CsvoOpus { seq },
            CSVO_CODEC_ADPCM => SniffedVoicePacket::CsvoAdpcm { seq },
            _ => SniffedVoicePacket::MilesAdpcm,
        };
    }
    if let Some(steam) = parse_steam_voice_packet(data) {
        let first_seq = steam.chunks.iter().find_map(|c| match c {
            SteamVoiceChunk::OpusFrame { seq, .. } => Some(*seq),
            _ => None,
        });
        let has_reset = steam
            .chunks
            .iter()
            .any(|c| matches!(c, SteamVoiceChunk::Reset));
        let has_silence = steam
            .chunks
            .iter()
            .any(|c| matches!(c, SteamVoiceChunk::Silence { .. }));
        return SniffedVoicePacket::SteamOpusPlc {
            first_seq,
            sample_rate: steam.sample_rate,
            has_reset,
            has_silence,
        };
    }
    if let Some((quality, seq, _)) = parse_speex_nb_packet(data) {
        return SniffedVoicePacket::SpeexNb { quality, seq };
    }
    SniffedVoicePacket::MilesAdpcm
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
        self.insert_ordered(seq, payload);
    }

    fn push_control(&mut self, seq: u16, payload: Vec<u8>) {
        self.insert_ordered(seq, payload);
    }

    fn insert_ordered(&mut self, seq: u16, payload: Vec<u8>) {
        if let Some(expected) = self.next_seq
            && seq_lt(seq, expected)
        {
            return;
        }
        if self.queue.len() >= 32
            && let Some(&first) = self.queue.keys().next()
        {
            self.queue.remove(&first);
        }
        self.queue.insert(seq, payload);
        if self.next_seq.is_none() {
            self.next_seq = Some(seq);
        }
    }

    fn next_append_seq(&self) -> u16 {
        self.queue
            .keys()
            .next_back()
            .map_or_else(|| self.next_seq.unwrap_or(0), |k| k.wrapping_add(1))
    }

    fn reset_spurt(&mut self) {
        self.primed = false;
        self.next_seq = None;
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
fn sanitize_opus_rate(rate: u32) -> u32 {
    match rate {
        8_000 | 12_000 | 16_000 | 24_000 | 48_000 => rate,
        _ => VOICE_SAMPLE_RATE,
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeOpusCodec {
    encoder: opus::Encoder,
    decoders: HashMap<u8, (u32, opus::Decoder)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeOpusCodec {
    fn new_encoder() -> Option<opus::Encoder> {
        let mut encoder =
            opus::Encoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
                .ok()?;
        let _ = encoder.set_bitrate(opus::Bitrate::Bits(24_000));
        Some(encoder)
    }

    fn new() -> Option<Self> {
        let encoder = Self::new_encoder()?;
        Some(Self {
            encoder,
            decoders: HashMap::new(),
        })
    }

    fn reset_encoder(&mut self) {
        if let Some(enc) = Self::new_encoder() {
            self.encoder = enc;
        }
    }

    fn reset_decoder(&mut self, slot: u8) {
        self.decoders.remove(&slot);
    }

    fn encode_frame(&mut self, pcm_i16: &[i16]) -> Option<Vec<u8>> {
        let mut out = vec![0u8; 512];
        let len = self.encoder.encode(pcm_i16, &mut out).ok()?;
        out.truncate(len);
        Some(out)
    }

    fn decode_frame(&mut self, slot: u8, sample_rate: u32, payload: Option<&[u8]>) -> Option<Vec<i16>> {
        let rate = sanitize_opus_rate(sample_rate);
        let needs_new = self
            .decoders
            .get(&slot)
            .is_none_or(|(existing_rate, _)| *existing_rate != rate);
        if needs_new {
            let dec = opus::Decoder::new(rate, opus::Channels::Mono).ok()?;
            self.decoders.insert(slot, (rate, dec));
        }
        let (_, dec) = self.decoders.get_mut(&slot)?;
        let max_frame = ((rate as usize) / 50).max(VOICE_FRAME_SAMPLES) * 2;
        let mut out = vec![0i16; max_frame];
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
    steam_compat: bool,
    steam_id: u64,
    was_recording: bool,
    transmitted_in_spurt: bool,
    send_seq: u16,
    capture_buf: Vec<f32>,
    agc: VoiceAgc,
    speakers: HashMap<u8, VoiceJitterBuffer>,
    last_sniffed: HashMap<u8, SniffedVoicePacket>,
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
            steam_compat: true,
            steam_id: DEFAULT_STEAM_VOICE_ID,
            was_recording: false,
            transmitted_in_spurt: false,
            send_seq: 0,
            capture_buf: Vec::with_capacity(VOICE_FRAME_SAMPLES * 2),
            agc: VoiceAgc::default(),
            speakers: HashMap::new(),
            last_sniffed: HashMap::new(),
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

    pub fn steam_compat(&self) -> bool {
        self.steam_compat
    }

    pub fn set_steam_compat(&mut self, enabled: bool) {
        self.steam_compat = enabled;
    }

    pub fn steam_id(&self) -> u64 {
        self.steam_id
    }

    pub fn set_steam_id(&mut self, steam_id: u64) {
        self.steam_id = steam_id;
    }

    /// Updates `-voicerecord` / `+voicerecord` state. When a transmission spurt ends in
    /// Steam compat mode (`voice_steam_compat 1`), emits a `frame_len = -1` reset packet.
    pub fn set_recording(&mut self, recording: bool) {
        if self.was_recording && !recording {
            self.capture_buf.clear();
            if self.transmitted_in_spurt {
                self.transmitted_in_spurt = false;
                if self.uses_steam_outbound() {
                    let reset =
                        encode_steam_opus_plc_reset(self.steam_id, VOICE_SAMPLE_RATE as u16);
                    self.push_outgoing(reset);
                    self.reset_opus_encoder();
                }
            }
        }
        self.was_recording = recording;
    }

    fn push_outgoing(&mut self, pkt: Vec<u8>) {
        if self.outgoing.len() >= 32 {
            self.outgoing.pop_front();
        }
        self.outgoing.push_back(pkt);
    }

    fn uses_steam_outbound(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        let has_opus = self.opus.is_some();
        #[cfg(target_arch = "wasm32")]
        let has_opus = false;
        if !has_opus || self.codec == VoiceCodecKind::None {
            return false;
        }
        match self.codec {
            VoiceCodecKind::SteamOpusPlc => true,
            VoiceCodecKind::NativeOpus
            | VoiceCodecKind::MilesAdpcm
            | VoiceCodecKind::SpeexNb { .. } => self.steam_compat,
            VoiceCodecKind::None => false,
        }
    }

    fn reset_opus_encoder(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ref mut opus) = self.opus {
            opus.reset_encoder();
        }
    }

    fn reset_opus_decoder(&mut self, slot: u8) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(ref mut opus) = self.opus {
            opus.reset_decoder(slot);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = slot;
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
        let sniffed = sniff_voice_packet(data);
        self.last_sniffed.insert(player_slot, sniffed);

        if let Some(steam) = parse_steam_voice_packet(data) {
            let opus_count = steam
                .chunks
                .iter()
                .filter(|c| matches!(c, SteamVoiceChunk::OpusFrame { .. }))
                .count();
            if opus_count == 1
                && let Some(first_seq) = steam.chunks.iter().find_map(|c| match c {
                    SteamVoiceChunk::OpusFrame { seq, .. } => Some(*seq),
                    _ => None,
                })
            {
                let has_reset = steam
                    .chunks
                    .iter()
                    .any(|c| matches!(c, SteamVoiceChunk::Reset));
                let jb = self.speakers.entry(player_slot).or_default();
                jb.push(first_seq, data.to_vec(), now);
                if has_reset {
                    jb.primed = true;
                }
                return;
            }
            let steam_id = steam.steam_id;
            let sample_rate = steam.sample_rate;
            let mut deferred_reset = false;
            for chunk in steam.chunks {
                let jb = self.speakers.entry(player_slot).or_default();
                match chunk {
                    SteamVoiceChunk::OpusFrame { seq, data: frame } => {
                        let sub = encode_steam_opus_plc(steam_id, sample_rate, seq, frame);
                        jb.push(seq, sub, now);
                    }
                    SteamVoiceChunk::Silence { samples } => {
                        let seq = jb.next_append_seq();
                        let sub = encode_steam_voice_packet(
                            steam_id,
                            sample_rate,
                            &[SteamVoiceChunk::Silence { samples }],
                        );
                        jb.push(seq, sub, now);
                    }
                    SteamVoiceChunk::Reset => {
                        if jb.queue.is_empty() && !jb.primed {
                            jb.reset_spurt();
                            deferred_reset = true;
                        } else {
                            let seq = jb.next_append_seq();
                            let sub = encode_steam_opus_plc_reset(steam_id, sample_rate);
                            jb.push_control(seq, sub);
                            jb.primed = true;
                        }
                    }
                }
            }
            if deferred_reset {
                self.reset_opus_decoder(player_slot);
            }
            return;
        }

        let jb = self.speakers.entry(player_slot).or_default();
        match sniffed {
            SniffedVoicePacket::CsvoOpus { seq }
            | SniffedVoicePacket::CsvoAdpcm { seq }
            | SniffedVoicePacket::SpeexNb { seq, .. } => {
                jb.push(seq, data.to_vec(), now);
            }
            SniffedVoicePacket::SteamOpusPlc { .. } | SniffedVoicePacket::MilesAdpcm => {
                let seq = jb.next_append_seq();
                jb.push(seq, data.to_vec(), now);
            }
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
        let outbound_steam = self.uses_steam_outbound();
        let compat_adpcm_fallback = self.steam_compat
            && !outbound_steam
            && matches!(self.codec, VoiceCodecKind::SpeexNb { .. } | VoiceCodecKind::MilesAdpcm);
        let target_rate = if outbound_steam || compat_adpcm_fallback {
            VOICE_SAMPLE_RATE
        } else {
            self.codec.nominal_sample_rate()
        };
        resample_into(
            mono_samples,
            input_sample_rate.max(8000),
            target_rate,
            &mut self.capture_buf,
        );
        self.local_speaking_until = now + 0.3;

        let frame_len = if outbound_steam || compat_adpcm_fallback {
            VOICE_FRAME_SAMPLES
        } else {
            match self.codec {
                VoiceCodecKind::NativeOpus | VoiceCodecKind::SteamOpusPlc => VOICE_FRAME_SAMPLES,
                VoiceCodecKind::MilesAdpcm => 512,
                VoiceCodecKind::SpeexNb { .. } => 160,
                VoiceCodecKind::None => return,
            }
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
            if let Some(pkt) = self.encode_packet(seq, &pcm_i16, outbound_steam, compat_adpcm_fallback)
            {
                self.was_recording = true;
                self.transmitted_in_spurt = true;
                if voice_loopback {
                    self.push_incoming_packet(255, &pkt, now);
                }
                self.push_outgoing(pkt);
            }
        }
    }

    fn encode_packet(
        &mut self,
        seq: u16,
        pcm_i16: &[i16],
        outbound_steam: bool,
        compat_adpcm_fallback: bool,
    ) -> Option<Vec<u8>> {
        if outbound_steam {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(ref mut opus) = self.opus
                && let Some(payload) = opus.encode_frame(pcm_i16)
            {
                return Some(encode_steam_opus_plc(
                    self.steam_id,
                    VOICE_SAMPLE_RATE as u16,
                    seq,
                    &payload,
                ));
            }
        }
        if compat_adpcm_fallback {
            let adpcm = encode_ima_adpcm(pcm_i16);
            return Some(encode_csvo_packet(seq, CSVO_CODEC_ADPCM, &adpcm));
        }
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
                        self.steam_id,
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
            // PLC on missing frame, honoring the speaker's sniffed wire format.
            let (use_opus_plc, rate) = match self.last_sniffed.get(&slot) {
                Some(SniffedVoicePacket::CsvoOpus { .. }) => (true, VOICE_SAMPLE_RATE),
                Some(SniffedVoicePacket::SteamOpusPlc { sample_rate, .. }) => {
                    (true, u32::from(*sample_rate))
                }
                Some(SniffedVoicePacket::CsvoAdpcm { .. }) => (false, VOICE_SAMPLE_RATE),
                Some(SniffedVoicePacket::SpeexNb { .. }) => (false, 8_000),
                Some(SniffedVoicePacket::MilesAdpcm) => (false, 11_025),
                None => (
                    matches!(self.codec, VoiceCodecKind::NativeOpus | VoiceCodecKind::SteamOpusPlc),
                    self.codec.nominal_sample_rate(),
                ),
            };
            #[cfg(not(target_arch = "wasm32"))]
            if use_opus_plc
                && let Some(ref mut opus) = self.opus
                && let Some(pcm) = opus.decode_frame(slot, rate, None)
            {
                return Some((rate, to_f32(pcm)));
            }
            #[cfg(target_arch = "wasm32")]
            let _ = use_opus_plc;
            let len = match rate {
                8_000 => 160,
                11_025 => 512,
                _ => VOICE_FRAME_SAMPLES,
            };
            return Some((rate, vec![0.0; len]));
        };

        if let Some((_seq, tag, payload)) = parse_csvo_packet(pkt) {
            if tag == CSVO_CODEC_OPUS {
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ref mut opus) = self.opus
                    && let Some(pcm) = opus.decode_frame(slot, VOICE_SAMPLE_RATE, Some(payload))
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
        if let Some(steam) = parse_steam_voice_packet(pkt) {
            let mut combined = Vec::new();
            let mut reset_after = false;
            for chunk in steam.chunks {
                match chunk {
                    SteamVoiceChunk::OpusFrame { data, .. } => {
                        #[cfg(not(target_arch = "wasm32"))]
                        if let Some(ref mut opus) = self.opus
                            && let Some(pcm) =
                                opus.decode_frame(slot, u32::from(steam.sample_rate), Some(data))
                        {
                            combined.extend(to_f32(pcm));
                        }
                        #[cfg(target_arch = "wasm32")]
                        let _ = data;
                    }
                    SteamVoiceChunk::Silence { samples } => {
                        let count = usize::from(samples).min(MAX_STEAM_SILENCE_SAMPLES);
                        combined.resize(combined.len() + count, 0.0);
                    }
                    SteamVoiceChunk::Reset => {
                        reset_after = true;
                    }
                }
            }
            if reset_after {
                self.reset_opus_decoder(slot);
                if let Some(jb) = self.speakers.get_mut(&slot) {
                    jb.reset_spurt();
                }
            }
            if !combined.is_empty() {
                return Some((u32::from(steam.sample_rate), combined));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_steam_opus_plc_wire_layout_with_reset_and_silence() {
        let steam_id: u64 = 0x0110_0001_0000_04D2;
        let opus_bytes = [0xF8u8, 0x11, 0x22, 0x33];
        let pkt = encode_steam_voice_packet(
            steam_id,
            24_000,
            &[
                SteamVoiceChunk::OpusFrame {
                    seq: 7,
                    data: &opus_bytes,
                },
                SteamVoiceChunk::Reset,
                SteamVoiceChunk::Silence { samples: 480 },
            ],
        );

        let mut expected_body = Vec::new();
        expected_body.extend_from_slice(&steam_id.to_le_bytes()); // 8 bytes
        expected_body.push(0x0B); // SampleRate tag
        expected_body.extend_from_slice(&24_000u16.to_le_bytes());
        expected_body.push(0x06); // OpusPLC tag
        // outer_len = 2 (frame_len) + 2 (seq) + 4 (opus) + 2 (-1 reset) = 10
        expected_body.extend_from_slice(&10u16.to_le_bytes());
        expected_body.extend_from_slice(&4i16.to_le_bytes());
        expected_body.extend_from_slice(&7u16.to_le_bytes());
        expected_body.extend_from_slice(&opus_bytes);
        expected_body.extend_from_slice(&(-1i16).to_le_bytes());
        expected_body.push(0x00); // Silence tag
        expected_body.extend_from_slice(&480u16.to_le_bytes());

        let expected_crc = crc32fast::hash(&expected_body);
        let mut expected = expected_body;
        expected.extend_from_slice(&expected_crc.to_le_bytes());

        assert_eq!(pkt, expected);

        let parsed = parse_steam_voice_packet(&pkt).expect("valid SteamPipe OPUS_PLC packet");
        assert_eq!(parsed.steam_id, steam_id);
        assert_eq!(parsed.sample_rate, 24_000);
        assert_eq!(
            parsed.chunks,
            vec![
                SteamVoiceChunk::OpusFrame {
                    seq: 7,
                    data: &opus_bytes,
                },
                SteamVoiceChunk::Reset,
                SteamVoiceChunk::Silence { samples: 480 },
            ]
        );
        assert_eq!(
            decode_steam_opus_plc(&pkt),
            Some((24_000, vec![(7, opus_bytes.as_slice())]))
        );

        // Standalone reset packet (`frame_len = -1` / 0xFFFF) parses cleanly too.
        let reset_pkt = encode_steam_opus_plc_reset(steam_id, 24_000);
        let reset_parsed = parse_steam_voice_packet(&reset_pkt).expect("valid reset packet");
        assert_eq!(reset_parsed.chunks, vec![SteamVoiceChunk::Reset]);
        assert!(decode_steam_opus_plc(&reset_pkt).is_none());
    }

    #[test]
    fn per_packet_sniffing_decodes_mixed_speakers_regardless_of_voiceinit() {
        let mut mgr = VoiceManager::new();
        // Server advertised legacy `voice_speex` (like retail hlds.exe).
        mgr.handle_event(
            VoiceEvent::Init {
                codec: "voice_speex".to_string(),
                quality: 5,
            },
            0.0,
        );
        assert_eq!(mgr.codec(), VoiceCodecKind::SpeexNb { quality: 5 });

        let sine_i16: Vec<i16> = (0..960)
            .map(|i| {
                let s = (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 24_000.0).sin();
                (s * 12_000.0) as i16
            })
            .collect();

        // Speaker 1 sends CSVO ADPCM (e.g. cs-web browser).
        let c0 = encode_csvo_packet(0, CSVO_CODEC_ADPCM, &encode_ima_adpcm(&sine_i16[..480]));
        let c1 = encode_csvo_packet(1, CSVO_CODEC_ADPCM, &encode_ima_adpcm(&sine_i16[480..]));
        assert_eq!(sniff_voice_packet(&c0), SniffedVoicePacket::CsvoAdpcm { seq: 0 });
        mgr.handle_event(VoiceEvent::Data { player_slot: 1, data: c0 }, 1.0);
        mgr.handle_event(VoiceEvent::Data { player_slot: 1, data: c1 }, 1.02);

        // Speaker 2 sends SteamOpusPlc (e.g. retail hl.exe) + silence tag.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut enc = NativeOpusCodec::new().unwrap();
            let op0 = enc.encode_frame(&sine_i16[..480]).unwrap();
            let op1 = enc.encode_frame(&sine_i16[480..]).unwrap();
            let s0 = encode_steam_voice_packet(
                76561197960265729,
                24_000,
                &[
                    SteamVoiceChunk::OpusFrame { seq: 10, data: &op0 },
                    SteamVoiceChunk::Silence { samples: 120 },
                ],
            );
            let s1 = encode_steam_opus_plc(76561197960265729, 24_000, 11, &op1);
            assert!(matches!(
                sniff_voice_packet(&s0),
                SniffedVoicePacket::SteamOpusPlc {
                    first_seq: Some(10),
                    sample_rate: 24_000,
                    has_reset: false,
                    has_silence: true,
                }
            ));
            mgr.handle_event(VoiceEvent::Data { player_slot: 2, data: s0 }, 1.0);
            mgr.handle_event(VoiceEvent::Data { player_slot: 2, data: s1 }, 1.02);
        }

        let decoded = mgr.drain_decoded(true);
        let s1_frames: Vec<_> = decoded.iter().filter(|(slot, _, _)| *slot == 1).collect();
        assert_eq!(s1_frames.len(), 2);
        assert!(s1_frames.iter().all(|(_, rate, pcm)| *rate == 24_000 && pcm.len() == 480));

        #[cfg(not(target_arch = "wasm32"))]
        {
            let s2_frames: Vec<_> = decoded.iter().filter(|(slot, _, _)| *slot == 2).collect();
            assert_eq!(s2_frames.len(), 2);
            assert_eq!(s2_frames[0].1, 24_000);
            assert_eq!(s2_frames[0].2.len(), 480 + 120);
            assert!(s2_frames[0].2[480..].iter().all(|&s| s == 0.0));
            assert_eq!(s2_frames[1].2.len(), 480);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn outbound_steam_compat_emits_steam_opus_plc_and_reset_on_release() {
        let mut mgr = VoiceManager::new();
        assert!(mgr.steam_compat(), "voice_steam_compat must default to enabled");
        mgr.handle_event(
            VoiceEvent::Init {
                codec: "voice_speex".to_string(),
                quality: 5,
            },
            0.0,
        );

        let mic: Vec<f32> = (0..960)
            .map(|i| 0.35 * (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 24_000.0).sin())
            .collect();
        mgr.set_recording(true);
        mgr.push_mic_samples(&mic, 24_000, 1.0, true, false, 0.5, 5.0);
        mgr.set_recording(false);

        let out = mgr.take_outgoing();
        assert_eq!(out.len(), 3, "2 Opus frames + 1 end-of-spurt -1 reset packet");
        assert!(decode_steam_opus_plc(&out[0]).is_some());
        assert!(decode_steam_opus_plc(&out[1]).is_some());
        let reset = parse_steam_voice_packet(&out[2]).expect("valid reset trailer packet");
        assert_eq!(reset.chunks, vec![SteamVoiceChunk::Reset]);

        // Subsequent spurt restarting at lower seq after reset is still accepted cleanly.
        let mut rx = VoiceManager::new();
        rx.handle_event(
            VoiceEvent::Data {
                player_slot: 5,
                data: out[0].clone(),
            },
            1.0,
        );
        rx.handle_event(
            VoiceEvent::Data {
                player_slot: 5,
                data: out[1].clone(),
            },
            1.02,
        );
        rx.handle_event(
            VoiceEvent::Data {
                player_slot: 5,
                data: out[2].clone(),
            },
            1.04,
        );
        let spurt1 = rx.drain_decoded(true);
        assert_eq!(spurt1.len(), 2);

        // New spurt with seq = 0 must not be dropped by the previous spurt's next_seq!
        let mut enc = NativeOpusCodec::new().unwrap();
        let pcm_i16: Vec<i16> = vec![1000; 480];
        let op = enc.encode_frame(&pcm_i16).unwrap();
        let spurt2_a = encode_steam_opus_plc(DEFAULT_STEAM_VOICE_ID, 24_000, 0, &op);
        let spurt2_b = encode_steam_opus_plc(DEFAULT_STEAM_VOICE_ID, 24_000, 1, &op);
        rx.handle_event(VoiceEvent::Data { player_slot: 5, data: spurt2_a }, 2.0);
        rx.handle_event(VoiceEvent::Data { player_slot: 5, data: spurt2_b }, 2.02);
        let spurt2 = rx.drain_decoded(true);
        assert_eq!(spurt2.len(), 2);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn outbound_steam_compat_disabled_uses_native_csvo_for_voice_opus() {
        let mut mgr = VoiceManager::new();
        mgr.set_steam_compat(false);
        mgr.handle_event(
            VoiceEvent::Init {
                codec: "voice_opus".to_string(),
                quality: 4,
            },
            0.0,
        );
        let mic = vec![0.25f32; 480];
        mgr.set_recording(true);
        mgr.push_mic_samples(&mic, 24_000, 1.0, true, false, 0.5, 5.0);
        mgr.set_recording(false);
        let out = mgr.take_outgoing();
        assert_eq!(out.len(), 1, "no Steam reset packet when steam_compat=false");
        assert!(matches!(
            sniff_voice_packet(&out[0]),
            SniffedVoicePacket::CsvoOpus { seq: 0 }
        ));
    }
}
