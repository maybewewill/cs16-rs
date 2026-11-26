# cs16-rs

**Counter-Strike 1.6** rewrite in pure Rust (`#![forbid(unsafe_code)]`, Rust edition 2024), with the server/game logic ported from ReHLDS / ReGameDLL_CS and the Protocol 48 client/renderer ported from Xash3D FWGS and `cs16-client` events (`THIRD-PARTY-NOTICES.md`), featuring GoldSrc BSP/MDL/SPR rendering, VGUI2 menu/console/options (`TrackerScheme.res` + `BackgroundLayout.txt`), developer console (`~`), `CVar`/`config.cfg` engine, `pm_shared` movement physics, ReGameDLL-calibrated `KickBack` gunplay across **all 28 CS 1.6 weapons**, `EF_BONEMERGE` third-person weapons, hostage rescue & bomb defusal modes, offline bots, and 100 Hz UDP multiplayer with LAN server discovery. For the licensing of all four reference projects, see [`docs/LICENSING.md`](docs/LICENSING.md).

Assets are mounted **read-only** directly from an existing Steam Half-Life / Counter-Strike 1.6 installation (`cstrike`, `valve`, `platform`, optional `cstrike_hd`); zero Valve assets are stored in this repository.

Copyright (C) 2026 cs16-rs contributors. Licensed under GPL-3.0-or-later; see LICENSE. See [`docs/LICENSING.md`](docs/LICENSING.md) for the licensing decision and reference-project attribution.

## Workspace Architecture (27 Crates)

- **`cs-vfs`** — Multi-layer case-insensitive GoldSrc VFS (`mods/<id>/` → `cstrike_downloads` → optional `cstrike_hd` → `cstrike` → `valve` → `platform`) with automatic Steam `libraryfolders.vdf` discovery, pluggable `VfsBackend` (`CachingBackend`), and in-memory layered browser mounting (`from_layered_memory`).
- **`cs-cvar`** — Quake/GoldSrc command & CVar engine achieving **100% (`735/735` total registered, `0` missing in `cvars.toml`)** vanilla CS 1.6 + Phase 15 mod-platform console parity: `bind`, `unbind`, `alias`, frame-deferred `wait` scripting, `bindtoggle`, `incrementvar`/`multvar`, `condump`, `cfg_diff`, `exec`, `toggle`, `cvarlist`, `cmdlist`, `find`, `map`, `connect`, `retry`, `buy`, `give`, `god`, `noclip`, `notarget`, `fly`, `sv_restart`, `bot_add`/`_t`/`_ct`, `bot_kick`, `say`/`say_team`, `radio1..3`, `nightvision`, `impulse 201`, `screenshot`, `mod_list`/`amx_plugins`/`mod_stats`/`mod_pause`/`mod_unpause`/`mod_reload`/`mod_unload`, `amx_language`, `sv_mod_autoreload`, `Tab` autocomplete, `FCVAR_USERINFO`/`FCVAR_PROTECTED`/`FCVAR_SPONLY` enforcement, and local `config.cfg` persistence.
- **`cs-assets`** — Pure-Rust parsers for `WAD3`/`MipTex`, `BSP v30` (all 15 lumps, PVS RLE, padded 2048×2048 lightmap atlas, `func_ladder`, `func_buyzone`, `func_bomb_target`, `func_hostage_rescue`, `func_breakable`, `func_door`, `hostage_entity`), `StudioModel v10` (`IDST` bones, RLE animations, clamped death frames, true `EF_BONEMERGE`, companion `*T.mdl`, hitboxes), `Sprite v2` (`IDSP`), Valve `.nav` v1–v5 meshes, `HPAK` (`.hpk`) custom spray archives, `sound/materials.txt`, `TrackerScheme.res` (`yres` font tiers & `marlett.ttf` TrueType rasterization via `ab_glyph`), `ClientScheme.res`, multi-language UTF-16LE `*_*.txt` localization, recursive VGUI2 `.res` layouts, `.scr`/`commandmenu.txt`/`titles.txt`/`overviews/<map>.txt` + `.bmp` chromakey, `classes/*.res`, `maps/*.txt`, `BackgroundLayout.txt`, `sprites/hud.txt`, and skyboxes.
- **`cs-sim`** — Deterministic 100 Hz game simulation:
  - GoldSrc BSP hull tracing (`Point`/`Standing`/`Large`/`Crouched`) + anatomical 6-hitbox player raycast + wallbang material penetration + shattering `func_breakable` glass/vents + interactive `func_door`s.
  - Full `pm_shared` physics (counter-strafing, air-strafe/bunnyhop `wishspd` cap 30, 18u stairs, duck-jump, ladders, fall damage, surface footsteps).
  - Older prototype `pmove` physics module remains for legacy `cs-sim` unit tests; shared `pm_shared` movement now lives in **`cs-pm`**.
  - **All 28 CS 1.6 weapons + C4 + NightVision**: Knife, Glock-18 (burst), USP (silencer), P228, Deagle, Five-SeveN, Dual Elites, M3 & XM1014 (interruptible shell-by-shell reload), MAC-10, TMP, MP5, UMP-45, P90, Galil, FAMAS (burst), AK-47, M4A1 (silencer), SG552, AUG, Scout, AWP, G3SG1, SG550, M249, HE/Flash/Smoke grenades, C4 — with ReGameDLL `KickBack` recoil & `PM_DropPunchAngle` spring recovery.
  - Both **Bomb/Defuse (`de_*`)** and **Hostage Rescue (`cs_*`)** modes, persistent round corpses, economy (`$800..$16000` + loss bonus), automatic BSP `NavGraph` generator + Bot AI.
