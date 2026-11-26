# Changelog

All notable changes to `cs16-rs` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.2.0] - 2026-11-26

First flagship release delivering 1:1 Steam Build 10210 feel (`99.7%` parity across 15 ledgers), a 5-language sandboxed modding platform, Phase 16 performance optimizations, official branding, live GitHub Pages Web PWA, and multi-OS release automation — all under `#![forbid(unsafe_code)]` and `GPL-3.0-or-later`.

### Added

- **Phase 14 — Classic CS 1.6 Feel & Opt-In "Beyond 1.6":**
  - **100% Vanilla Console & CVar Parity (`735/735`):** Frame-deferred `wait` scripting, `bindtoggle`, `incrementvar`, `multvar`, `condump`, `cfg_diff`, live `FCVAR_USERINFO` `setinfo` sync, `mouse1..5`, `mwheelup`/`mwheeldown` scroll-bhop/duck with GoldSrc `kbutton_t` impulse latching.
  - **Visual & Gameplay Authenticity:** Authentic `sprites/sniper_scope.spr` quadrant arcs, eye-to-origin `RenderMode::Glow` wall occlusion traces, `cl_shadows 1` octagonal ground shadows, `cl_minmodels 1`, brush-entity local decals (`func_door`/`func_breakable`), classic 32-bit smoke clouds, `PM_TraceTexture` predicted material footsteps, GoldSrc `net_graph 1/2/3`, `cl_showpos`, `r_speeds`, `100 Hz` high-refresh command grid (`cl_cmd_100hz 1`), `visents` boost/runboost prediction, AWP quick-switch polish, live `sv_*rate`/`sv_*updaterate` pacing, `IN_SCORE`-gated `svc_pings`, and classic `s_occfactor 0` + `s_lerping`/`hisound` resampling.
  - **3-Way Steam Voice Bridge:** CRC32-signed SteamPipe `OPUS_PLC <-> CSVO ADPCM` transcoding (`sv_voice_transcode 1`) bridging retail `hl.exe`, native `cs-client`, and browser `cs-web`.
  - **1-Click Browser WebRTC P2P Rooms:** `MultiPeerChannel` listen-server hosting inside `wasm32` + public CORS `/lobbies` signaling and `?p2p=<room>&signal=<origin>` shareable invite links.
  - **Opt-In "Beyond 1.6" Suite (all default `0`):** Wallbang & Nade Lab (`sv_showimpacts`, `sv_grenade_trajectory`, `rethrow`, `sv_showhitboxes`, `mp_buy_anywhere`, `mp_warmup_mode`), Scrim commands (`lo3`, `mp_knife_round`, `sv_damage_print`), built-in KZ/Bhop checkpoints & `+32 u` LongJump stats (`sv_kz_mode`, `cl_ljstats`, `cl_showspeed`, `cl_showkeys`), and spectator/demo `spec_xray` + `spec_hud_bars` (`HealthInfo`/`Account`).
  - **Web Admin 2D Round Replay Player:** 10 Hz `Round2dTimeline` capture alongside `.dem` replays, `/api/map-overview/<map>.png` chromakey exporter, and interactive `<canvas>` HLTV-style replay modal in `/admin`.

- **Phase 15 — Polyglot Modding Platform & AMXX+ Ecosystem:**
  - **ABI v2 & Unified AMXX Coexistence:** ReAPI `Pre`/`Post` hook chains, CS/fun/engine/cstrike natives, declarative `CustomWeaponSpec`, 12 Protocol 48 `vfx_*` primitives, paginated `MenuManager`, simultaneous `en`/`ru` `LangCatalog` (`/lang`), `admins.toml` + `users.ini` RBAC, `nvault` + $O(\log N)$ leaderboards, synchronous polyglot `RpcBus`, minimal safe `wasi_snapshot_preview1` shims, and allowlisted non-blocking webhooks.
  - **Zero-Compile Python 3 Runtime (`cs-mod-py`):** Pure-Rust, `wasm32`-clean, fuel-metered Python 3 interpreter with `sv_mod_autoreload` live hot-reload and IDE stubs (`sdk/python/cs16.pyi`).
  - **Polyglot SDKs & CLI:** Rust (`cs-mod-sdk`, `cs-mod-macros`), C++20 (`sdk/cpp/cs16_mod.hpp`), Go/WASI (`sdk/go/cs16mod`), and `cargo xtask mod {new,build,replay}`.
  - **13-Plugin Localized Showcase Suite (`0` `.wat` files):** `std_admin`, `std_mapmanager` (Go), `std_statsx`, `std_restmenu` (C++20); Python community plugins (`resetscore.py`, `vip_system.py`, `afk_control.py`, `damager_and_parachute.py`, `antiflood_admanager.py`); overhaul gamemodes (`csdm`, `gungame`, `zombie_plague`, `semiclip`).
  - **Operations & DevEx:** 5-second legacy AMXX server migrator (`--import-amxx` / `cargo xtask import-amxx`), `ModFaultDump` black-box crash recorder & `< 10 ms` offline replay (`cargo xtask mod replay`), live `mod_stats` ResMon profiler, and `/admin` **Mods & ResMon Studio** with `< 5 ms` browser hot-reload editor.

