# Phase 13 Sign-Off Report: Beyond Original (Operations, Web Admin, Replays, XDG, Lobbies & Benchmarks)

Date: 2026-11-25  
Branch: `phase13-beyond`

## 1. Summary of Completed Work
- **Single-Binary Server, Docker & Embedded Web Assets (`cs-server`, `Dockerfile`, `docker-compose.yml`, `docs/DEPLOYMENT.md`):**
  - Cherry-picked `p13-ops` (`cda4fa6`) and wired `mod admin_http;` and `mod replays;` into `crates/cs-server/src/main.rs` and `proto48.rs` (moving `cs-clientcore` and `cs-proto48` to `[dependencies]` in `crates/cs-server/Cargo.toml`).
  - Added CLI flags `--admin`, `--replay-rounds <n>`, and `--embed-web` in `crates/cs-server/src/proto48.rs` with embedded `web/` assets (`index.html`, `app.js`, `vfs_loader.js`, `transport.js`, `input.js`, `audio.js`, `sw.js`, `manifest.webmanifest`, `admin.html`, `admin.js`).
  - Updated `Dockerfile` builder stage to copy `web/` before `cargo build --locked --release -p cs-server --bin cs-server`.
- **Web Admin HTTP Router & Live Server Bridge (`crates/cs-transport`, `crates/cs-server`):**
  - Added `HttpHook` (`read_http_request_with_leftover` + `WsServerConfig::http_hook`) in `crates/cs-transport/src/{ws_proto,ws_native}.rs` so `WsServerTransport` dispatches non-upgrade HTTP requests (including `POST /api/rcon`) before static fallback.
  - Wired `AdminRouter` over `WsServerTransport` serving `/admin`, `/admin.html`, `/admin.js`, `/api/status`, `/api/players`, `/api/stats`, `/api/replays`, `/api/replays/<id>.dem`, `POST /api/rcon`, `/api/invite`, and CORS-enabled `GET /servers` JSON, authenticated in constant time against `rcon_password`.
- **Per-Round HLDEMO v5 Replays & Live Match Stats (`crates/cs-engine`, `crates/cs-game`, `crates/cs-server`):**
  - Added `GameStatEvent` (`RoundStart`, `RoundEnd`, `Damage`, `Kill`, `BombPlant`, `BombDefuse`, `HostageRescue`) on `EngineApi` / `Outbox` (`crates/cs-engine/src/game.rs`) and emitted events from `crates/cs-game/src/{rules,player,score,grenade,hostage,weapons/c4}.rs`.
  - Added `Server::encode_replay_signon`, `Server::enable_replay_capture`, and `Server::take_replay_svc_packets` in `crates/cs-engine/src/server.rs` feeding `RoundReplayStore` and `MatchStatsTracker` (`ADR`, `HS%`, assists, objectives, MVPs).
- **Cross-Platform XDG / macOS / Steam Deck Discovery (`crates/cs-platform`, `crates/cs-vfs`):**
  - Exposed `pub mod xdg;` in `crates/cs-platform/src/lib.rs` and wired `cs_platform::xdg` into `crates/cs-vfs/src/lib.rs` (`candidate_game_dirs_with`, `xdg_config_dir`, `xdg_data_dir`, `candidate_user_config_paths`, `parse_steam_library_paths`), preserving `--gamedir` > `CS16_GAMEDIR` > Steam `libraryfolders.vdf` > fallback priority.
- **Invite Links & Master Lobby Signaling (`web/app.js`, `crates/cs-master`):**
  - Added `crates/cs-master/src/lobbies.rs` (WASM-clean `LobbyRegistry` + signaling queue) and HTTP endpoints `/lobbies`, `/lobbies/<room>`, and `/lobbies/<room>/signal` (`GET`/`POST`/`DELETE`/`OPTIONS`) in `crates/cs-master/src/server.rs`.
  - Wired `?join=<host:port>&room=<code>` invite link parsing and `/lobbies` listing in `web/app.js`.
- **Performance Benchmark Harness (`xtask bench`):**
  - Added `xtask/src/bench.rs` (`cargo xtask bench`) measuring startup readiness, 32-player 100 Hz server tick timing (`avg`/`p50`/`p95`/`p99`/`max`), and delta snapshot packet sizes/bandwidth.

## 2. Benchmark Results (`cargo xtask bench`)
> **Build Profile Note:** All measurements were captured using the default unoptimized **`dev`** Cargo profile (`--release` was NEVER used, per workspace safety rules). Production release numbers will be substantially faster.

Command: `cargo xtask bench --ticks 100 --players 32 --map de_dust2` (`CS16_GAMEDIR="D:/SteamLibrary/steamapps/common/Half-Life"`)
- **Build profile:** `dev` (unoptimized)
- **Map:** `de_dust2` (`gamedir`)
- **Players:** `32` (1 loopback `Connection` at `cl_updaterate 20`, `rate 30000` + 31 ZBot players)
- **Startup readiness:** `285.726 ms` (target budget `< 2000 ms` — **PASS** even in `dev` profile)
- **32-player server tick timing (100 steady-state ticks after navmesh warmup):**
  - `avg`: `1.6973 ms`
  - `p50 / p95 / p99`: `1.5105 / 2.3670 / 3.1846 ms`
  - `max`: `3.5702 ms` (target budget `< 1 ms` is for release builds; `dev` unoptimized p50 is `1.5105 ms`)
- **Delta snapshot packet sizes:** `20` packets over 1.0 s simulated time, `avg 53.1 B`, `max 264 B`, `8.50 kbps` per client at `cl_updaterate 20` (well within `rate 30000`).

## 3. Rule-7 Gate Verification (Executed After Final Code & Parity Edits)
- `cargo clippy --workspace --all-targets -- -D warnings`: **PASSED** (0 warnings)
- `cargo clippy -p cs-game --features test-hooks --all-targets -- -D warnings`: **PASSED** (0 warnings)
- `cargo test --workspace --no-fail-fast`: **PASSED** (`165` suites, `1,942` passed, `0` failed, `4` ignored)
- `wasm32-unknown-unknown` checks (17 library crates including `cs-master` & `cs-platform`, `cs-relay --lib`, `cs-client --lib`, `cs-web`): **PASSED** (0 errors)
- `cargo test -p xtask`: **PASSED** (`18` unit + `1` bin + `1` licence + `4` registry_sync = `24` passed, `0` failed)
- `cargo xtask parity`: **PASSED** (`TOTAL verified: 6.6%  tested: 90.9%  implemented: 95.7%`)
- `node --check web/*.js`: **PASSED**
- `graphify update .`: **PASSED** (`744/744` files, `17584` nodes, `42239` edges, `785` communities)

## 4. Parity & Checklists
- Updated [`docs/parity/engine.toml`](../../docs/parity/engine.toml) (`beyond_web_admin_and_rcon_http`, `beyond_round_replays_and_match_stats`, `beyond_xdg_macos_steamdeck_discovery`, `beyond_perf_bench_harness` -> `tested`)
- Updated [`docs/parity/web.toml`](../../docs/parity/web.toml) (`web_invite_links_and_master_lobbies` -> `tested`, `web_listen_server_webrtc_stretch` note updated)
- Checklist written at [`docs/parity/checklists/phase13-beyond.md`](../../docs/parity/checklists/phase13-beyond.md)
