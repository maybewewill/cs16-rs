//! GoldSrc audio engine: RIFF WAV cue/smpl loops, `sentences.txt` VOX speech,
//! 29-preset room DSP & underwater muffling, and platform output/voice capture.

pub mod dsp;
pub mod sentences;
pub mod wav;

pub use dsp::{NUM_ROOM_TYPES, ROOM_PRESETS, RoomDsp, RoomPreset};
pub use sentences::{SentenceTable, VoxWord};
pub use wav::{DecodedWav, WavError, decode_wav, encode_test_wav_pcm16, parse_wav_loop_points};
