//! Dedicated server cross-client voice bridge (`sv_voice_transcode`, default `"1"`):
//! - Passes `SteamOpusPlc` packets straight through untouched to `hl.exe` and native
//!   `cs-client` listeners (zero generation loss).
//! - Passes `CSVO` IMA-ADPCM packets straight through untouched between browser `cs-web` slots.
//! - Transcodes `SteamOpusPlc <-> CSVO ADPCM` (and `CSVO Opus -> CSVO ADPCM`) between
//!   native/retail and browser `cs-web` slots so all three clients share two-way voice chat.

use std::collections::HashMap;

use cs_engine::voice_bridge::{VoiceBridge, VoiceBridgeAction, VoiceSlotKind};

const VOICE_SAMPLE_RATE: u32 = 24_000;
const VOICE_FRAME_SAMPLES: usize = 480;
const MAX_ADPCM_SAMPLES_PER_PACKET: usize = 7_680;
const MAX_ADPCM_BUFFER_SAMPLES: usize = VOICE_FRAME_SAMPLES * 16;
const MAX_STEAM_SILENCE_SAMPLES: usize = 48_000;

const STEAM_VOICE_TAG_SILENCE: u8 = 0x00;
const STEAM_VOICE_TAG_OPUS_PLC: u8 = 0x06;
const STEAM_VOICE_TAG_SAMPLE_RATE: u8 = 0x0B;
const STEAM_VOICE_RESET_LEN: i16 = -1;
const SYNTHETIC_STEAM_ID_BASE: u64 = 76_561_197_960_265_728;

const CSVO_MAGIC: [u8; 4] = *b"CSVO";
const CSVO_CODEC_OPUS: u8 = 1;
const CSVO_CODEC_ADPCM: u8 = 2;

const IMA_INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const IMA_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

#[derive(Default)]
struct ImaState {
    predictor: i32,
    step_index: i32,
}

