# Phase 15 — Next-Gen Polyglot Modding Platform & Full AMXX+ Ecosystem (`phase15-mod-platform`) Verification Checklist

## Part A — Automated Checks (Workspace CI Gate)

Run sequentially on Windows / CI:

1. `cargo clippy --workspace --all-targets -- -D warnings` — 0 warnings.
2. `cargo clippy -p cs-game --all-targets --features test-hooks -- -D warnings` — 0 warnings.
3. `cargo test --workspace --no-fail-fast` — 0 failures, covering:
   - ABI v2 ReAPI Pre/Post hook chains (`PlayerSpawn`, `PlayerTakeDamage` override/supercede, `PlayerKilled`, `PlayersShouldCollide`, `CanBuyItem`, `CanHavePlayerItem`, `WeaponDeploy`, `WeaponPrimaryAttack`, `WeaponReload`, `PlayerSay`, `MenuSelect`, `RoundStart`, `RoundEnd`, bomb/hostage/grenade hooks) on real `ModdedGame<CsGame>`
   - Declarative `CustomWeaponSpec` + 12 `vfx_*` Protocol 48 primitives + HUD/DHUD/MOTD
   - Shared `MenuManager` pagination, simultaneous `ru`+`en` `LangCatalog` (`configs/lang/*.toml` + `data/lang/*.txt` + `/lang`), `AdminRegistry` RBAC (`configs/admins.toml` + `users.ini`), `NVaultStore`, $O(\log N)$ `LeaderboardStore`, bidirectional polyglot `RpcBus`, WASI preview1 shims, and non-blocking allowlisted webhooks
   - Pure-Rust Python 3 runtime (`cs-mod-py`) with fuel cutoff (`OutOfFuel` + Python traceback) and `sv_mod_autoreload`
   - Rust / C++20 / Go / Python SDKs & `cargo xtask mod {new,build,pack,replay}`
   - Full localized 13-plugin showcase suite (`mods/std/`, `mods/community/`, `mods/gamemodes/`) with **zero `.wat` files** remaining under `mods/`
   - 5-second legacy AMXX migrator (`--import-amxx` / `cargo xtask import-amxx`) with strict path-traversal & symlink hygiene
   - Black-box `ModFaultDump` capture (`.wasm`, `.py`, `.amxx`) and offline `< 10 ms` replay (`cargo xtask mod replay`)
   - Per-plugin ResMon profiler (`mod_list` / `amx_plugins`, `mod_stats`, `mod_pause`, `mod_unpause`, `mod_reload`) and `/api/mods*` Plugin Studio live hot-reload + path-traversal sandbox
   - Simultaneous 5-language E2E test (`phase15_five_language_polyglot_resmon_and_studio_e2e`) running Rust `.wasm`, C++20 `.wasm`, Go/WASI `.wasm`, Python `.py`, and `.amxx` Pawn 3.2 on one `ModdedGame<CsGame>` server
4. All CI `wasm32-unknown-unknown` `cargo check` commands (including `cs-mod-py`, `cs-mod-api`, `cs-mod-host`, `cs-mod-sdk`, `cs-amxx`, `cs-bot`, `cs-master --lib`, `cs-relay --lib`, `cs-client --lib`, `cs-web`) — 0 warnings / 0 errors.
5. `cargo xtask mod build` — packs all showcase `.csmod` archives cleanly into `dist/mods/` with 0 `.wat` files under `mods/`.
6. `cargo test -p xtask` and `cargo run -p xtask -- parity` — 0 failures.
7. `node --check web/*.js` — 0 syntax errors.
8. `graphify update .` — AST graph updated cleanly.

## Part B — Manual Live Server & Browser Studio Verification

Prerequisite: start `cs-server --map de_dust2 --port 27015 --plugins-toml configs/plugins.toml --admin` with `rcon_password` configured.

- [ ] **Original CS 1.6 (`hl.exe`) & `cs-client` simultaneous bilingual playtest:** Connect two clients (`en` and `/lang ru`), test `/rs`, `/vip`, `/top15` (authentic black-and-orange `MOTD.res` HTML table), `rtv`, `nominate`, `amxmodmenu` (`1..7` / `8. Back` / `9. More` / `0. Exit`), and weapon restrictions (`std_restmenu`).
- [ ] **Overhaul gamemodes (`csdm`, `gungame`, `zombie_plague`, `semiclip`):** Enable in `configs/plugins.toml`, verify `Golden AK-47`, FrostNade cyan `vfx_shockwave_ring`, LaserMine `vfx_laser_beam`, VGUI2 client HUD badges (`0px` corners), and smooth teammate `semiclip`.
- [ ] **Web Admin Mods & ResMon Studio (`/admin`):** Open **Mods & ResMon Studio** tab, verify live color-coded CPU bars (`< 50 us` green / `< 250 us` yellow / `>= 250 us` red), test 1-click Pause/Resume/Reload, edit `mods/community/resetscore.py` in-browser and verify `< 5 ms` **Save & Hot-Reload**, and download a fault `.json` dump for `cargo xtask mod replay`.
- [ ] **Legacy AMX Mod X Migration:** Run `cs-server --import-amxx <cstrike/addons/amxmodx> --out-dir <tmp>` against a real AMXX server folder and verify `admins.toml`, `plugins.toml`, and `configs/lang/amxx/` boot cleanly.
