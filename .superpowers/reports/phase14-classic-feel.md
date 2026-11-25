# Phase 14 — "99% Authentic CS 1.6 Feel" Completion Report

Branch: `phase14-classic-feel` (branched from `main` at `14d0d28`).
Date: 2026-11-26.

## 1. What Was Done

Phase 14 completed all 15 engineering tasks plus documentation/ledger synchronization (18 commits total on `phase14-classic-feel`):

- **Task 1 (`086afa2`) — Full Mouse Bind Pipeline & Scroll-Wheel Bhop / Duck-Scroll:** Wired `mouse1..5`, `mwheelup`/`mwheeldown` (`invprev`/`invnext` defaults), `hud_fastswitch 0/1`, and GoldSrc `kbutton_t` impulse latching across `cs-cvar`, `cs-client`, `cs-web`, and `web/input.js`.
- **Task 2 (`fddc12e`) — Client-Predicted Material Footsteps (`PM_TraceTexture`):** Added BSP surface-extent & `WorldCollision::trace_texture` (`PM_CL_TraceTexture` / `SurfaceAtPoint`) in `cs-pm`, wired `ClientPmWorld::trace_texture` in `cs-clientcore`, and unified `cs-engine` surface lookup.
- **Task 3 (`b7e424c`) — 100% Vanilla Console Command & CVar Parity + Classic Audio Defaults:** Closed all `726/726` entries in `docs/parity/cvars.toml` (`0` missing), implemented frame-deferred `wait` scripting, `bindtoggle`/`incrementvar`/`multvar`/`condump`, session/server commands (`retry`, `reconnect`, `restart`, `fullupdate`, `maps`, `ping`, `stats`, `notarget`, `fly`, `mp_infinite_ammo 0/1/2`, `mp_mirrordamage`, `violence_*`), live `FCVAR_USERINFO` `setinfo` sync, `cl_showfps`, `con_notifytime`, classic `s_occfactor 0` unmuffled wall sound, and Xash3D `s_lerping`/`hisound` resampling.
- **Task 4 (`e65cd3a`) — Authentic `sniper_scope.spr`, Glow Wall Occlusion & UI Polish:** Replaced synthetic scope mil-dots with `sprites/sniper_scope.spr` quadrant arcs + 1px black crosshairs, added eye-to-origin BSP/solid-brush occlusion traces for `RenderMode::Glow` / `TE_GLOWSPRITE` (`R_SpriteGlowBlend`), resolved raw `#Token` strings via localization tables, and fixed `MainBuyMenu.res` button padding.
- **Task 5 (`a54a1ef`) — Player Blob Shadows (`cl_shadows 1`), `cl_minmodels 1`, Brush-Entity Local Decals & 32-bit Smoke Clouds:** Ported `GL_StudioDrawShadow`/`R_StudioRenderShadow` octagonal ground shadows, `cl_minmodels` (`leet`/`gign`/`vip` + `cl_min_ct`/`cl_min_t`), entity-local `*N` brush decals (moving with doors and evicting on `func_breakable` break), and `gas_puff_01.spr` 30 s grey smoke clouds.
- **Task 6 (`64393b0`) — Authentic GoldSrc `net_graph 1/2/3`, `cl_showpos` & `r_speeds`:** Added `NetGraphTelemetry` 256-entry ring buffer in `cs-clientcore` and ported `SCR_DrawNetGraph` (`net_graph 1/2/3`, `net_graphpos`, `net_graphwidth`, `net_graphheight`, `net_graphsolid`, `net_scale`) plus `cl_showpos` and `r_speeds`.
- **Task 7 (`3798f1b`) — High-Hz Render Decoupling, Boost/Runboost Prediction & Quick-Switch Polish:** Added 100 Hz `usercmd` cadence grid under `fps_override 1` (`cl_cmd_100hz 1`), `visents` player boost/runboost `PM_TraceLine` + `fuser2`/`flDuckTime` replay carry, and AWP->Knife->AWP quick-switch `m_flNextAttack`/FOV/single-deploy sync.
- **Task 8 (`09d1d04`) — Server Network Pacing, `SV_ShouldUpdatePing`, `MsgDest::Broadcast` & `sendinfo_time`:** Wired live `sv_minrate`/`sv_maxrate`/`sv_minupdaterate`/`sv_maxupdaterate`, `IN_SCORE`-gated `svc_pings`, chronological `Broadcast` datagram order, and 1.0 s `sendinfo_time` coalescing in `cs-engine` (`engine.toml` `81/81` implemented, `0` missing).
- **Task 9 (`1e40cf7`) — Opt-In "Beyond 1.6" Practice, Wallbang Lab, Demo & Scrim Commands:** Implemented `sv_showimpacts` (+ wallbang entry/exit logger), `sv_grenade_trajectory`, `sv_showhitboxes`, `rethrow`, `mp_buy_anywhere`, `mp_warmup_mode`, `give_all`, `ammo_full`, `cl_showspeed`, `cl_showkeys`, optional crosshair tweaks, `demo_timescale`/`demo_goto`/`demo_togglepause`, `cl_mute_enemy_team`, `cfg_diff`, `sv_damage_print`, `lo3`, and `mp_knife_round` (all defaulting to `0`).
- **Task 10 (`b9a72eb`) — Built-In KZ / Bhop Mode & `+32 u` LongJump Stats:** Implemented `sv_kz_mode`, `/cp`/`/tp`/`/stuck`/`/start` checkpoints, classic KZ `func_button` timer hooks, `sv_kz_keep_teleport_vel`, and `cl_ljstats`/`sv_ljstats` (`LJ`/`HJ`/`CJ`/`BJ`/`SBJ`/`DropBJ` classification, strafe count, sync %, `+32.0 u` distance convention).
- **Task 11 (`cc85c0f`) + Fix (`ab7af9b`) — Spectator/Demo `spec_xray`, `spec_hud_bars` & Build 10210 `HealthInfo`/`Account`:** Wired `HealthInfo`/`Account`/`ReqState` observer-only emission + `.dem` inclusion (excluding `DEAD_DYING` deathcam frames so `CBasePlayer::Killed` message order stays vanilla), wall-occluded Fresnel `spec_xray` gated strictly to `!alive && observer_mode != 0` or `.dem` playback, and CT/T `spec_hud_bars` sidebars.
- **Task 12 (`cd6dcca`) — Zero-Config ZBot Support for Custom & Arena Maps:** Added `armoury_entity` & `game_player_equip` nav seeds, automatic CRC32-keyed `nav_cache/<map>_<crc>.nav` generation (`bot_nav_autogen 1`), `BotTacticalState::FetchGroundWeapon` floor scavenging (`fy_*`/`aim_*`), and knife-arena (`35hp_*`) strafe-weave combat.
- **Task 13 (`c1ce2a9`) — Interactive 2D Round Replay Player in Web Admin:** Recorded 10 Hz `Round2dTimeline` alongside `.dem` replays, added `/api/replays/<id>/timeline` & `/api/map-overview/<map>{,.png}` (chromakey BMP->PNG + BSP bounds fallback), and added the interactive `<canvas>` 2D replay modal to `web/admin.{html,js}`.
- **Task 14 (`20de634`) — Two-Way `hl.exe <-> cs-client <-> cs-web` Steam Voice Bridge:** Implemented exact CRC32-signed SteamPipe `OPUS_PLC` (`0x0B`/`0x06`/`0x00`/`-1` reset) encode/decode, per-packet codec sniffing (`CSVO` vs `SteamOpusPlc` vs legacy), `voice_steam_compat`, and `cs-server` `sv_voice_transcode` transcoding (`SteamOpusPlc <-> CSVO ADPCM`).
- **Task 15 (`8a058d4`) + Stack Fix (`ccbde23`) — 1-Click Browser P2P Listen Server over WebRTC DataChannels:** Added `MultiPeerChannel` (`PeerAddr::Web(1..=16)`), boxed `LocalGame` `Server`/`Connection` to keep Windows `dev` main-thread stack well under 1 MiB, exposed `startP2pHost`/`pushP2pPacket`/`drainP2pPackets` on `WebClientState` and `window.cs16Web`, mounted public CORS `/lobbies` signaling routes on `cs-server --web-port`, implemented `WebRtcHostHub` & `WebRtcGuestTransport` (`ordered: false, maxRetransmits: 0`, `<= 4096 B`, `<= 16` peers), and wired 1-click hosting + `?p2p=<room>&signal=<origin>` auto-join in `web/{index.html,app.js,transport.js}` (`web_listen_server_webrtc_stretch` -> `tested`).
- **Task 16 (final commit) — Docs, Notices, Report & Exit Gate:** Refreshed `README.md` (`26 Crates`, accurate Phase 11–14 capabilities, removed stale Phase 3/11/12 gaps), synchronized `THIRD-PARTY-NOTICES.md` and `docs/LICENSING.md` for all Phase 14 upstream files, and recorded the Section 17 Exit Gate below.

