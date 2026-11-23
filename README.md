# cs16-rs

**Counter-Strike 1.6** rewrite in pure Rust (`#![forbid(unsafe_code)]`, Rust edition 2024), with the server/game logic ported from ReHLDS / ReGameDLL_CS and the Protocol 48 client/renderer ported from Xash3D FWGS and `cs16-client` events (`THIRD-PARTY-NOTICES.md`), featuring GoldSrc BSP/MDL/SPR rendering, VGUI2 menu/console/options (`TrackerScheme.res` + `BackgroundLayout.txt`), developer console (`~`), `CVar`/`config.cfg` engine, `pm_shared` movement physics, ReGameDLL-calibrated `KickBack` gunplay across **all 28 CS 1.6 weapons**, `EF_BONEMERGE` third-person weapons, hostage rescue & bomb defusal modes, offline bots, and 100 Hz UDP multiplayer with LAN server discovery. For the licensing of all four reference projects, see [`docs/LICENSING.md`](docs/LICENSING.md).

Assets are mounted **read-only** directly from an existing Steam Half-Life / Counter-Strike 1.6 installation (`cstrike`, `valve`, `platform`, optional `cstrike_hd`); zero Valve assets are stored in this repository.

Copyright (C) 2026 cs16-rs contributors. Licensed under GPL-3.0-or-later; see LICENSE. See [`docs/LICENSING.md`](docs/LICENSING.md) for the licensing decision and reference-project attribution.

## Workspace Architecture (16 Crates)

- **`cs-vfs`** — Multi-layer case-insensitive GoldSrc VFS (`cstrike_downloads` → optional `cstrike_hd` → `cstrike` → `valve` → `platform`) with automatic Steam `libraryfolders.vdf` discovery.
- **`cs-cvar`** — Quake/GoldSrc command & CVar engine (`bind`, `unbind`, `alias`, `exec`, `toggle`, `cvarlist`, `cmdlist`, `find`, `map`, `connect`, `buy`, `give`, `god`, `noclip`, `sv_restart`, `bot_add`/`_t`/`_ct`, `bot_kick`, `say`/`say_team`, `radio1..3`, `nightvision`, `impulse 201`, `screenshot`, `Tab` autocomplete, local `config.cfg` persistence).
- **`cs-assets`** — Pure-Rust parsers for `WAD3`/`MipTex`, `BSP v30` (all 15 lumps, PVS RLE, padded 2048×2048 lightmap atlas, `func_ladder`, `func_buyzone`, `func_bomb_target`, `func_hostage_rescue`, `func_breakable`, `func_door`, `hostage_entity`), `StudioModel v10` (`IDST` bones, RLE animations, clamped death frames, true `EF_BONEMERGE`, companion `*T.mdl`, hitboxes), `Sprite v2` (`IDSP`), `sound/materials.txt`, `TrackerScheme.res`, `ClientScheme.res`, UTF-16LE `cstrike_english.txt`, `classes/*.res`, `maps/*.txt`, `BackgroundLayout.txt`, `sprites/hud.txt`, and skyboxes.
- **`cs-sim`** — Deterministic 100 Hz game simulation:
  - GoldSrc BSP hull tracing (`Point`/`Standing`/`Large`/`Crouched`) + anatomical 6-hitbox player raycast + wallbang material penetration + shattering `func_breakable` glass/vents + interactive `func_door`s.
  - Full `pm_shared` physics (counter-strafing, air-strafe/bunnyhop `wishspd` cap 30, 18u stairs, duck-jump, ladders, fall damage, surface footsteps).
  - Older prototype `pmove` physics module remains for legacy `cs-sim` unit tests; shared `pm_shared` movement now lives in **`cs-pm`**.
  - **All 28 CS 1.6 weapons + C4 + NightVision**: Knife, Glock-18 (burst), USP (silencer), P228, Deagle, Five-SeveN, Dual Elites, M3 & XM1014 (interruptible shell-by-shell reload), MAC-10, TMP, MP5, UMP-45, P90, Galil, FAMAS (burst), AK-47, M4A1 (silencer), SG552, AUG, Scout, AWP, G3SG1, SG550, M249, HE/Flash/Smoke grenades, C4 — with ReGameDLL `KickBack` recoil & `PM_DropPunchAngle` spring recovery.
  - Both **Bomb/Defuse (`de_*`)** and **Hostage Rescue (`cs_*`)** modes, persistent round corpses, economy (`$800..$16000` + loss bonus), automatic BSP `NavGraph` generator + Bot AI.
