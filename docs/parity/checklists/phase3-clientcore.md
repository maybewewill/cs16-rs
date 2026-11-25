# Checklist: our client on cs-clientcore (phase 3)

Two parts. Part A records what was compared against the original HLDS (Steam, build 10210) by running our client core headless on 2026-11-24 (`verified` registry items cite these rows). Part B is the owner's visual check with our own window (`cs-client`); no row of Part B has been done.

## Part A: headless run against the original HLDS (2026-11-24)

Method: `crates/cs-clientcore/tests/hlds.rs` (ignored by default, gated by `CS16_HLDS_ADDR`). Two `cs_clientcore::Connection` over `cs_transport::UdpTransport`, with the map's collision from `cs-pm` loaded, join CT through the server's text menus (`_vgui_menus 0`, `menuselect 2`, `menuselect 1`), wait 8 s for the server's round restart, then the first client ("player") walks a scripted 30 s (forward, strafe left and right, jumps, ducks, a turn of 45 degrees a second) and fires the USP 12 times; the second ("observer", same team, standing still) is there because the server does not send a shooter its own weapon events (`FEV_NOTHOST`, the client predicts them: phase 6). No window was opened and no game client was started. The server: `hlds.exe -console -game cstrike -port 27016 +sv_lan 1 +mp_consistency 0 +maxplayers 8 +mp_freezetime 0 +map <map>` (build 10210), restarted for each run, stopped by its own PID.

```powershell
$env:CS16_GAMEDIR = 'D:\SteamLibrary\steamapps\common\Half-Life'
$env:CS16_HLDS_ADDR = '127.0.0.1:27016'; $env:CS16_HLDS_MAP = 'de_dust2'
cargo test -p cs-clientcore --test hlds -- --ignored --nocapture
```