## 2. What Was Verified Against the Original & How

- **Pre-existing Steam Build 10210 `hlds.exe` captures (`main` baseline preserved):** `cs_office`, `de_dust2`, `as_oilrig` `cs-probe --diff` captures, 25-map baseline edict audit (`24/25` exact), 65 s `cs-pm` differential movement trace (`0.00446` u mean, `0.01810` u p99), and `cs-clientcore` `hlds.rs` prediction/HUD verification remain intact with zero regressions.
- **Headless GPU Visual Smoke Verification (Phase 14):** Rendered `1280x800` Vulkan screenshots on `de_dust2` via `cs-client --headless`:
  - `smoke-hud-netgraph1.png`: spawned CT HUD (`100` HP, `$800`, `1:42`, radar, buyzone, USP viewmodel, `net_graph 1` bars + telemetry, `cl_shadows 1`)
  - `smoke-scope-netgraph3.png`: spawned CT on `de_dust2` with `net_graph 3` compact readout (`fps`, `ms`, `in`/`out`, `loss`/`choke`)
  - `smoke-spec-xray-bars.png`: spectator UI with `spec_hud_bars 1` CT/T team stacks (`100 HP`, `$800`, weapon icons, `C4` badge) and `spec_xray 1` active.

## 3. What Is Only Tested (Automated Suite, Not Yet Live-Diffed vs `hl.exe`)