- **`cs-pm`** — Shared GoldSrc `pm_shared` movement physics and BSP/box hull collision (`PM_Move`, `PM_RecursiveHullCheck`, surface materials) ported from ReGameDLL_CS & ReHLDS behind the `PmWorld` trait so `cs-game` and `cs-clientcore` prediction run identical movement.
- **`cs-proto48`** — `#![forbid(unsafe_code)]` implementation of Valve's **GoldSrc Protocol 48** (Counter-Strike 1.6 / Half-Life 1 network wire protocol), ported deterministically from `ReHLDS`, `ReGameDLL_CS`, and `Xash3D FWGS`.
- **`cs-platform`** — Platform seams shared by native and `wasm32` builds: the `Clock` time source (`NativeClock`, `ManualClock`).
- **`cs-transport`** — Datagram transports behind one `PacketTransport` trait: non-blocking UDP and an in-process loopback hub with injectable loss, duplication and reordering.
- **`cs-game`** — The game logic of CS 1.6 (the `mp.dll` equivalent) behind the engine's `GameDll` trait, ported from ReGameDLL_CS (vanilla `mp.dll` rules): team and class menus, spawning, freeze periods, timers, win conditions, economy/loser bonus, scoreboard/kill messages, chat, radio, radar/status bar, spectator observer, buy zones, map cycles, **all 28 weapons/grenades**, **C4 bomb planting/defusing/drop/pickup & defuser kits (`de_*`)**, **classic hostages + `CLocalNav` (`cs_*`)**, **VIP assassination & escape (`as_*` / `es_*`)**, and **engine-hosted fake-client bots** (`bot_add*`/`bot_kick`/`bot_quota`) with BSP `NavGraph` combat & objective play. Still unported: tactical shield, `autobuy`/`rebuy`, interactive Phase 7 map entities (`func_door`/`func_breakable`/`func_button`/`ambient_generic`), and full ZBot/`.nav` AI (Phase 12). Player movement runs through `cs-pm` (`cs_pm::pm`) in `SV_RunCmd` order.
- **`cs-engine`** — Protocol 48 server engine, the equivalent of the original dedicated server: connectionless queries, connect and sign-on, per-client delta frames with PVS culling, user commands, fake clients (`CreateFakeClient` / `RunPlayerMove`), timeouts and map changes, with the game behind a `GameDll` trait.
- **`cs-probe`** — Headless Protocol 48 test client (library and `cs-probe` binary): connects, signs on, walks and records/diffs captures, used by the engine's tests and against live HLDS.
- **`cs-clientcore`** — The platform-free Protocol 48 client: connection and sign-on, precache tables, entity frames and interpolation, movement prediction on `cs-pm`, events, temp entities, sound queue and HUD state from the server's messages.
- **`cs-local`** — Local play: `cs-engine` and `cs-game` in the client's process, reached by a `cs-clientcore` connection over loopback; also the shared start-up helpers of the Protocol 48 server.
- **`cs-server`** — Standalone headless 100 Hz dedicated UDP server: `cs-engine` with `cs-game` (`--walk-game`: the older walking-only `WalkGame`) for the original CS 1.6 client and for `cs-client`. `--proto48` is still accepted and does nothing.
- **`xtask`** — Repository tasks (`cargo xtask parity`: the parity ledger report in `docs/parity/`).
- **`cs-client`** — `winit` + `wgpu 30` shell on `cs-clientcore`: every entity is drawn by `modelindex` (BSP world, brush entities with their transform, skinned studio models with the held weapon bone-merged, sprites, temp entities, beams), the first-person view model animated from the server's weapon animations, predicted camera, `rodio` audio from the core's sound queue, the sprite HUD from the server's usermsgs (health, armor, money, ammo, timer, status icons, kill feed, chat, radar), the scoreboard, the server's text menus answered with `menuselect`, the MOTD, and the VGUI2 main menu, console and options. `connect <addr>`, `disconnect` and `map <name>` (a local game with `bot_quota` bots) are console commands.

