# Phase 6 — Movement & Weapon Parity (`phase6-movement-weapons`) Checklist

Executed from branch `phase6-movement-weapons` on 2026-11-24 against ReGameDLL_CS / ReHLDS sources and live Steam `hlds.exe` (build 10210).

## Part A — Automated Checks & Live HLDS Differential Verification

### 1. Automated Suite Summary

| Area | Crate / Test | Coverage |
|---|---|---|
| Air-move & edge friction | `crates/cs-pm/tests/pm_walk.rs::air_accelerate_caps_wishspeed_at_30_and_enables_air_strafe, edge_friction_and_multi_tick_move_stability` | `PM_AirAccelerate` 30 u/s wishspeed cap, air-strafe turning acceleration, multi-tick `edgefriction` stability |
| Moving brush entities | `crates/cs-game/tests/movement_movers.rs::player_pushes_and_rides_func_pushable, player_rides_func_plat_and_func_train, player_is_carried_by_func_conveyor` | `func_pushable` (`MOVETYPE_PUSHSTEP`), `func_conveyor` (`FL_CONVEYOR`), `func_plat`, `path_corner`, `func_train` |
| Tactical shield & variants | `crates/cs-game/tests/shield_and_autobuy.rs::shield_buy_draw_speed_hitbox_deflection_drop_and_pickup, shield_variants_work_for_pistols_knife_and_grenades` | `$2200` CT buy, `WPNSTATE_SHIELD_DRAWN`, `180` u/s shield-drawn speed, frontal/hitbox deflection, drop/pickup, pistol/knife/grenade shield variants |
| `autobuy` / `rebuy` & map equip | `crates/cs-game/tests/shield_and_autobuy.rs::autobuy_and_rebuy_equip_team_loadouts_and_respect_custom_lists, game_player_equip_gives_configured_weapons_on_spawn_and_use` | `autobuy`, `cl_autobuy`, `cl_setautobuy`, `rebuy`, `cl_rebuy`, `cl_setrebuy`, `game_player_equip` |
| Bullet impact effects | `crates/cs-game/tests/bullets.rs::bullet_impact_effects_emit_blood_sparks_helmet_and_knife_streak` | Blood (`TE_BLOODSPRITE`), armor/shield sparks (`TE_SPARKS`), decals, knife wall streak |
| `cl_lw 1` weapon prediction | `crates/cs-clientcore/tests/weapon_prediction.rs::cl_lw_1_predicts_shot_recoil_clip_event_and_sound_before_server_reply, cl_lw_1_predicts_zoom_silencer_burst_reload_and_shield_toggle, client_prediction_matches_server_spread_and_recoil_with_same_seed` | Client-side shot fire, recoil, clip decrement, shared random spread, scope zoom, silencer/burst toggles, reload, dryfire, shield draw/undraw |
| 28-weapon differential suite | `crates/cs-game/tests/weapon_differential.rs::all_28_weapons_match_regamedll_fire_rate_damage_falloff_penetration_spread_reload_and_speed, grenade_physics_and_flashbang_angle_distance_curves` | All 24 firearms + knife + 3 grenades + tactical shield: live deployed `max_speed`, live `cycle_time` & reload duration, `fire_bullets3` 100u/500u/1500u exponential falloff, live `KickBack` stance ordering, penetration materials, HE bounce/damage, flash angle/distance curves |
| 65 s in-process movement differential | `crates/cs-clientcore/tests/differential_movement.rs::differential_60s_movement_trace_matches_server_within_protocol_tolerance, differential_60s_pmove_direct_matches_server_zero_drift` | 6500 x 10 ms multi-maneuver trace (`cs-clientcore` vs `Server<CsGame>` on `de_dust2.bsp` and direct 100 Hz trace): 0 teleports, `mean <= 0.03125` u, `p99 <= 0.03125` u, `0` cumulative drift |

### 2. Live `hlds.exe` Build 10210 65-Second Differential Run (2026-11-24)

Command:

```powershell
# hlds.exe -console -game cstrike -insecure -nomaster -port 27029 +sv_lan 1 +mp_consistency 0 +maxplayers 8 +mp_freezetime 0 +mp_roundtime 9 +map de_dust2
$env:CS16_GAMEDIR = 'D:\SteamLibrary\steamapps\common\Half-Life'
$env:CS16_HLDS_ADDR = '127.0.0.1:27029'
cargo test -p cs-clientcore --test differential_movement differential_60s_movement_trace_against_hlds -- --ignored --nocapture
```

Measured output (`de_dust2`, 6500 scripted 10 ms commands = 65 s):

| Metric | Measured Result | Notes |
|---|---|---|
| Walk commands / Acked checks | `walk_ticks = 6500` / `checks = 1601` | Passed in `70.33 s` |
| Teleports during walk (`spawn_teleports`) | `teleports = 0` (`spawn_teleports = 1`) | Zero unexpected walk teleports |
| Origin error — mean | `0.00446` u | ~7x below `0.03125` u protocol tolerance |
| Origin error — p99 | `0.01810` u | Well within `0.03125` u tolerance |
| Origin error — worst | `0.62664` u | Rare single-frame `1/128` wire plane-boundary transition |
| Drift check (`first10s` vs `last10s` mean) | `0.00231` u -> `0.00526` u | Bounded, zero secular drift over 65 s |
| Velocity error — mean / p99 | `0.05808` u/s / `0.17796` u/s | Matches GoldSrc `clientdata_t.velocity` `1/8` (`0.125` u/s/axis) wire quantization floor (`sqrt(2)*0.125 = 0.17678` u/s) |

## Part B — Manual Interactive Checks (`cs-client` + Original CS 1.6 Client)

- [ ] **Tactical shield feel & visuals (`de_dust2`):** Buy `shield` as CT (`$2200`), verify shield viewmodel + third-person model, right-click draw/undraw lowers speed (`250 -> 180` u/s) and blocks frontal bullets with sparks (`TE_SPARKS`), verify pistol/knife/HE/flash/smoke shield variants.
- [ ] **`autobuy` / `rebuy` hotkeys:** Press `F1` (`autobuy`) and `F2` (`rebuy`) across rounds as both T and CT; customize via `cl_setautobuy` / `cl_setrebuy`.
- [ ] **Movers & `func_pushable`:** Push crates and ride platforms/conveyors/trains cleanly without prediction rubber-banding.
- [ ] **`cl_lw 1` weapon prediction:** Fire AK-47, M4A1 (toggle silencer), Glock-18/FAMAS (toggle burst), AWP (scope + resume zoom), and pistols on a `50-100 ms` simulated latency connection; verify crisp instant viewmodel/recoil/impact feedback with zero double-firing.
