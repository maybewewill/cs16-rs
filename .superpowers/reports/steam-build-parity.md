# Steam Build 10210 Ghidra Parity Audit & Fix Report

**Target Build:** Steam Counter-Strike 1.6 / Half-Life Build 10210 (`D:\SteamLibrary\steamapps\common\Half-Life`)
**Branch:** `steam-build-parity` (merged to `main`)
**Compliance:** `#![forbid(unsafe_code)]` workspace-wide; GPL-3.0-or-later clean-room Rust implementations citing retail binary names and addresses/symbols only.

---

## 1. Inspected Steam Build 10210 Binaries

| Binary | Ghidra Program ID | Role Audited | Outcome |
|---|---|---|---|
| `client.dll` | `/client.dll-89615f` | HUD, crosshair, radar, view bob/idle, mouse accel & zoom sens | 11 discrepancies fixed & tested |
| `vgui2.dll` | `/vgui2.dll-aa334c` | Font rasterization (`blur`, `scanlines`, `underline`, `strikeout`, bold threshold) | 1 discrepancy (5 sub-behaviors) fixed & tested |
| `GameUI.dll` | `/GameUI.dll-430a72` | Options sliders (`volume`, `gamma`) & Mouse `LowLatency` checkbox | 3 discrepancies fixed & tested |
| `ServerBrowser.dll` | `/ServerBrowser.dll-ef08e7` | Initial dialog sizing, columns (`IPAddr`, `Bots`), filters, Favorites & `DialogGameInfo` | 4 discrepancies fixed & tested |
| `hw.dll` | `/hw.dll-48a4af` | Gamma/brightness, `CL_Move` `msec`, `ex_interp`, `fps_max`, sound spatialization, 29x9 `sx_presets` | 6 discrepancies fixed & tested |
| `swds.dll` | `/swds.dll-405923` | `sys_ticrate`, updaterate/rate cvars, `sv_accelerate`, OOB query limits, connect/challenge, RCON | 5 discrepancies fixed & tested |
| `mp.dll` | `/mp.dll-3376a5` | `FireBullets3`, `KickBack`, rifle spread/recoil, `PM_Move` family, `CheckWinConditions` & economy | **0 discrepancies** (full parity confirmed) |
| `hlds.exe` | `/hlds.exe-aaacd0` | Dedicated server launcher entry point | Thin launcher delegating to `swds.dll` |

---

## 2. Discrepancies Found & Clean-Room Rust Fixes

### Group A — `client.dll` (HUD, Crosshair, Audio Radar Trigger, View & Mouse)

1. **NVG crosshair translucency (`client.dll:FUN_10043ff0`, `0x10044530..10044560`)**
   - **Retail behavior:** Night-vision overrides crosshair RGB to `(250, 50, 50)` while still honoring `cl_crosshair_translucent` and distance/accuracy-derived `alpha` (`120..255`).
   - **Rust fix:** `crosshair_color` in `crates/cs-client/src/ui/mod.rs`.
   - **Tests:** `crates/cs-client/src/ui/mod.rs::tests::nvg_crosshair_honors_cl_crosshair_translucent_and_alpha`.

2. **Sniper dynamic crosshair suppression (`client.dll:FUN_10043a60`, `CHudAmmo::Draw`)**
   - **Retail behavior:** Dynamic 4-bar crosshair is suppressed whenever `m_iFOV <= 54` (`0x36`) or the active weapon is Scout (`3`), SG550 (`13`), AWP (`18`), or G3SG1 (`24`).
   - **Rust fix:** `should_draw_dynamic_crosshair` in `crates/cs-client/src/ui/mod.rs` wired into `crates/cs-client/src/app.rs`.
   - **Tests:** `crates/cs-client/src/ui/mod.rs::tests::sniper_weapons_and_zoomed_fov_suppress_dynamic_crosshair`.

3. **HUD health visibility & digit layout (`client.dll:FUN_1004d9e0`, `CHudHealth::Draw`)**
   - **Retail behavior:** Health/armor HUD requires live `health > 0` (when alive); cross and digits are positioned from scaled sprite widths (`cross_w / 2` and `cross_w + digit_w / 2`).
   - **Rust fix:** Live `hud_health` passed to `compute_hud_visibility` in `crates/cs-client/src/app.rs`; `health_layout_x` in `crates/cs-client/src/ui/hud.rs`.
   - **Tests:** `crates/cs-client/src/ui/hud.rs::tests::health_visibility_layout_and_status_anchor_match_build_10210`.