impl ImaState {
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

fn encode_ima_adpcm(samples: &[i16]) -> Vec<u8> {
    let mut state = ImaState::default();
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

fn decode_ima_adpcm(data: &[u8]) -> Option<Vec<i16>> {
    if data.len() < 4 {
        return None;
    }
    let mut state = ImaState {
        predictor: i32::from(i16::from_le_bytes([data[0], data[1]])),
        step_index: i32::from(data[2]).clamp(0, 88),
    };
    let mut out = Vec::with_capacity((data.len() - 4) * 2);
    for &b in &data[4..] {
        out.push(state.decode_nibble(b & 0x0F));
        out.push(state.decode_nibble((b >> 4) & 0x0F));
    }
    Some(out)
}

fn encode_csvo_packet(seq: u16, codec_tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(7 + payload.len());
    out.extend_from_slice(&CSVO_MAGIC);
    out.extend_from_slice(&seq.to_le_bytes());
    out.push(codec_tag);
    out.extend_from_slice(payload);
    out
}

fn parse_csvo_packet(packet: &[u8]) -> Option<(u16, u8, &[u8])> {
    if packet.len() < 7 || packet[0..4] != CSVO_MAGIC {
        return None;
    }
    let seq = u16::from_le_bytes([packet[4], packet[5]]);
    let tag = packet[6];
    Some((seq, tag, &packet[7..]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamChunk<'a> {
    OpusFrame { seq: u16, data: &'a [u8] },
    Reset,
    Silence { samples: u16 },
}

struct ParsedSteamPacket<'a> {
    steam_id: u64,
    sample_rate: u16,
    chunks: Vec<SteamChunk<'a>>,
}

fn parse_steam_voice_packet(packet: &[u8]) -> Option<ParsedSteamPacket<'_>> {
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
                        chunks.push(SteamChunk::Reset);
                    } else if frame_len > 0 {
                        let len = frame_len as usize;
                        if pos + 2 + len > end {
                            return None;
                        }
                        let seq = u16::from_le_bytes([body[pos], body[pos + 1]]);
                        pos += 2;
                        chunks.push(SteamChunk::OpusFrame {
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
                chunks.push(SteamChunk::Silence { samples });
            }
            _ => return None,
        }
    }
    (!chunks.is_empty()).then_some(ParsedSteamPacket {
        steam_id,
        sample_rate,
        chunks,
    })
}

fn encode_steam_opus_frames(steam_id: u64, sample_rate: u16, frames: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let inner_len: usize = frames.iter().map(|(_, data)| 4 + data.len()).sum();
    let mut out = Vec::with_capacity(18 + inner_len);
    out.extend_from_slice(&steam_id.to_le_bytes());
    out.push(STEAM_VOICE_TAG_SAMPLE_RATE);
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.push(STEAM_VOICE_TAG_OPUS_PLC);
    out.extend_from_slice(&(u16::try_from(inner_len).unwrap_or(u16::MAX)).to_le_bytes());
    for (seq, data) in frames {
        let len = i16::try_from(data.len().min(i16::MAX as usize)).unwrap_or(i16::MAX);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&data[..len as usize]);
    }
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

fn sanitize_opus_rate(rate: u32) -> u32 {
    match rate {
        8_000 | 12_000 | 16_000 | 24_000 | 48_000 => rate,
        _ => VOICE_SAMPLE_RATE,
    }
}

fn resample_i16(src: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if src.is_empty() || src_rate == dst_rate || src_rate == 0 {
        return src.to_vec();
    }
    let ratio = f64::from(src_rate) / f64::from(dst_rate);
    let out_len = ((src.len() as f64) / ratio).floor() as usize;
    let mut dst = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = (i as f64) * ratio;
        let idx = pos as usize;
        let frac = (pos - (idx as f64)) as f32;
        let s0 = f32::from(src[idx.min(src.len() - 1)]);
        let s1 = f32::from(src[(idx + 1).min(src.len() - 1)]);
        dst.push((s0 + (s1 - s0) * frac).round().clamp(-32_768.0, 32_767.0) as i16);
    }
    dst
}

struct SlotTranscoder {
    encoder: opus::Encoder,
    decoder: Option<(u32, opus::Decoder)>,
    last_steam_id: Option<u64>,
    adpcm_pcm_buf: Vec<i16>,
    next_steam_seq: u16,
    next_web_seq: u16,
}

impl SlotTranscoder {
    fn new() -> Option<Self> {
        let mut encoder =
            opus::Encoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)
                .ok()?;
        let _ = encoder.set_bitrate(opus::Bitrate::Bits(24_000));
        Some(Self {
            encoder,
            decoder: None,
            last_steam_id: None,
            adpcm_pcm_buf: Vec::with_capacity(VOICE_FRAME_SAMPLES * 2),
            next_steam_seq: 0,
            next_web_seq: 0,
        })
    }

    fn reset_decoder(&mut self) {
        self.decoder = None;
    }

    fn decode_opus(&mut self, sample_rate: u32, payload: &[u8]) -> Option<Vec<i16>> {
        let rate = sanitize_opus_rate(sample_rate);
        if self.decoder.as_ref().is_none_or(|(r, _)| *r != rate) {
            let dec = opus::Decoder::new(rate, opus::Channels::Mono).ok()?;
            self.decoder = Some((rate, dec));
        }
        let (_, dec) = self.decoder.as_mut()?;
        let max_frame = ((rate as usize) / 50).max(VOICE_FRAME_SAMPLES) * 2;
        let mut out = vec![0i16; max_frame];
        let samples = dec.decode(payload, &mut out, false).ok()?;
        out.truncate(samples);
        Some(out)
    }

    fn encode_opus_frame(&mut self, pcm_i16: &[i16]) -> Option<Vec<u8>> {
        let mut out = vec![0u8; 512];
        let len = self.encoder.encode(pcm_i16, &mut out).ok()?;
        out.truncate(len);
        Some(out)
    }
}

/// Real Opus <-> IMA-ADPCM voice bridge installed by `cs-server`.
pub struct ServerVoiceBridge {
    slots: HashMap<u8, SlotTranscoder>,
}

