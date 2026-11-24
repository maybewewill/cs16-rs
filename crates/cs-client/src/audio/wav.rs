//! Pure-Rust RIFF WAVE decoder with GoldSrc `cue ` and `smpl` loop point extraction.

use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct DecodedWav {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved normalized samples in `[-1.0, 1.0]`.
    pub samples: Arc<[f32]>,
    /// Optional loop start frame (not sample scalar index).
    pub loop_start: Option<usize>,
    /// Optional exclusive loop end frame.
    pub loop_end: Option<usize>,
}

impl DecodedWav {
    pub fn frame_count(&self) -> usize {
        let ch = usize::from(self.channels.max(1));
        self.samples.len() / ch
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    TooShort,
    InvalidRiffHeader,
    UnsupportedFormat(u16),
    UnsupportedChannels(u16),
    UnsupportedBitsPerSample(u16),
    InvalidSampleRate,
    MissingFmtOrData,
}

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "WAV buffer too short"),
            Self::InvalidRiffHeader => write!(f, "invalid RIFF/WAVE header"),
            Self::UnsupportedFormat(t) => write!(f, "unsupported WAV format tag {t}"),
            Self::UnsupportedChannels(c) => write!(f, "unsupported WAV channel count {c}"),
            Self::UnsupportedBitsPerSample(b) => write!(f, "unsupported WAV bit depth {b}"),
            Self::InvalidSampleRate => write!(f, "invalid WAV sample rate"),
            Self::MissingFmtOrData => write!(f, "WAV missing fmt or data chunk"),
        }
    }
}

impl std::error::Error for WavError {}

fn read_u16_le(bytes: &[u8], off: usize) -> Option<u16> {
    let slice = bytes.get(off..off + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], off: usize) -> Option<u32> {
    let slice = bytes.get(off..off + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Parse RIFF `fmt `, `data`, `cue `, and `smpl` chunks into normalized interleaved `f32` PCM.
pub fn decode_wav(bytes: &[u8]) -> Result<DecodedWav, WavError> {
    if bytes.len() < 12 {
        return Err(WavError::TooShort);
    }
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::InvalidRiffHeader);
    }

    let mut format_tag = None;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut data_slice: Option<&[u8]> = None;
    let mut cue_start: Option<usize> = None;
    let mut smpl_loop: Option<(usize, usize)> = None;

    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let chunk_len = read_u32_le(bytes, pos + 4).ok_or(WavError::TooShort)? as usize;
        let payload_start = pos + 8;
        let payload_end = payload_start.saturating_add(chunk_len).min(bytes.len());
        let chunk = &bytes[payload_start..payload_end];

        match id {
            b"fmt " => {
                if chunk.len() < 16 {
                    return Err(WavError::TooShort);
                }
                let tag = read_u16_le(chunk, 0).unwrap_or(0);
                let ch = read_u16_le(chunk, 2).unwrap_or(0);
                let rate = read_u32_le(chunk, 4).unwrap_or(0);
                let bps = read_u16_le(chunk, 14).unwrap_or(0);
                if tag != 1 && tag != 3 {
                    return Err(WavError::UnsupportedFormat(tag));
                }
                if ch != 1 && ch != 2 {
                    return Err(WavError::UnsupportedChannels(ch));
                }
                if rate == 0 || rate > 384_000 {
                    return Err(WavError::InvalidSampleRate);
                }
                if (tag == 1 && bps != 8 && bps != 16) || (tag == 3 && bps != 32) {
                    return Err(WavError::UnsupportedBitsPerSample(bps));
                }
                format_tag = Some(tag);
                channels = ch;
                sample_rate = rate;
                bits_per_sample = bps;
            }
            b"data" => {
                if data_slice.is_none() {
                    data_slice = Some(chunk);
                }
            }
            b"cue " => {
                // `dwCuePoints` (4 bytes) followed by 24-byte `CuePoint` structs (`dwSampleOffset` at +20).
                if chunk.len() >= 28 {
                    let count = read_u32_le(chunk, 0).unwrap_or(0);
                    if count >= 1
                        && let Some(off) = read_u32_le(chunk, 4 + 20)
                    {
                        cue_start = Some(off as usize);
                    }
                }
            }
            b"smpl" => {
                // `cSampleLoops` at +28 (4 bytes), first `SampleLoop` at +36 (`dwStart` +8, `dwEnd` +12).
                if chunk.len() >= 36 + 24 {
                    let count = read_u32_le(chunk, 28).unwrap_or(0);
                    if count >= 1
                        && let (Some(start), Some(end)) =
                            (read_u32_le(chunk, 36 + 8), read_u32_le(chunk, 36 + 12))
                    {
                        smpl_loop = Some((start as usize, end as usize));
                    }
                }
            }
            _ => {}
        }

        let padded = chunk_len.saturating_add(chunk_len & 1);
        let Some(next) = payload_start.checked_add(padded) else {
            break;
        };
        if next <= pos {
            break;
        }
        pos = next;
    }

    let (Some(tag), Some(pcm)) = (format_tag, data_slice) else {
        return Err(WavError::MissingFmtOrData);
    };

    let bytes_per_sample = usize::from(bits_per_sample / 8);
    let block_align = bytes_per_sample * usize::from(channels);
    if block_align == 0 {
        return Err(WavError::MissingFmtOrData);
    }
    let usable_bytes = (pcm.len() / block_align) * block_align;
    let pcm = &pcm[..usable_bytes];

    let samples: Vec<f32> = match (tag, bits_per_sample) {
        (1, 8) => pcm.iter().map(|&b| (f32::from(b) - 128.0) / 128.0).collect(),
        (1, 16) => pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f32::from(i16::from_le_bytes(*c)) / 32768.0)
            .collect(),
        (3, 32) => pcm
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                let s = f32::from_le_bytes(*c);
                if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 }
            })
            .collect(),
        _ => return Err(WavError::UnsupportedBitsPerSample(bits_per_sample)),
    };

    let frame_count = samples.len() / usize::from(channels);
    let (loop_start, loop_end) = if let Some((s, e)) = smpl_loop
        && frame_count > 0
        && s < e
    {
        let clamped_s = s.min(frame_count.saturating_sub(1));
        let clamped_e = e.min(frame_count);
        if clamped_s < clamped_e {
            (Some(clamped_s), Some(clamped_e))
        } else {
            (None, None)
        }
    } else if let Some(s) = cue_start
        && frame_count > 0
    {
        let clamped_s = s.min(frame_count.saturating_sub(1));
        (Some(clamped_s), Some(frame_count))
    } else {
        (None, None)
    };

    Ok(DecodedWav {
        sample_rate,
        channels,
        samples: Arc::from(samples),
        loop_start,
        loop_end,
    })
}

