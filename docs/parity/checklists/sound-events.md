# Counter-Strike 1.6 Gameplay Sound Events Map

This document maps CS 1.6 gameplay sound events, wire mechanisms, sample/sentence names, mixer/DSP behaviors, and automated verification tests.

## Round Lifecycle & Radio Events

| Gameplay Event | Wire Mechanism | Sample / Sentence Name | Automated Test |
| --- | --- | --- | --- |
| Round freeze end / start | `SendAudio` | `%!MRAD_LOCKNLOAD`, `%!MRAD_GO`, `%!MRAD_MOVEOUT`, `%!MRAD_LETSGO` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| Terrorists Win / CT Win / Round Draw | `SendAudio` | `%!MRAD_TERWIN`, `%!MRAD_CTWIN`, `%!MRAD_ROUNDDRAW` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| Bomb planted announcement | `SendAudio` | `%!MRAD_BOMBPL` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| Bomb defused / Target saved | `SendAudio` | `%!MRAD_BOMBDEF` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| Hostage rescued / All hostages rescued | `SendAudio` | `%!MRAD_RESCUED`, `%!MRAD_ALLHOSTAGES` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| VIP assassinated / escaped | `SendAudio` | `%!MRAD_VIP` / VIP win radio | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| `radio1` / `radio2` / `radio3` menus | `SendAudio` | `radio/*.wav` (`coverme`, `takepoint`, `roger`, `fireinhole`, `enemyspot`, etc.) | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |

## C4 Bomb & Hostage Positional Events

| Gameplay Event | Wire Mechanism | Sample Name | Automated Test |
| --- | --- | --- | --- |
| C4 plant arming & planted | `svc_sound` (`CHAN_WEAPON` / `CHAN_ITEM`) | `weapons/c4_plant.wav` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| C4 timer ticks (accelerating cadence) | `svc_sound` | `weapons/c4_beep1.wav` .. `c4_beep5.wav` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| C4 disarming & disarmed | `svc_sound` | `weapons/c4_disarm.wav`, `weapons/c4_disarmed.wav` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| C4 detonation | `svc_sound` | `weapons/c4_explode1.wav` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| Hostage use / unuse / pain / rescue | `svc_sound` | `hostage/hos1.wav` .. `hos5.wav` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |

## Weapons, Footsteps & World Acoustics

| Gameplay Event | Wire Mechanism | Sample / Behaviour | Automated Test |
| --- | --- | --- | --- |
| Weapon fire, reload, dryfire, knife, pin | `svc_event` (`events/*.sc`) & `svc_sound` | `weapons/*.wav`, 3D rolloff + stereo pan + pitch | `goldsrc_distance_attenuation_and_stereo_pan`, `goldsrc_fractional_pitch_shifting` |
| Grenade bounce / flashbang / HE / smoke | `svc_sound` | `weapons/he_bounce-1.wav`, `flashbang-1.wav`, `sg_explode.wav`, `explode3.wav` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| Material footsteps (concrete, metal, vent, dirt, grate, tile, snow, ladder, water) | Predicted / `svc_sound` (`PM_PlayStepSound`) | `player/pl_step1..4.wav`, `pl_metal1..4.wav`, `pl_duct1..4.wav`, `pl_ladder1..4.wav`, `pl_slosh1..4.wav`, `pl_snow1..6.wav` | `gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer` |
| `env_sound` acoustic zone transition | `svc_roomtype` | 29 GoldSrc `room_type` DSP presets (`0..=28`) | `roomtype_29_presets_modulate_wet_reverb` |
| Wall occlusion & underwater muffling | Trace occlusion & `waterlevel >= 3` | Lowpass filter + `0.7` volume scaling when occluded; underwater cutoff | `goldsrc_wall_occlusion_attenuates_sound`, `underwater_muffling_lowpass_filters_audio` |
| Ambient loops (`ambient_generic`) | `svc_spawnstaticsound` | WAV `cue`/`smpl` loop points | `wav_cue_and_smpl_loop_points_wrap` |
| Music & VOX / Fade | `svc_cdtrack`, `svc_soundfade`, `mp3` | `media/gamestartup.mp3` / `media/*.mp3`, master fade recovery | `soundfade_attenuates_and_recovers_master_gain`, `cdtrack_and_mp3_console_commands`, `real_mp3_menu_music_decode_if_installed` |
