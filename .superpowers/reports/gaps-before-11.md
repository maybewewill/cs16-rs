# Pre-Phase-11 Parity Gaps Closure Report (`gaps-before-11`)

Date: `2026-11-25`
Branch: `gaps-before-11` -> local `--no-ff` merge into `main`

## Summary of Gaps Closed (Gaps 1–7 + Bonus `wasm32` Cleanliness)

### Gap 1 — Jump / Duck-Jump Prediction Root Cause & 5-Way Live UDP Matrix (`cs-clientcore` / `cs-server`)
- Removed temporary `[PRED_DBG]` / `[STUCK_DBG]` logging from `crates/cs-clientcore/src/predict.rs` and updated `crates/cs-clientcore/tests/hlds.rs` (`CS16_HLDS_UPDATERATE`, root-cause note, `limit = 10.0`).
- Deterministic check: `cargo test -p cs-clientcore --test differential_movement` -> `2 passed; 0 failed` (`worst <= 0.0625`, `0` teleports).
- Live UDP 5-way server matrix (`crates/cs-clientcore/tests/hlds.rs`) -> `5 / 5 passed` (`parse_errors: 0`, `teleports: 0`, `12/12 events/usp.sc`, peak `9` tempents):
  - `cs-server de_dust2` (20 Hz): `874` dgrams, `681` checks, `worst 2.8972`, `mean 0.0185`, `> 0.1`: `22`
  - `cs-server de_dust2` (60 Hz): `2557` dgrams, `1776` checks, `worst 2.1328`, `mean 0.0126`, `> 0.1`: `63`
  - `cs-server cs_office` (20 Hz): `883` dgrams, `682` checks, `worst 1.7687`, `mean 0.0096`, `> 0.1`: `12`
  - `cs-server cs_office` (60 Hz): `2569` dgrams, `1811` checks, `worst 2.1013`, `mean 0.0071`, `> 0.1`: `30`
  - Retail `hlds.exe` build 10210 `de_dust2` (22–25 Hz): `1063` dgrams, `844` checks, `worst 3.2590`, `mean 0.0252`, `> 0.1`: `28`
- Root cause proven & documented in [phase3-clientcore.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/phase3-clientcore.md): `DELTA_WriteField` `1/128` truncation toward zero maps negative-facing axial plane positions (`-1600.000`, `-1528.000`) onto `d == 0.0`, where `PM_HullPointContents` (`d >= 0.0 -> children[0]`, ReHLDS `pmovetst.cpp:125`) returns `CONTENTS_SOLID`, triggering `PM_CheckStuck` (`-0.125` brush nudge or `50 ms` `PM_CHECKSTUCK_MINTIME` cooldown on player bbox contact) identically on both `cs-server` and retail `hlds.exe`.

### Gap 2 — WebTransport Honesty (`cs-web`)
- Clarified in `README.md:37`, `web/transport.js:1-3`, and `docs/parity/checklists/phase4-web.md:23` that browser transport runs over the WebSocket `/relay` bridge and WebTransport remains unimplemented (`docs/parity/web.toml` stays `status = "missing"`).

### Gap 3 — Genuine GoldSrc `r_turbsin` Water Surface Warp (`cs-client`)
- Ported Xash3D FWGS `engine/warpsin.h` 256-entry `R_TURBSIN` + `EmitWaterPolys` (`ref/gl/gl_rsurf.c`) into `crates/cs-client/src/render/bsp_renderer.rs` and `crates/cs-client/src/render/shaders/bsp.wgsl` (`#[allow(clippy::excessive_precision)]` preserving exact upstream table literals).
- Verified `r_turbsin_water_warp_matches_xash_warpsin` passes.

### Gap 4 — Phase 4 / 8 / 9 / 2E Verification & Sound Events Map
- **Phase 4 (Web):** Verified `cargo build --target wasm32-unknown-unknown -p cs-web` (`0` warnings), `node --check` on all 6 JS files (`web/*.js`), and live `/relay` HTTP/WS smoke test (`GET /` `200 OK`, unallowlisted WS `/relay?target=127.0.0.1:9999` `403`, allowlisted WS `/relay?target=127.0.0.1:27125` `101 Switching Protocols`); updated [phase4-web.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/phase4-web.md).
- **Phase 8 (Render):** Captured & inspected `1280x800` Vulkan headless screenshots (`target/render-parity/{de_dust2,de_aztec,cs_office}.png`) and split Part A vs Part B in [phase8-render.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/phase8-render.md).
- **Phase 2E (Objectives):** Re-verified `full_round` (3/3), `vip_escape` (10/10), `zones` (14/14); recorded in [phase2e-objectives.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/phase2e-objectives.md).
- **Phase 9 (Sound):** Created [sound-events.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/sound-events.md) mapping radio, C4, hostage, footstep, weapon/grenade, `env_sound`, occlusion/underwater, `svc_soundfade`, and music events; updated [phase9-sound.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/phase9-sound.md).

### Gap 5 — Omnibus Test Splits & Parity Ledger Update
- Replaced mega-tests with focused tests:
  - `crates/cs-client/tests/phase9_sound.rs`: `16` focused tests (including `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer`).
  - `crates/cs-client/tests/phase8_render.rs`: `27` focused tests covering all 42 `render.toml` items.
  - `crates/cs-game/tests/phase7_env_items.rs` (`12` tests), `phase7_io_triggers.rs` (`11` tests), `phase7_brushes_movers.rs` (`8` tests).
  - `crates/cs-proto48/src/tempent.rs` (`11` tests, 4 disjoint TE category roundtrips + `temp_entities_cover_all_59_active_ids`) & `crates/cs-clientcore/src/tempents.rs` (`27` tests in module, 4 disjoint category outcome tests + `temp_effects_cover_all_59_active_ids`).