impl Default for ServerVoiceBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerVoiceBridge {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }

    fn slot_mut(&mut self, sender_slot: u8) -> Option<&mut SlotTranscoder> {
        if !self.slots.contains_key(&sender_slot)
            && let Some(t) = SlotTranscoder::new()
        {
            self.slots.insert(sender_slot, t);
        }
        self.slots.get_mut(&sender_slot)
    }

    fn transcode_csvo_adpcm_to_steam(&mut self, sender_slot: u8, payload: &[u8]) -> VoiceBridgeAction {
        let Some(pcm) = decode_ima_adpcm(payload) else {
            return VoiceBridgeAction::Passthrough;
        };
        let Some(slot) = self.slot_mut(sender_slot) else {
            return VoiceBridgeAction::Passthrough;
        };
        if slot.adpcm_pcm_buf.len() + pcm.len() > MAX_ADPCM_BUFFER_SAMPLES {
            slot.adpcm_pcm_buf.clear();
        }
        slot.adpcm_pcm_buf.extend_from_slice(&pcm);

        let mut encoded_frames = Vec::new();
        while slot.adpcm_pcm_buf.len() >= VOICE_FRAME_SAMPLES {
            let frame: Vec<i16> = slot.adpcm_pcm_buf.drain(..VOICE_FRAME_SAMPLES).collect();
            if let Some(opus_bytes) = slot.encode_opus_frame(&frame) {
                let seq = slot.next_steam_seq;
                slot.next_steam_seq = seq.wrapping_add(1);
                encoded_frames.push((seq, opus_bytes));
            }
        }
        if encoded_frames.is_empty() {
            return VoiceBridgeAction::Suppress;
        }
        let steam_id = SYNTHETIC_STEAM_ID_BASE + u64::from(sender_slot) + 1;
        VoiceBridgeAction::Transcoded(encode_steam_opus_frames(
            steam_id,
            VOICE_SAMPLE_RATE as u16,
            &encoded_frames,
        ))
    }

    fn transcode_csvo_opus_to_adpcm(
        &mut self,
        sender_slot: u8,
        seq: u16,
        payload: &[u8],
    ) -> VoiceBridgeAction {
        let Some(slot) = self.slot_mut(sender_slot) else {
            return VoiceBridgeAction::Passthrough;
        };
        let Some(mut pcm) = slot.decode_opus(VOICE_SAMPLE_RATE, payload) else {
            return VoiceBridgeAction::Suppress;
        };
        if pcm.is_empty() {
            return VoiceBridgeAction::Suppress;
        }
        pcm.truncate(MAX_ADPCM_SAMPLES_PER_PACKET);
        let adpcm = encode_ima_adpcm(&pcm);
        VoiceBridgeAction::Transcoded(encode_csvo_packet(seq, CSVO_CODEC_ADPCM, &adpcm))
    }

    fn transcode_steam_to_csvo_adpcm(&mut self, sender_slot: u8, data: &[u8]) -> VoiceBridgeAction {
        let Some(parsed) = parse_steam_voice_packet(data) else {
            return VoiceBridgeAction::Passthrough;
        };
        let Some(slot) = self.slot_mut(sender_slot) else {
            return VoiceBridgeAction::Passthrough;
        };
        if slot.last_steam_id != Some(parsed.steam_id) {
            slot.reset_decoder();
            slot.last_steam_id = Some(parsed.steam_id);
        }

        let mut combined_pcm = Vec::new();
        let mut first_seq: Option<u16> = None;
        let src_rate = sanitize_opus_rate(u32::from(parsed.sample_rate));

        for chunk in parsed.chunks {
            match chunk {
                SteamChunk::OpusFrame { seq, data: opus_bytes } => {
                    if first_seq.is_none() {
                        first_seq = Some(seq);
                    }
                    if let Some(decoded) = slot.decode_opus(src_rate, opus_bytes) {
                        let resampled = resample_i16(&decoded, src_rate, VOICE_SAMPLE_RATE);
                        combined_pcm.extend_from_slice(&resampled);
                    }
                }
                SteamChunk::Silence { samples } => {
                    let raw = usize::from(samples).min(MAX_STEAM_SILENCE_SAMPLES);
                    let scaled = if src_rate != VOICE_SAMPLE_RATE && src_rate > 0 {
                        (raw * (VOICE_SAMPLE_RATE as usize)) / (src_rate as usize)
                    } else {
                        raw
                    };
                    let room = MAX_ADPCM_SAMPLES_PER_PACKET.saturating_sub(combined_pcm.len());
                    combined_pcm.resize(combined_pcm.len() + scaled.min(room), 0);
                }
                SteamChunk::Reset => {
                    slot.reset_decoder();
                }
            }
        }

        if combined_pcm.is_empty() {
            return VoiceBridgeAction::Suppress;
        }
        combined_pcm.truncate(MAX_ADPCM_SAMPLES_PER_PACKET);
        let seq = first_seq.unwrap_or_else(|| {
            let s = slot.next_web_seq;
            slot.next_web_seq = s.wrapping_add(1);
            s
        });
        let adpcm = encode_ima_adpcm(&combined_pcm);
        VoiceBridgeAction::Transcoded(encode_csvo_packet(seq, CSVO_CODEC_ADPCM, &adpcm))
    }
}

