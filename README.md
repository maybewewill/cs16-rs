# cs16-rs

**Counter-Strike 1.6** rewrite in pure Rust, with the protocol layer ported from the MIT-licensed ReHLDS / ReGameDLL_CS (`#![forbid(unsafe_code)]`, Rust edition 2024) preserving the authentic GoldSrc visual style, **1-to-1 Valve VGUI2 interface** (`ClientScheme.res` + `TrackerScheme.res` + `BackgroundPanel.res`), developer console (`~`), `CVar`/`config.cfg` engine, `pm_shared` movement physics, ReGameDLL-calibrated `KickBack` gunplay across **all 28 CS 1.6 weapons**, `EF_BONEMERGE` third-person weapons, hostage rescue & bomb defusal modes, offline bots, and 100 Hz UDP multiplayer with LAN server discovery.

Assets are mounted **read-only** directly from an existing Steam Half-Life / Counter-Strike 1.6 installation (`cstrike`, `valve`, `platform`, optional `cstrike_hd`); zero Valve assets are stored in this repository.

Licensing: MIT; see [`docs/LICENSING.md`](docs/LICENSING.md) for the licensing decision and reference-project attribution.

## Workspace Architecture (8 Crates)

- **`cs-vfs`** — Multi-layer case-insensitive GoldSrc VFS (`cstrike_downloads` → optional `cstrike_hd` → `cstrike` → `valve` → `platform`) with automatic Steam `libraryfolders.vdf` discovery.
- **`cs-cvar`** — Quake/GoldSrc command & CVar engine (`bind`, `unbind`, `alias`, `exec`, `toggle`, `cvarlist`, `cmdlist`, `find`, `map`, `connect`, `buy`, `give`, `god`, `noclip`, `sv_restart`, `bot_add`/`_t`/`_ct`, `bot_kick`, `say`/`say_team`, `radio1..3`, `nightvision`, `impulse 201`, `screenshot`, `Tab` autocomplete, local `config.cfg` persistence).
- **`cs-assets`** — Pure-Rust parsers for `WAD3`/`MipTex`, `BSP v30` (all 15 lumps, PVS RLE, padded 2048×2048 lightmap atlas, `func_ladder`, `func_buyzone`, `func_bomb_target`, `func_hostage_rescue`, `func_breakable`, `func_door`, `hostage_entity`), `StudioModel v10` (`IDST` bones, RLE animations, clamped death frames, true `EF_BONEMERGE`, companion `*T.mdl`, hitboxes), `Sprite v2` (`IDSP`), `sound/materials.txt`, `TrackerScheme.res`, `ClientScheme.res`, UTF-16LE `cstrike_english.txt`, `classes/*.res`, `maps/*.txt`, `BackgroundLayout.txt`, `sprites/hud.txt`, and skyboxes.
- **`cs-sim`** — Deterministic 100 Hz game simulation:
  - GoldSrc BSP hull tracing (`Point`/`Standing`/`Large`/`Crouched`) + anatomical 6-hitbox player raycast + wallbang material penetration + shattering `func_breakable` glass/vents + interactive `func_door`s.
  - Full `pm_shared` physics (counter-strafing, air-strafe/bunnyhop `wishspd` cap 30, 18u stairs, duck-jump, ladders, fall damage, surface footsteps).
  - **All 28 CS 1.6 weapons + C4 + NightVision**: Knife, Glock-18 (burst), USP (silencer), P228, Deagle, Five-SeveN, Dual Elites, M3 & XM1014 (interruptible shell-by-shell reload), MAC-10, TMP, MP5, UMP-45, P90, Galil, FAMAS (burst), AK-47, M4A1 (silencer), SG552, AUG, Scout, AWP, G3SG1, SG550, M249, HE/Flash/Smoke grenades, C4 — with ReGameDLL `KickBack` recoil & `PM_DropPunchAngle` spring recovery.
  - Both **Bomb/Defuse (`de_*`)** and **Hostage Rescue (`cs_*`)** modes, persistent round corpses, economy (`$800..$16000` + loss bonus), automatic BSP `NavGraph` generator + Bot AI.
- **`cs-net`** — `postcard`-over-UDP client/server protocol, `WorldSnapshot` replication, client-side `pm_shared` prediction, 250 ms lag-compensation ring buffer, and UDP LAN server discovery (`LanPing` / `LanInfo`).
- **`cs-proto48`** — `#![forbid(unsafe_code)]` implementation of Valve's **GoldSrc Protocol 48** (Counter-Strike 1.6 / Half-Life 1 network wire protocol), ported deterministically from `ReHLDS` and `ReGameDLL_CS`.
- **`cs-server`** — Standalone headless 100 Hz dedicated UDP server.
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

```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --workspace
```
