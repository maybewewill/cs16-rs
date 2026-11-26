# Rewrite Advantages: Legacy GoldSrc (C/C++) vs `cs16-rs` (Safe Rust)

This document details the measurable engineering, safety, performance, modding, and deployment advantages achieved by rewriting the Counter-Strike 1.6 / GoldSrc Protocol 48 stack in 100% Safe Rust (`#![forbid(unsafe_code)]`, Rust `1.100.0`, Edition 2024, 27 workspace crates, 457 `.rs` files, ~321,686 non-empty LOC).

Every optimization preserves **100% Protocol 48 wire compatibility** with retail Steam Build 10210 (`hl.exe` and `hlds.exe`) and **0% visual vibe drift** against the original game.

---

## 1. Memory Safety & Security Without Compromise

Legacy `hl.exe`, `hlds.exe`, `mp.dll`, Metamod, and native AMXX C++ modules accumulated more than two decades of CVEs: stack/heap buffer overflows in BSP/MDL/WAD/SPR parsers, malformed `svc_delta` / `NetChan` packet crashes, client-side slowhacking (`svc_stufftext` overwriting `config.cfg` binds), and arbitrary native code execution from untrusted server plugins.

`cs16-rs` eliminates entire vulnerability classes at compile time:

- **Workspace-Wide `#![forbid(unsafe_code)]`:** Enforced via `[workspace.lints.rust] unsafe_code = "forbid"` in the root `Cargo.toml` across all 27 crates — including binary asset parsers (`BSP v30`, `WAD3`, `MDL v10`, `SPR v2`, `.nav`, `HPAK`, `HLDEMO v5`), the Protocol 48 bitstream/delta codec, the RFC 6455 WebSocket + HTTP/1.1 server, the Pawn 3.2 `.amxx` VM, the `wasmi` WASM host, and the pure-Rust Python 3 plugin interpreter.
- **Hardened Protocol & Web Boundaries:**
  - `cl_filterstuffcmd` allowlists `svc_stufftext` commands and blocks path traversal or command injection (`;`, `..`, NUL).
  - `cs-relay` enforces explicit `--relay-allow` allowlists, per-IP/global session caps, deterministic token-bucket rate limits, and OOB/NetChan packet validation.
  - Web Admin (`/admin`, `/api/*`) verifies `rcon_password` Bearer tokens in constant time and sandboxes Plugin Studio file access against path traversal and symlink escapes.
- **Sandboxed Multi-Language Plugin Execution:** Every Rust/C++20/Go (`.wasm`), Python 3 (`.py`), and AMXX (`.amxx`) plugin executes under explicit fuel and memory ceilings. An infinite loop, division by zero, or out-of-bounds access faults only that plugin, keeps the server ticking at 100 Hz, and writes a bounded (`<= 256 KiB`) deterministic `ModFaultDump` JSON snapshot for offline `< 10 ms` replay.

---

## 2. Measured Performance (Phase 16 Before vs After Benchmarks)

All measurements below were captured deterministically by `cargo xtask bench --suite all --gpu` on the same machine, real Steam `de_dust2` map assets, NVIDIA GeForce RTX 5070 Laptop GPU (Vulkan), and unoptimized `dev` profile (`CARGO_PROFILE_DEV_DEBUG=0`). Production builds additionally apply `[profile.release]` (`opt-level = 3`, `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`).

### Suite 1 — 32-Player `de_dust2` Server + 31 ZBots (`100` ticks)

| Metric | Pre-Phase 16 (`63e4049`) | Post-Phase 16 | Improvement |
|---|---:|---:|---:|
| Cold startup readiness | `266.858 ms` | **`228.559 ms`** | **-14.4%** (`1.17x`) |
| Server tick avg | `1.0902 ms` | **`1.0152 ms`** | **-6.9%** (`1.07x`) |
| Server tick p95 | `1.8264 ms` | **`1.5926 ms`** | **-12.8%** (`1.15x`) |
| Server tick p99 | `2.2675 ms` | **`1.8740 ms`** | **-17.4%** (`1.21x`) |
| Server tick max | `2.3085 ms` | **`1.9461 ms`** | **-15.7%** (`1.19x`) |
| Outgoing delta wire output | `20 pkts`, `52.6 B` avg (`8.42 kbps`) | **`20 pkts`, `52.6 B` avg (`8.42 kbps`)** | **0 B drift** |

