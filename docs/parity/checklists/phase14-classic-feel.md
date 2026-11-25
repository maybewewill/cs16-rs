# Phase 14 — Classic CS 1.6 Feel & Opt-In "Beyond 1.6" Checklist

## Part A — Automated Verification

All automated checks below passed on `phase14-classic-feel` (`ccbde23`) with `CS16_GAMEDIR="D:/SteamLibrary/steamapps/common/Half-Life"`:

| Area | Scope | Automated Evidence |
|---|---|---|
| Input & Wheel Binds | `mouse1..5`, `mwheelup`/`mwheeldown` (`invprev`/`invnext`), `kbutton_t` impulse latching | `cs-client::input::tests::*`, `cs-web::input_web::tests::*` |
| Material Footsteps | `PM_TraceTexture` world + rotated brush submodels (`WorldCollision::trace_texture`) | `cs-clientcore::tests::predict::*` |
| Vanilla CVar/Cmd Parity | `726/726` `cvars.toml` entries (`0` missing), `wait` deferral, `s_occfactor 0`, `s_lerping`/`hisound` | `xtask::registry_sync::*`, `cs-cvar`, `cs-engine`, `cs-game`, `cs-client::tests::phase9_sound` |
| Classic Visuals & UI | `sniper_scope.spr`, `RenderMode::Glow` occlusion traces, `#Token` localization, `MainBuyMenu.res`, `cl_shadows 1`, `cl_minmodels 1`, brush-entity decals, 32-bit smoke, `net_graph 1/2/3`, `cl_showpos`, `r_speeds` | `cs-client::tests::{phase10_ui,phase14_task5,phase8_render}`, headless `1280x800` Vulkan smoke screenshots |
| Prediction & Server Pacing | 100 Hz high-Hz `cmd` grid, `visents` boost/runboost, AWP quick-switch, live `sv_*updaterate`, `IN_SCORE` `svc_pings`, chronological `Broadcast`, 1 s `sendinfo_time` | `cs-clientcore::tests::{predict,weapons_predict}`, `cs-engine::tests::lifecycle` |
| Opt-In Beyond 1.6 & KZ | `sv_showimpacts` (+ wallbang), `sv_grenade_trajectory`, `rethrow`, `mp_warmup_mode`, `sv_damage_print`, `lo3`, `mp_knife_round`, `sv_kz_mode`, `cl_ljstats` `+32 u` | `cs-game::tests::{phase14_task9,phase14_task10}`, `cs-client::tests::phase10_ui` |
| Spec X-Ray, ZBot, Admin 2D, Voice, P2P | `HealthInfo`/`Account` anti-leak + `.dem`, `spec_xray`/`spec_hud_bars`, CRC32 `nav_cache` + `fy_*`/`35hp_*`, 2D replay timeline + overviews, Steam `OPUS_PLC` + bridge, WebRTC P2P `LocalGame` + `/lobbies` signaling | `cs-game::tests::{phase14_task11,phase14_zbot_arena}`, `cs-client::tests::phase14_task11`, `cs-server` unit tests, `cs-web::state::tests::p2p_host_exchanges_signon_movement_and_shots_with_web_peer`, `node --check web/*.js` |

## Part B — Manual Live-Play Verification (Owner)

- [ ] **Scroll-wheel bhop & duck-scroll:** Bind `mwheelup +jump` and `mwheeldown +duck` in `cs-client`, test bunnyhop chains and silent-run double-ducks at `100 FPS` and `144 Hz` (`fps_override 1`).
- [ ] **Predicted footsteps, `sniper_scope.spr` & glows:** Walk across `cs_office` snow, `de_nuke` metal/vents, and `de_dust2` sand; scope with AWP/Scout (`sniper_scope.spr`); verify planted C4 `TE_GLOWSPRITE` does not bleed through solid walls.
- [ ] **Shadows, `cl_minmodels`, brush decals & smoke:** Check `cl_shadows 1` player/hostage discs, `cl_minmodels 1`, shoot `cs_assault` vents / `de_nuke` doors and verify decals move or clear on break, and inspect 32-bit grey smoke clouds.
- [ ] **`net_graph 1/2/3` & `s_occfactor 0`:** Compare `net_graph 1` / `3` layout and unmuffled through-wall footsteps on `de_dust2` against retail `hl.exe`.
- [ ] **Opt-In Practice, Scrim & KZ/LJ:** Verify `sv_cheats 1` wallbang `sv_showimpacts 1`, `sv_grenade_trajectory 1`, `rethrow`, `lo3`, `mp_knife_round 1` (`.stay`/`.switch`), `sv_kz_mode 1` (`/cp`, `/tp`), and `cl_ljstats 1` distance/sync accuracy.
- [ ] **Spectator `spec_xray` & `spec_hud_bars`:** Open a `.dem` or spectate a bot match with `spec_xray 1` and `spec_hud_bars 1`.
- [ ] **3-Way Steam Voice (`hl.exe <-> cs-client <-> cs-web`):** Host `cs-server --web-port 27126`, join from retail `hl.exe`, native `cs-client`, and browser `cs-web`, and confirm two-way push-to-talk audio.
- [ ] **1-Click Browser WebRTC P2P:** Open `http://127.0.0.1:27126/`, mount Half-Life assets, click **Host P2P Room (1v1 / 5v5 in Browser)**, open the copied `?p2p=<room>&signal=<origin>` URL in a second browser tab/device, and play a round over WebRTC DataChannel.