- **`cs-pm`** — Shared GoldSrc `pm_shared` movement physics and BSP/box hull collision (`PM_Move`, `PM_RecursiveHullCheck`, `PM_TraceTexture` / `WorldCollision::trace_texture`, `visents` boost/runboost traces, surface materials) ported from ReGameDLL_CS, ReHLDS, and Xash3D FWGS behind the `PmWorld` trait so `cs-game` and `cs-clientcore` prediction run identical movement and predicted material footsteps.
- **`cs-proto48`** — `#![forbid(unsafe_code)]` implementation of Valve's **GoldSrc Protocol 48** (Counter-Strike 1.6 / Half-Life 1 network wire protocol) + HLDEMO Protocol 5 `.dem` recording/playback + `HealthInfo`/`Account`/`ReqState` observer messages, ported deterministically from `ReHLDS`, `ReGameDLL_CS`, and `Xash3D FWGS`.
- **`cs-platform`** — Platform seams shared by native and `wasm32` builds: `Clock` (`NativeClock`, `WebClock`, `ManualClock`), `ConfigStorage` (`MemoryStorage`, `NativeFileStorage`, `WebLocalStorage`), and XDG data cache (`nav_cache/`).
- **`cs-transport`** — Datagram transports behind one `PacketTransport` trait: non-blocking UDP, in-process loopback hub with injectable loss/duplication/reordering, `ChannelTransport` and `MultiPeerChannel` for `wasm32` JS/WebRTC bridges, `MultiTransport` (`Udp` + `Web`), and pure-Rust RFC 6455 WebSocket + static HTTP/1.1 hosting (`ws_proto` / `ws_native`).
- **`cs-relay`** — Hardened WebSocket-to-UDP Protocol 48 proxy enforcing explicit `--allow` allowlists, per-IP and global session caps, deterministic token-bucket rate limits, idle timeouts, and GoldSrc OOB/NetChan packet validation.
- **`cs-bot`** — Port of Counter-Strike ZBot AI from ReGameDLL_CS: `.nav` v1–v5 loading and automatic BSP navigation mesh generation/caching (`nav_cache/<map>_<crc>.nav`), `BotProfile.db` difficulty/skill templates, weighted A* pathfinding, eco/force/full-buy economy, combat/aim/recoil/grenade tactics, floor weapon scavenging (`fy_*`/`aim_*`) and knife-arena (`35hp_*`) combat, audio/visual perception, and radio chatter.
- **`cs-game`** — The game logic of CS 1.6 (the `mp.dll` equivalent) behind the engine's `GameDll` trait, ported from ReGameDLL_CS (vanilla `mp.dll` rules): team and class menus, spawning, freeze periods, timers, win conditions, economy/loser bonus, scoreboard/kill messages, chat, radio, radar/status bar, spectator observer + `HealthInfo`/`Account` observer-only updates, buy zones, map cycles + `votemap`/`vote`, **all 28 weapons/grenades + Tactical Shield (`$2200` CT buy + pistol/knife/grenade shield variants)**, **`autobuy`/`rebuy`/`cl_setautobuy`/`cl_setrebuy`**, **all 160/160 `entities.toml` map entities & triggers** (`func_door*`, `func_water`, `func_button*`, `func_breakable`, `func_tracktrain`, `func_vehicle`, `trigger_*`, `multi_manager`/`multisource`/`env_global`, `game_*`, `env_*`, lights, sounds, `ammo_*`, `item_*`, all 25 stock CS 1.6 `.bsp` maps), **C4 bomb planting/defusing/drop/pickup & defuser kits (`de_*`)**, **classic hostages + `CLocalNav` (`cs_*`)**, **VIP assassination & escape (`as_*` / `es_*`)**, **ZBot `.nav` fake-client bots** (`bot_add*`/`bot_kick`/`bot_quota`), and opt-in **"Beyond 1.6"** practice/scrim/KZ modes (`sv_showimpacts`, `sv_grenade_trajectory`, `sv_showhitboxes`, `rethrow`, `mp_buy_anywhere`, `mp_warmup_mode`, `sv_damage_print`, `lo3`, `mp_knife_round`, `sv_kz_mode`, `sv_ljstats`, all defaulting to `0`). Player movement runs through `cs-pm` (`cs_pm::pm`) in `SV_RunCmd` order.
- **`cs-engine`** — Protocol 48 server engine, the equivalent of the original dedicated server: connectionless queries, connect and sign-on, per-client delta frames with PVS culling and live `sv_minrate`/`sv_maxrate`/`sv_minupdaterate`/`sv_maxupdaterate` pacing, `IN_SCORE`-gated `svc_pings`, chronological `MsgDest::Broadcast` datagrams, 1.0 s `sendinfo_time` spacing, user commands, fake clients (`CreateFakeClient` / `RunPlayerMove`), HL standard logging + `logaddress`, RCON challenge/rate limiting, IP/AuthID bans, `dlfile`/`mp_consistency`/`svc_customization`, server-side voice bridge hook, timeouts, and map changes behind a `GameDll` trait.
- **`cs-master`** — Pure-Rust GoldSrc Master Server (`0x31` query protocol, challenge-verified server heartbeats, TTL expiration, HTTP `/servers` directory, and bounded `/lobbies` + `/lobbies/<room>/signal` WebRTC/invite signaling registry).
- **`cs-hltv`** — GoldSrc Protocol 48 HLTV proxy, configurable time-delay spectator relay buffer, `DRC_CMD_*` auto-director, and optional `.dem` recorder.
- **`cs-amxx`** — `#![forbid(unsafe_code)]` Pawn 3.2 bytecode VM, `.amxx` container loader, plugin scheduler, and Counter-Strike 1.6 native/forward runtime (`cstrike`, `fun`, `engine`, `fakemeta`, `hamsandwich`, `csx`, `nvault`, `newmenus`, `lang`, ReAPI `rg_*`, dynamic natives & multi-forwards) unified with `cs-mod-host` inside `cs-server --plugins-toml` / `--amxx-dir`.
- **`cs-probe`** — Headless Protocol 48 test client (library and `cs-probe` binary): connects, signs on, walks and records/diffs captures, used by the engine's tests and against live HLDS.
- **`cs-clientcore`** — The platform-free Protocol 48 client: connection and sign-on, precache tables, entity frames and interpolation, `NetGraphTelemetry` rolling NetChan ring buffer, HLDEMO v5 `.dem` recording & `DemoPlayer` playback, movement prediction on `cs-pm` (including `PM_TraceTexture` footsteps and `visents` boost/runboost), `cl_lw 1` client-side weapon prediction + AWP quick-switch polish, events, temp entities, sound queue, and HUD state (`HealthInfo`/`Account`).
- **`cs-local`** — Local play: `cs-engine` and `cs-game` in the client's process, reached by a `cs-clientcore` connection over `LoopbackHub` plus optional `MultiPeerChannel` (`PeerAddr::Web(1..=16)`) for browser P2P hosting; also the shared start-up helpers of the Protocol 48 server.
- **`cs-server`** — Standalone headless 100 Hz dedicated UDP + optional WebSocket/HTTP/Web Admin/relay/signaling server (`--web-port`, `--web-dir`, `--admin`, `--embed-web`, `--replay-rounds`, `--relay-allow`, `--mods-dir`, `--amxx-dir`, `--plugins-toml`, `--import-amxx`, `--out-dir`, `--master`, `-sys_ticrate`): `cs-engine` with `cs-game` (or `ModdedGame<G>` hosting `.wasm`, `.py`, and `.amxx` simultaneously), interactive Web Admin (`/admin`) with **Mods & ResMon Studio** (live CPU bars, crash dump downloads, sandboxed `< 5 ms` hot-reload editor) + 10 Hz 2D round replay player + map overviews, public CORS `/lobbies` signaling for P2P browser rooms, and 3-way `hl.exe <-> cs-client <-> cs-web` Steam Voice `OPUS_PLC <-> CSVO ADPCM` transcoding (`sv_voice_transcode 1`). `--proto48` is still accepted and does nothing.
- **`cs-mod-api`** — Versioned polyglot modding ABI (`ABI_VERSION = 2`), ReAPI `HookId`/`HookPhase`/`HookAction`, `CustomWeaponSpec`, `VfxOp`, `HudDrawCmd`, `ModManifest` (`mod.toml`) + capability permissions, shared platform services (`MenuManager`, `LangCatalog`, `AdminRegistry`, `NVaultStore`, `LeaderboardStore`, `RpcBus`), SHA-256, and `.csmod` ZIP reader/writer.
- **`cs-mod-host`** — `#![forbid(unsafe_code)]` sandboxed polyglot mod host (`ModRuntime` / `PythonModRuntime` traits + deterministic `wasmi` backend + WASI preview1 shims + unified `AmxxRuntime`): ReAPI pre/post hook chains, CS/fun/engine/cstrike natives, 1-line VFX builder, non-blocking allowlisted webhooks, per-plugin ResMon profiler, `ModFaultDump` black-box crash recorder & offline replay, 5-second legacy AMXX migrator (`amxx_import`), `ModdedGame<G>`, hot reload, VFS asset pack overlay, and capability-gated SHA-256 client auto-download (`ClientModTransfer` + `ClientModHost`).
- **`cs-mod-py`** — `#![forbid(unsafe_code)]`, `wasm32`-clean, fuel-metered pure-Rust Python 3 scripting runtime (`mods/*.py` zero-compile plugins, `OutOfFuel` with Python line tracebacks, `sv_mod_autoreload` live mtime hot-reload, full `cs16` API bindings & IDE type stubs in `sdk/python/cs16.pyi`).
- **`cs-mod-macros`** — Pure-Rust attribute macros (`#[cs_mod_init]`, `#[cs_mod_hook]`, `#[cs_mod_hud]`, `#[cs_mod_command]`, `#[cs_mod_rpc]`, `#[cs_mod_event]`) generating `cs16_mod` WASM exports.
- **`cs-mod-sdk`** — Ergonomic guest SDK (`HostBridge` + `MockHostBridge`, `Player`, `vfx`, `CustomWeaponBuilder`, `PaginatedMenu`, `HudCanvas`, `scaffold_mod_project`) alongside C++20 (`sdk/cpp/cs16_mod.hpp`), Go (`sdk/go/cs16mod`), and Python (`sdk/python/cs16.pyi`) SDKs.
- **`xtask`** — Repository tasks (`cargo xtask parity`: parity ledger validation in `docs/parity/`; `cargo xtask bench --suite all --gpu`: 4-pillar server/delta/client/GPU benchmark suite; `cargo xtask visual-diff`: headless `de_dust2` RGB parity verifier; `cargo xtask web-build`: stages `web/` PWA assets and builds `cs-web` for `wasm32-unknown-unknown`; `cargo xtask mod {new,build,pack,replay}`: polyglot mod scaffolding, building/packing into `dist/mods/`, and `< 10 ms` offline crash dump replay; `cargo xtask import-amxx`: 5-second legacy AMXX server migrator).
- **`cs-client`** — Reusable `wgpu 30` renderer (WebGPU + WebGL2 downlevel fallback, `cl_shadows 1`, `cl_minmodels 1`, brush-entity local decals, occluded `RenderMode::Glow` traces, `spec_xray` occluded silhouettes), camera, input (`mouse1..5`, `mwheelup`/`mwheeldown`, `kbutton_t` impulse latching, 100 Hz high-refresh command grid), view builder, GoldSrc `s_occfactor 0` + `s_lerping`/`hisound` mixer + Steam `OPUS_PLC` voice, and declarative VGUI2 `.res`/HUD library (`VguiRuntime` 12-control library, `sniper_scope.spr`, `net_graph 1/2/3`, `cl_showpos`, `r_speeds`, `spec_hud_bars`, `cl_ljstats`, HTML MOTD viewer, all CS 1.6 screens/dialogs, overview radar, and scheme TrueType/Marlett fonts) shared with `cs-web`, plus the native `winit` + `rodio` desktop shell on `cs-clientcore`.
- **`cs-web`** — `#![forbid(unsafe_code)]` `wasm32-unknown-unknown` browser client entrypoint (`window.cs16Web` via `js_sys`/`web_sys`) paired with the static PWA shell in [`web/`](web/index.html) (`showDirectoryPicker` / drag-and-drop + IndexedDB cache, Pointer Lock raw mouse & scroll binds, Gamepad/touch, gesture-unlocked Web Audio, WebSocket networking with browser-side WebTransport fallback, offline ZBots, and 1-click browser WebRTC DataChannel P2P listen-server hosting/joining via `?p2p=<room>&signal=<origin>`).

