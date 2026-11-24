# Phase 6 — Movement & Weapon Parity (`phase6-movement-weapons`)

Executed from `main` (`7dae526`, 2026-11-24) per `docs/superpowers/specs/2026-11-20-cs16-rs-master-plan-design.md` (§6 Phase 6, §7 tolerances).

## Scope
1. **Movement Parity (`cs-pm`, `cs-engine`, `cs-game`)**:
   - `PM_AirMove` / `PM_AirAccelerate` air-strafe (`wishspd` 30 cap) & multi-tick edge-friction / waterjump tests.
   - `FuncPushable`, `FuncConveyor`, `FuncPlat`, `PathCorner`, and `FuncTrain` movers in `cs-game` with `MOVETYPE_PUSH` / `MOVETYPE_PUSHSTEP` & `FL_CONVEYOR`.
2. **Weapon & Equipment Completeness (`cs-game`)**:
   - Tactical Shield (`weapon_shield`, `$2200` CT buy, `is_hitting_shield` frontal/hitbox deflection, drop/pickup, shield variants for pistols/knife/grenades, crosshair hide).
   - `autobuy`, `cl_autobuy`, `cl_setautobuy`, `rebuy`, `cl_rebuy`, `cl_setrebuy` + `game_player_equip`.
   - Bullet impact effects & grenade/flashbang curve coverage.
3. **Client-Side Weapon Prediction (`cs-clientcore`)**:
   - Deterministic `cl_lw 1` firing, recoil/kickback, spread seed parity, scope zoom, silencer/burst toggles, reload, dryfire sounds, and shield draw/undraw in `weapons_predict.rs`.
4. **Differential Verification & Gate**:
   - 65 s differential movement trace (`cs-clientcore` vs `cs-game` within `0.03125` u / `0.1` u/s, plus live `hlds.exe` build 10210 differential run).
   - 28-weapon differential verification suite + `docs/parity/movement.toml`, `docs/parity/weapons.toml`, `docs/parity/checklists/phase6-movement-weapons.md`, `README.md`.
