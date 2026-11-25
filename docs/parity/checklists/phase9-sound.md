# Phase 9 — GoldSrc Sound & Voice Checklist

Closed all **18/18** `docs/parity/sound.toml` items across native `cs-client` (`rodio` + `cpal` + `opus` + pure-Rust GoldSrc DSP/mixer) and browser `cs-web` (gesture-gated WebAudio API graph in `web/audio.js`).

## Part A — Automated Verification (re-verified 2026-11-25)

See also the gameplay event mapping in [sound-events.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/sound-events.md).

| Check | Scope | Evidence |
|---|---|---|
| Protocol 48 wire (`svc_stopsound`, `svc_roomtype`, `svc_voiceinit`, `svc_voicedata`, `clc_voicedata`, `vban`) | `cs-proto48`, `cs-engine` | `crates/cs-proto48/src/messages.rs::tests::voice_roomtype_stopsound_roundtrip`, `crates/cs-engine/tests/voice.rs::server_voice_init_relay_vban_and_enable_flag` |
| `env_sound` & `ambient_generic` ramping | `cs-game` | `crates/cs-game/tests/phase7_env_items.rs::env_sound_sends_svc_roomtype_and_ambient_generic_ramps` |
| WAV 2D playback & RIFF `cue `/`smpl` loop points | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::wav_2d_playback_renders_pcm`, `wav_cue_and_smpl_loop_points_wrap` |
| 128+128 channels, `CHAN_*` replace, priority eviction, `SND_CHANGE_VOL`/`PITCH`, `stopsound` | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::channels_priorities_alter_and_stopsound` |
| `ATTN_*` falloff, right-vector stereo pan, fractional pitch (`50..=255`), wall occlusion (`s_occfactor`) | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::goldsrc_distance_attenuation_and_stereo_pan`, `goldsrc_fractional_pitch_shifting`, `goldsrc_wall_occlusion_attenuates_sound` |
| `sentences.txt` & VOX speech concatenation (`!sentence`, `%!MRAD_*`, `(p v s e t)` modifiers, retail `cstrike/sound/sentences.txt`) | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::sentences_txt_and_vox_concatenation` |
| 29-preset `RoomDsp` (`room_type 0..=28`, `room_off`) & underwater 550 Hz LPF (`waterlevel = 3`) | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::roomtype_29_presets_modulate_wet_reverb`, `underwater_muffling_lowpass_filters_audio` |
| `svc_soundfade`, `svc_cdtrack`, `mp3`/`cd` commands, `mp3fadetime`, retail MP3 decode (`Half-Life01.mp3`) | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::soundfade_attenuates_and_recovers_master_gain`, `cdtrack_and_mp3_console_commands`, `real_mp3_menu_music_decode_if_installed` |
| Gameplay event routing (radio `%!MRAD_*`, C4 plant/beep/disarm/explode, hostages, footsteps, weapons, `env_sound`) | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| Bullet surface material impacts, ricochets, reload sounds, retail VFS verification | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::material_impacts_and_predicted_footsteps` |
| Native RFC 6716 `opus` voice capture/AGC/jitter buffer/PLC/`voice_enable`/`voice_scale` | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::voice_opus_native_loopback_jitter_and_plc` |
| GoldSrc voice interop (`OPUS_PLC` CRC32, `voice_miles` IMA-ADPCM 11025 Hz, `voice_speex` framing & PLC) | `cs-client` | `crates/cs-client/tests/phase9_sound.rs::voice_goldsrc_compat_steam_opus_miles_and_speex` |
| Browser WebAudio backend (`WebAudioEvent` 12 variants, gesture gate, loops, DSP, music, voice PCM loopback) | `cs-web` | `crates/cs-web/src/state.rs::tests::audio_events_gated_until_user_gesture`, `web_audio_backend_full_event_pipeline` |

## Part B — Manual Live Verification (Owner)

- [ ] Launch `cargo run -p cs-client -- --map de_dust2`, confirm gunfire stereo panning tracks horizontal look direction, tunnel `env_sound` triggers reverberation (`room_type`), and `waterlevel = 3` (e.g. `de_aztec` water) muffles gunfire/footsteps.
- [ ] Hold `K` (`+voicerecord`) with `voice_loopback 1` on native `cs-client` and hear local voice echo + `[VOICE]` HUD indicator.
- [ ] Run `mp3 play media/Half-Life01.mp3` and `mp3 stop` in console and verify smooth `mp3fadetime` fade-out.
- [ ] Open `cs-web` in Chromium, unlock audio via click/key, confirm weapon/ambient/music/VOX playback via WebAudio API.