- Scroll-wheel bhop/duck `kbutton_t` impulse latching, `PM_TraceTexture` predicted footsteps, `sniper_scope.spr` sprite loading, `RenderMode::Glow` occlusion traces, `cl_shadows 1`, `cl_minmodels 1`, brush-entity local decals, `net_graph 1/2/3`, `visents` runboost prediction, AWP quick-switch timing, `SV_ShouldUpdatePing` / `sendinfo_time` / `Broadcast` datagram ordering, all opt-in Beyond 1.6 / KZ / LJ commands, `spec_xray` / `spec_hud_bars` / `HealthInfo` / `Account`, ZBot `.nav` cache & `fy_*`/`35hp_*` arena bots, Web Admin 2D replay player, Steam Voice `OPUS_PLC` container & bridge transcoding, and browser `WebRtcHostHub` / `LocalGame` multi-peer Protocol 48 signon/movement/shots.

## 4. What Is Approximate or Assumed

- `web_transport_webtransport_datagrams` remains `missing` in `docs/parity/web.toml` (browser client tries WebTransport and falls back to WebSocket; `cs-server` does not yet host an HTTP/3 QUIC endpoint).
- Real-browser WebRTC ICE/NAT traversal (`RTCPeerConnection` between two distinct browser tabs/hosts) is syntax-checked (`node --check`) and tested up to the Rust/WASM `ChannelTransport` + `MultiPeerChannel` + HTTP `/lobbies` signaling boundaries; live browser-to-browser ICE connectivity awaits manual Part B verification.
- `cs-amxx` provides the core Pawn 3.2 VM and common `cstrike`/`fun`/`engine`/`fakemeta`/`hamsandwich` natives; obscure third-party AMXX binary modules are out of scope.

