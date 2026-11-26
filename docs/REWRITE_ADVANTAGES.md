# Rewrite Advantages: Legacy GoldSrc (C/C++) vs `cs16-rs` (Safe Rust)

This document details the measurable engineering, safety, performance, footprint, modding, and deployment advantages achieved by rewriting the Counter-Strike 1.6 / GoldSrc Protocol 48 stack in 100% Safe Rust (`#![forbid(unsafe_code)]`, Rust `1.100.0`, Edition 2024, 27 workspace crates, 466 `.rs` files, ~330K non-empty LOC, **`100.0%` parity** across all 15 ledgers).

Every optimization preserves **two-way Protocol 48 wire compatibility** with retail Steam Build 10210 (`hl.exe` and `hlds.exe`) and **0% visual vibe drift** against the original game.

---

## 1. Memory Safety & Security Without Compromise

Legacy `hl.exe`, `hlds.exe`, `mp.dll`, Metamod, and native AMXX C++ modules accumulated more than two decades of CVEs: stack/heap buffer overflows in BSP/MDL/WAD/SPR parsers, malformed `svc_delta` / `NetChan` packet crashes, client-side slowhacking (`svc_stufftext` overwriting `config.cfg` binds), and arbitrary native code execution from untrusted server plugins.

`cs16-rs` eliminates entire vulnerability classes at compile time:

- **Workspace-Wide `#![forbid(unsafe_code)]`:** Enforced via `[workspace.lints.rust] unsafe_code = "forbid"` in the root `Cargo.toml` across all 27 crates — including binary asset parsers (`BSP v30`, `WAD3`, `MDL v10`, `SPR v2`, `.nav`, `HPAK`, `HLDEMO v5`), the Protocol 48 bitstream/delta codec, the pure-Rust HTTP/3 WebTransport (`wtransport` / `quinn` + `rustls`/`ring`) & RFC 6455 WebSocket + HTTP/1.1 servers, the Pawn 3.2 `.amxx` VM, the `wasmi` WASM host, the pure-Rust Python 3 plugin interpreter, and the `cs16_wasm_exports!` zero-`unsafe` Rust guest mod macros.
- **Hardened Protocol & Web Boundaries:**
  - `cl_filterstuffcmd` allowlists `svc_stufftext` commands and blocks path traversal or command injection (`;`, `..`, NUL).
  - `cs-relay` enforces explicit `--relay-allow` allowlists, per-IP/global session caps, deterministic token-bucket rate limits, and OOB/NetChan packet validation.
  - WebTransport HTTP/3 datagram sessions enforce identical per-peer rate/size limits and advertise ephemeral ECDSA P-256 SHA-256 certificate fingerprints over `/api/server-info` (plus optional `--wt-cert` / `--wt-key` production TLS).
  - Web Admin (`/admin`, `/api/*`) verifies `rcon_password` Bearer tokens in constant time and sandboxes Plugin Studio file access against path traversal and symlink escapes.
- **Sandboxed Multi-Language Plugin Execution:** Every Rust/C++20/Go (`.wasm`), Python 3 (`.py`), and AMXX (`.amxx`) plugin executes under explicit fuel and memory ceilings. An infinite loop, division by zero, or out-of-bounds access faults only that plugin, keeps the server ticking at 100 Hz, and writes a bounded (`<= 256 KiB`) deterministic `ModFaultDump` JSON snapshot for offline `< 10 ms` replay.

---

## 2. 3-Way Head-to-Head: Retail Steam `hlds.exe` (Build 10210) vs `cs16-rs` (`dev`) vs `cs16-rs` (`--release` LTO)

All measurements below were captured back-to-back on the exact same machine (Windows 11 `x86_64`, NVIDIA GeForce RTX 5070 Laptop GPU / Vulkan, Rust `1.100.0`) against real Steam `de_dust2` assets and the live retail Steam dedicated server (`hlds.exe`, `Exe version 1.1.2.7 (cstrike)`, `Exe build: 19:00:16 Oct  7 2024 (10210)`). Production builds use `[profile.release]` (`opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`).