### Suite 2 — Protocol 48 Delta Codec (`100,000` mixed player/entity/clientdata/usercmd ops)

| Metric | Pre-Phase 16 (`63e4049`) | Post-Phase 16 | Improvement |
|---|---:|---:|---:|
| Encode latency | `8,206.24 ns/op` | **`2,016.44 ns/op`** | **4.07x faster** (`-75.4%`) |
| Decode latency | `7,397.44 ns/op` | **`1,405.91 ns/op`** | **5.26x faster** (`-81.0%`) |
| Total encode + decode | `15,603.68 ns/op` | **`3,422.35 ns/op`** | **4.56x faster** (`-78.1%`) |
| Throughput | `64,087.5 ent/s` | **`292,197.2 ent/s`** | **4.56x throughput** |
| Total serialized bytes | `1,795,811 B` | **`1,795,811 B`** | **Bit-exact (`0 B` diff)** |

### Suite 3 — Client CPU (`de_dust2`, `1,000` frames, 32 players, 64 tempents, 24 decals, 5 unacked cmds)

| Metric | Pre-Phase 16 (`63e4049`) | Post-Phase 16 | Improvement |
|---|---:|---:|---:|
| Frame CPU avg | `416.37 us` | **`306.41 us`** | **1.36x faster** (`-26.4%`) |
| Frame CPU p50 | `413.60 us` | **`305.00 us`** | **1.36x faster** (`-26.3%`) |
| Frame CPU p99 | `455.70 us` | **`337.00 us`** | **1.35x faster** (`-26.1%`) |
| Frame CPU max | `510.30 us` | **`374.70 us`** | **1.36x faster** (`-26.6%`) |

### Suite 4 — Headless GPU Frame (`de_dust2`, `1280x800`, `200` frames, 16 studio models + 3 smoke clouds = 45 sprites)

| Metric | Pre-Phase 16 (`63e4049`) | Post-Phase 16 | Improvement |
|---|---:|---:|---:|
| Frame avg | `13.8197 ms` (`72.4 FPS`) | **`1.8798 ms` (`532.0 FPS`)** | **7.35x faster** (`-86.4%`) |
| Frame p50 | `13.7954 ms` | **`1.8553 ms`** | **7.44x faster** (`-86.6%`) |
| Frame p99 | `14.6244 ms` | **`2.2902 ms`** | **6.39x faster** (`-84.3%`) |
| Total draw calls / frame | `111.0` | **`67.0`** | **-39.6%** |
| Smoke/sprite draw calls / frame | `45.0` | **`1.0`** | **45.0x fewer draw calls** |
| CPU->GPU upload bytes / frame | `1,869,016 B` (`1,825.21 KiB`) | **`76,648 B` (`74.85 KiB`)** | **24.38x less upload** (`-95.9%`) |

---

## 3. Modern GPU Rendering with 0% Visual Drift

Legacy GoldSrc renders through 1998 immediate-mode OpenGL 1.1 (`glBegin`/`glEnd`), performing `.mdl` vertex skinning on the CPU every frame and issuing one draw call per smoke sprite puff. On modern drivers, triple smoke stacks on `de_dust2` B-site still cause severe frame drops in retail CS 1.6.

`cs16-rs` renders on `wgpu 30` (Vulkan, DX12, Metal, WebGPU, and WebGL2 downlevel) while preserving exact GoldSrc lightmap gamma, chrome reflection, and `SHADE_LAMBERT = 1.4953241` shading:

- **GPU `.mdl` Vertex Skinning:** Static rest-pose vertex buffers upload once per `(bodypart, submodel)`. Per frame, the CPU writes only `<= 128` 3x4 affine bone matrices (`6,144 B` UBO — well below WebGL2's `16 KiB` limit), while `mdl.wgsl` performs skinning, exact GoldSrc directional lighting, `STUDIO_NF_CHROME` UVs, and optional `spec_xray` Fresnel shading.
- **1-Draw-Call Smoke Batching:** `SpriteRenderer` interns textures into `(sprite_idx: u32, frame: u32)` slab handles and run-length coalesces consecutive quads into single draw calls (`45` smoke puffs -> `1` draw call).
- **Cached `GammaTables`:** Gamma LUTs recompute only when `gamma`/`texgamma`/`lightgamma`/`brightness` change (`0` steady-state `powf` calls).
- **Verified Visual Parity (`cargo xtask visual-diff` on `de_dust2` `1280x720`):**
  - Retail CS 1.6 `right_wall` reference: RGB **`(113, 96, 65) +/- 4`**
  - `cs16-rs` measured: RGB **`(112.6, 95.2, 65.0)`** — delta **`(-0.4, -0.8, +0.0)`** (**PASS**).

---

## 4. Concurrency, Audio & Input-to-Photon Latency

- **Deterministic Multi-Core Server Snapshots (`cs-engine`):** `send_client_frames` isolates sequential ReHLDS-order `AddToFullPack` capture (Stage 1), fans out Protocol 48 delta compression + `NetChan` framing across CPU cores via `std::thread::scope` for `>= 4` active clients (Stage 2), and commits UDP packets in deterministic `0..max_clients` order (Stage 3) — proven byte-for-byte identical to single-threaded encoding.
- **Zero-Alloc Server Hot Paths:** Bit-packed `Vec<u64>` PVS/PAS rows (`1`-instruction visibility tests), persistent 32-entry `StudioCache` across ticks, and generation-stamped $O(1)$-reset `PathScratch` for ZBot A* pathfinding.
- **Lock-Free Real-Time Audio:** `GoldSrcMixer` lives exclusively on the audio callback thread fed by a bounded non-blocking `sync_channel` (`try_send`, zero shared `Mutex`), while `.wav` decoding and eager `InGame` signon precache warmup execute on the main thread for `0.00 ms` round-1 gunshot hitch.
- **Sub-ms Input-to-Photon Scanout:** `gl_vsync 0` selects `PresentMode::Immediate`, `gl_lowlatency 1` configures `desired_maximum_frame_latency = 1`, raw OS mouse deltas flush right before `queue_usercmd` and camera matrix build, and `cl_cmd_100hz 1` decouples high-refresh rendering (`> 105 FPS`) onto a clean `100 Hz` command grid.

---

## 5. Polyglot Modding Ecosystem & Zero-Friction Migration

| Capability | Legacy AMX Mod X / Metamod | `cs16-rs` Mod Platform (ABI v2) |
|---|---|---|
| Languages | Pawn 3.2 (`.sma` -> `.amxx`) or unsafe C++ Metamod `.dll`/`.so` | **Python 3** (zero-compile), **Rust**, **C++20**, **Go/WASI**, and legacy **`.amxx`** simultaneously |
| Live Development | Edit -> external `amxxpc` compile -> upload -> map change | Edit `.py` on disk or in `/admin` Plugin Studio -> **`< 5 ms` hot-reload** preserving storage |
| Crash Debugging | Segfault kills server process; guesswork from logs | Automatic `ModFaultDump` JSON snapshot -> **`< 10 ms` offline replay** (`cargo xtask mod replay`) |
| Performance Profiling | External profiler plugins | Built-in per-plugin `resmon` (`mod_stats` + live `/admin` CPU microsecond bars) |
| Legacy Migration | Manual rewrite | **`cargo xtask import-amxx`** migrates `users.ini`, `plugins.ini`, `data/lang/*.txt`, and `amxx.cfg` in 5 seconds |

---

## 6. Cross-Platform & Browser-First Deployment

Legacy GoldSrc shipped 32-bit x86 binaries tied to Windows/Linux desktops. Because `cs16-rs` shares its engine, movement, protocol, AI, renderer, and VGUI2 stack cleanly across native and `wasm32-unknown-unknown`:

- **Native Binaries:** Automated CI releases for Windows (`x86_64`), Linux / SteamOS / Steam Deck (`x86_64`), and macOS Apple Silicon (`arm64`).
- **Browser PWA (`https://maybewewill.github.io/cs16-rs/`):** Installable desktop/mobile web app with local IndexedDB asset caching, Pointer Lock raw mouse + scroll wheel binds, Web Audio room reverb, offline ZBots, and 1-click WebRTC DataChannel P2P rooms.
- **Modern Operations:** Single-binary `cs-server --embed-web --admin`, multi-stage Docker container (`ghcr.io/maybewewill/cs16-rs`), and an authenticated browser Admin Panel with 10 Hz 2D HLTV-style round replays.
