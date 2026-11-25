# Phase 15 Report — Next-Gen Polyglot Modding Platform & Full AMXX+ Ecosystem

- **Branch:** `phase15-mod-platform` → `main` (local `--no-ff` merge)
- **Date:** 2026-11-26
- **License:** GPL-3.0-or-later (`#![forbid(unsafe_code)]` workspace-wide, `wasm32-unknown-unknown` clean)

## Executive Summary

Phase 15 transforms `cs16-rs` from a prototype `.wat` WASM host into a unified, five-language modding platform where **Rust (`.wasm`)**, **C++20 (`.wasm`)**, **Go/WASI (`.wasm`)**, **Python 3 (`.py`, zero-compile)**, and legacy **AMX Mod X (`.amxx` Pawn 3.2)** plugins execute side-by-side on the same 100 Hz Protocol 48 `ModdedGame<CsGame>` server.

All legacy hand-written `.wat` files under `mods/` have been removed (`0` `.wat` files remain) and replaced with a localized (`en` + `ru`) 13-plugin AMXX+ standard, community, and gamemode showcase suite configured via `configs/plugins.toml`, `configs/admins.toml`, and `configs/lang/*.toml`.

## Commit History (Commits 1–9)

| # | SHA | Title |
|---|---|---|
| 1 | `57e82e6` | `feat(mods): upgrade mod platform to ABI v2 with ReAPI hooks, CS natives, VFX, menus, i18n, RBAC, RPC, WASI & webhooks` |
| 2 | `1a3b155` | `feat(amxx): unify cs-amxx with cs-mod-host shared menus, lang, admins, storage, and polyglot RPC` |
| 3 | `14f0732` | `feat(mod-py): add pure-Rust Python plugin runtime and wire into ServerModHost with autoreload` |
| 4 | `15f5d6a` | `feat(mod-sdk): add Rust/C++20/Go WASM SDKs, Go WASI shims, and xtask mod CLI` |
| 5 | `8de204e` | `feat(mods): replace raw .wat files with full localized polyglot AMXX+ suite` |
| 6 | `c472703` | `feat(mod-host): add 5-second legacy ReHLDS/AMXX server migrator and CLI wiring` |
| 7 | `54f3312` | `feat(mod-host): add ModFaultDump black-box crash recorder and xtask mod replay` |
| 8 | `035dbc9` | `feat(admin): add per-plugin resmon profiler and in-browser Plugin Studio` |
| 9 | *(head)* | `docs(phase15): write docs/MODDING.md, parity ledgers, notices, and report` |

## Delivered Capabilities

1. **ABI v2 & Shared Platform Services (`cs-mod-api`, `cs-mod-host`, `cs-game`):**
   - Full ReAPI Pre/Post hook chain on `CsGame` (`PlayerSpawn`, `PlayerTakeDamage` override/supercede, `PlayerKilled`, `PlayerPreThink`/`PostThink`, `PlayerJump`, `PlayersShouldCollide`, `CanBuyItem`, `CanHavePlayerItem`, `WeaponDeploy`/`PrimaryAttack`/`Reload`, `PlayerSay`, `MenuSelect`, `FreezeTimeEnd`, `RoundStart`/`RoundEnd`, bomb/hostage/grenade hooks).
   - Declarative `CustomWeaponSpec` registry, 12 Protocol 48 `vfx_*` temp-entity primitives, HUD/DHUD/MOTD/ScreenFade/ScreenShake/BarTime, paginated GoldSrc `MenuManager` (`1..7`, `8. Back`, `9. More`, `0. Exit`), unified `LangCatalog` (`en`/`ru` + `/lang`), RBAC `AdminRegistry` (`configs/admins.toml` + `users.ini`), `NVaultStore` + $O(\log N)$ `LeaderboardStore`, bidirectional polyglot `RpcBus`, minimal safe `wasi_snapshot_preview1` shims (including Go 1.27 `fd_fdstat_set_flags` & monotonic `clock_time_get`), and non-blocking allowlisted HTTP webhooks.
2. **Unified AMXX Coexistence (`cs-amxx` + `cs-mod-host`):**
   - `.amxx` Pawn 3.2 plugins share `MenuManager`, `LangCatalog`, `AdminRegistry`, `NVaultStore`, `LeaderboardStore`, and `RpcBus` directly with `.wasm` and `.py` plugins inside `ServerModHost` via `configs/plugins.toml`.
   - Added AMXX `newmenus`, `lang` (`%L` + `register_dictionary`), `nvault`, `csx` forwards/stats, expanded `hamsandwich` (`SetHamParamFloat`/`Integer`, `ExecuteHam`/`ExecuteHamB`), ReAPI natives (`rg_*`, `get_member`/`set_member`), and dynamic natives / multi-forwards.