### Live Head-to-Head: Retail Steam `hlds.exe` (Build 10210) vs `cs-server.exe` (`--release` LTO)

| Metric (`32p de_dust2`, identical hardware) | Retail Steam `hlds.exe` (Build 10210, C/C++) | `cs-server.exe` (`--release` LTO, Vanilla) | `cs-server.exe` (`--release` LTO, +31 ZBots & 9 Mods) | Advantage vs Retail `hlds.exe` |
|---|---:|---:|---:|---|
| **Cold Startup -> First `A2S_INFO` Reply** *(median of 3)* | `324.28 ms` | **`152.41 ms`** (0 bots) / **`156.92 ms`** (31 ZBots) | **`192.84 ms`** (31 ZBots + 9 WASM/Py mods) | **2.13x faster** vanilla; **1.68x faster** even w/ 31 ZBots + 9 mods |
| **Idle RAM (Working Set / Private)** | `89.93 / 80.64 MiB` (0 bots) | **`77.34 / 72.12 MiB`** (0 bots) | **`90.73 / 88.55 MiB`** (31 ZBots + 9 mods) | **-14.0% WS / -10.6% Private** apples-to-apples |
| **Active Match RAM (Working Set / Private)** | `110.44 / 101.09 MiB` (1 player, 0 bots) | **`77.52 / 72.25 MiB`** (**31 active ZBots**) | **`97.20 / 93.82 MiB`** (**32p full + 9 mods**) | **-29.8% WS** (31 ZBots); **-12.0% WS** (32p + 9 mods) |
| **Server Tick Rate Ceiling (`Hz`)** | `97.81 Hz` default (`14%` idle CPU); **`646.12 Hz`** uncapped (`sys_ticrate 10000`, 0 bots) | **`2,506 Hz`** (`0.3991 ms` avg tick w/ **32p + 31 ZBots**) | **`2,506 Hz`** (25.1x headroom at 100 Hz) | **3.88x higher** tick ceiling even under 31-bot combat |
| **Server Disk Binary Footprint** | `29.83 MiB` (`12` loaded DLLs/EXE: `7.88 MiB` game + `21.95 MiB` Steam runtime, x86) | **`15.87 MiB`** (`1` standalone x86_64 binary) | **`15.87 MiB`** (incl. ZBots, Web Admin, WebTransport, 5-lang mods) | **-46.8%**, single binary, zero external runtime DLLs |
| **Live Protocol 48 `cs-probe` Parse Errors** | `0` (`0.887 s` signon, `58` deltas) | **`0`** (`1.235 s` signon, `55` deltas) | **`0`** (`1.228 s` signon, `55` deltas) | **100% two-way wire parity** |

---

## 3. Full 4-Pillar `xtask bench` Results (`dev` vs `--release` LTO)

### Suite 1 — 32-Player `de_dust2` Server + 31 ZBots (`100` ticks)

| Metric | Pre-Phase 16 (`dev`) | Post-Phase 16 (`dev`) | **`--release` LTO (`opt-level = 3`)** | `--release` vs `dev` |
|---|---:|---:|---:|---:|
| Cold startup readiness (in-process + 31 ZBots) | `266.858 ms` | `223.864 ms` | **`62.809 ms`** | **3.56x faster** (`-71.9%`) |
| Server tick avg | `1.0902 ms` (`917 Hz`) | `1.1811 ms` (`847 Hz`) | **`0.3991 ms` (`2,506 Hz`)** | **2.96x faster** (`-66.2%`) |
| Server tick p50 | — | `1.0092 ms` | **`0.3779 ms`** | **2.67x faster** (`-62.6%`) |
| Server tick p95 | `1.8264 ms` | `1.9913 ms` | **`0.4947 ms`** | **4.03x faster** (`-75.2%`) |
| Server tick p99 | `2.2675 ms` | `2.3925 ms` | **`0.7627 ms`** | **3.14x faster** (`-68.1%`) |
| Server tick max | `2.3085 ms` | `2.5335 ms` | **`0.7636 ms`** | **3.32x faster** (`-69.9%`) |
| Outgoing delta wire output | `20 pkts`, `52.6 B` avg (`8.42 kbps`) | `20 pkts`, `52.6 B` avg (`8.42 kbps`) | **`20 pkts`, `52.6 B` avg (`8.42 kbps`)** | **0 B drift** |

