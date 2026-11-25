# Phase 13 — Beyond Original (Operations, Web Admin, Replays, XDG, Lobbies & Benchmarks) Checklist

Automated unit, integration, and benchmark verification for Phase 13 deliverables in [`docs/parity/engine.toml`](../engine.toml) and [`docs/parity/web.toml`](../web.toml).

## Part A — Automated Verification

| Check | Scope | Evidence |
|---|---|---|
| Single-binary server & embedded web assets (`--admin`, `--replay-rounds`, `--embed-web`, `GET /servers`) | `cs-server`, `cs-transport`, `Dockerfile` | [`proto48.rs::tests::parses_phase13_admin_replay_and_embed_flags`](../../../crates/cs-server/src/proto48.rs), [`proto48.rs::tests::admin_http_endpoints_replays_stats_rcon_and_servers_work_over_ws_transport`](../../../crates/cs-server/src/proto48.rs) |
| Web Admin HTTP API (`/admin`, `/api/status`, `/api/players`, `/api/stats`, `/api/replays`, `/api/rcon`, `/api/invite`, constant-time Bearer auth) | `cs-server`, `web/admin.html`, `web/admin.js` | [`admin_http.rs::tests::constant_time_compare_and_auth_guards`](../../../crates/cs-server/src/admin_http.rs), [`admin_http.rs::tests::routes_status_players_replays_rcon_and_invite`](../../../crates/cs-server/src/admin_http.rs), [`admin_http.rs::tests::reads_http_post_with_content_length_and_caps_oversized`](../../../crates/cs-server/src/admin_http.rs) |
| Per-round HLDEMO v5 `.dem` replay store & live match stats (`ADR`, `HS%`, assists, objectives, MVPs) | `cs-server`, `cs-engine`, `cs-game` | [`replays.rs::tests::records_valid_hldemo_v5_per_round_and_enforces_ring_buffer`](../../../crates/cs-server/src/replays.rs), [`replays.rs::tests::match_stats_tracker_computes_adr_hs_assists_objectives_mvp_and_json`](../../../crates/cs-server/src/replays.rs), [`proto48.rs::tests::admin_http_endpoints_replays_stats_rcon_and_servers_work_over_ws_transport`](../../../crates/cs-server/src/proto48.rs) |
| Linux XDG, macOS, Windows & SteamOS / Steam Deck path & Steam library discovery | `cs-platform`, `cs-vfs` | [`xdg.rs::tests::resolves_xdg_and_platform_config_and_data_dirs`](../../../crates/cs-platform/src/xdg.rs), [`xdg.rs::tests::detects_steam_deck_from_env_gamescope_and_steamos_release`](../../../crates/cs-platform/src/xdg.rs), [`xdg.rs::tests::discovers_linux_flatpak_deck_sd_and_macos_steam_half_life_dirs`](../../../crates/cs-platform/src/xdg.rs), [`cs-vfs/src/lib.rs::tests::xdg_discovery_preserves_cs16_gamedir_priority_and_resolves_configs`](../../../crates/cs-vfs/src/lib.rs) |
| Lobby directory/signaling (`/lobbies`) & `?join=<host:port>&room=<code>` invite links | `cs-master`, `cs-server`, `web/app.js` | [`lobbies.rs::tests::lobby_registration_signaling_ttl_and_validation`](../../../crates/cs-master/src/lobbies.rs), [`cs-master/src/server.rs::tests::end_to_end_heartbeat_paginated_query_http_shutdown_and_ttl`](../../../crates/cs-master/src/server.rs), [`admin_http.rs::tests::routes_status_players_replays_rcon_and_invite`](../../../crates/cs-server/src/admin_http.rs) |
| 32-player performance & delta-packet benchmark harness (`cargo xtask bench`) | `xtask` | [`bench.rs::tests::parses_bench_flags_and_rejects_invalid`](../../../xtask/src/bench.rs), [`bench.rs::tests::bench_smoke_measures_32_player_ticks_and_delta_packets`](../../../xtask/src/bench.rs) |

## Part B — Manual Operational & Container Checks (Owner)

- [ ] Build and start the container stack (`docker compose up -d --build`) with a read-only `/gamedir` mount and verify UDP `27015` + Web/Admin `27016` reachability.
- [ ] Open `http://127.0.0.1:27016/admin`, authenticate with `rcon_password`, run RCON commands, switch maps, download a round `.dem` replay, and verify playback in `cs-client` / retail `hl.exe`.
- [ ] Open a `?join=<host:port>&room=<code>` invite link in Chromium/Firefox and verify prefilled join target and connection.
- [ ] Run `cargo xtask bench --ticks 200 --players 32` in a release build locally if comparing against the `< 1 ms` production tick budget.