## Quick Start

### Play Offline (5v5 with Bots)

```powershell
cargo run --release -p cs-client
```

Jump straight into `de_dust2` or `cs_office`:

```powershell
cargo run --release -p cs-client -- --map de_dust2 --exec "net_graph 1"
cargo run --release -p cs-client -- --map cs_office
```

### Dedicated Server + Multiplayer (with LAN Discovery & Original CS 1.6 Compatibility)

Terminal 1 (start headless 100 Hz Protocol 48 server):

```powershell
cargo run --release -p cs-server -- --map de_dust2 --port 27015
```

Terminal 2 (connect from `cs-client` or click **Find Servers → LAN → Refresh LAN**, or connect from the original Counter-Strike 1.6 client console with `connect 127.0.0.1:27015`):

```powershell
cargo run --release -p cs-client -- --exec "connect 127.0.0.1:27015"
```

Optional flags: `--port <n>` (default 27015), `--maxplayers <n>` (default 16), `--bots <n>` (sets initial `bot_quota`), `--hostname <name>`, `--gamedir <path>`, `--plugins-toml <file>` (unified manifest loading `.wasm`, `.py`, and `.amxx` plugins + `configs/admins.toml` + `configs/lang/*.toml`), `--mods-dir <dir>` (loads WASM/Python mods & `.csmod` bundles via `ServerModHost` + VFS asset packs), `--amxx-dir <dir>` (loads compiled `.amxx`/`.amx` plugins via `cs-amxx`), `--import-amxx <dir> [--out-dir <dir>]` (5-second legacy ReHLDS/AMXX migrator), `--master <ip:port>` (repeatable master server heartbeat target), `--web-port <n>` (HTTP/WebSocket/Web Admin/lobby signaling/relay port), `--web-dir <dir>` (static PWA root to serve over HTTP), `--admin` (enables `/admin` + **Mods & ResMon Studio** on default web port `27016`), `--embed-web` (serve embedded PWA/admin assets directly from the binary), `--replay-rounds <n>` (HLDEMO v5 + 2D replay ring buffer size), `--relay-allow <ip:port>` (repeatable UDP relay allowlist target), `-sys_ticrate <hz>`, `--walk-game` (run the older walking-only `WalkGame` instead of `cs-game`); `--proto48` is still accepted for compatibility and does nothing (the server is always Protocol 48).