4. **Status icons vertical anchor (`client.dll:FUN_10065570`, `CHudStatusIcons::Draw`)**
   - **Retail behavior:** Starts `icon_bot` at `screen.center().y - 5.0 * scale` and steps upward by `5.0 * scale + icon_h`.
   - **Rust fix:** `status_icon_anchor_y` in `crates/cs-client/src/ui/hud.rs`.
   - **Tests:** `crates/cs-client/src/ui/hud.rs::tests::health_visibility_layout_and_status_anchor_match_build_10210`.

5. **DeathNotice expiration (`client.dll:FUN_10047d70`, `FUN_10048270`)**
   - **Retail behavior:** Killfeed entries expire once elapsed age exceeds `hud_deathnotice_time` (`<= 0` clears/displays none).
   - **Rust fix:** `filter_active_death_notices` in `crates/cs-client/src/ui/hud.rs` and `HudFeeds::expire` in `crates/cs-client/src/app.rs`.
   - **Tests:** `crates/cs-client/src/ui/hud.rs::tests::deathnotice_and_saytext_expire_and_respect_zero_cvar`, `crates/cs-client/src/app.rs::tests::hud_feeds_expire_respects_zero_deathnotice_and_saytext`.

6. **SayText expiration (`client.dll:FUN_10064340`)**
   - **Retail behavior:** Chat lines expire after `hud_saytext_time` (`<= 0` displays none).
   - **Rust fix:** `filter_active_chat_entries` in `crates/cs-client/src/ui/hud.rs` and `HudFeeds::expire` / `chat_times` wiring in `crates/cs-client/src/app.rs`.
   - **Tests:** `crates/cs-client/src/ui/hud.rs::tests::deathnotice_and_saytext_expire_and_respect_zero_cvar`, `crates/cs-client/src/app.rs::tests::hud_feeds_expire_respects_zero_deathnotice_and_saytext`.

7. **CenterPrint Y offset & fixed amber RGB (`client.dll:FUN_10069560`, `HUD_PRINTCENTER`)**
   - **Retail behavior:** Renders at `screen.top() + screen.height() * 0.35` with fixed RGB `(255, 180, 30)` (`1.0, 0.706, 0.118`) rather than `con_color`.
   - **Rust fix:** `center_print_layout` in `crates/cs-client/src/ui/hud.rs`.
   - **Tests:** `crates/cs-client/src/ui/hud.rs::tests::centerprint_and_radar_vip_dot_match_build_10210`.

8. **Radar VIP / Bomb-carrier dot (`client.dll:FUN_1004e6c0`)**
   - **Retail behavior:** VIP / bomb-carrier teammate dot renders at `dot_sz * 2` with RGBA `(185, 20, 20, 235)` (distinct from planted/dropped C4 slot flash `(250, 0, 0, 245)`).
   - **Rust fix:** `radar_player_dot_style` in `crates/cs-client/src/ui/hud.rs`.
   - **Tests:** `crates/cs-client/src/ui/hud.rs::tests::centerprint_and_radar_vip_dot_match_build_10210`.

9. **SendAudio radar flash trigger (`client.dll:FUN_10064bc0`)**
   - **Retail behavior:** Receiving `SendAudio` for `sender > 0` sets a radar flash lasting `22 * 0.15s = 3.3s`, cleared on `ResetHUD`.
   - **Rust fix:** `RADIO_FLASH_DURATION_SECS` and `HudState::radio_flashes` in `crates/cs-clientcore/src/hud.rs`, `play_send_audio_with_radar` in `crates/cs-client/src/audio/native.rs`, and fallback wiring in `crates/cs-client/src/ui/hud.rs` & `crates/cs-client/src/app.rs`.
   - **Tests:** `crates/cs-clientcore/src/hud.rs::tests::send_audio_sets_radio_flash_for_positive_sender`, `crates/cs-client/src/audio/native.rs::tests::send_audio_with_radar_arms_3_3s_flash_for_positive_sender`.

