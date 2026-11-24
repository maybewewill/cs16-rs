# Phase 10 — VGUI2 UI, HUD, Fonts & Localization Checklist

Headless implementation and unit/integration coverage for all **50/50** items in [`docs/parity/ui.toml`](../ui.toml) (automated parser, `.res` coordinate solver, VGUI2 12-control runtime, HTML MOTD subset, HUD reducers/widgets, and CVar bindings). Pixel comparison against the original Steam Counter-Strike 1.6 client has **not** yet been completed and lives in **Part B** below.

## Part A — Automated Verification

| Check | Scope | Evidence |
|---|---|---|
| TrueType & scheme fonts (`marlett.ttf`, `TrackerScheme.res` / `ClientScheme.res`, `yres` tiers, `CustomFontFiles`) | `cs-assets`, `cs-client` | [`phase10_ui.rs::ui_truetype_font_rasterizer_loads_marlett_and_scheme_tiers`](../../../crates/cs-client/tests/phase10_ui.rs) |
| Multi-language UTF-16LE localization (`cstrike_*.txt`, `gameui_*.txt`, `serverbrowser_*.txt`, `cl_language`) | `cs-assets`, `cs-client` | [`phase10_ui.rs::ui_all_languages_localization_decodes_multilang_utf16le`](../../../crates/cs-client/tests/phase10_ui.rs) |
| VGUI2 `.res` layout interpreter & all 12 controls (`Panel`, `Label`, `Button`, `ImagePanel`, `Frame`, `ListPanel`, `RichText`, `ComboBox`, `Slider`, `CheckButton`, `TextEntry`, `PropertySheet`) + server `.res` hot-reload | `cs-assets`, `cs-client` | Dedicated 1-per-item tests in [`phase10_ui.rs`](../../../crates/cs-client/tests/phase10_ui.rs) |
| Screens (`MainMenu` / `GameMenu.res` / `settings.scr`, `Options` 7 tabs + `user.scr`, `DialogServerBrowser.res`, `Teammenu.res` / `Classmenu_{CT,TER}.res` / `MainBuyMenu.res`, `MOTD.res` HTML subset, `ScoreBoard.res`, `Spectator.res` / `BottomSpectator.res`, `commandmenu.txt`, `LoadingDialog.res`, `PlayerListDialog.res`, `DemoPlayerDialog.res`, Password/Error dialogs) | `cs-client` | Dedicated 1-per-screen tests in [`phase10_ui.rs`](../../../crates/cs-client/tests/phase10_ui.rs) |
| HUD drivers & usermsgs (`Health`, `Battery`, `Money`, `CurWeapon`, `AmmoX`, `RoundTime`, `StatusIcon`, `BarTime`/`BarTime2`, `StatusText`/`StatusValue`, `Damage` quadrants/`dmg_*`, `WeaponList` buckets + `hud_fastswitch`, `HudText`/`HudTextArgs`/`titles.txt`/`TE_TEXTMESSAGE` FX `0..=2`, `Scenario`/`BombDrop`/`HostagePos`/`HostageK`, `Flashlight`/`FlashBat`, `overviews/<map>.txt` + `.bmp` chromakey, AMX `\w`/`\y`/`\r`/`\d`/`\R` `ShowMenu`) | `cs-proto48`, `cs-clientcore`, `cs-client` | [`usermsg.rs::tests::phase10_hud_usermsgs_roundtrip`](../../../crates/cs-proto48/src/usermsg.rs), [`hud.rs::tests::phase10_hud_text_spec_and_damage_history_reduce`](../../../crates/cs-clientcore/src/hud.rs), and [`phase10_ui.rs`](../../../crates/cs-client/tests/phase10_ui.rs) |

## Part B — Manual Live Screenshot & Visual Parity (Owner)

> Known open visual issues observed during GPU self-capture on `de_dust2`:
> 1. Raw `#Game_connected` / `#Game_join_ct` / `#Game_join_terrorist` / `#Team_Select_Spect` tokens appear on HUD notifications/text-menus when `cstrike/titles.txt` or fallback token lookups omit them.
> 2. Long category captions (`6 PRIMARY AMMO`, `7 SECONDARY AMMO`) wrap tightly inside fixed-height `MainBuyMenu.res` buttons.

- [ ] Compare side-by-side screenshots (`hl.exe` vs `cs-client`) at `1280×800` / `1024×768` across: Main Menu, New Game, Server Browser (`Internet`, `Favorites`, `History`, `Spectate`, `LAN`, `Friends`, `Filters`), Options (all 7 tabs + `Advanced...`), Developer Console (`con_color`).
- [ ] Join a live match with `_vgui_menus 1` and compare `Teammenu.res`, `Classmenu_CT.res` / `Classmenu_TER.res` (hover previews), `MainBuyMenu.res` + category submenus (`BuyPistols_*`..`BuyEquipment_*`), and `MOTD.res` HTML rendering.
- [ ] Verify in-game HUD overlays side-by-side: `overviews/de_dust2.bmp` with `cl_radartype 1`, `StatusText` target ID (`hud_centerid 0/1`), `Damage` edge chevrons + `dmg_*` hazard sprites, `BarTime` plant/defuse bar, `WeaponList` buckets (`hud_fastswitch 0` two-step click confirm), `ScoreBoard` (`TAB`), `Spectator` bottom bar + overview cones/names, and `+commandmenu` (`H`).
- [ ] Connect to an AMX Mod X server and verify multi-color `\w`/`\y`/`\r`/`\d` + `\R` right-aligned `ShowMenu` and channel `TE_TEXTMESSAGE` HUD/DHUD fade/flicker/typewriter effects match original CS 1.6.
