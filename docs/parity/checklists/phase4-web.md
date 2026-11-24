# Phase 4 — Web Client (`phase4-web`) Verification Checklist

## Part A — Automated Checks (Workspace CI Gate)

Run sequentially on Windows / CI:

1. `cargo clippy --workspace --all-targets -- -D warnings`
2. `cargo test --workspace` (covers `cs-vfs` layered/cached backends, `cs-platform` clock + `ConfigStorage`, `cs-transport` RFC 6455 codec + HTTP static serving + `ChannelTransport` + `MultiTransport`, `cs-relay` allowlist/rate-limit/protocol-validation + live bridge, `cs-server` multiplexed UDP + WebSocket round-trip, `cs-client` WebGL2 lightmap atlas limit check, and `cs-web` input/VFS/audio/`ChannelTransport` sign-on + real-asset local bots)
3. `cargo check --target wasm32-unknown-unknown -p cs-proto48 -p cs-cvar -p cs-assets -p cs-vfs -p cs-pm -p cs-sim -p cs-platform -p cs-transport -p cs-engine -p cs-probe -p cs-clientcore -p cs-game -p cs-local`
4. `cargo check --target wasm32-unknown-unknown -p cs-relay --lib`
5. `cargo check --target wasm32-unknown-unknown -p cs-client --lib`
6. `cargo check --target wasm32-unknown-unknown -p cs-web`
7. `cargo build --target wasm32-unknown-unknown -p cs-web`
8. `cargo xtask web-build` (stages `dist/web/` + emits `pkg/` bindings when `wasm-bindgen-cli` is installed)
9. `node --check web/app.js web/vfs_loader.js web/transport.js web/input.js web/audio.js web/sw.js`
10. `cargo xtask parity` (validates every `tested` citation and updates category/workspace totals)
11. Live static HTTP + relay smoke check against `cs-server --map de_dust2 --port 27125 --web-port 27126 --web-dir web --relay-allow 127.0.0.1:27125`:
    - `GET http://127.0.0.1:27126/` -> `200 OK`, contains `cs16-canvas`
    - `GET http://127.0.0.1:27126/relay?target=127.0.0.1:9999` -> `403 Forbidden`

## Part B — Manual Browser Checks (Chromium & Firefox)

Prerequisite: install `wasm-bindgen-cli` matching workspace `wasm-bindgen` (`0.2.130`), build `cargo xtask web-build`, and serve `dist/web` (via `cs-server --web-port 27126 --web-dir dist/web --relay-allow 127.0.0.1:27015` or HTTPS reverse proxy for WebTransport/PWA).

- [ ] **WebGPU primary backend (Chromium):** Open `http://localhost:27126/`, verify status pill reports WebGPU adapter, render `de_dust2` at smooth frame pacing without WGSL validation errors.
- [ ] **WebGL2 fallback backend (Firefox / Chromium ANGLE GL / disabled WebGPU):** Disable WebGPU or test WebGL2 context, verify `2048x2048` lightmap atlas & all BSP/MDL/sprite/egui passes render cleanly without `LIMIT` panics.
- [ ] **Local Half-Life folder mounting (`showDirectoryPicker`):** Click **Mount Half-Life Folder**, select Steam `Half-Life`, verify `cstrike_downloads` -> `cstrike_hd` -> `cstrike` -> `valve` -> `platform` layering and map list population (`de_dust2`, `cs_office`, etc.).
- [ ] **Fallback asset import & IndexedDB cache:** Test `<input webkitdirectory>` and drag-and-drop; reload page and confirm assets restore automatically from IndexedDB without re-picking; test **Clear Cache**.
- [ ] **Pointer Lock, raw mouse & Fullscreen:** Click canvas / start offline game, confirm Pointer Lock activates with `unadjustedMovement`, test mouse aim + WASD + `F11` / **Fullscreen** toggle.
- [ ] **Gamepad & Touch input:** Connect controller / emulate coarse pointer in DevTools, verify right-stick look, deadzone (`0.15`), and touch Fire/Jump/Reload/Use controls.
- [ ] **User-gesture Web Audio unlock:** Before any click/keypress, confirm zero `AudioContext` warnings; after clicking **Offline** or canvas, confirm gunshot/footstep WAV playback and positional attenuation.
- [ ] **PWA install & Offline Bots:** Serve over `localhost` or HTTPS, install PWA from browser omnibox, test Offline (`de_dust2`, `4` bots) without network server.
- [ ] **Multiplayer via WebSocket & `cs-relay`:** Connect to `ws://127.0.0.1:27126/ws` (direct multiplexed `cs-server`) and `ws://127.0.0.1:27126/relay?target=127.0.0.1:27015` (UDP relay), confirm full Protocol 48 join, movement, combat, HUD, and scoreboard alongside native UDP `cs-client`.