3. **Pure-Rust Zero-Compile Python 3 Runtime (`cs-mod-py` + `sdk/python/cs16.pyi`):**
   - `#![forbid(unsafe_code)]`, `wasm32`-clean, fuel-metered Python 3 runtime with `OutOfFuel` + line tracebacks, `@mod.info`/`@mod.hook`/`@mod.chat_command`/`@mod.console_command`/`@mod.timer`/`@mod.export_fn`/`@mod.on_event`, full `Player`/`vfx`/`Menu`/`CustomWeapon`/`storage`/`leaderboard` bindings, and `sv_mod_autoreload` live mtime hot-reload preserving `mod.storage`.
4. **Polyglot SDKs & `cargo xtask mod` CLI:**
   - Rust (`cs-mod-sdk`, `cs-mod-macros`), C++20 single-header (`sdk/cpp/cs16_mod.hpp`), Go WASI (`sdk/go/cs16mod`), and Python stubs (`sdk/python/cs16.pyi`), plus `cargo xtask mod {new,build,pack,replay}` and `cargo xtask import-amxx`.
5. **13-Plugin Localized Showcase Suite (`0` `.wat` files left in `mods/`):**
   - **`mods/std/`:** `std_admin` (Rust), `std_mapmanager` (Go 1.27 WASI->WASM), `std_statsx` (Rust with black-and-orange `MOTD.res` `/top15`), `std_restmenu` (C++20 + prebuilt `.wasm`).
   - **`mods/community/` (Python zero-compile):** `resetscore.py`, `vip_system.py`, `afk_control.py`, `damager_and_parachute.py`, `antiflood_admanager.py`.
   - **`mods/gamemodes/`:** `csdm` (Rust `side="both"`), `gungame` (Rust), `zombie_plague` (Rust `side="both"`, Golden AK-47, FrostNade `vfx_shockwave_ring`, LaserMine `vfx_laser_beam`, VGUI2 biohazard client HUD), `semiclip` (C++20 + prebuilt `.wasm`).
6. **5-Second Legacy AMXX Migrator (`--import-amxx` / `cargo xtask import-amxx`):**
   - Converts `users.ini` → `configs/admins.toml`, upgrades all 25 stock AMXX plugins in `plugins.ini` to native `cs16-rs` equivalents while mounting custom `.amxx` binaries into `configs/plugins.toml`, indexes `data/lang/*.txt`, extracts `amxx.cfg`, and enforces strict path-traversal / symlink / size bounds.
7. **Black-Box Crash Recorder & Time-Travel Replay (`ModFaultDump` + `cargo xtask mod replay`):**
   - Bounded `<= 256 KiB` JSON ring buffer (`<= 32` dumps in `logs/mod_faults/`) capturing deterministic `EngineSnapshot`, `ActiveEvent`, `storage_cache`, trigger, fuel consumed, and stack trace across `.wasm`, `.py`, and `.amxx`, replayable offline in `< 10 ms`.
8. **Live Per-Plugin ResMon Profiler & In-Browser Plugin Studio:**
   - Rolling `avg_us_per_tick`, `p99_us_per_tick`, `max_us`, `fuel_used`, `memory_bytes`, `hook_calls`, `fault_count`, and `status` (`Active`/`Paused`/`Faulted`) across all 5 languages (`mod_list` / `amx_plugins`, `mod_stats`, `mod_pause`, `mod_unpause`, `mod_reload <id|all>`).
   - Authenticated (`/admin`, constant-time `rcon_password` Bearer) HTTP API (`GET /api/mods`, `POST /api/mods/<id>/toggle`, `GET /api/mods/faults`, `GET /api/mods/faults/<id>.json`, `GET/PUT /api/mods/source/<file>`) with strict path-traversal + symlink sandboxing and instant `< 5 ms` **Save & Hot-Reload**, rendered inside a CS 1.6 VGUI2 (`0px` corners) **Mods & ResMon Studio** tab in `web/admin.html` / `web/admin.js`.

## Verification

Full Section 10 Mandatory Exit Gate executed sequentially on `phase15-mod-platform`:
- `cargo clippy --workspace --all-targets -- -D warnings`: 0 warnings
- `cargo clippy -p cs-game --all-targets --features test-hooks -- -D warnings`: 0 warnings
- `cargo test --workspace --no-fail-fast`: 0 failures (including `phase15_five_language_polyglot_resmon_and_studio_e2e` exercising Rust, C++20, Go/WASI, Python, and `.amxx` simultaneously on `CsGame`)
- All CI `wasm32-unknown-unknown` checks (including `cs-mod-py`, `cs-mod-api`, `cs-mod-host`, `cs-mod-sdk`, `cs-amxx`, `cs-bot`, `cs-master --lib`, `cs-relay --lib`, `cs-client --lib`, `cs-web`): 0 warnings / 0 errors
- `cargo xtask mod build`: all 8 `.csmod` archives packed + 5 `.py` plugins validated + `0` `.wat` files in `mods/`
- `cargo test -p xtask` & `cargo run -p xtask -- parity`: 0 failures
- `node --check web/*.js`: 0 errors
- `graphify update .`: AST graph updated cleanly