### Next-Gen Polyglot Modding Platform (Phase 15)

Write server & client plugins in **Python 3** (zero-compile, live hot-reload), **Rust**, **C++20**, **Go/WASI**, or drop in existing **AMX Mod X `.amxx`** binaries — all sharing paginated menus, simultaneous `en`/`ru` `i18n` (`/lang`), RBAC admins (`configs/admins.toml`), `nvault` + $O(\log N)$ leaderboards, and bidirectional synchronous RPC! Full developer guide: [`docs/MODDING.md`](docs/MODDING.md).

```powershell
cargo xtask mod build                                                   # pack all showcase mods into dist/mods/*.csmod
cargo run -p cs-server -- --map de_dust2 --plugins-toml configs/plugins.toml --admin
cargo xtask import-amxx /path/to/cstrike/addons/amxmodx --out-dir .     # 5-second legacy AMXX migration
cargo xtask mod replay logs/mod_faults/<dump>.json                      # < 10 ms offline black-box crash replay
```

Server console commands: `mod_list` / `amx_plugins`, `mod_stats` (live `resmon`), `mod_pause <id>`, `mod_unpause <id>`, `mod_reload <id|all>`, `mod_unload <id>`. Checklist: [`docs/parity/checklists/phase15-mod-platform.md`](docs/parity/checklists/phase15-mod-platform.md).