/// Convenience helper for extracting `(loop_start_frame, loop_end_frame, sample_rate)` cleanly.
pub fn parse_wav_loop_points(bytes: &[u8]) -> Option<(usize, usize, u32)> {
    let decoded = decode_wav(bytes).ok()?;
    Some((decoded.loop_start?, decoded.loop_end?, decoded.sample_rate))
}

/// Encode a PCM-16 WAV buffer with optional `cue ` and `smpl` loop chunks for deterministic tests.
pub fn encode_test_wav_pcm16(
    sample_rate: u32,
    channels: u16,
    samples: &[i16],
    cue_loop_start: Option<u32>,
    smpl_loop: Option<(u32, u32)>,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");

    // fmt chunk (16 bytes)
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes()); // PCM
    body.extend_from_slice(&channels.to_le_bytes());
    body.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * u32::from(channels) * 2;
    let block_align = channels * 2;
    body.extend_from_slice(&byte_rate.to_le_bytes());
    body.extend_from_slice(&block_align.to_le_bytes());
    body.extend_from_slice(&16u16.to_le_bytes());

    // data chunk
    let data_bytes = (samples.len() * 2) as u32;
    body.extend_from_slice(b"data");
    body.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        body.extend_from_slice(&s.to_le_bytes());
    }

    if let Some(start) = cue_loop_start {
        body.extend_from_slice(b"cue ");
        body.extend_from_slice(&(4u32 + 24u32).to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes()); // dwCuePoints
        body.extend_from_slice(&1u32.to_le_bytes()); // dwName
        body.extend_from_slice(&start.to_le_bytes()); // dwPosition
        body.extend_from_slice(b"data");
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&start.to_le_bytes()); // dwSampleOffset
    }

    if let Some((start, end)) = smpl_loop {
        body.extend_from_slice(b"smpl");
        body.extend_from_slice(&(36u32 + 24u32).to_le_bytes());
        body.extend_from_slice(&[0u8; 28]);
        body.extend_from_slice(&1u32.to_le_bytes()); // cSampleLoops
        body.extend_from_slice(&0u32.to_le_bytes()); // cbSamplerData
        body.extend_from_slice(&0u32.to_le_bytes()); // dwIdentifier
        body.extend_from_slice(&0u32.to_le_bytes()); // dwType (forward loop)
        body.extend_from_slice(&start.to_le_bytes());
        body.extend_from_slice(&end.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
    }

    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_pcm8_pcm16_cue_and_smpl_loops() {
        let wav = encode_test_wav_pcm16(22_050, 1, &[0, 16_384, -16_384, 32_767], Some(1), None);
        let dec = decode_wav(&wav).expect("valid wav with cue");
        assert_eq!(dec.sample_rate, 22_050);
        assert_eq!(dec.channels, 1);
        assert_eq!(dec.frame_count(), 4);
        assert!((dec.samples[1] - 0.5).abs() < 1e-4);
        assert_eq!(dec.loop_start, Some(1));
        assert_eq!(dec.loop_end, Some(4));

        let smpl_wav = encode_test_wav_pcm16(
            44_100,
            2,
            &[100, -100, 200, -200, 300, -300, 400, -400],
            Some(0),
            Some((1, 3)),
        );
        let dec2 = decode_wav(&smpl_wav).expect("valid stereo wav with smpl");
        assert_eq!(dec2.channels, 2);
        assert_eq!(dec2.frame_count(), 4);
        assert_eq!(dec2.loop_start, Some(1));
        assert_eq!(dec2.loop_end, Some(3));

        assert!(matches!(decode_wav(b"NOPE"), Err(WavError::TooShort)));
        assert!(matches!(
            decode_wav(b"BAD!1234WAVE"),
            Err(WavError::InvalidRiffHeader)
        ));
    }
}
