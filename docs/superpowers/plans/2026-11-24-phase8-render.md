# Phase 8 — Client Renderer & Visuals (`phase8-render`) Plan

Branch: `phase8-render` from `main` (`eb7073e`).
Goal: Full GoldSrc visual & client renderer parity across `cs-proto48`, `cs-cvar`, `cs-clientcore`, `cs-assets`, `cs-client`, and `cs-web`, closing **42/42 `render.toml`** and **59/59 `tempents.toml`**, passing all 8 CI gates, and fast-forward merging locally into `main`.

## Steps
1. **Proto48, CVars & ClientCore Foundation**: all 59/59 `TE_*` typed builders/decoders, `Fog`/`Flashlight`/`FlashBat`/`ReceiveW`/`Brass` typed `CsUserMsg` codecs, Phase 8 CVars + `"f" -> "impulse 100"`, `lightstyles[64]`, `FogState`, `custom_decals`, `EF_*` flags, `Latched` gait state, `Prediction` watertype/flags/effects, and full `TempEnts` simulation.
2. **Asset Parsers Upgrade**: `SURF_FLAG_SCROLL`, `FaceLightmapInfo` + `LightmapAtlas::rebuild_for_styles`, `+0..+9`/`+A..+J` animated/toggleable BSP textures, `visible_world_face_mask` PVS, raycast `sample_light_point_styled`, `query_faces_in_sphere`, `WadArchive::decode_decal` (`decals.wad`/`tempdecal.wad`), and full `StudioModel` (skin families, bone controllers, attachments, seq events, multi-blends, gait spine split, external `IDSQ` seqgroups).
3. **Core Renderer (`gamma`, `lighting`, `bsp`, `skybox`)**: exact `gamma`/`texgamma`/`brightness`/overbright + `r_software_mode` Bayer RGB565 dither, `TextureFilterMode` + POT mipmaps + `gl_ansio`, `DLightPool` + flashlight trace spotlights, live lightstyles, water warp/wave, scroll textures, `GL_EXP2` fog, PVS leaf culling, and configurable MSAA (`1 | 4`).
4. **MDL, Decals, Particles, Beams & Sprites**: full `MdlRenderer` (blending, controllers, gait, attachments, chrome/additive/masked/flatshade, `GlowShell`, floor `R_LightPoint`), `DecalRenderer` (Sutherland–Hodgman BSP polygon clipping + `decals.wad`/`tempdecal.wad` sprays), `ParticleSystem` + `BeamSystem` + `FxRenderer`, and `SpriteRenderer` (trace-occluded glow sprites + `ClampMinScale`).
5. **View Effects, Camera, App + Web Wiring, & Integration Tests**: `view_effects.rs` (`cl_bob*`, `cl_roll*`, `V_CalcViewModelLag`, `ScreenShake`, `ScreenFade`, underwater tint/FOV warp), camera/viewmodel Hor+ projection, `app.rs` & `cs-web` (`state.rs` + `wasm_entry.rs`) wiring, flashlight HUD, and `phase8_render.rs`.
6. **Parity Ledgers, 8-Gate CI Verification, & Local FF-Merge**.