impl VoiceBridge for ServerVoiceBridge {
    fn bridge_packet(
        &mut self,
        sender_slot: u8,
        _sender_kind: VoiceSlotKind,
        target_kind: VoiceSlotKind,
        data: &[u8],
    ) -> VoiceBridgeAction {
        if let Some((seq, tag, payload)) = parse_csvo_packet(data) {
            return match (tag, target_kind) {
                (CSVO_CODEC_ADPCM, VoiceSlotKind::WebBrowser) => VoiceBridgeAction::Passthrough,
                (CSVO_CODEC_ADPCM, VoiceSlotKind::NativeOrRetail) => {
                    self.transcode_csvo_adpcm_to_steam(sender_slot, payload)
                }
                (CSVO_CODEC_OPUS, VoiceSlotKind::NativeOrRetail) => VoiceBridgeAction::Passthrough,
                (CSVO_CODEC_OPUS, VoiceSlotKind::WebBrowser) => {
                    self.transcode_csvo_opus_to_adpcm(sender_slot, seq, payload)
                }
                _ => VoiceBridgeAction::Passthrough,
            };
        }

        if parse_steam_voice_packet(data).is_some() {
            return match target_kind {
                VoiceSlotKind::NativeOrRetail => VoiceBridgeAction::Passthrough,
                VoiceSlotKind::WebBrowser => self.transcode_steam_to_csvo_adpcm(sender_slot, data),
            };
        }

        VoiceBridgeAction::Passthrough
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_440_i16(samples: usize) -> Vec<i16> {
        (0..samples)
            .map(|i| {
                let s = (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / (VOICE_SAMPLE_RATE as f32))
                    .sin();
                (s * 12_000.0) as i16
            })
            .collect()
    }

    fn rms_i16(samples: &[i16]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        (sum_sq / (samples.len() as f64)).sqrt()
    }

    #[test]
    fn steam_opus_plc_to_csvo_adpcm_and_back_roundtrip() {
        let mut bridge = ServerVoiceBridge::new();

        // Build 2 valid SteamOpusPlc packets from a native/hl.exe speaker (440 Hz tone).
        let pcm = sine_440_i16(VOICE_FRAME_SAMPLES * 2);
        let mut enc = opus::Encoder::new(
            VOICE_SAMPLE_RATE,
            opus::Channels::Mono,
            opus::Application::Voip,
        )
        .unwrap();
        let _ = enc.set_bitrate(opus::Bitrate::Bits(24_000));

        let mut steam_pkts = Vec::new();
        for (seq, frame) in pcm.as_chunks::<VOICE_FRAME_SAMPLES>().0.iter().enumerate() {
            let mut buf = vec![0u8; 512];
            let len = enc.encode(frame, &mut buf).unwrap();
            buf.truncate(len);
            steam_pkts.push(encode_steam_opus_frames(
                SYNTHETIC_STEAM_ID_BASE + 99,
                VOICE_SAMPLE_RATE as u16,
                &[(seq as u16, buf)],
            ));
        }

        // 1. Native/retail target receives SteamOpusPlc untouched (zero generation loss).
        for pkt in &steam_pkts {
            assert_eq!(
                bridge.bridge_packet(
                    0,
                    VoiceSlotKind::NativeOrRetail,
                    VoiceSlotKind::NativeOrRetail,
                    pkt,
                ),
                VoiceBridgeAction::Passthrough
            );
        }

        // 2. WebBrowser target receives transcoded CSVO ADPCM packets with audible energy.
        let mut web_pkts = Vec::new();
        for pkt in &steam_pkts {
            match bridge.bridge_packet(
                0,
                VoiceSlotKind::NativeOrRetail,
                VoiceSlotKind::WebBrowser,
                pkt,
            ) {
                VoiceBridgeAction::Transcoded(csvo) => {
                    let (_seq, tag, payload) = parse_csvo_packet(&csvo).expect("valid CSVO");
                    assert_eq!(tag, CSVO_CODEC_ADPCM);
                    let decoded = decode_ima_adpcm(payload).expect("valid ADPCM");
                    assert_eq!(decoded.len(), VOICE_FRAME_SAMPLES);
                    web_pkts.push(csvo);
                }
                other => panic!("expected Transcoded CSVO ADPCM, got {other:?}"),
            }
        }
        // Check last transcoded ADPCM frame has strong tone energy (not silence).
        let (_, _, last_adpcm_payload) = parse_csvo_packet(&web_pkts[1]).unwrap();
        let last_adpcm_pcm = decode_ima_adpcm(last_adpcm_payload).unwrap();
        assert!(
            rms_i16(&last_adpcm_pcm) > 2000.0,
            "transcoded ADPCM RMS too low: {}",
            rms_i16(&last_adpcm_pcm)
        );

        // 3. Browser sends those CSVO ADPCM packets back -> NativeOrRetail gets valid SteamOpusPlc!
        let mut dec = opus::Decoder::new(VOICE_SAMPLE_RATE, opus::Channels::Mono).unwrap();
        let mut decoded_back_pcm = Vec::new();
        for csvo in &web_pkts {
            assert_eq!(
                bridge.bridge_packet(1, VoiceSlotKind::WebBrowser, VoiceSlotKind::WebBrowser, csvo),
                VoiceBridgeAction::Passthrough
            );
            match bridge.bridge_packet(
                1,
                VoiceSlotKind::WebBrowser,
                VoiceSlotKind::NativeOrRetail,
                csvo,
            ) {
                VoiceBridgeAction::Transcoded(steam_out) => {
                    let parsed =
                        parse_steam_voice_packet(&steam_out).expect("valid transcoded SteamOpusPlc");
                    assert_eq!(parsed.sample_rate, VOICE_SAMPLE_RATE as u16);
                    for chunk in parsed.chunks {
                        if let SteamChunk::OpusFrame { data, .. } = chunk {
                            let mut out = vec![0i16; VOICE_FRAME_SAMPLES * 2];
                            let n = dec.decode(data, &mut out, false).unwrap();
                            decoded_back_pcm.extend_from_slice(&out[..n]);
                        }
                    }
                }
                other => panic!("expected Transcoded SteamOpusPlc, got {other:?}"),
            }
        }
        assert_eq!(decoded_back_pcm.len(), VOICE_FRAME_SAMPLES * 2);
        assert!(
            rms_i16(&decoded_back_pcm[VOICE_FRAME_SAMPLES..]) > 2000.0,
            "roundtrip Opus RMS too low: {}",
            rms_i16(&decoded_back_pcm[VOICE_FRAME_SAMPLES..])
        );
    }

    #[test]
    fn steam_silence_and_reset_chunks_bridge_cleanly_to_web() {
        let mut bridge = ServerVoiceBridge::new();

        // Silence packet (0x00 tag) -> CSVO ADPCM silence.
        let mut body = Vec::new();
        body.extend_from_slice(&(SYNTHETIC_STEAM_ID_BASE + 5).to_le_bytes());
        body.push(STEAM_VOICE_TAG_SAMPLE_RATE);
        body.extend_from_slice(&24_000u16.to_le_bytes());
        body.push(STEAM_VOICE_TAG_SILENCE);
        body.extend_from_slice(&480u16.to_le_bytes());
        let crc = crc32fast::hash(&body);
        let mut silence_pkt = body;
        silence_pkt.extend_from_slice(&crc.to_le_bytes());

        match bridge.bridge_packet(
            2,
            VoiceSlotKind::NativeOrRetail,
            VoiceSlotKind::WebBrowser,
            &silence_pkt,
        ) {
            VoiceBridgeAction::Transcoded(csvo) => {
                let (_seq, tag, payload) = parse_csvo_packet(&csvo).unwrap();
                assert_eq!(tag, CSVO_CODEC_ADPCM);
                let pcm = decode_ima_adpcm(payload).unwrap();
                assert_eq!(pcm.len(), 480);
                assert!(pcm.iter().all(|&s| s == 0));
            }
            other => panic!("expected Transcoded silence, got {other:?}"),
        }

        // Standalone reset (-1 / 0xFFFF) -> Suppress for web, Passthrough for native/retail.
        let mut rbody = Vec::new();
        rbody.extend_from_slice(&(SYNTHETIC_STEAM_ID_BASE + 5).to_le_bytes());
        rbody.push(STEAM_VOICE_TAG_SAMPLE_RATE);
        rbody.extend_from_slice(&24_000u16.to_le_bytes());
        rbody.push(STEAM_VOICE_TAG_OPUS_PLC);
        rbody.extend_from_slice(&2u16.to_le_bytes());
        rbody.extend_from_slice(&STEAM_VOICE_RESET_LEN.to_le_bytes());
        let rcrc = crc32fast::hash(&rbody);
        let mut reset_pkt = rbody;
        reset_pkt.extend_from_slice(&rcrc.to_le_bytes());

        assert_eq!(
            bridge.bridge_packet(
                2,
                VoiceSlotKind::NativeOrRetail,
                VoiceSlotKind::NativeOrRetail,
                &reset_pkt,
            ),
            VoiceBridgeAction::Passthrough
        );
        assert_eq!(
            bridge.bridge_packet(
                2,
                VoiceSlotKind::NativeOrRetail,
                VoiceSlotKind::WebBrowser,
                &reset_pkt,
            ),
            VoiceBridgeAction::Suppress
        );
    }

    #[test]
    fn udp_and_websocket_server_bridges_voice_two_ways() {
        use std::time::Duration;
        use cs_clientcore::{ConnState, Connection, ConnectionConfig};
        use cs_cvar::CVarRegistry;
        use cs_engine::{Server, ServerConfig, ServerMap};
        use cs_proto48::crypto::md5_hex;
        use cs_proto48::delta::DeltaRegistry;
        use cs_sim::pmove::MoveParams;
        use cs_sim::walk_game::WalkGame;
        use cs_transport::{
            MultiTransport, PeerAddr, PeerKind, UdpTransport, WsClientTransport, WsServerConfig,
            WsServerTransport,
        };

        let udp = UdpTransport::bind("127.0.0.1:0").unwrap();
        let udp_addr = udp.local_addr();
        let ws_srv = WsServerTransport::bind(WsServerConfig {
            bind_addr: "127.0.0.1:0".into(),
            static_dir: None,
            ws_path: "/ws".into(),
            max_clients: 8,
            upgrade_hook: None,
            http_hook: None,
        })
        .unwrap();
        let ws_addr = ws_srv.local_addr();

        let mut multi = MultiTransport::new();
        multi.add(PeerKind::Udp, udp);
        multi.add(PeerKind::Web, ws_srv);

        let mut server = Server::new(
            ServerConfig {
                max_clients: 8,
                ..ServerConfig::default()
            },
            ServerMap::stub("de_dust2"),
            WalkGame::new(MoveParams::default()),
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        server.set_voice_bridge(Box::new(ServerVoiceBridge::new()));

        let mut udp_conn = Connection::new(
            UdpTransport::bind("127.0.0.1:0").unwrap(),
            PeerAddr::Udp(udp_addr),
            ConnectionConfig::new("hl-player", 76_561_197_960_265_740, &md5_hex(b"udp")),
        );
        let mut ws_conn = Connection::new(
            WsClientTransport::connect(&format!("ws://{ws_addr}/ws")).unwrap(),
            PeerAddr::Web(0),
            ConnectionConfig::new("web-player", 76_561_197_960_265_741, &md5_hex(b"ws")),
        );
        udp_conn.connect();
        ws_conn.connect();

        let mut now = 0.0;
        for _ in 0..800 {
            now += 0.01;
            udp_conn.pump(now);
            ws_conn.pump(now);
            server.frame(&mut multi, now);
            if udp_conn.state() == &ConnState::InGame && ws_conn.state() == &ConnState::InGame {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(udp_conn.state(), &ConnState::InGame);
        assert_eq!(ws_conn.state(), &ConnState::InGame);
        let _ = udp_conn.fx_mut().voice.drain();
        let _ = ws_conn.fx_mut().voice.drain();

        // 1. UDP client sends SteamOpusPlc -> WS client receives CSVO ADPCM.
        let pcm = sine_440_i16(VOICE_FRAME_SAMPLES);
        let mut enc = opus::Encoder::new(
            VOICE_SAMPLE_RATE,
            opus::Channels::Mono,
            opus::Application::Voip,
        )
        .unwrap();
        let mut opus_buf = vec![0u8; 512];
        let opus_len = enc.encode(&pcm, &mut opus_buf).unwrap();
        opus_buf.truncate(opus_len);
        let steam_pkt = encode_steam_opus_frames(
            76_561_197_960_265_740,
            VOICE_SAMPLE_RATE as u16,
            &[(1, opus_buf)],
        );
        udp_conn.send_voice_data(&steam_pkt);

        let mut ws_got_csvo = false;
        for _ in 0..100 {
            now += 0.01;
            udp_conn.pump(now);
            ws_conn.pump(now);
            server.frame(&mut multi, now);
            for ev in ws_conn.fx_mut().voice.drain() {
                if let cs_clientcore::VoiceEvent::Data { data, .. } = ev {
                    let (_, tag, _) = parse_csvo_packet(&data).expect("WS must receive CSVO");
                    assert_eq!(tag, CSVO_CODEC_ADPCM);
                    ws_got_csvo = true;
                }
            }
            if ws_got_csvo {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ws_got_csvo, "WebSocket client must receive transcoded CSVO ADPCM");

        // 2. WS client sends CSVO ADPCM -> UDP client receives valid CRC32 SteamOpusPlc.
        let csvo_pkt = encode_csvo_packet(5, CSVO_CODEC_ADPCM, &encode_ima_adpcm(&pcm));
        ws_conn.send_voice_data(&csvo_pkt);

        let mut udp_got_steam = false;
        for _ in 0..100 {
            now += 0.01;
            udp_conn.pump(now);
            ws_conn.pump(now);
            server.frame(&mut multi, now);
            for ev in udp_conn.fx_mut().voice.drain() {
                if let cs_clientcore::VoiceEvent::Data { data, .. } = ev {
                    let parsed =
                        parse_steam_voice_packet(&data).expect("UDP must receive SteamOpusPlc");
                    assert_eq!(parsed.sample_rate, VOICE_SAMPLE_RATE as u16);
                    assert!(matches!(
                        parsed.chunks.first(),
                        Some(SteamChunk::OpusFrame { .. })
                    ));
                    udp_got_steam = true;
                }
            }
            if udp_got_steam {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(udp_got_steam, "UDP client must receive transcoded SteamOpusPlc");
    }
}
