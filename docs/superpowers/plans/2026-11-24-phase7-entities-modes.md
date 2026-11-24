# Phase 7 — Map Entities & Modes (`phase7-entities-modes`)

Executed from `main` (`0253571`, 2026-11-24) per `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md` (§6 Phase 7: all 25 stock CS 1.6 BSP maps load with functional entities, `docs/parity/entities.toml` closed to `>= 95%`, targeting `160/160 = 100%`).

## Steps
1. **I/O System, Triggers, `trigger_camera` Use, & Voting (`votemap` / `vote`)**: `multi_manager`, `multisource`, `master` gating, `env_global`, `game_counter*`/`game_end`/`game_player_*`/`game_score`/`game_team_*`/`game_text`/`game_zone_player`, `player_weaponstrip`, `trigger_*`, `info_*`, `infodecal`, active `trigger_camera` view/path, `votemap` / `vote`.
2. **Doors, Buttons, Breakables, Utility Brushes, & Track/Vehicle Movers**: `func_door`, `func_water`, `func_door_rotating`, `momentary_door`, `func_button`, `func_rot_button`, `momentary_rot_button`, `button_target`, `func_breakable`, `func_wall_toggle`, `func_friction`, `func_rotating`, `func_pendulum`, `func_grencatch`, `func_weaponcheck`, `func_guntarget`, `func_platrot`, `path_track`, `func_tracktrain`, `func_traincontrols`, `func_vehicle`, `func_vehiclecontrols`.
3. **Environment, Lights, Sound, Ammo & World Items**: `env_*`, `beam`, `gibshooter`, `cycler*`, `bodyque`, `light`/`light_spot`/`light_environment`, `ambient_generic`, `env_sound`, `speaker`, `soundent`, all 10 `ammo_*`, `item_kevlar`/`item_assaultsuit`/`item_generic`, plus `Fog`/`ReceiveW`/`Train`/`HudText`/`ScreenShake`/`ScreenFade` message wiring.
4. **All 25 Stock CS 1.6 Maps Verification Suite**: load, spawn, round-restart, and entity-interaction checks across every stock `.bsp`.
5. **Parity Ledgers (`entities.toml`, `usermsg.toml`), Checklist, README, 8-step CI Gate, & Fast-Forward Merge into `main`**.
