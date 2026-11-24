# Visual Parity Checklist (Stage 1)

Acceptance baseline against original Counter-Strike 1.6 (GoldSrc Protocol 48) on `de_dust2` intro camera 1 (`1280x720`):

- [x] **Finding 1 — GoldSrc Gamma / Lightmap LUT Parity**:
  - `r_lightmap 0` renders normal shaded geometry (`app.rs`, `wasm_entry.rs`), not fullbright `0.5`.
  - Exact Xash3D FWGS `BuildGammaTable` (`texgammatable`, `lightgammatable`), `R_BuildLightMap`, `R_AddDynamicLights`, `CL_RunLightStyles`, `R_RecursiveLightPoint`, `R_EntityDynamicLight`, and `R_StudioLighting` (`SHADE_LAMBERT = 1.4953241`) replace ad-hoc shader `pow()` / `* 2.0`.
  - Acceptance: sunlit sandstone `right_wall` ROI `(1000, 260)-(1200, 330)` in `1280x720` Hor+ widescreen (with wide diagnostic `(900, 300)-(1200, 500)`) mean RGB within `+/- 4` per channel of Original `(113, 96, 65)` at retail `brightness "1.0"`. Measured `(112.6, 95.2, 65.0)` — Delta `(-0.4, -0.8, +0.0)` (**PASS**).
- [x] **Finding 2 — Texture Mipmapping & Resampling Parity**:
  - NPOT textures resample via `GL_SetTextureDimensions` (`gl_round_down 3`, `gl_picmip 0`, `gl_max_size 512`) + `Image_Resample32Lerp` and receive full `GL_BuildMipMap` 2x2 box-filter chains AFTER `texgammatable`.
  - `gl_ansio` defaults to `1` (OFF) and `gl_msaa` defaults to `1`.
- [x] **Finding 3 — Left Rock & Tool/Random-Tiled Brush Textures**:
  - `"black"` removed from `is_tool_texture`.
  - `-0..-9` static random tiling (`R_TextureAnimation` / `MOD_FRAMES = 20`) implemented in both batch and world-PVS face builders.
  - Visual inspection (`target/visual-diff/final/ours_cam1.png`) confirms textured lit rock on `de_dust2` cam 1 left slope with zero dark flat polygons.
- [x] **Finding 4 (Stage 1 Subset) — CLI `--exec`/`+connect`, Intro HUD `HideWeapon`, `#Token` Localization**:
  - Multi-token `--exec` / `-e`, `+cmd` GoldSrc startup flags, `--headless`, `--resolution`, `-w`/`-h`, `--wait-camera`, `--wait-seconds`.
  - `is_client_alive` (`FL_SPECTATOR` / `WEAPON_SUIT` / `iuser1`) + `compute_hud_visibility` + spectator-bar `observer_mode` gating show ONLY `$ 800` money on intro/team-select (no health/armor/radar/crosshair/timer/spectator bars).
  - `#Token` lookup via `LocalizationTable::resolve_with_titles` (`TitlesTable` fallback) resolves `#Game_connected`, `#Game_join_terrorist`, `#Team_Select_Spect`.
  - _(Deferred to Stage 2: MOTD VGUI2 `.res` HTML/briefing panel fidelity.)_

## Measured Runs (`cargo xtask visual-diff`)

### Run 1 — Retail default (`brightness 1.0`, matching installed `cstrike/config.cfg`)
Command: `cargo xtask visual-diff --build --map de_dust2 --out target/visual-diff/final --port 27099` -> **PASS** (exit `0`)

| ROI | Box | Mean R | Mean G | Mean B | Std Luma |
|---|---|---|---|---|---|
| `right_wall` | `(1000,260)-(1200,330)` | `112.6` | `95.2` | `65.0` | `22.3` |
| `right_wall_wide` | `(900,300)-(1200,500)` | `98.8` | `83.7` | `57.9` | `15.7` |
| `left_rock` | `(40,480)-(320,680)` | `129.5` | `100.0` | `63.1` | `23.4` |
| `ground_sand` | `(500,560)-(850,700)` | `155.0` | `137.2` | `79.4` | `38.6` |
| `crates` | `(600,450)-(760,580)` | `198.8` | `156.6` | `85.2` | `21.7` |
| `sky` | `(400,40)-(800,180)` | `141.6` | `127.1` | `119.5` | `24.5` |

- `right_wall` vs Original `(113, 96, 65) +/- 4`: **`(112.6, 95.2, 65.0)`** | Delta **`(-0.4, -0.8, +0.0)`** | **PASS**

### Run 2 — Diagnostic (`brightness 0.0`)
Command: `cargo xtask visual-diff --map de_dust2 --brightness 0.0 --out target/visual-diff/final-b0 --port 27098` -> exit `1`

| ROI | Box | Mean R | Mean G | Mean B | Std Luma |
|---|---|---|---|---|---|
| `right_wall` | `(1000,260)-(1200,330)` | `102.7` | `86.8` | `52.3` | `20.2` |
| `right_wall_wide` | `(900,300)-(1200,500)` | `82.2` | `69.6` | `41.8` | `15.8` |
| `left_rock` | `(40,480)-(320,680)` | `126.4` | `97.6` | `57.5` | `22.5` |
| `ground_sand` | `(500,560)-(850,700)` | `152.3` | `134.7` | `74.0` | `42.0` |
| `crates` | `(600,450)-(760,580)` | `198.3` | `156.2` | `81.6` | `23.3` |
| `sky` | `(400,40)-(800,180)` | `141.3` | `126.8` | `119.1` | `25.3` |

- `right_wall` vs Original `(113, 96, 65) +/- 4`: **`(102.7, 86.8, 52.3)`** | Delta **`(-10.3, -9.2, -12.7)`** | **FAIL** (confirms retail capture used `cstrike/config.cfg` `brightness "1.0"`).
