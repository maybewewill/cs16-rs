# Phase 12 — Official CS 1.6 ZBot & Navigation Mesh Checklist

Automated unit and integration verification for all **23** bot parity items in [`docs/parity/bots.toml`](../bots.toml) and all **30** `bot_*` CVars in [`docs/parity/cvars.toml`](../cvars.toml).

## Part A — Automated Verification

| Check | Scope | Evidence |
|---|---|---|
| `.nav` v1–v5 binary parse, v5 encode, Z-interpolation & validation | `cs-assets` | [`nav.rs`](../../../crates/cs-assets/src/nav.rs) (`parses_legacy_versions_1_through_4`, `roundtrip_v5_nav_file`, `z_interpolation_and_closest_point_match_quad`, `rejects_malformed_and_oversized_inputs`) |
| BSP navigation mesh generation, crouch/ladder links, place names & hiding spots | `cs-bot` | [`nav_gen.rs`](../../../crates/cs-bot/src/nav_gen.rs) (`generates_merged_areas_places_and_crouch_duct`, `generates_and_links_ladder_between_floors`), [`nav_mesh.rs`](../../../crates/cs-bot/src/nav_mesh.rs) (`danger_decay_and_nearby_propagation`, `hiding_sniper_and_ambush_queries`, `spatial_grid_and_place_lookup`) |
| `.nav` VFS load, `bot_nav_generate`, editing (`mark`/`split`/`crouch`/`place_set`/`check_consistency`) & `bot_nav_save` | `cs-game` | [`phase12_bots.rs::nav_vfs_load_generate_edit_and_save_roundtrip`](../../../crates/cs-game/tests/phase12_bots.rs) |
| Weighted A* pathfinding, danger/obstacle avoidance, ladders & string pulling | `cs-bot` | [`path.rs`](../../../crates/cs-bot/src/path.rs) (`astar_avoids_danger_and_obstacles_and_tags_metadata`, `astar_ladder_and_string_pull`) |
| `BotProfile.db` template inheritance, builtin roster fallback & `bot_add_t <name>` | `cs-bot`, `cs-game` | [`profile.rs`](../../../crates/cs-bot/src/profile.rs) (`parses_multi_level_template_inheritance_and_overrides`, `parses_builtin_roster_for_all_difficulties`, `errors_on_unknown_and_cyclic_templates`, `select_profile_avoids_duplicate_names_when_available`), [`phase12_bots.rs::bot_profile_db_vfs_load_and_builtin_fallback`](../../../crates/cs-game/tests/phase12_bots.rs) |
| Economy buy decisions (save/eco/force/full), armor/defuser & weapon restrictions (`bot_allow_*`, presets) | `cs-bot`, `cs-game`, `cs-cvar` | [`buy.rs`](../../../crates/cs-bot/src/buy.rs) (5 unit tests), [`bots.rs::bots_buy_at_round_start`](../../../crates/cs-game/tests/bots.rs), [`phase12_bots.rs::weapon_restrictions_presets_and_buy`](../../../crates/cs-game/tests/phase12_bots.rs) |
| Combat engagement ranges, sniper zoom, reaction delay, recoil, cover reload & grenade throw | `cs-bot`, `cs-game` | [`combat.rs`](../../../crates/cs-bot/src/combat.rs) (4 unit tests), [`bots.rs::bots_move_and_shoot_an_enemy`](../../../crates/cs-game/tests/bots.rs) |
| Perception (FOV/smoke/flash/sound), cover/ambush, radio chatter (`bot_chatter`, `followme`/`holdpos`/`report`) & `bot_stop` | `cs-bot`, `cs-game` | [`tactics.rs`](../../../crates/cs-bot/src/tactics.rs) (`perception_fov_smoke_flash_and_sound_events`, `cover_hide_ambush_and_radio_coordination`), [`phase12_bots.rs::radio_chatter_and_human_coordination`](../../../crates/cs-game/tests/phase12_bots.rs) |
| Objective tactics: bomb plant/guard/defuse, hostage guard/rescue & VIP escort/escape | `cs-bot`, `cs-game` | [`tactics.rs`](../../../crates/cs-bot/src/tactics.rs) (`bomb_fetch_plant_guard_escape_and_ct_defuse`, `hostage_guard_and_rescue_states`, `vip_escort_and_assassination_states`), [`full_round.rs`](../../../crates/cs-game/tests/full_round.rs) (3 tests), [`phase12_bots.rs::{bomb_plant_guard_and_defuse_synthetic_arena, vip_escort_and_safety_zone_escape}`](../../../crates/cs-game/tests/phase12_bots.rs) |
| 5v5 `bot_quota 10` multi-round match, `bot_join_after_player`, `bot_kill`, `bot_kick` | `cs-game` | [`phase12_bots.rs::{match_5v5_quota_10_multi_round_and_commands, bot_join_after_player_waits_for_human}`](../../../crates/cs-game/tests/phase12_bots.rs), [`bots.rs`](../../../crates/cs-game/tests/bots.rs) (7 tests) |

## Part B — Manual Live Gameplay & Visual Inspection (Owner)

- [ ] Launch `cs-server --map de_dust2 --bots 9` (with retail Steam `cstrike` mounted) and connect with `cs-client` or retail `hl.exe` to observe full 5v5 `de_dust2.nav` navigation, radio place callouts, and bomb site tactics.
- [ ] Toggle `bot_nav_edit 1` / `bot_show_nav 1` in-game to inspect `.nav` overlays and interactive mesh editing (`bot_nav_mark`, `bot_nav_split`, `bot_nav_save`).