### Play in Browser (`wasm32` + WebGPU / WebGL2 PWA)

```powershell
cargo install wasm-bindgen-cli   # once, matching workspace wasm-bindgen (0.2.130)
cargo xtask web-build
cargo run -p cs-server -- --map de_dust2 --port 27015 --web-port 27126 --web-dir dist/web --relay-allow 127.0.0.1:27015
```

Open `http://127.0.0.1:27126/` in Chromium or Firefox, mount your local Half-Life / Counter-Strike 1.6 folder (cached locally in IndexedDB), and either launch **Offline** bots, click **Host P2P Room (1v1 / 5v5 in Browser)** to copy a `?p2p=<room>&signal=<origin>` WebRTC DataChannel invite link, or connect to `ws://127.0.0.1:27126/ws` (or another UDP server via `cs-relay`). Checklist: [`docs/parity/checklists/phase4-web.md`](docs/parity/checklists/phase4-web.md).

## Parity & Phase Status (Phases 0–16)

`cs-server` and `cs-client` run the full GoldSrc Protocol 48 / ReGameDLL_CS / Xash3D FWGS stack (`de_*` bomb/defusal, `cs_*` hostages + `CLocalNav`, `as_*`/`es_*` VIP/escape, all 28 weapons + Tactical Shield, `cs-pm` shared `pm_shared` movement, `cl_lw 1` weapon prediction, all 160/160 `entities.toml` classes, ZBot `.nav` AI, unified polyglot WASM/Python/AMXX mod platform, HLTV + `.dem` replay, Web Admin ResMon & 2D replay player, and Steam Voice `OPUS_PLC` bridging):