10. **View bob, `V_CalcRefdef` +0.03125 nudge & `V_AddIdle` (`client.dll:FUN_1006ab20`, `0x1006b21b`, `0x1006b52f..1006b7a0`, `0x101065b4..1010662c`)**
    - **Retail behavior:** Caches and freezes last ground `bob` while airborne; leaves `cl_bobup` unclamped; nudges `vieworg` by `+0.03125` on all axes; projects `bob * 0.4` along 3D view `forward` for the viewmodel; applies `V_AddIdle` sinusoidal roll (`0.5` cycle, `0.1` level), pitch (`1.0`, `0.3`), and yaw (`2.0`, `0.3`) scaled by `v_idlescale`, plus GoldSrc viewmodel pitch/yaw/roll offsets.
    - **Rust fix:** `BobState`, `calc_view_idle`, `calc_viewmodel_angles` in `crates/cs-client/src/view_effects.rs`; `crates/cs-client/src/view.rs`; `viewmodel_transform_with_angles` in `crates/cs-client/src/render/mdl_renderer.rs`; `crates/cs-client/src/app.rs`.
    - **Tests:** `crates/cs-client/src/view_effects.rs::tests::bob_and_roll_follow_goldsrc_bounds`, `bob_state_freezes_airborne_and_allows_unclamped_bobup`, `view_idle_and_viewmodel_angles_match_goldsrc_cycles`; `crates/cs-client/src/view.rs::tests::build_parts_nudges_vieworg_by_one_32nd_and_freezes_airborne_bob`; `crates/cs-client/src/render/mdl_renderer.rs::tests::viewmodel_transform_with_angles_applies_roll_and_righthand_mirror`.

11. **Mouse acceleration & zoom sensitivity (`client.dll:0x10036130..10036270`, `IN_MouseMove`)**
    - **Retail behavior:** Removes arbitrary `[0.1, 4.0]` exponent clamp; computes `sensitivity + mag.powf(exp) * scale`, caps total by `m_customaccel_max` (`> 1e-4`), then multiplies by `zoom_sensitivity_ratio * (fov / default_fov)` when zoomed (no deadzone or `[0.05, 2.0]` clamp); when `m_customaccel == 2`, scales `mx *= m_yaw` and `my *= m_pitch`.
    - **Rust fix:** `compute_zoom_sens_scale` and `compute_mouse_move_params` in `crates/cs-client/src/app.rs`.
    - **Tests:** `crates/cs-client/src/app.rs::tests::mouse_accel_zoom_sens_and_multiplayer_fps_floor_match_build_10210`.

---

### Group B — `vgui2.dll`, `GameUI.dll`, `ServerBrowser.dll`

1. **VGUI2 font rasterizer effects (`vgui2.dll:FUN_1001a9b0`, `FUN_1001aea0`, `FUN_1000daf0`)**
   - **Retail behavior:** Post-rasterization Gaussian blur for `blur > 1` (`sigma = blur * 0.683`), periodic `scanlines` darkening (`> 1`), `underline` & `strikeout` horizontal strokes, and GDI `FW_BOLD` (`weight >= 700`) emboldening threshold.
   - **Rust fix:** `crates/cs-assets/src/vgui_font.rs` (`apply_gaussian_blur`, `apply_scanlines`, `paint_horizontal_stroke`, `RasterizeOptions`).
   - **Tests:** `crates/cs-assets/src/vgui_font.rs::tests::scheme_parses_blur_scanlines_underline_strikeout_and_bold_threshold`, `gaussian_blur_expands_and_softens_only_when_blur_gt_1`, `scanlines_darken_non_zero_modulus_rows`, `rasterize_underline_strikeout_and_blur_on_font`.

2. **Audio volume slider bounds (`GameUI.dll:FUN_100321e0`)**
   - **Retail behavior:** `volume` slider ranges `0.0..=2.0` (was `0.0..=1.0`).
   - **Rust fix:** `VOLUME_SLIDER_MIN..=VOLUME_SLIDER_MAX` in `crates/cs-client/src/ui/options.rs`.
   - **Tests:** `crates/cs-client/src/ui/options.rs::tests::audio_volume_slider_allows_up_to_2_0_and_video_gamma_down_to_1_0`.

3. **Video gamma slider bounds (`GameUI.dll:FUN_10037ed0`)**
   - **Retail behavior:** `gamma` slider ranges `1.0..=3.0` (was `1.8..=3.0`).
   - **Rust fix:** `GAMMA_SLIDER_MIN..=GAMMA_SLIDER_MAX` in `crates/cs-client/src/ui/options.rs`.
   - **Tests:** `crates/cs-client/src/ui/options.rs::tests::audio_volume_slider_allows_up_to_2_0_and_video_gamma_down_to_1_0`.

