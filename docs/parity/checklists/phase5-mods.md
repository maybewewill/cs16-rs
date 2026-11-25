# Phase 5 — Built-in WASM Mods (`phase5-mods`) Verification Checklist

## Part A — Automated Checks (Workspace CI Gate)

Run sequentially on Windows / CI:

1. `cargo clippy --workspace --all-targets -- -D warnings`
2. `cargo test --workspace` (covers `cs-proto48` dynamic usermsg + raw write, `cs-engine` capability gating `_cs16rs_mods` & `world_collision_arc`, `cs-vfs` `mods/<id>/` asset overlay mount/unmount, `cs-mod-api` ABI codecs/manifest/SHA-256/`.csmod` ZIP, `cs-mod-host` `wasmi` sandbox + hookchains + server/client natives + transfer + 6 executable reference mods, `cs-mod-sdk` macros/builders/scaffold, `cs-server` `--mods-dir`, `cs-client` `draw_mod_hud` + approval modal, `cs-web` client mod runtime)
3. `cargo check --target wasm32-unknown-unknown -p cs-proto48 -p cs-cvar -p cs-assets -p cs-vfs -p cs-pm -p cs-sim -p cs-platform -p cs-transport -p cs-engine -p cs-probe -p cs-clientcore -p cs-game -p cs-local -p cs-mod-api -p cs-mod-host -p cs-mod-sdk`
4. `cargo check --target wasm32-unknown-unknown -p cs-relay --lib`
5. `cargo check --target wasm32-unknown-unknown -p cs-client --lib`
6. `cargo check --target wasm32-unknown-unknown -p cs-web`
7. `cargo xtask mod build` (packs all showcase polyglot mods in `mods/` into `dist/mods/*.csmod` with SHA-256 digests and verifies zero `.wat` files remain)
8. `cargo xtask parity` (validates every `tested` citation in `docs/parity/mods.toml` and prints ledger totals)

> **Phase 15 Update:** All raw `.wat` prototype mods from Phase 5 were replaced by the full localized polyglot AMXX+ suite (`mods/std/`, `mods/community/`, `mods/gamemodes/`) running across Rust, C++20, Go/WASI, Python 3, and AMXX. See [`phase15-mod-platform.md`](phase15-mod-platform.md) and [`docs/MODDING.md`](../../MODDING.md) for the complete Phase 15 verification checklist.

## Part B — Manual Multi-Client Checks (Original CS 1.6 + `cs-client` + `cs-web`)

Prerequisite: launch `cs-server --map de_dust2 --port 27015 --web-port 27126 --web-dir dist/web --relay-allow 127.0.0.1:27015 --mods-dir mods` (after `cargo xtask web-build` if testing browser `cs-web`).

- [ ] **Original CS 1.6 client compatibility:** Connect from stock Steam Counter-Strike 1.6 (`connect 127.0.0.1:27015`); verify vanilla HUD/gameplay works without `CsModManifest`/`CsModChunk` packets and server-side mod rules (`deathmatch` auto-respawn, `gungame` weapon progression, `zombie` HP/speed boost, `admin_menu` classic `ShowMenu`, `player_stats` `/rank` chat reply) apply seamlessly.
- [ ] **First-join client mod approval modal (`cs-client`):** Connect from `cs-client -- --exec "connect 127.0.0.1:27015"`, verify **Server Mod Download** window displays `custom_hud` version, byte size, SHA-256 prefix, and declared permissions; test **Deny** skips download and **Allow** triggers chunked `.csmod` transfer, SHA-256 verification, VFS overlay mount, and custom HUD overlay rendering.
- [ ] **Browser client (`cs-web`) capability & HUD overlay:** Connect from Chromium/Firefox to `ws://127.0.0.1:27126/ws`, run `mod_approve custom_hud` in console, verify custom HUD overlay renders inside WebGPU/WebGL2 egui pass.
- [ ] **Runtime mod management (`cs-server` console):** Test `mod_list`, `mod_reload custom_hud`, and `mod_unload player_stats` while connected without restarting the server.