## Quick Start

### Play Offline (5v5 with Bots)

```powershell
cargo run --release -p cs-client
```

Jump straight into `de_dust2` or `cs_office`:

```powershell
cargo run --release -p cs-client -- --map de_dust2 --exec "net_graph 1"
cargo run --release -p cs-client -- --map cs_office
```

### Dedicated Server + Multiplayer (with LAN Discovery & Original CS 1.6 Compatibility)

Terminal 1 (start headless 100 Hz Protocol 48 server):

```powershell
cargo run --release -p cs-server -- --map de_dust2 --port 27015
```

Terminal 2 (connect from `cs-client` or click **Find Servers → LAN → Refresh LAN**, or connect from the original Counter-Strike 1.6 client console with `connect 127.0.0.1:27015`):

```powershell
cargo run --release -p cs-client -- --exec "connect 127.0.0.1:27015"
```

Optional flags: `--port <n>` (default 27015), `--maxplayers <n>` (default 16), `--bots <n>` (sets initial `bot_quota`), `--hostname <name>`, `--gamedir <path>`, `--walk-game` (run the older walking-only `WalkGame` instead of `cs-game`); `--proto48` is still accepted for compatibility and does nothing (the server is always Protocol 48).

Since phases 2C–2E `cs-server` runs `cs-game`, ported from ReGameDLL_CS (vanilla `mp.dll` behaviour; the 2C game as a whole has not been compared with the original client, only its join and message streams and the checks listed below): team/class menus (VGUI and text), spawns, freeze periods, timers, money rewards and loser bonus, scoreboard and kill feed, chat, radio, radar, crosshair status bar, spectator observer, buy zones, map cycle (`changelevel`), all 24 buyable guns + knife + 3 grenades, hit-group/hitbox combat and client weapon prediction (`cl_lw 1`), and map objectives:

- **Bomb defusal (`de_*`)**: `func_bomb_target` / `info_bomb_target`, `GiveC4` rotation, carrying/dropping/picking up the C4 (`CWeaponBox::BombThink`), planting/cancelling (`BarTime`), accelerating beeps & `TE_GLOWSPRITE` LED blinks, detonation (`TE_SPRITE`, `RadiusDamage2`, firing zone targets), and CT defusal (`10 s` / `5 s` with bought or dropped `item_thighpack`).
- **Hostage rescue (`cs_*`)**: `func_hostage_rescue` / `info_hostage_rescue`, classic `CHostage` + `CLocalNav` `+use` following/rescue/injury penalties/`HostageK`, round restarts and `mp_hostagepenalty`.
- **VIP & Escape (`as_*` / `es_*`)**: `func_vip_safetyzone` / `info_vip_start`, `become_vip` / `jointeam 3` queue, `vip` model/200 armor/half armor ratio/`as_` buy restrictions, `func_escapezone` ratio and 3-round `SwapAllPlayers`, plus `info_map_parameters` (`buying`/`bombradius`).
- **Engine-backed bots**: ReHLDS `CreateFakeClient` / `RunPlayerMove` bots driven by `cs_sim::bots`'s directed `NavGraph` (`bot_add`, `bot_add_t`, `bot_add_ct`, `bot_kick`, `bot_quota`), buying loadout aliases, fighting in LOS, planting/defusing C4 and rescuing hostages.