- **Phase 11 (Engine Completeness & AMX Mod X)**: HLDEMO Protocol 5 `.dem` recording/playback + `DemoPlayer`, `cs-hltv` time-delay director proxy, RCON challenge/auth/rate-limit, HL standard logging & `logaddress`, IP/AuthID bans, `dlfile`/`mp_consistency`/`svc_customization`/`.hpk` sprays, `FCVAR_PROTECTED`/`FCVAR_SPONLY`, and pure-Rust Pawn 3.2 `.amxx` host (`cs-amxx`).
- **Phase 12 (ZBot & `.nav`)**: Valve `.nav` v1–v5 binary load/save, automatic BSP `.nav` generation cached in `nav_cache/<map>_<crc>.nav`, `BotProfile.db`, weighted A*, eco/full-buy, floor weapon scavenging (`fy_*`/`aim_*`), knife-arena (`35hp_*`) tactics, and radio chatter (`cs-bot`).
- **Phase 13 (Hosting, Admin & Browser Lobbies)**: Single-binary server (`--admin`, `--embed-web`, `Dockerfile`), `cs-master` `/servers` & `/lobbies` directory, Web Admin panel (`/admin`) with live stats/RCON/`.dem` downloads and interactive 10 Hz 2D round replay + map overviews.
- **Phase 14 (Classic CS 1.6 Feel & Opt-In "Beyond 1.6")**:
  - **100% vanilla CVar & console command parity** (`735/735` entries in `cvars.toml`, `0` missing) with frame-deferred `wait` scripting, `bindtoggle`/`incrementvar`/`multvar`, `cfg_diff`, and live `FCVAR_USERINFO` `setinfo` sync.
  - **Classic 1.6 feel**: full `mouse1..5` + `mwheelup`/`mwheeldown` scroll-bhop/duck with `kbutton_t` impulse latching, client-predicted `PM_TraceTexture` material footsteps, authentic `sniper_scope.spr` + occluded `RenderMode::Glow` traces + `#Token` resolution, `cl_shadows 1`, `cl_minmodels 1`, brush-entity local decals, classic 32-bit smoke clouds, GoldSrc `net_graph 1/2/3` + `cl_showpos` + `r_speeds`, `100 Hz` high-refresh command grid + boost/runboost `visents` prediction + AWP quick-switch polish, live `sv_*updaterate` / `IN_SCORE` `svc_pings` / chronological `Broadcast` / 1 s `sendinfo_time` spacing, classic `s_occfactor 0` + `s_lerping`/`hisound` resampling, 3-way `hl.exe <-> cs-client <-> cs-web` Steam Voice `OPUS_PLC` bridge (`sv_voice_transcode 1`), and 1-click browser WebRTC DataChannel P2P listen-server (`?p2p=<room>&signal=<origin>`).
  - **Opt-In "Beyond 1.6" Feature Suite** (all new CVars default to `"0"` so vanilla gameplay/wire traffic stays untouched unless enabled): Wallbang & Nade Lab (`sv_showimpacts`, `sv_grenade_trajectory`, `sv_showhitboxes`, `rethrow`, `mp_buy_anywhere`, `mp_warmup_mode`), competitive Scrim commands (`sv_damage_print`, `lo3`, `mp_knife_round` + `.stay`/`.switch`), built-in KZ/Bhop mode & `+32 u` LongJump stats (`sv_kz_mode`, `cl_ljstats`), and spectator/demo `spec_xray` & `spec_hud_bars` (`HealthInfo`/`Account`).
- **Phase 15 (Next-Gen Polyglot Modding Platform & Full AMXX+ Ecosystem)**:
  - **ABI v2 & Unified AMXX Coexistence**: ReAPI pre/post hook chains, CS/fun/engine/cstrike natives, declarative `CustomWeaponSpec`, 12 Protocol 48 `vfx_*` primitives, paginated `MenuManager`, simultaneous `en`/`ru` `LangCatalog` (`/lang`), `admins.toml` + `users.ini` RBAC, `nvault` + $O(\log N)$ leaderboards, bidirectional synchronous `RpcBus`, WASI preview1 shims, and allowlisted async webhooks — shared seamlessly across Rust, C++20, Go/WASI, Python 3, and `.amxx` Pawn 3.2 plugins.
  - **Zero-Compile Python 3 & Polyglot SDKs**: Pure-Rust fuel-metered `cs-mod-py` (`sv_mod_autoreload 1`) + `sdk/python/cs16.pyi`, `cs-mod-sdk`/`cs-mod-macros`, `sdk/cpp/cs16_mod.hpp`, `sdk/go/cs16mod`, and `cargo xtask mod {new,build,pack,replay}`.
  - **13-Plugin Localized AMXX+ Showcase Suite (`0` `.wat` files)**: `std_admin`, `std_mapmanager` (Go/WASI), `std_statsx` (black-and-orange `/top15` MOTD), `std_restmenu` (C++20); Python community plugins (`resetscore.py`, `vip_system.py`, `afk_control.py`, `damager_and_parachute.py`, `antiflood_admanager.py`); overhaul gamemodes (`csdm`, `gungame`, `zombie_plague` with Golden AK-47 / FrostNade / LaserMine, `semiclip`).
  - **Operations & DevEx**: 5-second legacy AMXX migrator (`--import-amxx`), black-box `ModFaultDump` crash recorder & `< 10 ms` offline `cargo xtask mod replay`, live per-plugin ResMon profiler (`mod_list`/`amx_plugins`/`mod_stats`), and in-browser VGUI2 **Mods & ResMon Studio** in `/admin` with sandboxed `< 5 ms` **Save & Hot-Reload**.