4. **Mouse `LowLatency` checkbox binding (`GameUI.dll:FUN_10034f90`)**
   - **Retail behavior:** Mouse tab `LowLatency` checkbox binds to `gl_lowlatency` (was bound to `m_customaccel`).
   - **Rust fix:** `LOW_LATENCY_CVAR` (`gl_lowlatency`) and `ensure_lowlatency_cvar` in `crates/cs-client/src/ui/options.rs`.
   - **Tests:** `crates/cs-client/src/ui/options.rs::tests::mouse_tab_low_latency_binds_to_gl_lowlatency_not_m_customaccel`.

5. **ServerBrowser initial size (`ServerBrowser.dll:FUN_100102d0`, `DialogServerBrowser.res:7-10`)**
   - **Retail behavior:** First-run size defaults to `640x384` scaled by `s_chrome` and clamped to screen, persisting user resizes.
   - **Rust fix:** `resolve_dialog_size` in `crates/cs-client/src/ui/screens/server_browser.rs`.
   - **Tests:** `crates/cs-client/src/ui/screens/server_browser.rs::tests::default_size_is_640x384_scaled_and_clamped_and_persists_custom`.

6. **ServerBrowser `IPAddr` & `Bots` columns (`ServerBrowser.dll:FUN_10001080`)**
   - **Retail behavior:** Toggleable column 8 `IPAddr` (width `64`, hidden by default) and `Bots` count rendered when `bots > 0`.
   - **Rust fix:** `crates/cs-client/src/ui/screens/server_browser.rs`.
   - **Tests:** `crates/cs-client/src/ui/screens/server_browser.rs::tests::bots_column_toggleable_ip_col_favorites_and_game_info_auto_retry_render`.

7. **ServerBrowser filters (`ServerBrowser.dll:FUN_10002e70`, `InternetGamesPage_Filters.res:171-326`)**
   - **Retail behavior:** `GameFilter` combo, `PingFilter` (`All`, `<50`, `<100`, `<150`, `<250`, `<350`, `<600`), 3-state `SecureFilter` (`All`, `SecureOnly`, `InsecureOnly`), and `ValidSteamAccountFilterCheck`.
   - **Rust fix:** `SecureFilter` and filter controls in `crates/cs-client/src/ui/screens/server_browser.rs`.
   - **Tests:** `crates/cs-client/src/ui/screens/server_browser.rs::tests::filters_handle_game_ping_3_state_secure_and_steam_account`.

8. **ServerBrowser Favorites `AddCurrentServer` & `DialogGameInfo` (`InternetGamesPage.res:98-117`, `DialogGameInfo.res`)**
   - **Retail behavior:** Favorites tab exposes `AddCurrentServer` when connected; `DialogGameInfo` modal supports server info inspection and auto-retry when slots open.
   - **Rust fix:** `AutoRetryMode`, `open_game_info`, `set_current_server`, and modal rendering in `crates/cs-client/src/ui/screens/server_browser.rs`.
   - **Tests:** `crates/cs-client/src/ui/screens/server_browser.rs::tests::bots_column_toggleable_ip_col_favorites_and_game_info_auto_retry_render`.

---

### Group C — `hw.dll` & `swds.dll`

1. **Gamma default & brightness clamp (`hw.dll:0x1031e138`, `0x10230fa7`, `0x10231470`)**
   - **Retail behavior:** `gamma` defaults to `2.2` (falling back to `2.5` only when `gamma == 0.0`); `brightness` clamps to `[0.0, 2.0]` in multiplayer (`maxclients > 1`) and `[0.0, 100.0]` in singleplayer.
   - **Rust fix:** `crates/cs-cvar/src/defaults.rs`, `crates/cs-assets/src/gamma.rs`, `crates/cs-client/src/render/gamma.rs`.
   - **Tests:** `crates/cs-assets/src/gamma.rs::tests::retail_gamma_default_zero_fallback_and_brightness_clamps`, `crates/cs-client/src/render/gamma.rs::tests::gamma_texgamma_brightness_and_software_dither_work`.