## 5. What Needs the Owner (Part B)

See [`docs/parity/checklists/phase14-classic-feel.md`](../../docs/parity/checklists/phase14-classic-feel.md) for the concise owner live-play sign-off checklist (scroll-wheel bhop, `sniper_scope.spr`, `net_graph`, KZ/LJ feel, `spec_xray`/`spec_hud_bars`, 3-way `hl.exe <-> cs-client <-> cs-web` Steam Voice chat, and 1-click browser WebRTC P2P).

## 6. Section 17 Exit Gate Measurements (Post-Final-Code-Edit `ccbde23`)

| Gate | Command | Result |
|---|---|---|
| 1. Workspace Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** (0 warnings) |
| 2. `cs-game` test-hooks Clippy | `cargo clippy -p cs-game --all-targets --features test-hooks -- -D warnings` | **PASS** (0 warnings) |
| 3. Workspace Tests | `cargo test --workspace --no-fail-fast` (`CS16_GAMEDIR` set) | **PASS** (`174` suites: **2,068 passed**, **0 failed**, **4 ignored** live-HLDS/soak) |
| 4a. `wasm32` Library Crates | `cargo check --target wasm32-unknown-unknown -p cs-proto48 ... -p cs-hltv` (19 crates) | **PASS** (0 warnings, 0 errors) |
| 4b. `wasm32` Entry Crates | `cargo check --target wasm32-unknown-unknown -p cs-master --lib`, `cs-relay --lib`, `cs-client --lib`, `cs-web` | **PASS** (0 warnings, 0 errors) |
| 4c. Reference WASM Mods | `cargo xtask mod-build` | **PASS** (6 `.csmod` archives built) |
| 5. `xtask` Tests | `cargo test -p xtask` | **PASS** (`18 + 1 + 1 + 5 = 25 passed`, 0 failed) |
| 6. Parity Report | `cargo run -p xtask -- parity` | **PASS** (`TOTAL verified: 6.6%  tested: 96.3%  implemented: 99.7%`) |
| 7. Browser JS Syntax | `node --check web/*.js` (`admin.js`, `app.js`, `audio.js`, `input.js`, `sw.js`, `transport.js`, `vfs_loader.js`) | **PASS** (0 errors) |
| 8. Headless Visual Smoke | `cs-client --headless --resolution 1280x800 --map de_dust2 ... --screenshot` | **PASS** (3 clean `1280x800` screenshots verified) |
| 9. Code Graph | `graphify update .` | **PASS** (764 files, 18,603 nodes, 45,070 edges, 824 communities) |

### Full `cargo run -p xtask -- parity` Table

```text
bots                 weight   4  total   23  implemented   23  tested   23  verified    0
cvars                weight   6  total  726  implemented  726  tested  726  verified    0
delta                weight   3  total  213  implemented  213  tested  108  verified   12
engine               weight   7  total   81  implemented   81  tested   78  verified    9
entities             weight  12  total  160  implemented  160  tested  160  verified    6
mods                 weight   5  total   16  implemented   16  tested   16  verified    0
movement             weight   8  total   50  implemented   50  tested   50  verified    3
render               weight  10  total   42  implemented   42  tested   42  verified    0
sound                weight   4  total   18  implemented   18  tested   18  verified    0
tempents             weight   4  total   59  implemented   59  tested   59  verified    0
ui                   weight   8  total   50  implemented   50  tested   50  verified    6
usermsg              weight   6  total   73  implemented   73  tested   73  verified   26
weapons              weight  10  total   82  implemented   82  tested   82  verified    7
web                  weight   5  total   18  implemented   17  tested   11  verified    0
wire                 weight   8  total   72  implemented   72  tested   72  verified    7
TOTAL verified: 6.6%  tested: 96.3%  implemented: 99.7%
```