### Suite 2 — Protocol 48 Delta Codec (`100,000` mixed player/entity/clientdata/usercmd ops)

| Metric | Pre-Phase 16 (`dev`) | Post-Phase 16 (`dev`) | **`--release` LTO (`opt-level = 3`)** | `--release` vs `dev` / Pre-P16 |
|---|---:|---:|---:|---:|
| Encode latency | `8,206.24 ns/op` | `2,286.75 ns/op` | **`615.92 ns/op`** | **3.71x** vs `dev` / **13.3x** vs Pre-P16 |
| Decode latency | `7,397.44 ns/op` | `1,409.25 ns/op` | **`155.07 ns/op`** | **9.09x** vs `dev` / **47.7x** vs Pre-P16 |
| Total encode + decode | `15,603.68 ns/op` | `3,696.00 ns/op` | **`770.99 ns/op`** | **4.79x** vs `dev` / **20.2x** vs Pre-P16 |
| Throughput | `64,087.5 ent/s` | `270,562.5 ent/s` | **`1,297,035.4 ent/s` (`1.30M/s`)** | **4.79x** vs `dev` / **20.2x** vs Pre-P16 |
| Total serialized bytes | `1,795,811 B` | `1,795,811 B` | **`1,795,811 B`** | **Bit-exact (`0 B` diff)** |

### Suite 3 — Client CPU (`de_dust2`, `1,000` frames, 32 players, 64 tempents, 24 decals, 5 unacked cmds)

| Metric | Pre-Phase 16 (`dev`) | Post-Phase 16 (`dev`) | **`--release` LTO (`opt-level = 3`)** | `--release` vs `dev` |
|---|---:|---:|---:|---:|
| Frame CPU avg | `416.37 us` | `408.82 us` (`2,446 FPS`) | **`36.78 us` (`27,189 FPS` ceiling)** | **11.12x faster** (`-91.0%`) |
| Frame CPU p50 | `413.60 us` | `406.50 us` | **`36.70 us`** | **11.08x faster** (`-91.0%`) |
| Frame CPU p99 | `455.70 us` | `445.60 us` | **`44.00 us`** | **10.13x faster** (`-90.1%`) |
| Frame CPU max | `510.30 us` | `455.30 us` | **`74.50 us`** | **6.11x faster** (`-83.6%`) |

### Suite 4 — Headless Vulkan GPU (`de_dust2`, `1280x800`, `200` frames, 16 studio models + 3 smoke clouds = 45 sprites)

| Metric | Pre-Phase 16 (`dev`) | Post-Phase 16 (`dev`) | **`--release` LTO (`opt-level = 3`)** | `--release` vs `dev` / Pre-P16 |
|---|---:|---:|---:|---:|
| Frame avg | `13.8197 ms` (`72.4 FPS`) | `2.2163 ms` (`451.2 FPS`) | **`0.4098 ms` (`2,440.2 FPS`)** | **5.41x** vs `dev` / **33.7x** vs Pre-P16 |
| Frame p50 | `13.7954 ms` | `2.1956 ms` | **`0.3361 ms`** | **6.53x** vs `dev` / **41.0x** vs Pre-P16 |
| Frame p99 | `14.6244 ms` | `2.8256 ms` | **`0.8060 ms`** | **3.51x** vs `dev` / **18.1x** vs Pre-P16 |
| Total draw calls / frame | `111.0` | `67.0` | **`67.0`** (`20` BSP, `40` MDL, `1` smoke) | **-39.6%** total (`45x` fewer smoke DCs) |
| CPU->GPU upload bytes / frame | `1,869,016 B` (`1,825.21 KiB`) | `76,648 B` (`74.85 KiB`) | **`76,648 B` (`74.85 KiB`)** | **24.38x less upload** (`-95.9%`) |

---

## 4. Production Binary & Browser PWA Footprint