Since the `pm_shared` port the movement of both `cs-game` and `cs-clientcore` prediction uses the shared `cs-pm` code (`PM_Move` with engine/world traces, movevars, and surface materials in `cs-engine`'s `pm` and `texture` modules): run, jump, duck, stairs, ladders, water, drowning, spectators and footsteps follow ReGameDLL_CS and ReHLDS, and Phase 3 headless runs against stock HLDS verified movement prediction on `de_dust2` and `cs_office` (see `docs/parity/checklists/phase3-clientcore.md` and `docs/parity/checklists/pm-movement.md`).

Phase 2E status, honestly: the objectives are ported from the sources and covered by the automated suite, but the original client has not played a full round of `de_dust2` or `cs_office` on this server yet (the owner's Part B in the 2E checklist is open). Against the live original server only three join scenarios were diffed (`cs_office` hostage baselines and `HostagePos`, `de_dust2` `GiveC4` and `drop weapon_c4`, `as_oilrig` VIP join); planting, beeps, detonation and defusal, hostage following, rescue and the penalty, the VIP and escape rounds, and the bots playing them are tested against the source only. Not ported: `SVC_DIRECTOR` (HLTV) messages and the log lines of the objectives, the scripted `weapon_c4`, the observer's C4/defuser icons and `SetProgressBarTime2`, the Condition Zero hostage AI, and ZBot behaviour beyond the minimal `NavGraph` bots (post-plant guarding is phase 12).

Live `hlds.exe` (build 10210) `cs-probe --diff` captures verified `cs_office` (`hostage_entity` baselines `#59..#62`, `HostagePos`, `StatusIcon`, `ItemStatus`), `de_dust2` (`GiveC4`, `ScoreAttrib` bomb bit, `drop weapon_c4` & 1 Hz `BombDrop`), and `as_oilrig` (`info_vip_start` spawn, `ScoreAttrib` VIP bit, VIP join audio/HUD). Manual checklists for the original client live in [`docs/parity/checklists/phase2c-game.md`](docs/parity/checklists/phase2c-game.md), [`docs/parity/checklists/phase2d-weapons.md`](docs/parity/checklists/phase2d-weapons.md), and [`docs/parity/checklists/phase2e-objectives.md`](docs/parity/checklists/phase2e-objectives.md). Ledger totals (`cargo xtask parity`, 2026-11-24): **5.1% verified**, **39.1% tested**, **49.4% implemented**. Remaining gaps: tactical shield, `autobuy`/`rebuy`, `game_player_equip`, interactive Phase 7 map entities (doors/buttons/lifts/trains/breakables), full ZBot `.nav` AI (Phase 12), and HLDS update-rate frame batching (`~24 Hz` vs our `~55–60 Hz`).

Since phase 2B the world is real: players collide with each other, brush entities (`func_wall`, `func_breakable` and the like) are solid, and entities from the map's entity lump are linked into the world with traces against them, touches, the movement types (toss, bounce, fly, step, push) and pushers, ported from ReHLDS (not yet compared with the original server). Client commands are lag compensated and sounds travel through the PAS.

The server console (type in the terminal where the server runs) accepts these commands:

- `status` - print the hostname, the map and one line per connected client (1-based slot, state, name, address/BOT, ping).
- `kick #<userid> [reason]` or `kick <slot> [reason]` - disconnect a client; the reason is shown to the client (a name that matches exactly one client also works).
- `bot_add`, `bot_add_t`, `bot_add_ct`, `bot_kick [t|ct]`, `bot_quota <n>` - add, kick, or maintain fake-client bots.
- `say <text>` - print `Console: <text>` to every client.
- `changelevel <map>` or `map <map>` - change the map; connected clients reload it automatically.
- `quit` or `exit` - shut the server down, telling the clients.
- `<cvar>` or `<cvar> <value>` - read or set a cvar.

Ctrl+C (and SIGTERM/SIGHUP on unix) shuts the server down gracefully: connected clients are told and disconnect at once instead of timing out.

Compatibility with the original client and HLDS is verified in part (connect, MOTD, intro camera, menus, CT spawn, walking, `de_dust2` C4 join+drop, `cs_office` hostages & HUD, `as_oilrig` VIP spawn: Part A of [`docs/parity/checklists/phase2d-weapons.md`](docs/parity/checklists/phase2d-weapons.md) and [`docs/parity/checklists/phase2e-objectives.md`](docs/parity/checklists/phase2e-objectives.md)); earlier manual checklists are [`docs/parity/checklists/phase1-original-client.md`](docs/parity/checklists/phase1-original-client.md), [`docs/parity/checklists/phase2a-console.md`](docs/parity/checklists/phase2a-console.md), [`docs/parity/checklists/phase2b-world.md`](docs/parity/checklists/phase2b-world.md), [`docs/parity/checklists/phase2c-game.md`](docs/parity/checklists/phase2c-game.md), and [`docs/parity/checklists/pm-movement.md`](docs/parity/checklists/pm-movement.md). The headless probe connects the same way: `cargo run -p cs-probe -- --connect 127.0.0.1:27015 --seconds 20 --walk`.

## Phase 3 status: our own client

`cs-client` now runs on `cs-clientcore` for both offline play (a local server over loopback) and `connect <addr>`. Verified headless on 2026-11-24 against the original HLDS (build 10210) on `de_dust2` and `cs_office`: sign-on, delta entity updates (0 parse errors), the join through the text menus, movement prediction (worst error 0.42 and 0.13 units, no teleports), the HUD values (health, money, weapon, ammo) and the events, temp entities and sounds of another player's shots (numbers: Part A of [`docs/parity/checklists/phase3-clientcore.md`](docs/parity/checklists/phase3-clientcore.md); rerun with `CS16_HLDS_ADDR=127.0.0.1:27016 cargo test -p cs-clientcore --test hlds -- --ignored --nocapture`).

**Not verified visually**: how the picture, the HUD, the sound, the menus, buying, the scoreboard and a map change look and feel in our window compared with the original client (Part B of the same checklist, not done yet). Your own shots' events come from the server only (the client does not predict weapons until phase 6).

## Controls & Shortcuts

| Key | Action |
|---|---|
| `WASD` / `Space` / `Ctrl` / `Shift` | Move / Jump / Crouch / Walk |
| `Mouse1` / `Mouse2` / `R` | Fire / Zoom, Silencer, Burst-Fire or Heavy Knife / Reload |
| `1`–`5` / `Q` / `G` | Weapon Slots / Last Weapon (`lastinv`) / Drop Weapon |
| `B` | Server Text Buy Menu (`1`–`9`/`0`, `buyequip`) |
| `,` / `.` | Buy Primary / Secondary Ammo |
| `E` | Defuse C4 / Open Door / Lead Hostage (`cs_*`) |
| `N` / `T` | Toggle NightVision Goggles / Spray Lambda Logo |
| `Z` / `X` / `C` | Classic Radio Command Menus (`1`–`9`) |
| `TAB` / `M` / `Y` / `U` | Scoreboard / Team & Class Select / Say / Team Say |
| `Mouse1` / `Mouse2` (when dead) | Cycle Spectated Teammate / Toggle Free-Look Spectator |
| `` ` `` (or `Ё`) / `Esc` / `F5` | Developer Console / Pause Menu / GPU PNG Screenshot |

## Verification

The toolchain is pinned to Rust 1.100.0 by `rust-toolchain.toml`; `rustup` installs it on the first `cargo` run; if it does not, run `rustup toolchain install`.

```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --workspace
```
