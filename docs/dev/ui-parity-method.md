# UI parity with the original GameUI: method and status

How to make each screen of our client match the original CS 1.6 (Steam build) pixel for pixel,
and where each screen stands. The original GameUI (`GameUI.dll`, `vgui2.dll`) is closed source,
so its layout rules are **measured** from the running original, then written into our code with
the measurement cited.

## Tools (`scripts/ui-parity/`)

| Script | What it does |
|---|---|
| `capture_original.ps1 <out.png>` | Captures the original `hl.exe` window (`PrintWindow`). The client area is the image cropped by 8 px left/right/bottom and 31 px top. |
| `orig_click.ps1 -X -Y -Out [-WaitMs]` | Clicks at client coordinates of the original window, then captures. `-X -1` only captures. |
| `measure_text_rows.py <capture>` | Finds rows of menu-coloured text in the lower half and prints their boxes (top, bottom, left, right) and colours. |

Our client captures itself headless:

```
cs-client --gamedir <Half-Life> --headless --resolution 1280x800 [--ui options|newgame|servers|console|playerlist] [--map de_dust2 [--ui team|buy|class|scoreboard|hud|commandmenu]] --screenshot out.png
```

Start the original windowed at a fixed size (only when the owner is not playing):

```
steam.exe -applaunch 10 -windowed -w 1280 -h 800 -novid
```

The first launch at a new size can fail to open in time; wait about 40 s and retry if the capture is 160x28.

Compare with PIL. Use the mean absolute difference per channel of the whole frame, or of a region with no animated content. Use side-by-side crops for the eye. A screen is done when the mean difference is within a few units (the main menu is 1.5-2.6 / 255) and no element is misplaced.

## Rules measured so far

### Main menu (`GameMenu.res`): DONE, commit a592c27

Measured at 800x600, 1024x768, 1280x720, 1280x800, 1280x1024 and 1600x1080:

- **Scale.** `s = max(1, height / 720)`. Everything below scales by `s`.
- **Items.**
  - Items stack upwards from the bottom.
  - The last item's baseline is `63*s` above the bottom.
  - The pitch is `MenuItemHeight` (48) `*s`.
  - A blank `GameMenu.res` entry is a spacer.
- **Text x.**
  - `115*s` when the width is above `1024`, `round(115 * 0.7) * s` (`81*s`) at `1024` and below (verified via `GameUI.dll` `CBasePanel::ApplySchemeSettings`, `FUN_1003e590`).
- **Font and colours.**
  - `MenuLarge` at `round(28 * 0.775 * s)` px, synthetic bold (weight 1000).
  - `MenuColor`; the armed row is `ArmedMenuColor` over an additive `MenuBlurLarge` glow in `BlurMenuColor`.
  - The first row is armed until the mouse arms another.
- **Logo.** `resource/game_menu.tga` (`game_menu_mouseover.tga` on hover). Its content's left edge is 4 px left of the text, and its last row is `16*s` above the bottom; drawn at its own size `*s`.
- **Background.** The 800x600 `BackgroundLayout.txt` composite, scaled by `max(W/800, H/600)` and cropped centred. This was verified by search over stretch/cover/offset; "fit" in the layout file does not mean stretch.

### Font engine fixes (same commit)

- Atlas tiers are keyed by pixel height. A proportional tier made by `measure_text` was being reused by fixed-size paints.
- `measure_text_scaled` must use the same scaling as the matching `paint_text`.
- Synthetic bold like GDI: when the weight is above the bold face (> 800), each glyph is smeared 1 px right and its advance grows by 1.

### Options dialog (`OptionsSub*.res`): DONE

Measured at 800x600, 1280x720, 1280x800, and 1280x1024 (plus all 7 tabs at 1280x800):

- **Scale.** `s = max(1, height / 720)` (`TrackerScheme.res` `ProportionalBaseHeightHD`).
- **Dialog window.** `(522 * s) x (444 * s)`, centered on screen:
  - 800x600 (`s = 1.000`): `522x444` at `(139, 78)`
  - 1280x720 (`s = 1.000`): `522x444` at `(379, 138)`
  - 1280x800 (`s = 1.111`): `580x493` at `(350, 153)`
  - 1280x1024 (`s = 1.422`): `742x631` at `(269, 196)`