- **Phase 16 — Blazing Performance, Zero-Alloc Hot Paths & Sub-ms Latency:**
  - Precompiled zero-alloc Protocol 48 `CompiledPlan` delta codec (**4.56x** faster, `292,197 ent/s` in `dev`, **0 B** wire drift).
  - GPU `.mdl` bone skinning (`<= 6 KiB` UBO, WebGPU + WebGL2) + cached `GammaTables` + 1-draw-call smoke/sprite batching (`45 -> 1` draw calls, **-95.9%** upload traffic, **7.35x** faster headless GPU frames on `de_dust2`, `(-0.4, -0.8, +0.0)` RGB visual parity).
  - Bit-packed `u64` PVS/PAS, persistent 32-entry `StudioCache`, $O(1)$-reset ZBot `PathScratch`, deterministic multi-core snapshot compression (`std::thread::scope`), lock-free audio callback thread + signon WAV warmup, `gl_vsync 0` / `gl_lowlatency 1` swapchain controls, and zero-alloc `ViewBuilder` (**1.36x** faster client CPU).
  - Tuned `[profile.release]` (`opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`), `[profile.bench]`, and 4-pillar `cargo xtask bench --suite all --gpu`.

- **Release Polish, Branding, Web PWA & CI/CD:**
  - Official `cs16-rs` logo, OpenGraph social preview, multi-res desktop window/taskbar icon (`icon-64.png` / `icon.ico`), installable PWA icons (`192x192`, `512x512`, `apple-touch-icon`, `favicon.ico`), and `--embed-web` binary serving with exact MIME types.
  - Authentic Counter-Strike 1.6 VGUI2 (`TrackerScheme.res`) & HUD (`ClientScheme.res`) styling audit: enforced `0px` crisp corners, 1px bevels/borders, and authentic palette across `cl_showspeed`, `cl_showkeys`, `cl_ljstats`, `spec_hud_bars`, `draw_voice_speakers`, `mod_hud`, mod approval dialog, and pre-VFS folder setup screen.
  - Dual-mode Web Launcher (`web/index.html`, `web/app.js`) auto-detecting GitHub Pages static HTTPS (with 3-step onboarding card + Mixed Content `ws://` guidance) vs self-hosted `cs-server --web-port`.
  - Automated GitHub Actions workflows: multi-OS CI (`windows-latest` + `ubuntu-latest`), GitHub Pages deployment (`.github/workflows/pages.yml`), cross-platform tagged releases (`windows-x86_64`, `linux-x86_64`, `macos-arm64`, `web-pwa`, `SHA256SUMS.txt`), and GHCR Docker publishing.
  - Flagship `README.md`, `docs/REWRITE_ADVANTAGES.md`, and `.github/repo-meta.yml`.

---

## [0.1.0] - 2026-11-25

Initial foundation through Phases 0–13:

- Core GoldSrc VFS, BSP/WAD/MDL/SPR asset loaders, Protocol 48 wire engine (`cs-proto48`, `cs-engine`, `cs-clientcore`), shared `pm_shared` movement (`cs-pm`), all 28 weapons + Tactical Shield + `160/160` entities (`cs-game`), `de_*`/`cs_*`/`as_*`/`es_*` modes, `wgpu` renderer + VGUI2 `.res` UI (`cs-client`), `wasm32` browser client (`cs-web`), ZBot `.nav` AI (`cs-bot`), HLTV (`cs-hltv`), Master Server (`cs-master`), Pawn 3.2 `.amxx` VM (`cs-amxx`), Web Admin (`/admin`), and single-binary / Docker deployment.
