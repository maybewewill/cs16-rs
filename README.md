# cs16-rs

**Counter-Strike 1.6** rewrite in pure Rust, with the protocol layer ported from ReHLDS / ReGameDLL_CS (`#![forbid(unsafe_code)]`, Rust edition 2024) preserving the authentic GoldSrc visual style, **1-to-1 Valve VGUI2 interface** (`ClientScheme.res` + `TrackerScheme.res` + `BackgroundPanel.res`), developer console (`~`), `CVar`/`config.cfg` engine, `pm_shared` movement physics, ReGameDLL-calibrated `KickBack` gunplay across **all 28 CS 1.6 weapons**, `EF_BONEMERGE` third-person weapons, hostage rescue & bomb defusal modes, offline bots, and 100 Hz UDP multiplayer with LAN server discovery. For the licensing of ReHLDS and ReGameDLL_CS, see [`docs/LICENSING.md`](docs/LICENSING.md).

Assets are mounted **read-only** directly from an existing Steam Half-Life / Counter-Strike 1.6 installation (`cstrike`, `valve`, `platform`, optional `cstrike_hd`); zero Valve assets are stored in this repository.

Copyright (C) 2026 cs16-rs contributors. Licensed under GPL-3.0-or-later; see LICENSE. See [`docs/LICENSING.md`](docs/LICENSING.md) for the licensing decision and reference-project attribution.

## Workspace Architecture (13 Crates)

- **`cs-vfs`** — Multi-layer case-insensitive GoldSrc VFS (`cstrike_downloads` → optional `cstrike_hd` → `cstrike` → `valve` → `platform`) with automatic Steam `libraryfolders.vdf` discovery.
- **`cs-cvar`** — Quake/GoldSrc command & CVar engine (`bind`, `unbind`, `alias`, `exec`, `toggle`, `cvarlist`, `cmdlist`, `find`, `map`, `connect`, `buy`, `give`, `god`, `noclip`, `sv_restart`, `bot_add`/`_t`/`_ct`, `bot_kick`, `say`/`say_team`, `radio1..3`, `nightvision`, `impulse 201`, `screenshot`, `Tab` autocomplete, local `config.cfg` persistence).
- **`cs-assets`** — Pure-Rust parsers for `WAD3`/`MipTex`, `BSP v30` (all 15 lumps, PVS RLE, padded 2048×2048 lightmap atlas, `func_ladder`, `func_buyzone`, `func_bomb_target`, `func_hostage_rescue`, `func_breakable`, `func_door`, `hostage_entity`), `StudioModel v10` (`IDST` bones, RLE animations, clamped death frames, true `EF_BONEMERGE`, companion `*T.mdl`, hitboxes), `Sprite v2` (`IDSP`), `sound/materials.txt`, `TrackerScheme.res`, `ClientScheme.res`, UTF-16LE `cstrike_english.txt`, `classes/*.res`, `maps/*.txt`, `BackgroundLayout.txt`, `sprites/hud.txt`, and skyboxes.
- **`cs-sim`** — Deterministic 100 Hz game simulation:
  - GoldSrc BSP hull tracing (`Point`/`Standing`/`Large`/`Crouched`) + anatomical 6-hitbox player raycast + wallbang material penetration + shattering `func_breakable` glass/vents + interactive `func_door`s.
  - Full `pm_shared` physics (counter-strafing, air-strafe/bunnyhop `wishspd` cap 30, 18u stairs, duck-jump, ladders, fall damage, surface footsteps).
  - `pm` module: the original `pm_shared` movement code of ReGameDLL_CS (vanilla branch), ported function by function behind a `PmWorld` trait, so the server and a client's prediction can share it. The older `pmove` above stays for the legacy offline game and `cs-net`.
  - **All 28 CS 1.6 weapons + C4 + NightVision**: Knife, Glock-18 (burst), USP (silencer), P228, Deagle, Five-SeveN, Dual Elites, M3 & XM1014 (interruptible shell-by-shell reload), MAC-10, TMP, MP5, UMP-45, P90, Galil, FAMAS (burst), AK-47, M4A1 (silencer), SG552, AUG, Scout, AWP, G3SG1, SG550, M249, HE/Flash/Smoke grenades, C4 — with ReGameDLL `KickBack` recoil & `PM_DropPunchAngle` spring recovery.
  - Both **Bomb/Defuse (`de_*`)** and **Hostage Rescue (`cs_*`)** modes, persistent round corpses, economy (`$800..$16000` + loss bonus), automatic BSP `NavGraph` generator + Bot AI.
