# Phase 9 — Sound & Voice (`phase9-sound`) Implementation Plan

Goal: Close **18/18** items in [`docs/parity/sound.toml`](../../parity/sound.toml) across `cs-proto48`, `cs-cvar`, `cs-engine`, `cs-game`, `cs-clientcore`, `cs-client`, and `cs-web`.

## Milestones
1. **Protocol, CVars, Server, Game & ClientCore**: `svc_voicedata`/`clc_voicedata`/`svc_roomtype`/`svc_stopsound` codecs, audio & voice CVars + commands (`mp3`, `cd`, `play`, `playvol`, `spk`/`speak`, `stopsound`, `+voicerecord`), `cs-engine` sign-on `sv_voicecodec`/`sv_voicequality`, `vban` muting & voice relay, `EnvSound` `svc_roomtype`, `AmbientGeneric` volume/pitch ramping, and `ClientEffects::voice`.
2. **Pure-Rust WAV, Sentences/VOX & Room DSP**: RIFF PCM8/16 parser with `cue`/`smpl` loop points, `sound/sentences.txt` + VOX parser (`!N`, `dir/`, `(p.. t.. e.. s.. v..)`), and 29-preset GoldSrc `RoomDsp` + underwater biquad muffling.
3. **GoldSrc Mixer, Music, Impacts & Voice Engine**: 128+128 channel pool with priority eviction & `SND_CHANGE_VOL`/`SND_CHANGE_PITCH`/`SND_STOP`, `ATTN_*` + stereo pan + BSP wall occlusion, `svc_soundfade` envelope, `MusicController` (`svc_cdtrack`, `MP3FadeTime`, `mp3`/`cd`), material bullet impact & reload SFX, `VoiceManager` (RFC 6716 Opus, SteamPipe `OPUS_PLC`, `voice_miles` IMA-ADPCM, `voice_speex` framing, jitter buffer + PLC, AGC), and `rodio`/`cpal` native output/capture.
4. **Native `cs-client` & Browser `cs-web` Integration**: full wiring in `app.rs`, `view.rs`, `session.rs`, `ui/{options,hud}.rs`, `cs-web/src/{state,wasm_entry}.rs`, and `web/audio.js`.
5. **Verification & Parity Closure**: `phase9_sound` integration suite, updated `sound.toml`/`wire.toml`/`cvars.toml`, `docs/parity/checklists/phase9-sound.md`, `README.md`, and all 8 sequential CI gates.