2. **`CL_Move` `msec` cap (`hw.dll:FUN_101a3a40`, `0x101a3f65`)**
   - **Retail behavior:** Caps `cmd.msec` at `255` instead of `100` while preserving sub-ms carry up to `255 ms`.
   - **Rust fix:** `CmdClock::msec` in `crates/cs-client/src/input.rs`.
   - **Tests:** `crates/cs-client/src/input.rs::tests::frame_times_carry_their_remainder`.

3. **`cl_updaterate` / `ex_interp` quantization & spectator cap (`hw.dll:FUN_101a3a40`, `0x101a3c79`, `DAT_102b6df8`)**
   - **Retail behavior:** Defaults `cl_updaterate` to `"20"`, resets `cl_updaterate < 10.0` to `20.0`, computes `min_ms = floor(1000.0 / updaterate).max(1.0)`, and clamps `ex_interp` to `[min_ms, max_ms]` (`200 ms` spectator/HLTV, `100 ms` otherwise) with `1 ms` tolerance.
   - **Rust fix:** `crates/cs-clientcore/src/cvars.rs` and `InterpSettings::apply_cvars` in `crates/cs-clientcore/src/interp.rs`.
   - **Tests:** `crates/cs-clientcore/src/interp.rs::tests::apply_cvars_quantizes_min_ms_resets_low_updaterate_and_caps_spectator`, `crates/cs-clientcore/src/cvars.rs::tests::userinfo_cvars_are_the_probe_defaults`.

4. **Multiplayer `fps_max >= 20.0` floor (`hw.dll:FUN_101d43d0`, `1 < DAT_113fcc44`)**
   - **Retail behavior:** Enforces `fps_max >= 20.0` whenever `maxclients > 1`.
   - **Rust fix:** `effective_fps_limit` in `crates/cs-client/src/app.rs`.
   - **Tests:** `crates/cs-client/src/app.rs::tests::mouse_accel_zoom_sens_and_multiplayer_fps_floor_match_build_10210`.

5. **`S_Spatialize` & `S_StartDynamicSound` (`hw.dll:FUN_101fd050`, `FUN_10202bf0`, `FUN_101fe3e0`)**
   - **Retail behavior:** No `48.0`-unit wall occlusion gain multiplier in `SND_Spatialize`; brush-model sounds originate at `origin + (mins + maxs) * 0.5`; pitch scales by `(timescale + 1.0) * 0.5`; duplicate dynamic sounds receive `0..10%` sample-rate jitter.
   - **Rust fix:** `crates/cs-client/src/audio/mixer.rs` (`spatialize`, `brush_model_sound_origin`, timescale pitch, duplicate jitter).
   - **Tests:** `crates/cs-client/tests/phase9_sound.rs::goldsrc_spatialize_no_wall_occlusion_plus_timescale_and_duplicate_jitter`, `goldsrc_fractional_pitch_shifting`.

6. **Exact `sx_presets` 29x9 DSP table (`hw.dll:0x1031c6c0..0x1031cad4`)**
   - **Retail behavior:** 29 room presets with 9 columns `[lp, mod, size, refl, rvblp, dly, feedback, dlylp, left]` (no extra `room_rvb` field).
   - **Rust fix:** `ROOM_PRESETS` in `crates/cs-client/src/audio/dsp.rs`.
   - **Tests:** `crates/cs-client/src/audio/dsp.rs::tests::all_29_presets_generate_tail_and_underwater_muffles_highs`, `crates/cs-client/tests/phase9_sound.rs::roomtype_29_presets_modulate_wet_reverb`.

7. **Server `sys_ticrate` & `-sys_ticrate` (`swds.dll:0x1031154c`, `FUN_101d2040`, `0x101d221e`)**
   - **Retail behavior:** Registers `sys_ticrate` (`100.0`), accepts `-sys_ticrate` CLI override, skips early frames when `dt < 1.0 / (ticrate + 1.0)`, and clamps `host_frametime <= 0.25`.
   - **Rust fix:** `crates/cs-cvar/src/defaults.rs`, `crates/cs-engine/src/server.rs`, `crates/cs-server/src/proto48.rs`.
   - **Tests:** `crates/cs-engine/src/server.rs::tests::sys_ticrate_gates_early_frames_and_rate_cvars_apply_dynamically`, `crates/cs-server/src/proto48.rs::tests::parses_phase13_admin_replay_and_embed_flags`.