- **`cs-net`** — `postcard`-over-UDP client/server protocol, `WorldSnapshot` replication, client-side `pm_shared` prediction, 250 ms lag-compensation ring buffer, and UDP LAN server discovery (`LanPing` / `LanInfo`).
- **`cs-proto48`** — `#![forbid(unsafe_code)]` implementation of Valve's **GoldSrc Protocol 48** (Counter-Strike 1.6 / Half-Life 1 network wire protocol), ported deterministically from `ReHLDS` and `ReGameDLL_CS`.
- **`cs-platform`** — Platform seams shared by native and `wasm32` builds: the `Clock` time source (`NativeClock`, `ManualClock`).
- **`cs-transport`** — Datagram transports behind one `PacketTransport` trait: non-blocking UDP and an in-process loopback hub with injectable loss, duplication and reordering.
- **`cs-game`** — The game logic of CS 1.6 (the `mp.dll` equivalent) behind the engine's `GameDll` trait, ported from ReGameDLL_CS (vanilla `mp.dll` rules): team and class menus, spawning, freeze periods, timers, win conditions, economy/loser bonus, scoreboard/kill messages, chat, radio, radar/status bar, spectator observer, buy zones, map cycles, **all 28 weapons/grenades**, **C4 bomb planting/defusing/drop/pickup & defuser kits (`de_*`)**, **classic hostages + `CLocalNav` (`cs_*`)**, **VIP assassination & escape (`as_*` / `es_*`)**, and **engine-hosted fake-client bots** (`bot_add*`/`bot_kick`/`bot_quota`) with BSP `NavGraph` combat & objective play. Still unported: tactical shield, `autobuy`/`rebuy`, interactive Phase 7 map entities (`func_door`/`func_breakable`/`func_button`/`ambient_generic`), and full ZBot/`.nav` AI (Phase 12). Player movement runs through `cs_sim::pm` in `SV_RunCmd` order.
- **`cs-engine`** — Protocol 48 server engine, the equivalent of the original dedicated server: connectionless queries, connect and sign-on, per-client delta frames with PVS culling, user commands, fake clients (`CreateFakeClient` / `RunPlayerMove`), timeouts and map changes, with the game behind a `GameDll` trait.
- **`cs-probe`** — Headless Protocol 48 test client (library and `cs-probe` binary): connects, signs on, walks and records/diffs captures, used by the engine's tests and against live HLDS.
- **`cs-server`** — Standalone headless 100 Hz dedicated UDP server; with `--proto48` it runs `cs-engine` with `cs-game` (`--walk-game`: the older walking-only `WalkGame`) for the original CS 1.6 client.
- **`xtask`** — Repository tasks (`cargo xtask parity`: the parity ledger report in `docs/parity/`).
- **`cs-client`** — `winit` + `wgpu 30` (Vulkan/DX12/Metal) multi-pass renderer (BSP overbright ×2 lightmaps, live `gl_texturemode`, dynamic broken/door brush culling, skinned 3D player/hostage/corpse MDLs with `EF_BONEMERGE` `p_*.mdl` in hands, depth-cleared viewmodel with `cl_righthand`, 3D decals/sprays/tracers/grenades/smoke/C4 LED/muzzleflashes, `F5` GPU PNG readback), `rodio` 2D/3D spatial WAV audio + UI clicks + Main Menu MP3 music, **1-to-1 Valve VGUI2 UI** (`ClientScheme.res` Buy Menu with real `gfx/vgui/*.tga` silhouettes & 9-row specs, Team Menu with `maps/*.txt` briefings, Class Menu with 256×196 portraits, translucent rounded `TAB` Scoreboard, Steam 2003 `Options` with live crosshair preview & `icon_steam.tga`), classic `640hud7`/`640hud1` sprite HUD, AWP scope, NightVision overlay, Radio menus, and Spectator letterbox camera.

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

### Dedicated Server + Multiplayer (with LAN Discovery)

Terminal 1 (start headless 100 Hz server):

```powershell
cargo run --release -p cs-server -- --map de_dust2 --port 27015 --bots 9
```

Terminal 2 (connect directly or click **Find Servers → LAN → Refresh LAN**):

```powershell
cargo run --release -p cs-client -- --exec "connect 127.0.0.1:27015"
```

### Protocol 48 Server for the Original CS 1.6 Client

```powershell
cargo run -p cs-server -- --proto48 --map de_dust2
```

Optional flags: `--port <n>` (default 27015), `--maxplayers <n>` (default 16), `--bots <n>` (sets initial `bot_quota` in `--proto48` too), `--hostname <name>`, `--gamedir <path>`, `--walk-game` (run the older walking-only `WalkGame` instead of `cs-game`). Then, in the original Counter-Strike 1.6 client, open the console (`~`) and type `connect 127.0.0.1:27015`.

Since phases 2C–2E this mode runs `cs-game`, ported from ReGameDLL_CS (vanilla `mp.dll` behaviour; the 2C game as a whole has not been compared with the original client, only its join and message streams and the checks listed below): team/class menus (VGUI and text), spawns, freeze periods, timers, money rewards and loser bonus, scoreboard and kill feed, chat, radio, radar, crosshair status bar, spectator observer, buy zones, map cycle (`changelevel`), all 24 buyable guns + knife + 3 grenades, hit-group/hitbox combat and client weapon prediction (`cl_lw 1`), and map objectives:

- **Bomb defusal (`de_*`)**: `func_bomb_target` / `info_bomb_target`, `GiveC4` rotation, carrying/dropping/picking up the C4 (`CWeaponBox::BombThink`), planting/cancelling (`BarTime`), accelerating beeps & `TE_GLOWSPRITE` LED blinks, detonation (`TE_SPRITE`, `RadiusDamage2`, firing zone targets), and CT defusal (`10 s` / `5 s` with bought or dropped `item_thighpack`).
- **Hostage rescue (`cs_*`)**: `func_hostage_rescue` / `info_hostage_rescue`, classic `CHostage` + `CLocalNav` `+use` following/rescue/injury penalties/`HostageK`, round restarts and `mp_hostagepenalty`.
- **VIP & Escape (`as_*` / `es_*`)**: `func_vip_safetyzone` / `info_vip_start`, `become_vip` / `jointeam 3` queue, `vip` model/200 armor/half armor ratio/`as_` buy restrictions, `func_escapezone` ratio and 3-round `SwapAllPlayers`, plus `info_map_parameters` (`buying`/`bombradius`).
- **Engine-backed bots**: ReHLDS `CreateFakeClient` / `RunPlayerMove` bots driven by `cs_sim::bots`'s directed `NavGraph` (`bot_add`, `bot_add_t`, `bot_add_ct`, `bot_kick`, `bot_quota`), buying loadout aliases, fighting in LOS, planting/defusing C4 and rescuing hostages.

Since the pm_shared port the movement of this mode is the original `pm_shared` code (`cs_sim::pm`) with the engine's physents, traces and movevars (`cs-engine`'s `pm` and `texture` modules): run, jump, duck, stairs, ladders, water, drowning, spectators and footsteps follow ReGameDLL_CS and ReHLDS. It is tested against the sources and has **not been compared with the original client yet**; the manual checks are in [`docs/parity/checklists/pm-movement.md`](docs/parity/checklists/pm-movement.md).

Phase 2E status, honestly: the objectives are ported from the sources and covered by the automated suite, but the original client has not played a full round of `de_dust2` or `cs_office` on this server yet (the owner's Part B in the 2E checklist is open). Against the live original server only three join scenarios were diffed (`cs_office` hostage baselines and `HostagePos`, `de_dust2` `GiveC4` and `drop weapon_c4`, `as_oilrig` VIP join); planting, beeps, detonation and defusal, hostage following, rescue and the penalty, the VIP and escape rounds, and the bots playing them are tested against the source only. Not ported: `SVC_DIRECTOR` (HLTV) messages and the log lines of the objectives, the scripted `weapon_c4`, the observer's C4/defuser icons and `SetProgressBarTime2`, the Condition Zero hostage AI, and ZBot behaviour beyond the minimal `NavGraph` bots (post-plant guarding is phase 12).

Live `hlds.exe` (build 10210) `cs-probe --diff` captures verified `cs_office` (`hostage_entity` baselines `#59..#62`, `HostagePos`, `StatusIcon`, `ItemStatus`), `de_dust2` (`GiveC4`, `ScoreAttrib` bomb bit, `drop weapon_c4` & 1 Hz `BombDrop`), and `as_oilrig` (`info_vip_start` spawn, `ScoreAttrib` VIP bit, VIP join audio/HUD). Manual checklists for the original client live in [`docs/parity/checklists/phase2c-game.md`](docs/parity/checklists/phase2c-game.md), [`docs/parity/checklists/phase2d-weapons.md`](docs/parity/checklists/phase2d-weapons.md), and [`docs/parity/checklists/phase2e-objectives.md`](docs/parity/checklists/phase2e-objectives.md). Ledger totals (`cargo xtask parity`): **4.2% verified**, **37.3% tested**, **47.8% implemented**. Remaining gaps: tactical shield, `autobuy`/`rebuy`, `game_player_equip`, interactive Phase 7 map entities (doors/buttons/lifts/trains/breakables), full ZBot `.nav` AI (Phase 12), and HLDS update-rate frame batching (`~24 Hz` vs our `~55–60 Hz`).

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

## Controls & Shortcuts

| Key | Action |
|---|---|
| `WASD` / `Space` / `Ctrl` / `Shift` | Move / Jump / Crouch / Walk |
| `Mouse1` / `Mouse2` / `R` | Fire / Zoom, Silencer, Burst-Fire or Heavy Knife / Reload |
| `1`–`5` / `Q` / `G` | Weapon Slots / Last Weapon (`lastinv`) / Drop Weapon |
| `B` (`B-4-2`, `B-4-3`, `B-5-1`, `B-8-2`) | 1-to-1 VGUI2 Buy Menu + Quick-Buy Hotkeys + `F1` Autobuy |
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