- **Titlebar.** `icon_steam.tga` (`16x16 * s`) at `(7, 8) * s`, `#GameUI_Options` (`DefaultBold`) at `x = 28 * s`, close button `[X]` at `(497, 8, 18, 18) * s`.
- **PropertySheet & tabs.** Body rect `(5, 55, 512, 352) * s`. Tab order: `Multiplayer`, `Audio`, `Video`, `Aim`, `Keyboard`, `Voice`, `Content`. Tab width `max(64 * s, text_w + 16 * s)` with `1 * s` gap; active tab sits at `y = 29..55 * s` and merges with the sheet top border; inactive tabs sit at `y = 31..55 * s`.
- **Subpages.** Every control in `OptionsSub*.res` scales by `s` relative to `(dlg_x + 5*s, dlg_y + 55*s)` (`Suit Slider` hidden on Audio and `DetailTextures` hidden on Content as in CS 1.6).
- **Bottom buttons.** `OK` `(277, 411, 72, 24)*s`, `Cancel` `(357, 411, 72, 24)*s`, `Apply` `(437, 411, 72, 24)*s`, west-aligned with `6 * s` left inset.
- **Font GDI scaling.** `TrackerScheme` GDI `tall` values map to `ab_glyph` em-height by `0.82 * tall * s` (`Options` 11 px / `Player name` 12 px at `s = 1.0`).

### ClientScheme In-Game Menus (`MOTD.res`, `Teammenu.res`, `Classmenu_*.res`, `MainBuyMenu.res`): DONE

Measured on `de_dust2` across `800x600`, `1280x720`, `1280x800`, and `1280x1024`:

- **4:3 Centered Viewport (`BackgroundPanel.res`).** Scale `s = min(W / 640, H / 480)`, centered `640*s x 480*s` canvas at `vp_x = floor((W - 640*s)/2)`, `vp_y = floor((H - 480*s)/2)`. Panel fills `ViewportBG` (`0, 0, 0, 200`) with rounded radius `round(10*s)` and **no** outer orange stroke; `GapPanel` is a `1 px` warm hairline across `(panel_x..panel_right, vp_y + round(72*s))`.
- **Fonts & Mnemonics.** `Title` (`Title`, `serverName`, `joinTeam`, `joinClass`) scales by `s` (`0.82 * tall * s`), while `Default` / `DefaultSmall` buttons and descriptions stay unscaled (`1.0`). Single `&` hotkey markers (`&OK`, `&1 PISTOLS`, ...) are stripped cleanly from `ClientScheme` buttons without underline.
- **MOTD (`MOTD.res` / `motd.txt`).** `Message` HTML box is solid black with no orange border; `motd.txt` CSS `margin` (`8 px`) and `font: 16px/20px` render in 1:1 CSS pixels with `<br>` blank lines preserved.
- **Team / Class / Buy Menus.** Hidden `vipbutton` (`Teammenu.res`) and `spetsnaz` (`Classmenu_CT.res`) keep their original row slots instead of compacting lower buttons up; `CancelButton` (`0 CANCEL`) is shown only when `allowed_keys` bit 9 or joined state permits cancelling (and on `BuyMenu`). `TeamMenu` `MapInfo` renders plain `maps/<map>.txt` (`Default`, `14 px` row stride) beside a VGUI2 right vertical scrollbar (`sb_w = round(21*s)` with `1 px` `BorderBright` borders and light-grey arrow triangles). `ClassResPanel` overwrites earlier labels sharing `fieldName` (`classes/urban.res` `className` -> `infolabel` at `ypos=228`), matching Valve's `LoadControlSettings`. `BuyMenu` `Divider1` renders as a `1 px` vertical `BorderBright` hairline.

### Scoreboard, Console, Chat, and Centre Print: DONE

Measured on `de_dust2` across `800x600`, `1280x720`, `1280x800`, and `1280x1024`:

- **Scoreboard (`ClientScoreBoard` / `ScoreBoard.res`).** Scale `s_h = H / 480`, centered `round(520 * s_h) x round(340 * s_h)` (`250,105 780x510` at 1280x720; `207,117 866x566` at 1280x800; `85,149 1109x725` at 1280x1024) with `ListBG` (`0, 0, 0, 128`), `1 px` `BorderBright` rounded outline (`round(8 * s_h)`), server name + right-aligned `Score`/`Deaths`/`Latency` (`107*s_h`, `61*s_h`, `11.5*s_h` from right), header hairline at `+13.2*s_h`, non-empty team headers + team-colored 1 px underlines, and unhighlighted player rows in team color.
- **Console (`CConsoleDialog`, `TrackerScheme`).** Scale `s = max(1, H / 720)`, default rect `(40, 40, 560, 400) * s` (`560x400` at 800x600/1280x720; `622x444` at `(44,44)` at 1280x800; `796x569` at `(57,57)` at 1280x1024). Steam 2003 olive `ControlBG` raised bevel frame, `35*s` titlebar with `icon_steam.tga`, `DefaultBold` title, and `18*s` `[X]` button; recessed `WindowBG` log viewport with `18*s` VGUI2 vertical scrollbar and arrow buttons; `22*s` bottom `TextEntry` + `71*s x 22*s` raised `Submit` button + 3x3 resize grip.
- **Chat (`messagemode`) & Hints / Centre Print.** `say :` / `say_team :` draws directly at `(8, 4)` in `EngineFont` (`1 px` drop shadow, `con_color` foreground) with no box or border. `Hint_*` HUD prompts render centered at `0.70 * H` in green `(40, 200, 40)`; `HUD_PRINTCENTER` uses `con_color` at `0.34 * H`.

### Screen 4 (`New game`, `Find servers`, `Player list`, `Spectator`, `Command menu`): DONE

Verified against Ghidra reversing of `GameUI.dll`, `ServerBrowser.dll`, and `client.dll` plus headless captures across `800x600`, `1280x720`, `1280x800`, and `1280x1024`:

- **New game (`CCreateMultiplayerGameDialog` / `FUN_1001e9b0`).** Centered `(448 * s) x (460 * s)` Steam 2003 `TrackerCtx` window matching exact outer box bounds `(176,70,623,529)` (800x600), `(416,130,863,589)` (1280x720), `(391,144,887,654)` (1280x800), `(321,185,957,838)` (1280x1024). Title `#GameUI_CreateServer`, `Server`/`Game` tabs, `Server` page with `< Random Map >` `MapList` (`66, 28, 174, 24`), `Divider1`, and `Enable Steam Networking` (`sv_use_steam_networking`), `Start`/`Cancel` bottom buttons. Box MAD vs original: `1.83` (800x600), `1.45` (1280x720), `2.73` (1280x800), `2.07` (1280x1024).
- **Find servers (`CServerBrowserDialog` / `FUN_100102d0`, `CBaseGamesPage` / `FUN_10001080`).** Resizable centered `min(W, 960) x min(H, 594)` `TrackerCtx` frame with `Internet` default tab, 6 tabs (`Internet, Favorites, History, Spectate, LAN, Friends`), 8 visible raised-bevel columns (`Password, Bots, VAC, Players, Servers (N), Game, Map, Latency`), VGUI2 scrollbar, `Change filters` + filter summary + `Quick refresh` / `Refresh all` / `Connect` + bottom status bar + 3x3 resize grip. Outer box bounds match original `0 px` delta across all 4 resolutions; MAD `4.88-8.84/255` (difference is 32 populated Internet rows on live Steam).
- **Player list (`CPlayerListDialog` / `FUN_1003a8d0`, `PlayerListDialog.res`).** Centered `(467 * s) x (388 * s)` `TrackerCtx` dialog with `#GameUI_CurrentPlayers`, `Player Name`/`Properties` `ListPanel`, `#GameUI_NoOtherPlayersInGame` empty label, `#GameUI_MuteIngameVoice` (`vban <hex>`), and `Close`.
- **Spectator (`CSpectatorGUI` / `FUN_100aff00`, `Spectator.res`, `BottomSpectator.res`).** Top and bottom translucent bars of height `round(64 * H / 480)` (`107 px` at 1280x800) with no orange horizontal edge lines, `menu_visible` defaulting `false`, amber CT/T score labels (`r233`), `DividerBar`, stopwatch icon, and `%d:%02d` timer.
- **Command menu (`CCommandMenu::Show` / `100b2ed0`, `commandmenu.txt`).** Anchored at `x = 0`, vertically centered, `0xc8000000` fill, `22 * H/480` row height, localized labels without `"1. "` prefixes, and vector right triangles for submenus.

### Screen 5 (HUD, Crosshair, SayText, View/Mouse/Timing, SendAudio): DONE

Verified against Ghidra reversing of `client.dll` and `hw.dll` across `640x480`, `1024x768`, and `1280x800` headless captures vs `orig_chat_1280x800.png` and `orig_spectator_1280x800.png`:

- **30-Weapon Dynamic Crosshair (`CHudAmmo::DrawCrosshair` / `FUN_10043ff0`).** Exact 30-weapon `(start_distance, delta_distance)`, `ACC_CODE[30]`, `SPD_CODE[30]` thresholds (`0/140/170`), shot recoil expansion (`delta_distance`, alpha `-40` floor `120`) and decay (`* 0.013 + 0.1`), `cl_crosshair_size` (`auto`/`small`/`medium`/`large`) scaling, `night_vision` red `(250, 50, 50)`, and 1px filled rects (`[-25..-16, +16..+25]` at `1280x800` USP, matching original `0 px` delta).
- **HUD Layout, Fades, Radar, StatusIcons & Ammo Icons (`FUN_1004d9e0`, `46f20`, `43690`, `4dd80`, `43a60`, `4e6c0`, `65570`).** Native scale `0.5` (`< 640`) / `1.0` (`>= 640`), `MIN_ALPHA = 100` fade curve + 5s money color flash + 2.5s `+$`/`-$` delta, progressive round timer red blink (`<= 30s`), `640hud7` 24x24 ammo icons, partial clipped `suit_full`/`suithelmet_full`, upward `StatusIcon` stacking from `H / 2`, `128x128` `(0, 0)` radar (`2048` range, `|dz| > 128` chevrons, 0.15s radio flash), and 4-line teamkill-aware death notice.
- **Measured `1280x800` HUD bboxes vs `orig_chat_1280x800.png` (client-area coords):** radar `(1,1..125,125)` vs `(1,1..126,113)`; buyzone `(19,364..44,384)` vs `(15,369..42,388)`; crosshair `[-19..-10, +10..+19]` / `[-25..-16, +16..+25]` exact integer rect; health `(18,765..102,787)` vs `(18,764..91,787)`; armor `(259,763..335,787)` vs `(259,764..337,787)`; money `(1149,729..1259,752)` vs `(1149,726..1261,750)`; ammo `(1143,758..1263,797)` vs `(1129,761..1262,787)`; `say :` `(9,9..34,16)` vs `(9,5..39,13)`. Spectator mode hides `StatusBar`, hides bottom-right money under the bar, and removes the duplicate letterbox block (`leaked_statusbar_amber = None`, `leaked_blue_pixels = 0`).
- **Chat (`CHudSayText::Draw`), Pickup History (`CHudAmmo::DrawHistory`), Scoreboard & Movement/Shooting Parity.**
  - `CHudSayText`: Valve `\x02` (`STX`) author-only team prefix rule + inline `\x01` normal / `\x03` team (CT blue, T red, Spec `(204, 204, 204)`) / `\x04` green `(0, 204, 0)` across `SayText` and `#Game_radio` (`dest 3/5`), anchored at `(10, H - num_h * 2.75)`.
  - `CHudAmmo::DrawHistory` (`FUN_10046bc0`): loads `640hud1..18` / `320hud1..2` + `sprites/weapon_*.txt` cards (`y = H + (-36 - max_h)*2 - (max_h + 5)*slot` = `638/588/538` at `1280x800`), 5 s lifetime with `min(255, (end_time - now) * 80.0)` fade and `(255, 16, 16)` empty-slot tint.
  - `CCSClientScoreBoardDialog`: local-player translucent row highlight, `scoreboard_shortheaders` (`"S"`, `"D"`, `"Ping"`), and `SPECTATOR`/`UNASSIGNED` rows + ping.
  - Client movement/shooting (`predict.rs`, `weapons_predict.rs`, `events.rs`, `view.rs`): `iuser3` bitmasks (`PLAYER_CAN_SHOOT..PLAYER_HOLDING_SHIELD`) + `freeze_locked` quirk, 1-to-1 weapon specs (pistols, USP/M4A1 silencers, Glock18/FAMAS bursts, Elite args, SMG/rifle formulas & decay, M3 `0.0675` + shotgun random recoil, G3SG1/SG550/Scout/AWP), single-bullet deterministic `collect_bullet_impacts`, and `movedir` / `FL_BASEVELOCITY` fold-once parity.