8. **Server updaterate & rate cvars (`swds.dll:0x10312b7c`, `0x10312b90`, `0x10312bcc`, `0x10312bb8`, `FUN_102081e0`)**
   - **Retail behavior:** Defaults `max_update_rate = 30`; registers `sv_maxupdaterate` (`30.0`), `sv_minupdaterate` (`10.0`), `sv_minrate` (`0`), `sv_maxrate` (`0`), `sv_lan` (`0`), `sv_lan_rate` (`20000.0`) and syncs them dynamically per frame.
   - **Rust fix:** `crates/cs-cvar/src/defaults.rs`, `ServerConfig::default` and `sync_rate_cvars` in `crates/cs-engine/src/server.rs`.
   - **Tests:** `crates/cs-engine/src/server.rs::tests::sys_ticrate_gates_early_frames_and_rate_cvars_apply_dynamically`, `server_config_has_the_cs16_defaults`.

9. **Engine `sv_accelerate` default (`swds.dll:0x103133a4`)**
   - **Retail behavior:** Engine registers `sv_accelerate` default `"10"`, while `cs-game` `RestartRound`/`Think` overrides it to `"5"`.
   - **Rust fix:** `crates/cs-cvar/src/defaults.rs`.
   - **Tests:** `crates/cs-game/tests/movement_world.rs::movement_cvars_are_hard_locked`.

10. **OOB query rate limits, challenge & connect spam (`swds.dll:FUN_1020b250`, `FUN_10205640`, `FUN_10206040`)**
    - **Retail behavior:** Registers `max_queries_sec` (`3.0`), `max_queries_sec_global` (`30`), `max_queries_window` (`60`) and `sv_contact` (`""`); enforces sliding-window per-IP and global A2S query limits; distinguishes `"No challenge for your address.\n"` from `"Bad challenge.\n"`; emits distinct newer/older protocol strings (`sv_contact` when client `> 48`); rejects `> 5` simultaneous pending connections from the same IP.
    - **Rust fix:** `crates/cs-cvar/src/defaults.rs`, `QueryRateLimiter` and `handle_oob` in `crates/cs-engine/src/oob.rs`, `crates/cs-engine/src/connect.rs`, `crates/cs-engine/src/server.rs`.
    - **Tests:** `crates/cs-engine/src/oob.rs::tests::query_rate_limiter_enforces_per_ip_and_global_windows`, `connect_with_bad_or_missing_challenge_is_rejected`, `connect_with_wrong_protocol_is_rejected`; `crates/cs-engine/src/server.rs::tests::rejects_more_than_five_pending_connections_from_same_ip`.

11. **RCON validation & policy clamping (`swds.dll:FUN_10204010`, `FUN_1020aed0`, `FUN_10208a50`)**
    - **Retail behavior:** Clamps `sv_rcon_{min,max}failures` to `[1, 20]` (`max >= min`) and `sv_rcon_minfailuretime >= 1`; replies `"Bad rcon_password.\nNo password set for this server.\n"` when `rcon_password` is empty; does not count bad challenge toward `record_failure`; enforces 512-byte body / 1023-char command caps.
    - **Rust fix:** `RconPolicy::clamped` and `RconState::evaluate` in `crates/cs-engine/src/rcon.rs`, `crates/cs-engine/src/connect.rs`.
    - **Tests:** `crates/cs-engine/src/rcon.rs::tests::rcon_brute_force_triggers_ban_and_empty_password_rejects`.

---

## 3. `mp.dll` Systems Audited with Zero Discrepancies

Ghidra inspection of `mp.dll` (`/mp.dll-3376a5`) confirmed full formula and behavioral parity across all audited gameplay systems:
- **`FireBullets3`:** Penetration counts, distance falloff, wall material attenuation/penetration multipliers, and damage attenuation.
- **`KickBack`:** Punchangle velocity/ground/duck thresholds, lateral sign alternation, and per-weapon caps.
- **Rifle Spread & Recoil:** FAMAS (auto/burst), AK-47, M4A1 (unsilenced/silenced), and AUG spread polynomials, `m_flAccuracy`, and firing interval schedules.
- **`PM_Move` Family:** `PM_Duck`, `PM_Jump`, `PM_LadderMove`, and `PM_WaterMove` constants, hull transitions, friction/acceleration, and step/stamina rules.
- **`CheckWinConditions` & Economy:** Round end checks, bomb/hostage/elimination conditions, consecutive loss streak escalation, and round-end/kill/objective cash awards.
