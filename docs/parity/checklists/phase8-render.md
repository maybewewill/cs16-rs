# Phase 8 — Client Renderer & Visuals Verification Checklist

Branch: `phase8-render`
Completed: 2026-11-24

## Scope Covered
1. **Protocol 48, CVar & ClientCore Foundations**:
   - All **59/59 `TE_*`** temp entities have typed builders, skip-length verification, typed `TempEffect` decoders (`cs-proto48/src/tempent.rs`), and full `TempEnts` handlers (`cs-clientcore/src/tempents.rs`). Verified stale 16B notes removed (`TE_BEAMSPRITE` 16B, `TE_EXPLODEMODEL` 13B).
   - Typed user messages (`Fog`, `Flashlight`, `FlashBat`, `ReceiveW`, `Brass`) + `svc_lightstyle`, `svc_decalname`, `svc_particle`, `svc_spawnstatic`, and `svc_customization`.
   - Phase 8 CVars + `"f" -> "impulse 100"` default bind (`default_fov`, `viewmodel_fov`, `r_lightmap`, `r_decals`, `gl_ansio`, `gl_msaa`, `r_software_mode`, `cl_bob*`, `cl_roll*`, `cl_viewmodel_lag`).
2. **Asset Upgrades (`cs-assets`)**:
   - BSP v30: `SURF_FLAG_SCROLL`, `FaceLightmapInfo` + `LightmapAtlas::rebuild_for_styles`, `TextureAnimTable` (`+0..+9` 5 Hz & `+A..+J` toggles), PVS `visible_world_face_mask` / `WorldFaceSpan`, `sample_light_point_styled`, `query_faces_in_sphere`.
   - WAD3 Decals: `WadArchive::decode_decal` (`decals.wad` index-alpha `palette[255]` tint vs custom blue-keyed RGB sprays).
   - StudioModel v10: `STUDIO_NF_{FLATSHADE, CHROME, ADDITIVE, MASKED}`, `skin_families`, bone controllers, attachments, sequence events, multi-blends, `"Bip01 Spine"` gait split, external `IDSQ` sequence groups (`*01.mdl`).
3. **GPU Renderer (`cs-client` + `cs-web`)**:
   - `gamma.rs`: exact `gamma`/`texgamma`/`brightness`/overbright curve + optional `r_software_mode` 4×4 Bayer RGB565 dithering.
   - `lighting.rs`: all 6 `gl_texturemode` values, POT mipmaps (WebGL2-safe), `gl_ansio`, `DLightPool`, and `WorldCollision` flashlight spotlight traces.
   - `bsp_renderer.rs` + `skybox.rs`: live lightstyles (`write_texture`), PVS leaf index caching, `GL_EXP2` fog, water ripple/wave, scroll UVs, sky horizon fog, MSAA `1 | 4`.
   - `mdl_renderer.rs`: all 6 `EntRenderMode`s, `GlowShell`/`Explode`/`LightMultiplier`, `STUDIO_NF_*`, bodygroups, skins, bone controllers, gait/crossfade blending, floor `R_LightPoint` + dynamic lights/spotlights, opaque/translucent split pass.
   - `decal_renderer.rs`: Sutherland–Hodgman BSP face polygon clipping, `r_decals` ring buffer, `decals.wad` + custom spray support.
   - `particles.rs` + `fx_renderer.rs` + `sprite_renderer.rs`: GoldSrc 256-color palette, all `ParticleEffect` bursts, `ReceiveW` rain/snow weather, `BeamSystem` (noisy lightning, rings, follow trails, `KillBeam`), `SpriteRenderer` with `WorldCollision` glow occlusion and `ClampMinScale`.
   - `view_effects.rs` + `camera.rs` + `ui/hud.rs`: `cl_bob*`, `cl_roll*`, `V_CalcViewModelLag`, `ScreenShake`, `ScreenFade` (`FFADE_MODULATE`/`STAYOUT`/`PURGE`), underwater tint/FOV warp, Hor+ widescreen FOV projection, `TE_TEXTMESSAGE`, and flashlight battery HUD.

## Parity Closures
- `docs/parity/render.toml`: **42 / 42** items `tested`.
- `docs/parity/tempents.toml`: **59 / 59** items `tested`.
- `docs/parity/usermsg.toml`: `Fog`, `Flashlight`, `FlashBat`, `ReceiveW`, `Brass` `tested`.
- `docs/parity/wire.toml`: `svc_lightstyle`, `svc_decalname`, `svc_particle`, `svc_spawnstatic`, `svc_customization` `tested`.

## Manual Verification Checkpoints (Optional Visual QA)
- [ ] Inspect `de_dust2` dynamic lighting, flashlight beam, and weapon glow shell in desktop `cs-client` and browser `cs-web`.
- [ ] Toggle `r_software_mode 1`, `gl_msaa 4`, `gl_ansio 8`, and `gl_texturemode GL_NEAREST` live via console.
- [ ] Submerge in `de_aztec` / `cs_siege` water to verify wave surface warp and underwater blue tint + FOV ripple.