- **Phase 16 (Blazing Performance, Zero-Alloc Hot Paths & Sub-ms Latency)**: Precompiled Protocol 48 `CompiledPlan` delta codec (**4.56x** throughput, 0 B wire drift), GPU `.mdl` bone skinning + cached `GammaTables` (**7.35x** faster headless GPU frames, **24.4x** less CPU→GPU upload), 1-draw-call smoke/sprite batching (**45→1** draw calls for 3 smoke clouds), bit-packed `u64` PVS + persistent `StudioCache` + $O(1)$-reset `PathScratch`, lock-free audio thread + signon WAV warmup, `gl_lowlatency 1` / `gl_vsync 0` scanout + zero-alloc `ViewBuilder` (**1.36x** faster client CPU), and deterministic multi-core snapshot compression — all under `#![forbid(unsafe_code)]` with `0%` visual/wire regression (`de_dust2` `right_wall` RGB delta `(-0.4, -0.8, +0.0)`).

### Live HLDS & Honest Part B Checklists

Live `hlds.exe` (build 10210) `cs-probe --diff` captures verified `cs_office` (`218/218` baseline edicts, `HostagePos`, `StatusIcon`, `ItemStatus`), `de_dust2` (`GiveC4`, `51/51` baseline edicts, `ScoreAttrib`, `drop weapon_c4` & 1 Hz `BombDrop`), `as_oilrig` (`info_vip_start`, `ScoreAttrib` VIP bit), a **25-map live baseline audit** (`24/25` stock maps exact; `de_airstrip` `359` vs `363` due to 4 CZ `item_generic` foliage props absent in retail CS 1.6 `mp.dll`), `65 s` differential `cs-pm` movement traces (`0.00446` u mean, `0.01810` u p99, `0` teleports), and `cs-clientcore` headless `hlds.rs` verification (`de_dust2` / `cs_office` movement prediction, HUD, events, tempents, sounds).

Automated tests cover all 15 parity ledgers (`>99.5%` tested), while **manual Part B live-play sign-off by the project owner** remains open in the phase checklists:
- [`docs/parity/checklists/phase15-mod-platform.md`](docs/parity/checklists/phase15-mod-platform.md) (5-language bilingual playtest, overhaul gamemodes, `/admin` Mods & ResMon Studio, `--import-amxx` on a live AMXX server)
- [`docs/parity/checklists/phase14-classic-feel.md`](docs/parity/checklists/phase14-classic-feel.md) (scroll-wheel bhop/duck, `sniper_scope.spr`, `net_graph`, KZ/LJ stats, `spec_xray`, Steam Voice `hl.exe <-> cs-client <-> cs-web`, browser WebRTC P2P)
- [`docs/parity/checklists/phase10-ui.md`](docs/parity/checklists/phase10-ui.md) (side-by-side VGUI2 `.res`/HUD visual parity against `hl.exe`)
- [`docs/parity/checklists/phase4-web.md`](docs/parity/checklists/phase4-web.md) (Chromium/Firefox live WebGPU/WebGL2 + PWA + WebRTC P2P playtest)
- [`docs/parity/checklists/phase2e-objectives.md`](docs/parity/checklists/phase2e-objectives.md) (full live round with stock `hl.exe` on `de_dust2` & `cs_office`)
- Earlier checklists: [`phase1-original-client.md`](docs/parity/checklists/phase1-original-client.md), [`phase2a-console.md`](docs/parity/checklists/phase2a-console.md), [`phase2b-world.md`](docs/parity/checklists/phase2b-world.md), [`phase2c-game.md`](docs/parity/checklists/phase2c-game.md), [`phase2d-weapons.md`](docs/parity/checklists/phase2d-weapons.md), [`phase3-clientcore.md`](docs/parity/checklists/phase3-clientcore.md), [`phase5-mods.md`](docs/parity/checklists/phase5-mods.md), [`phase6-movement-weapons.md`](docs/parity/checklists/phase6-movement-weapons.md), [`phase7-entities-modes.md`](docs/parity/checklists/phase7-entities-modes.md), [`phase8-render.md`](docs/parity/checklists/phase8-render.md), [`phase9-sound.md`](docs/parity/checklists/phase9-sound.md), [`pm-movement.md`](docs/parity/checklists/pm-movement.md).

## Performance Architecture

Every performance optimization in `cs16-rs` preserves `#![forbid(unsafe_code)]`, `wasm32-unknown-unknown` + WebGL2 downlevel compatibility (`<= 16 KiB` UBOs), and bit-for-bit Protocol 48 / GoldSrc visual parity:

- **Precompiled Protocol 48 Delta Codec (`cs-proto48`):** `DeltaSchema` lazily compiles a `OnceLock<Box<CompiledPlan<F>>>` mapping schema slots (`<= 56`) to `#[repr(u8)] Copy` field IDs. `write_struct_delta` / `read_struct_delta` diff and apply structs directly via a stack `u64` bitmask (`trailing_zeros()` iteration) and stack `DeltaMarks`, eliminating per-entity `DeltaRecord` heap allocations and `HashMap` lookups (**4.56x** faster encode+decode, 0 B wire drift).
- **GPU StudioModel Skinning & Cached Gamma LUTs (`cs-client`):** Rest-pose `.mdl` vertex buffers are uploaded once per `(bodypart, submodel)` at load time. Per frame, the CPU evaluates only `<= 128` 3×4 affine bone matrices (`6,144 B` UBO, well within WebGL2 limits) while `mdl.wgsl` performs vertex skinning, exact GoldSrc `SHADE_LAMBERT = 1.4953241` lighting, `STUDIO_NF_CHROME` UVs, and `spec_xray` Fresnel shading. `GammaTables` are cached across BSP/MDL/Decal renderers (`0` steady-state `powf` calls), cutting headless GPU frame time by **7.35x** and per-frame CPU→GPU upload traffic by **95.9%** (`1,825 KiB` → `75 KiB`).
- **1-Draw-Call Smoke & Sprite Batching (`cs-client`):** `SpriteRenderer` interns textures into a `u32` slab handle and run-length coalesces consecutive `(blend, sprite_idx, frame)` quads using persistent scratch buffers — collapsing 3 full smoke clouds (`45` `gas_puff_01.spr` puffs) from `45` draw calls to **1 draw call** alongside stack `[BspDlight; 32]` and zero-alloc BSP/Decal/FX staging reuse.
- **Bit-Packed PVS, Persistent `StudioCache` & $O(1)$ ZBot A* (`cs-engine`, `cs-bot`):** `ServerMap` stores flat `Vec<u64>` PVS/PAS rows (`viewer_pvs_row(eye) -> Option<&[u64]>`, 0 allocations, 1-instruction leaf bit test); `StudioCache` persists a 32-entry fixed ring of inline `[StudioHull; 24]` poses on `WorldLinks` with automatic pose/origin/model invalidation (`sv_unlag`-safe); `PathScratch` resets ZBot A* grids in $O(1)$ via generation stamps.
- **Lock-Free Real-Time Audio & Signon Precache Warmup (`cs-client`):** `GoldSrcMixer` lives exclusively on the audio callback thread fed by a bounded non-blocking `sync_channel` (`try_send` / `try_iter`, zero shared `Mutex`), while `.wav` decoding runs on the main thread into `Arc<DecodedWav>` with eager `InGame` signon warmup so round-1 gunshots have `0.00 ms` hitch.
- **Sub-ms Input-to-Photon Latency & Zero-Alloc `ViewBuilder` (`cs-client`, `cs-cvar`):** Dynamic swapchain reconfiguration honors `gl_vsync 0` (`Immediate` → `Mailbox` → `Fifo`) and `gl_lowlatency 1` (`desired_maximum_frame_latency = 1`), raw OS mouse deltas are sampled immediately before `queue_usercmd` and camera matrix build, and `ViewBuilder::build_into` reuses `FrameView` buffers with a flat `Vec<Option<Vec3>>` entity-position table (**-26.4%** client CPU time).
- **Deterministic Multi-Core Server Snapshots (`cs-engine`):** `send_client_frames` separates sequential ReHLDS-order `AddToFullPack`/game capture (Stage 1) from parallel `std::thread::scope` delta compression + `NetChan` framing across `>= 4` due active clients (Stage 2, inline on `wasm32`) and deterministic `0..max_clients` UDP commit (Stage 3), verified byte-for-byte identical to sequential encoding.

Run the 4-pillar deterministic benchmark suite locally (and see `.superpowers/reports/phase16-blazing-perf.md` for full Before vs After metrics):

```powershell
cargo xtask bench --suite all --gpu
```

## Controls & Shortcuts

| Key | Action |
|---|---|
| `WASD` / `Space` / `Ctrl` / `Shift` | Move / Jump / Crouch / Walk |
| `Mouse1` / `Mouse2` / `R` | Fire / Zoom, Silencer, Burst-Fire or Heavy Knife / Reload |
| `1`–`5` / `Q` / `G` | Weapon Slots / Last Weapon (`lastinv`) / Drop Weapon |
| `B` | Server Text Buy Menu (`1`–`9`/`0`, `buyequip`) |
| `,` / `.` | Buy Primary / Secondary Ammo |
| `E` | Defuse C4 / Open Door / Lead Hostage (`cs_*`) |
| `N` / `T` | Toggle NightVision Goggles / Spray Lambda Logo |
| `Z` / `X` / `C` | Classic Radio Command Menus (`1`–`9`) |
| `TAB` / `M` / `Y` / `U` | Scoreboard / Team & Class Select / Say / Team Say |
| `Mouse1` / `Mouse2` (when dead) | Cycle Spectated Teammate / Toggle Free-Look Spectator |
| `` ` `` (or `Ё`) / `Esc` / `F5` | Developer Console / Pause Menu / GPU PNG Screenshot |

## Verification

The toolchain is pinned to Rust 1.100.0 by `rust-toolchain.toml`; `rustup` installs it on the first `cargo` run; if it does not, run `rustup toolchain install`.

```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p xtask -- parity
```