- Updated `docs/parity/{sound,render,entities,tempents,usermsg,cvars,wire}.toml` with zero stale test names; `cargo xtask parity` passes cleanly (`TOTAL verified: 6.3% tested: 81.3% implemented: 87.6%`).

### Gap 6 — 25 Stock Maps Live `hlds.exe` Baseline Audit
- Captured & diffed all 25 stock CS 1.6 `.bsp` maps live against retail `hlds.exe` build 10210 via `cs-probe --diff`.
- Identified and fixed missing `de_cbble` `env_beam` (`#19, #24, #45`) baselines (`BeamEntity::spawn` in `crates/cs-game/src/env.rs`: set `eng.set_model(id, &self.texture)` + `FL_CUSTOMENTITY` for continuous `ServerSide()` beams, verified by `phase7_env_items::env_beam_lightning_and_laser` and re-capture `167/167` exact).
- Result: **24 / 25 stock maps** match retail `hlds.exe` on exact baseline edict sets (`diff = 0`). `de_airstrip` has `359` vs `363` (`+4`, `#404..#407`) because retail CS 1.6 `mp.dll` omits Condition Zero's `item_generic` while `cs-game` intentionally spawns its 4 `models/pred_plant.mdl` foliage entities per `entities.toml`.
- Recorded complete 25-map table in [phase7-entities-modes.md](file:///c:/Users/slash/cs16-rs/docs/parity/checklists/phase7-entities-modes.md) and updated `README.md`.

### Gap 7 — Constant Citations & `ArmorType` Fix
- Sourced constant definitions from GPL-3 ReHLDS / ReGameDLL_CS / Xash3D FWGS / FWGS hlsdk-portable and retail Ghidra verification across:
  - `crates/cs-clientcore/src/hud.rs`: Fixed wrong `ArmorType` doc (`0` = vest only, `1` = vest + helmet per Ghidra `FUN_10046f20`); cited `ScoreAttrib` (`regamedll/dlls/cdll_dll.h:63-67`).
  - `crates/cs-clientcore/src/cvars.rs`: Cited `cl_lc` (`engine/client/cl_main.c:3731`) and `cl_filterstuffcmd` (`engine/common/cvar.c:51`).
  - `crates/cs-clientcore/src/entities.rs`: Cited `HISTORY_MAX` (`cl_entity.h:62`), `MOVETYPE_*` (`ReHLDS const.h:95-108`, `Xash3D const.h:83`), `lastmove` (`cl_entity.h:86`).
  - `crates/cs-transport/src/lib.rs`: Cited `MAX_UDP_PACKET = 4010` (`rehlds/engine/net.h:117`).
  - `crates/cs-client/src/render/scene.rs`: Cited `EF_NODRAW = 128` (`rehlds/common/const.h:138`).
  - `crates/cs-client/src/ui/hud.rs`: Cited `ScreenFade` / `FFADE_*` (`regamedll/engine/shake.h:44-57`, `screenfade.h:23`).
  - `crates/cs-pm/src/pm/consts.rs`: Cited `ReHLDS common/const.h` (`FL_*`, `MOVETYPE_*`, `CONTENTS_*`).
  - `crates/cs-game/src/{entity,player}.rs` & `crates/cs-engine/src/events.rs`: Cited `USE_TYPE` (`hlsdk-portable dlls/cbase.h:82-88`), `DMG_SHOWNHUD`/`DMG_TIMEBASED` (`cbase.h:613,629`, `player.cpp:5045,7808,7824`), and `MAX_EVENT_QUEUE` (`sv_main.cpp`).
- Kept genuine clean-room / design assumptions honestly labeled (e.g., `MAX_WORLD_COORD` safety bound in `cs-pm/src/collision.rs`, clean-room tempent physics heuristics, unused `svc_damage` / `svc_spawnstatic` wire fallbacks).
- Bonus `wasm32` fix: silenced unused `bold` warning on `wasm32` in `crates/cs-assets/src/vgui_font.rs:540-564`.

## Rule-7 Full Verification Gate Results

- `cargo test --workspace --no-fail-fast`: **passed** (`0 failed`; `3 ignored` live-HLDS differential tests)
- `cargo clippy --workspace --all-targets -- -D warnings`: **passed** (`0` warnings)
- `cargo clippy -p cs-game --all-targets --features test-hooks -- -D warnings`: **passed** (`0` warnings)
- `cargo check --target wasm32-unknown-unknown` (all 4 CI commands covering 19 crates incl. `cs-relay`, `cs-client`, `cs-web`): **passed** (`0` warnings)
- `cargo xtask mod-build`: **passed** (`deathmatch.csmod`, `gungame.csmod`, `player_stats.csmod`, `zombie.csmod`)
- `cargo test -p xtask`: **passed** (`22 passed; 0 failed`)
- `cargo xtask parity`: **passed** (`TOTAL verified: 6.3% tested: 81.3% implemented: 87.6%`)
- `graphify update .`: **passed** (`15320` nodes, `36126` edges, `719` communities in `graphify-out/`)
