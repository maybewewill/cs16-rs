//! GoldSrc audio engine: RIFF WAV cue/smpl loops, `sentences.txt` VOX speech,
//! 29-preset room DSP & underwater muffling, software mixer, music controller,
//! material impacts, Opus/GoldSrc voice, and native `rodio`/`cpal` output.

pub mod dsp;
pub mod impacts;
pub mod mixer;
pub mod music;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
pub mod sentences;
pub mod voice;
pub mod wav;

pub use dsp::{NUM_ROOM_TYPES, ROOM_PRESETS, RoomDsp, RoomPreset};
pub use impacts::{build_impact_sounds, material_impact_sound, reload_sound_path, ricochet_sound};
pub use mixer::{
    GoldSrcMixer, ListenerState, MAX_DYNAMIC_CHANNELS, MAX_STATIC_CHANNELS, MIX_SAMPLE_RATE,
    SOUND_NOMINAL_CLIP_DIST, SoundFadeState, normalize_sound_path, snd_spatialize,
};
pub use music::{MusicCommand, MusicController};
#[cfg(not(target_arch = "wasm32"))]
pub use native::AudioEngine;
pub use sentences::{SentenceTable, VoxWord};
pub use voice::{
    DEFAULT_STEAM_VOICE_ID, SniffedVoicePacket, SteamVoiceChunk, SteamVoicePacket,
    VOICE_FRAME_SAMPLES, VOICE_SAMPLE_RATE, VoiceAgc, VoiceCodecKind, VoiceJitterBuffer,
    VoiceManager, decode_ima_adpcm, decode_steam_opus_plc, encode_ima_adpcm, encode_steam_opus_plc,
    encode_steam_opus_plc_reset, encode_steam_voice_packet, frame_speex_nb_packet,
    parse_csvo_packet, parse_speex_nb_packet, parse_steam_voice_packet, sniff_voice_packet,
    speex_nb_frame_bytes,
};
pub use wav::{DecodedWav, WavError, decode_wav, encode_test_wav_pcm16, parse_wav_loop_points};