| # | Check | de_dust2 | cs_office |
|---|-------|----------|-----------|
| A1 | Sign-on (`new`, `sendres`, `spawn`, `sendents`) reaches `InGame` | yes, 1.0 s after the start | yes, 1.25 s |
| A2 | Parse errors (player and observer) | 0 and 0 | 0 and 0 |
| A3 | Datagrams in (about 44 s), full updates, delta updates | 927, 1, 907 | 931, 1, 908 |
| A4 | Entity updates per second (HLDS sends about 22; our own server about 61) | 22 | 22 |
| A5 | Most entities visible after interpolation (player / observer) | 20 / 9 | 56 / 60 |
| A6 | Prediction checks of the walk (client origin replayed against the server's `clientdata` origin at the acknowledged command) | 733 | 733 |
| A7 | Prediction error: worst, mean, checks over 0.1 unit, teleports (error over 64) | 0.416, 0.0090, 20, 0 | 0.126, 0.0063, 9, 0 |
| A8 | Movement happened (the walk covered x, y, z) | x -101..858, y 2000..2512, z -110..17 | x -1808..-1024, y -1536..-1262, z -318..-255 |
| A9 | HUD after joining: health, money, weapon | 100, 800, USP (id 16) | 100, 800, USP (id 16) |
| A10 | HUD after 12 shots: clip, ammo (type 6) | 0, 24 | 0, 24 |
| A11 | Scoreboard rows from `ScoreInfo` | 2 | 2 |
| A12 | Events fired on the observer for the player's 12 shots | 12 (`events/usp.sc`) | 12 (`events/usp.sc`) |
| A13 | Temp entities, peak live (shells, smoke, impacts) on the observer | 9 | 9 |
| A14 | Sound commands on the observer (12 of them `weapons/usp_unsil-1.wav`; footsteps `player/pl_step1..4.wav`; map ambients) | 63 | 51 (with `ambience/tankidle2.wav`, `ambience/wind2.wav`) |
| A15 | Events on the shooter's own client for its own shots | 0 (the server skips the host: expected) | 0 |

Notes:

- The checks over 0.1 unit stay below the 0.25 unit smoothing threshold, except one of 0.416 on de_dust2; the stretches of about 0.12 are in the jump and duck-jump phases (a client origin is a 1/128 fixed-point number and the velocity of `clientdata` a 1/8 one on the wire, the original client has the same). `teleports` counts only the walk: the spawn teleport after the class choice is reported separately (1 per client) and is not part of A7.
- Before the walk started, a first version of the script began as soon as health read 100 and caught the server's round restart as a 1535-unit "teleport"; that was the script, not the client, and is why the walk now waits 8 s after both joined.
- Our own `cs-server` and stock `hlds.exe` (build 10210) side-by-side matrix measured on 2026-11-25 via [`crates/cs-clientcore/tests/hlds.rs`](file:///c:/Users/slash/cs16-rs/crates/cs-clientcore/tests/hlds.rs):
  - `cs-server` (`de_dust2`, `CS16_HLDS_UPDATERATE=20`, 20 Hz): 874 datagrams, 0 parse errors, 681 walk checks, worst `2.8972`, mean `0.0185`, over `0.1`: `22`, `0` teleports; 12 `events/usp.sc`, 9 peak tempents.
  - `cs-server` (`de_dust2`, 60 Hz): 2557 datagrams, 0 parse errors, 1776 walk checks, worst `2.1328`, mean `0.0126`, over `0.1`: `63`, `0` teleports; 12 `events/usp.sc`, 9 peak tempents.
  - `cs-server` (`cs_office`, `CS16_HLDS_UPDATERATE=20`, 20 Hz): 883 datagrams, 0 parse errors, 682 walk checks, worst `1.7687`, mean `0.0096`, over `0.1`: `12`, `0` teleports; 12 `events/usp.sc`, 9 peak tempents.
  - `cs-server` (`cs_office`, 60 Hz): 2569 datagrams, 0 parse errors, 1811 walk checks, worst `2.1013`, mean `0.0071`, over `0.1`: `30`, `0` teleports; 12 `events/usp.sc`, 9 peak tempents.
  - Stock `hlds.exe` (`de_dust2`, default 22–25 Hz): 1063 datagrams, 0 parse errors, 844 walk checks, worst `3.2590`, mean `0.0252`, over `0.1`: `28`, `0` teleports; 12 `events/usp.sc`, 9 peak tempents.
  - Stock `hlds.exe` (`cs_office`, `+sv_maxupdaterate 102`, 60 Hz, 2026-11-24/25): matches the exact same `~2.84–2.95` ledge spike at `(-1600.000, -1520, -261.258)` and up to `8.328` against player bboxes.
  - **Root cause verified (`DELTA_WriteField` -> `PM_HullPointContents` -> `PM_CheckStuck`):** `clientdata_t.origin` `1/128` truncation toward zero maps negative-facing axial plane positions (e.g. `-1600.003` -> `-1600.000` on `cs_office` `*129 func_wall`, `-1528.003` -> `-1528.000` against `observer` bbox) onto `d == 0.0`, which `PM_HullPointContents` (`d >= 0.0 -> children[0]`, ReHLDS `pmovetst.cpp:125`) treats as `CONTENTS_SOLID`. `PM_CheckStuck` (`pm_shared.cpp` `!server` branch) nudges brush collisions by `-0.125` (causing single-frame ledge catches on both `cs-server` and stock `hlds.exe`) or hits the `PM_CHECKSTUCK_MINTIME` (`50 ms`) cooldown on player bbox contact. Deterministic 65 s `cs-pm` differential trace (`crates/cs-clientcore/tests/differential_movement.rs`) stays at `worst <= 0.0625`, `p99 <= 0.03125`, `0` teleports.

Not verified against the HLDS by this run: the buy menu, the scoreboard drawing, a map change, damage and death, weapons other than the USP, grenades, the bomb, hostages, a second team.

## Part B: the owner's checks (our window, `cs-client`)

👤 Run `cargo run --release -p cs-client` and start the original dedicated server (or ours) for the remote rows. Fill in Date and Result. Nothing below has been done yet.

| # | Check | Expected | Date | Result |
|---|-------|----------|------|--------|
| B1 | Rendering: `de_dust2` and `cs_office` (offline, `map <name>` in the console): world, lightmaps, sky, brush entities (doors, plats), props, sprites | Looks as in the original client at the same place; no missing textures or black faces; compare screenshots | not done | not done |
| B2 | Players and weapons: bots with their weapons, the view model animations (draw, shoot, reload), shell ejection, muzzle flash | Held weapon in the bot's hand; the view model plays the server's animation numbers | not done | not done |
| B3 | HUD: health, armor, money, ammo, timer, status icons, kill feed, chat, centre text, radar | Same numbers and positions as the original HUD; the money flashes when it changes | not done | not done |
| B4 | Sound: shots, footsteps, reload, ambients, the 3D position of a bot's footsteps | Heard where they are; no clicking, no sound stuck on | not done | not done |
| B5 | Menus: team and class text menus answered with the number keys (`_vgui_menus 0`), the main menu, the console | The team menu appears after connecting; `2` then `1` joins CT; `~` opens the console | not done | not done |
| B6 | Buy: the buy text menu (`B`) and buying a rifle and armor | Money drops by the price; the weapon is in hand | not done | not done |
| B7 | Scoreboard (`Tab`): names, score, deaths, latency, bomb and VIP flags | As in the original, sorted by score | not done | not done |
| B8 | Walking: no jitter while walking, strafing, jumping and ducking against the original HLDS (`connect 127.0.0.1:27016`) | The view does not shake; no rubber-banding | not done | not done |
| B9 | Map change: `changelevel cs_office` on the server while connected | The client reloads the map, the world and the models change, the HUD stays | not done | not done |
| B10 | Shooting: the USP against a wall and a bot | Decal and smoke at the impact, the sound of the shot, the bot's blood | not done | not done |