| Deliverable | Stripped Size | Notes |
|---|---:|---|
| `cs-server.exe` (Windows x86_64) | **`15.87 MiB`** | Standalone binary: 100 Hz Protocol 48 engine, ZBot AI, Web Admin (`/admin`), embedded PWA, HTTP/3 WebTransport, 5-lang mod host |
| `cs-client.exe` (Windows x86_64) | **`26.61 MiB`** | Standalone `wgpu 30` Vulkan/DX12 client, VGUI2, lock-free audio, Steam Voice, full local listen server + `dm_rusthouse` demo pack |
| `cs-hltv.exe` / `cs-probe.exe` / `cs-master.exe` / `cs-relay.exe` | **`1.76` / `1.56` / `1.14` / `1.13 MiB`** | Ultra-compact dedicated infrastructure binaries |
| `cs_web_bg.wasm` (Browser PWA) | **`14.64 MiB` raw / `4.67 MiB` (`4,785.6 KiB`) `gzip -9`** | Full engine + renderer + ZBot AI + clean-room `dm_rusthouse` Instant Demo booting in **`< 150 ms`** with zero external asset downloads |

---

## 5. Modern GPU Rendering with 0% Visual Drift

Legacy GoldSrc renders through 1998 immediate-mode OpenGL 1.1 (`glBegin`/`glEnd`), performing `.mdl` vertex skinning on the CPU every frame and issuing one draw call per smoke sprite puff. On modern drivers, triple smoke stacks on `de_dust2` B-site still cause severe frame drops in retail CS 1.6.

`cs16-rs` renders on `wgpu 30` (Vulkan, DX12, Metal, WebGPU, and WebGL2 downlevel) while preserving exact GoldSrc lightmap gamma, chrome reflection, and `SHADE_LAMBERT = 1.4953241` shading:

- **GPU `.mdl` Vertex Skinning:** Static rest-pose vertex buffers upload once per `(bodypart, submodel)`. Per frame, the CPU writes only `<= 128` 3x4 affine bone matrices (`6,144 B` UBO — well below WebGL2's `16 KiB` limit), while `mdl.wgsl` performs skinning, exact GoldSrc directional lighting, `STUDIO_NF_CHROME` UVs, and optional `spec_xray` Fresnel shading.
- **1-Draw-Call Smoke Batching:** `SpriteRenderer` interns textures into `(sprite_idx: u32, frame: u32)` slab handles and run-length coalesces consecutive quads into single draw calls (`45` smoke puffs -> `1` draw call).
- **Cached `GammaTables`:** Gamma LUTs recompute only when `gamma`/`texgamma`/`lightgamma`/`brightness` change (`0` steady-state `powf` calls).
- **Verified `--release` LTO Visual Parity (`cargo run --release -p xtask -- visual-diff` on `de_dust2` `1280x720`):**
  - Retail CS 1.6 `right_wall` reference: RGB **`(113, 96, 65) +/- 4`**
  - `cs16-rs --release` measured: RGB **`(112.6, 95.2, 65.0)`** — delta **`(-0.4, -0.8, +0.0)`** (**PASS**).

---

## 6. Concurrency, Transport, Audio & Input-to-Photon Latency

- **100.0% Parity Ledger & HTTP/3 WebTransport Datagrams:** Server-side pure-Rust `WebTransport` HTTP/3 unreliable datagrams (`cs-transport::wt_native`, `quinn` + `rustls`/`ring`, zero `aws-lc-rs` C deps) eliminate WebSocket TCP head-of-line blocking for browser multiplayer alongside UDP and WebSocket in `MultiTransport`, closing the final `0.3%` gap to **`100.0%` implemented** (`1,783 / 1,783`).
- **Deterministic Multi-Core Server Snapshots (`cs-engine`):** `send_client_frames` isolates sequential ReHLDS-order `AddToFullPack` capture (Stage 1), fans out Protocol 48 delta compression + `NetChan` framing across CPU cores via `std::thread::scope` for `>= 4` active clients (Stage 2), and commits packets in deterministic `0..max_clients` order (Stage 3) — byte-for-byte identical to single-threaded encoding.
- **Zero-Alloc Server Hot Paths:** Bit-packed `Vec<u64>` PVS/PAS rows (`1`-instruction visibility tests), persistent 32-entry `StudioCache` across ticks, and generation-stamped $O(1)$-reset `PathScratch` for ZBot A* pathfinding.
- **Lock-Free Real-Time Audio:** `GoldSrcMixer` lives exclusively on the audio callback thread fed by a bounded non-blocking `sync_channel` (`try_send`, zero shared `Mutex`), while `.wav` decoding and eager `InGame` signon precache warmup execute on the main thread for `0.00 ms` round-1 gunshot hitch.
- **Sub-ms Input-to-Photon Scanout:** `gl_vsync 0` selects `PresentMode::Immediate`, `gl_lowlatency 1` configures `desired_maximum_frame_latency = 1`, raw OS mouse deltas flush right before `queue_usercmd` and camera matrix build, and `cl_cmd_100hz 1` decouples high-refresh rendering (`> 105 FPS`) onto a clean `100 Hz` command grid.

---

## 7. Polyglot Modding Ecosystem & Zero-Friction Migration

| Capability | Legacy AMX Mod X / Metamod | `cs16-rs` Mod Platform (ABI v2) |
|---|---|---|
| Languages | Pawn 3.2 (`.sma` -> `.amxx`) or unsafe C++ Metamod `.dll`/`.so` | **Python 3** (zero-compile), **Rust** (zero-`unsafe` `cs16_wasm_exports!`), **C++20**, **Go/WASI**, and legacy **`.amxx`** simultaneously |
| Live Development | Edit -> external `amxxpc` compile -> upload -> map change | Edit `.py` on disk or in `/admin` Plugin Studio -> **`< 5 ms` hot-reload** preserving storage |
| Crash Debugging | Segfault kills server process; guesswork from logs | Automatic `ModFaultDump` JSON snapshot -> **`< 10 ms` offline replay** (`cargo xtask mod replay`) |
| Performance Profiling | External profiler plugins | Built-in per-plugin `resmon` (`mod_stats` + live `/admin` CPU microsecond bars) |
| Legacy Migration | Manual rewrite | **`cargo xtask import-amxx`** migrates `users.ini`, `plugins.ini`, `data/lang/*.txt`, and `amxx.cfg` in 5 seconds |

---

## 8. Cross-Platform, 1-Click Instant Demo & Browser-First Deployment

Legacy GoldSrc shipped 32-bit x86 binaries requiring a local retail install before a single frame could render. Because `cs16-rs` shares its engine, movement, protocol, AI, renderer, and VGUI2 stack cleanly across native and `wasm32-unknown-unknown`:

- **1-Click Clean-Room Instant Demo (`dm_rusthouse`):** Clicking **`▶ Play Instant Demo`** in the browser PWA (or launching `cs-client --demo` / `cs-server --demo`) synthesizes a complete clean-room `BSP v30` arena, prebaked ZBot `.nav`, `WAD3` textures, `MDL v10` player/weapon models, `SPR v2` HUD sprites, and PCM `.wav` audio in memory in `< 150 ms` (`0` Valve bytes, `0` extra downloads), mounting only as a lowest-priority fallback so retail Steam assets are never shadowed.
- **Native Binaries:** Automated CI releases for Windows (`x86_64`), Linux / SteamOS / Steam Deck (`x86_64`), and macOS Apple Silicon (`arm64`).
- **Browser PWA (`https://maybewewill.github.io/cs16-rs/`):** Installable desktop/mobile web app (`4.67 MiB` gzipped WASM) with 1-click Instant Demo, local IndexedDB Steam folder caching, Pointer Lock raw mouse + scroll wheel binds, Web Audio room reverb, offline ZBots, 1-click WebRTC DataChannel P2P rooms, and HTTP/3 WebTransport datagram multiplayer.
- **Modern Operations:** Single-binary `cs-server --embed-web --admin --webtransport-port 27017`, multi-stage Docker container (`ghcr.io/maybewewill/cs16-rs`), and an authenticated browser Admin Panel with 10 Hz 2D HLTV-style round replays.
