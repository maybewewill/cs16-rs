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
  - `82*s` when the width is below 1280, `115*s` from 1280 up.
  - Assumption: the threshold between 1024 and 1280 was not measured.
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

## Status of the other screens (measured 2026-11-24 at 1280x800)

| Screen | State | Differences seen |
|---|---|---|
| Options (`OptionsDialog`) | **DONE** | Built from `OptionsSub*.res` + `MultiplayerAdvancedDialog.res` + `ContentControlDialog.res` in original tab order (`Multiplayer, Audio, Video, Aim, Keyboard, Voice, Content`). Full-frame MAD vs original: `2.72/255` (800x600), `3.33/255` (1280x720), `3.99/255` (1280x800), `3.72/255` (1280x1024); 7 tabs at 1280x800: `2.88-3.99/255` full / `3.69-6.97/255` dialog (remaining delta is player name / Russian Steam overlay strings). |
| New game (`CreateMultiplayerGame*.res`) | not compared | |
| Find servers (`ServerBrowser`) | not compared | |
| Console | not compared | The original GameUI console is a TrackerScheme frame (seen top-left in captures). |
| MOTD | differs | The original is the VGUI2 MOTD (black panel, orange text, HTML link, `OK` button, a 1000x750 frame on 1280x800). Ours is a smaller Steam-style frame with one line. |
| Team / class / buy menus | not compared | The original team menu shows the map briefing on the right (`maps/<map>.txt`). |
| Scoreboard, spectator, command menu | not compared | |
| HUD | partly | It is drawn from `hud.txt` sprites; compare positions and fades at 640x480, 1024x768 and 1280x800. |

## Order of work for the next screens

1. Options dialog from `.res`: property sheet, each `OptionsSub*.res` page, the original tab order, and the dialog position and size.
2. MOTD and team/class/buy menus (they appear on every join).
3. Scoreboard, console, chat, centre print.
4. New game, Find servers, Player list, Spectator, Command menu.

For each: capture the original, capture ours, measure, change the code, recapture. Commit with the numbers in the message.
