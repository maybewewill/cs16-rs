# Parity ledger

`weights.toml` gives each category a weight; the weights sum to 100 across all 15 categories (`wire`, `usermsg`, `delta`, `tempents`, `entities`, `weapons`, `movement`, `cvars`, `render`, `sound`, `ui`, `engine`, `bots`, `web`, `mods`). Each `<category>.toml` lists `[[item]]` entries with `id`, `status` and an optional `evidence`. Every category file needs a key in `weights.toml`.

## Category scope

- `wire.toml`, `usermsg.toml`, `delta.toml`, `tempents.toml`: Protocol 48 `svc_*`/`clc_*` opcodes, CS user messages, `delta.lst` fields and `TE_*` temporary entities. Opcodes and `TE_*` entries that only skip payload bytes without typed decoding, writing, emitting or client rendering are recorded as `missing`, not `implemented`.
- `entities.toml` (`entities` weight): the game rules and HUD items of `cs-game` (Phase 2C/2E) plus the full CS 1.6 multiplayer/map entity class inventory extracted from the original `mp.dll` exports and all 25 stock `.bsp` maps (excluding `weapon_*` in `weapons.toml` and HL1 single-player-only relics).
- `movement.toml` (`movement` weight): player movement (`cs_sim::pm`, engine `pmove` services, `SV_RunCmd` glue, client prediction and Phase 6 differential checks).
- `weapons.toml` (`weapons` weight): weapons, grenades, bullets, inventory, economy, C4/defuser, shield variants and client/differential prediction checks.
- `cvars.toml` (`cvars` weight): full vanilla CS 1.6 server and client cvar and console command inventory (extracted from vanilla ReHLDS/ReGameDLL sources, stock `.cfg` files and `kb_act.lst`), keeping all `CVarRegistry` defaults covered.
- `engine.toml` (`engine` weight): HLDS server engine services plus Phase 3 (`cs-clientcore`) and Phase 11 (demos, resource download, consistency, RCON, bans, logs, master server, AMXX/HLTV) completeness items.
- `render.toml`, `sound.toml`, `ui.toml` (`render`, `sound`, `ui` weights): GoldSrc rendering, audio and VGUI2 `.res`/HUD inventories covering both legacy `cs-client` prototype code (`implemented`/`tested`) and Phase 3/8/9/10 targets (`missing`).
- `bots.toml`, `web.toml`, `mods.toml` (`bots`, `web`, `mods` weights): engine bot bridge + Phase 12 ZBot/navmesh items, Phase 4 `wasm32`/WebGPU/WebSocket/relay items, and Phase 5 WASM modding ABI/sandbox/auto-download items.

The first `verified` items are the ones compared on 2026-11-22 and 2026-11-23 with the original client and the original HLDS; the evidence of each names the method and the line of `checklists/phase2d-weapons.md` (Part A) or the live client check.

## Statuses

- `missing`: no code for the item (or only a payload byte-skipper/placeholder stub without behaviour).
- `implemented`: real behaviour exists, but no test would fail if its behaviour broke. An item that a test only writes or parses on the way through is still `implemented`.
- `tested`: an existing test would fail if the item's behaviour broke. The test asserts on the item's value, either directly or through an equality assertion on a whole decoded message or struct that contains it. `evidence` names the test.
- `verified`: compared with the original game (Steam CS 1.6, protocol 48). `evidence` is required and must point at a golden-capture test or a dated checklist line.

## Running

From the repository root:

```
cargo xtask parity                  # per-category table and TOTAL verified/tested/implemented
cargo xtask parity --min 25         # exit 1 if verified % is below 25
cargo xtask parity import-names <category> <dump-file>   # add names from a cvarlist/cmdlist dump as missing
```