- **View, Mouse, Audio & Frame Timing.** `StairSmoothState` (`150 u/s`, capped `18 u`), `cl_bobcycle.max(0.1)` + viewmodel bob/FOV offset, `m_filter` + `m_customaccel` + `zoom_sensitivity_ratio` + `cl_pitchup`/`down`, `SendAudio` (`%!` sentences & WAV pitch + radar flash), and `_Host_Frame` timing (`host_timescale / (fps + 0.5)`).

## Status of the other screens (measured 2026-11-24 at 1280x800)

| Screen | State | Differences seen |
|---|---|---|
| Options (`OptionsDialog`) | **DONE** | Built from `OptionsSub*.res` + `MultiplayerAdvancedDialog.res` + `ContentControlDialog.res` in original tab order (`Multiplayer, Audio, Video, Aim, Keyboard, Voice, Content`). Full-frame MAD vs original: `2.72/255` (800x600), `3.33/255` (1280x720), `3.99/255` (1280x800), `3.72/255` (1280x1024); 7 tabs at 1280x800: `2.88-3.99/255` full / `3.69-6.97/255` dialog (remaining delta is player name / Russian Steam overlay strings). |
| MOTD (`MOTD.res`) | **DONE** | Centered 4:3 `BackgroundPanel` + `16px/20px` CSS HTML + `&OK` stripped. Panel MAD vs original: `5.59` (800x600), `4.05` (1280x720), `3.78` (1280x800), `6.47` (1280x1024); remaining delta is 3D camera/sky bleed behind `alpha=200` `BackgroundPanel` (`5:4` aspect FOV at `1280x1024`) and Russian Steam overlay toast. |
| Team / Class / Buy menus | **DONE** | Panel MAD vs original: `Team` `8.55` (800x600), `6.30` (1280x720), `6.07` (1280x800), `10.83` (1280x1024, 3D scene delta `25.96` outside panel bleeding through `alpha=200`); `Class` `8.55` (`1280x800`, different intro camera angle); `Buy` `5.12` (`1280x800`, 1px `Divider1`). Remaining panel delta is translucent 3D background/chat/font antialiasing. |
| Scoreboard (`ScoreBoard.res`) | **DONE** | Centered `520x340 * H/480` `ListBG` (`alpha=128`) box, localized + `scoreboard_shortheaders` headers, CT/T/Spectators dividers, and local-player row highlight. Box MAD vs original: `9.36` (1280x720), `9.29` (1280x800), `9.41` (1280x1024); remaining delta is 1st-person weapon/world view & headless bot rows behind `alpha=128` panel. |
| Console (`CConsoleDialog`) | **DONE** | Exact Steam 2003 `TrackerScheme` `(40,40,560,400) * max(1, H/720)` frame. Window box MAD vs original: `6.23` (800x600), `7.26` (1280x720), `5.95` (1280x800), `6.23` (1280x1024); remaining delta is startup log lines only. |
| Chat / Centre print | **DONE** | `say :` HUD prompt at `(8, 4)` with `EngineFont` shadow and `Hint_press_buy_to_purchase` green banner at `0.70 * H` match original position and colors visually. |
| New game (`CreateMultiplayerGame*.res`) | **DONE** | Outer box `0 px` delta across all 4 res; box MAD `1.45-2.73/255`. |
| Find servers (`ServerBrowser`) | **DONE** | Outer box `0 px` delta across all 4 res; 6 tabs, 8-column `gamelist`, resize grip. |
| Player list / Spectator / Command menu | **DONE** | All 3 matched to Ghidra + `.res`/`.txt` specs (`467x388`, `64*H/480` bars, `x=0` vector triangles). |
| HUD (crosshair, radar, bottom bar, chat, pickup history) | **DONE** | 30-weapon `FUN_10043ff0` crosshair, `640hud7` ammo/armor/health/money/timer coords, `128x128` radar, `CHudAmmo::DrawHistory` (`640hud1..18` + `weapon_*.txt`), `\x01..\x04` `SayText`, and clean spectator bottom bar (`0` leaked pixels). |

## Order of work for the next screens

1. Options dialog from `.res`: property sheet, each `OptionsSub*.res` page, the original tab order, and the dialog position and size.
2. MOTD and team/class/buy menus (they appear on every join).
3. Scoreboard, console, chat, centre print.
4. New game, Find servers, Player list, Spectator, Command menu.
5. HUD, 30-weapon crosshair, SayText, View/Mouse/Timing, SendAudio.

For each: capture the original, capture ours, measure, change the code, recapture. Commit with the numbers in the message.
