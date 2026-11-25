# Third-party notices

`cs16-rs` contains code ported from the following projects. Code ported from them is
distributed in this repository under GPL-3.0-or-later, the licence of this project; see
`docs/LICENSING.md`.

## ReHLDS

Upstream: https://github.com/rehlds/ReHLDS

The repository-level `LICENSE` of ReHLDS is MIT (notice below); the engine source files listed
and the engine headers `net.h`, `server.h` and `common/mathlib.h` (master, read 2026-11-21; `unicode_strtools.cpp` read 2026-11-22; in the pm_shared port also `pmove.cpp`, `pmovetst.cpp`, `sv_pmove.cpp`, `model.cpp` and `tmessage.cpp`, read 2026-11-22)
carry a header stating GPL v2-or-later with an exception permitting linking with Valve's
Half-Life engine and game libraries. The remaining headers listed (`common/const.h`,
`common/usercmd.h`, `common/entity_state.h`, `common/qlimits.h`, `public/rehlds/edict.h`) carry
Valve's own notices instead; see the last rows. Files not listed here were not examined.

Files read and ported from (the behaviour was ported function by function; each ported
function names its source file, function and the date read in its doc comment):

| File (`rehlds/engine/` unless noted) | What was taken |
|---|---|
| `sv_user.cpp` | `SV_AddLinksToPM_`, `SV_IsCollidableInPM`, `SV_CopyEdictToPhysent` (which entities block a player's movement trace), `SV_EstablishTimeBase` and `SV_RunCmd` (the time base of a move), `SV_FinishPMove` and `SV_ProcessImpacts` (impacts after a move), and lag compensation: `SV_IsLagCompensationActive`, `SV_GetUnlagTargetTime`, `SV_FindUnlagFrames`, `SV_FlagNonInterpolatedPlayers`, `SV_CalcUnlagFraction`, `SV_UnlagCheckTeleport`, `SV_SaveTruePositions`, `SV_ApplyAdjustedPositions`, `SV_SetupMove`, `SV_RestoreMove`, `SV_GetTrueOrigin`, `SV_GetTrueMinMax`, plus the ping bookkeeping of `SV_ExecuteClientMessage`, `SV_CalcClientTime` and `SV_ParseMove`. The player movement of the pm_shared port (read 2026-11-22): `SV_SetupPMove` and `SV_FinishPMove` (the `playermove_t` fields filled before and copied back after `PM_Move`, `cs-game`'s `pmove.rs`), `SV_RunCmd` in full (the order: the moving-ground check, `CmdStart`, `PreThink`, `PM_Move`, the relink, `SV_ProcessImpacts`, `PostThink`, `CmdEnd`; the per-command `svc_newmovevars` resend), `SV_AddLinksToPM` (physents by area-tree order, the 256-unit reach, the 600 cap), the `edgefriction`, `mp_footsteps`, `sv_rollangle` and `sv_rollspeed` cvars and their defaults, and `SV_ParseMove`'s rule for which clients run commands. |
| `world.cpp` | `SV_ClearWorld`, `SV_CreateAreaNode`, `SV_UnlinkEdict`, `SV_FindTouchedLeafs`, `SV_LinkEdict`, `SV_TouchLinks` (entity linking and trigger touches); `SV_Move`, `SV_ClipToLinks`, `SV_ClipMoveToEntity`, `SV_SingleClipMoveToEntity`, `SV_HullForEntity`, `SV_HullForBsp`, `SV_HullForBox`, `SV_InitBoxHull`, `SV_MoveBounds`, `SV_LinkContents` (entity traces). |
| `sv_phys.cpp` | `SV_Physics` and the movetype functions (`SV_Physics_None`, `_Follow`, `_Noclip`, `_Toss`, `_Bounce`, `_Step`, `_Pusher`), `SV_RunThink`, `SV_CheckVelocity`, `SV_AddGravity`, `SV_ClipVelocity`, `SV_FlyMove`, `SV_PushEntity`, `SV_PushMove`, `SV_PushRotate`, `SV_CheckMovingGround`, `SV_CheckWater`, `SV_CheckWaterTransition`, `SV_Submerged`, `SV_Impact`. In the pm_shared port: `SV_CheckMovingGround` as the engine runs it before each command, and the `sv_*` cvar defaults of this file the movevars carry. |
| `sv_move.cpp` | `SV_CheckBottom`, `SV_movetest`, `SV_movestep`, `SV_StepDirection`, `SV_FlyDirection`, `SV_FixCheckBottom`, `SV_NewChaseDir2`, `SV_MoveToOrigin_I` (entity walking). |
| `sv_main.cpp` | `SV_WriteEntitiesToClient` and `SV_SendClientDatagram` (the overflow rules), `SV_Frame_Internal` (frame order), `SV_ActivateServer` (settle passes), `SV_SpawnServer` (world edict, PAS at activation), `SV_SetMoveVars`, `SV_StartSound_internal`, `SV_Multicast`, `SV_ValidClientMulticast`, the `cl_lw` / `cl_lc` userinfo use. In the pm_shared port: `SV_SetMoveVars` (every `movevars_t` field from its own cvar), the `sv_spectatormaxspeed`, `sv_wateraccelerate`, `sv_waterfriction`, `sv_skycolor_*` and `sv_skyvec_*` cvars and their defaults, and `SV_StartSound_internal` for `PM_SV_PlaySound` (the sound that skips the mover). |
| `pr_cmds.cpp` | `PF_setorigin_I`, `PF_setsize_I`, `PF_setmodel_I`, `PF_traceline_DLL`, `TraceHull`, `TraceModel`, `PF_walkmove_I`, `PF_droptofloor_I`, `PF_lightstyle_I`. In the pm_shared port: `TraceTexture` and `SurfaceAtPoint` (`crates/cs-engine/src/texture.rs`). |
| `pr_edict.cpp` | `ED_Alloc_internal` (the edict reuse rule). |
| `host.cpp` | `Host_FilterTime` (the 0.25 s frame clamp). |
| `net_chan.cpp`, `net_ws.cpp` | The netchan of `crates/cs-proto48/src/netchan.rs` (sequence numbers, fragmentation, `Netchan_Transmit` and its overflow rule for unreliable data next to reliable data, the packet header). |
| `delta.cpp` | The delta compression of `crates/cs-proto48/src/delta.rs`: `DELTA_MarkSendFields` equality, `DELTA_WriteDelta`, `DELTA_ParseDelta`, the built-in `g_MetaDelta`. |
| `common.cpp` | The bit and byte stream reader and writer of `crates/cs-proto48/src/bitbuf.rs` (`MSG_*`, including the overflow clamping) and `COM_Munge`, `COM_Munge2`, `COM_Munge3` and their inverses (`crypto.rs`). |
| `crc.cpp` | `CRC32`, `COM_BlockSequenceCRCByte`, `CRC_MapFile` (`crypto.rs`). |
| `info.cpp` | The `\key\value` info string parser and serializer (`crates/cs-proto48/src/info.rs`). |
| `cmodel.cpp` | `CM_CalcPAS` (the PAS built from the PVS rows). |
| `mathlib.cpp` | `AngleVectors`, `AngleVectorsTranspose`. |
| `unicode_strtools.cpp` | `Q_StripUnprintableAndSpace` (with the `Q_iswprint` table it uses): the behaviour of the chat text clean-up of `crates/cs-game/src/chat.rs`, ported by function (master, read 2026-11-22). The file begins with the GPL v2-or-later header with the linking exception, like the other engine sources; it is the ReHLDS file `rehlds/engine/unicode_strtools.cpp`, not a ReGameDLL_CS file. |
| `pmove.cpp` | The player hull tables (`player_mins`, `player_maxs`, `pm_hullminsmaxs`) and the physent bookkeeping of the engine side of `pmove` (`crates/cs-pm/src/physent.rs`, `crates/cs-engine/src/pm.rs`), read 2026-11-22. |
| `pmovetst.cpp` | `PM_RecursiveHullCheck`, `PM_HullPointContents`, `PM_HullForBox`, `PM_HullForBsp`, `_PM_PlayerTrace`, `_PM_TestPlayerPosition`, `PM_LinkContents`, `PM_PointContents`, `PM_TraceModel` (`crates/cs-pm/src/collision.rs`, `crates/cs-pm/src/physent.rs`), read 2026-11-22. |
| `sv_pmove.cpp` | `PM_SV_TraceTexture` and `PM_SV_PlaySound` (what the engine does when `pm_shared` asks for the texture under the player or plays a sound), read 2026-11-22. The other `PM_SV_*` callbacks only forward to the functions above. |
| `model.cpp` | `CalcSurfaceExtents` and the overrides `Mod_LoadFaces` applies to `texturemins` and `extents` (for `TraceTexture`, `crates/cs-engine/src/texture.rs`), read 2026-11-22. |
| `tmessage.cpp` | `memfgets` (the 510-byte chunking of `PM_InitTextureTypes` reading `sound/materials.txt`, `crates/cs-pm/src/pm/materials.rs`), read 2026-11-22. |
| `r_studio.cpp` | Phase 2D (read 2026-11-22): `SV_HullForStudioModel`, `SV_StudioSetupBones` (the bone pose of a sequence frame with the blend and the controllers) and `R_StudioHull` (the hitbox hulls), ported function by function into `crates/cs-engine/src/studio.rs`, with the hitgroup of the clipped hitbox. `SV_CheckSphereIntersection` is not ported. The file carries the GPL v2-or-later header with the linking exception. |
| `pr_cmds.cpp`, `sv_user.cpp`, `sv_main.cpp` (phase 2E, read 2026-11-23) | Engine fake clients (`crates/cs-engine` + `crates/cs-engine/tests/fake_clients.rs`): `CreateFakeClient_internal`, `PF_RunPlayerMove_I`, `PF_MessageEnd_I` and `EV_Playback` fake-client skips (`pr_cmds.cpp`); `SV_RunCmd` fake-client paths skipping `SV_SetupMove`/`SV_RestoreMove`/movevars and empty `SV_PreRunCmd` (`sv_user.cpp`); `SV_InactivateClients`, `SV_CheckTimeouts`, `SV_SendClientMessages`, `SV_UpdateToReliableMessages`, `SV_GetFakeClientCount` (`sv_main.cpp`). All three files carry the GPL v2-or-later header with the linking exception. |
| `sv_main.cpp`, `sv_user.cpp`, `pr_cmds.cpp`, `world.cpp`, `sv_phys.cpp` (phase 2D, read 2026-11-22) | `SV_WriteClientdataToMessage` (the weapon data of `cl_lw` clients with its delta against the acknowledged frame), `SV_EmitEvents` and `SV_EmitEvents_internal` (the event queue), `SV_CreateResourceList` (events and decals in the resource list), `PF_precache_event` / `EV_Precache_internal`, `EV_Playback` and `PF_playbackevent` (flags, origin and angles filled from the invoker), `PF_DecalIndex`, `PF_sound_I` flags and ambient sounds, `SV_RunCmd` / `SV_ParseMove` (the per-command random seed from the sequence, backups and repeats, and `cl_lw` from the userinfo), `SV_ClipMoveToEntity` with the hit boxes of studio models (`sv_clienttrace`), `SV_HullForEntity`. |
| `decals.cpp`, `decals.h`, `wad.cpp` (phase 2D, read 2026-11-22) | `Decal_Init` and `Decal_MergeInDecals` (the decal table of `decals.wad` and the index `PF_DecalIndex` answers, 15-character names, the table cap) and `W_CleanupName`. The licence headers of these three files were not checked (see `docs/LICENSING.md`). |
| `common/event_flags.h`, `common/event_args.h`, `common/weaponinfo.h`, `common/resource.h`, `engine/delta.h` (phase 2D, read 2026-11-22) | Numeric values and names only: the `FEV_*` flags, the fields of `event_args_t` and `weapon_data_t` (also their offsets for `delta_offsets.rs`), `MAX_EVENTS`-type limits and `RES_FATALIFMISSING`. The licence headers of these files were not checked (see `docs/LICENSING.md`); no code and no other text was copied. |
| `net.h`, `server.h` (engine headers, GPL header) | Buffer size constants only (`NET_MAX_PAYLOAD`, `MAX_DATAGRAM`, the size of `signon_data`). |
| `common/mathlib.h` | Read for the value of `M_PI` (a double literal) in comments only; no constant or code was taken (the engine uses `std::f64::consts::PI`). The file carries the GPL v2-or-later header with the linking exception, like the engine sources; it is not a Valve-noticed header. |
| `common/const.h`, `common/usercmd.h`, `common/entity_state.h` | Numeric values and names only: from `const.h` the `SOLID_*`, `MOVETYPE_*`, `FL_*`, `CONTENTS_*` and `EF_*` constants (the engine's `world.rs`, `collision.rs`, `unlag.rs`, `physics_water.rs`); from `usercmd.h` and `entity_state.h` the field names and types of `usercmd_t`, `clientdata_t` and `entity_state_t` that the delta tables and the protocol need (`cs-proto48/src/delta.rs`). These three files carry Valve LLC's copyright notice (with Id Software's) and restrict use, distribution and modification of the source and of the resulting object code to non-commercial enhancements to products from Valve LLC, and prohibit all other use without Valve's written permission. They do not carry the GPL header of the engine sources. |
| `common/qlimits.h`, `public/rehlds/edict.h` | Numeric values and names only: `MAX_PACKET_ENTITIES` and `MAX_LIGHTSTYLE_SIZE` from `qlimits.h`, `MAX_ENT_LEAFS` from `edict.h` (`world.rs`). Both carry a plain copyright line of Valve LLC (1996-2002, all rights reserved) and no terms of use. |
| `sv_log.cpp`, `sv_main.cpp`, `sv_user.cpp`, `sv_upld.cpp`, `host_cmd.cpp`, `net_chan.cpp`, `common/hltv.h`, `common/cvardef.h` (phase 11, read 2026-11-25) | Phase 11 engine completeness: HL standard logging & `logaddress` UDP streaming (`crates/cs-engine/src/logging.rs`: `sv_log.cpp`, `sv_main.cpp`), RCON challenge/auth/rate limiting/chunked `l` replies and connectionless packets (`crates/cs-engine/src/rcon.rs`, `crates/cs-proto48/src/handshake.rs`: `sv_main.cpp`), `dlfile` path checks, `mp_consistency` 12-bit response index (`SV_ParseConsistencyResponse`), and custom spray upload/`svc_customization` (`crates/cs-engine/src/downloads.rs`: `sv_user.cpp`, `sv_upld.cpp`), IP/AuthID bans & `status full`/`users` (`crates/cs-engine/src/bans.rs`, `console.rs`: `sv_user.cpp`, `host_cmd.cpp`), HLTV/director `DRC_CMD_*`/`HLTV_*` constants and proxy signon/netchan framing (`crates/cs-proto48/src/messages.rs`, `crates/cs-hltv/src/{lib.rs,delay.rs,proxy.rs}`: `common/hltv.h`, `sv_main.cpp`), and `FCVAR_PROTECTED`/`FCVAR_SPONLY` rules (`crates/cs-cvar/src/lib.rs`: `common/cvardef.h`). |

The constants and names taken from the headers with Valve's notice are the numbers the original
client and game libraries and the wire protocol require to match, so that the code interoperates
with them: `SOLID_*`, `MOVETYPE_*`, `FL_*`, `CONTENTS_*`, `EF_*`, `MAX_PACKET_ENTITIES`,
`MAX_ENT_LEAFS`, `MAX_LIGHTSTYLE_SIZE` and the structure field names. No code and no other text of
these files was copied. Whether taking them is acceptable under GPL-3.0-or-later, given
Valve's restriction on `const.h`, `usercmd.h` and `entity_state.h`, is an open question for the
project owner; see `docs/LICENSING.md`.
```
Copyright (c) 2014–2025 ReHLDS Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## ReGameDLL_CS

Upstream: https://github.com/rehlds/ReGameDLL_CS

The repository-level `LICENSE` of ReGameDLL_CS is MIT (notice below). The source file
`regamedll/dlls/player.cpp` (master, read 2026-11-21) carries a header stating GPL v2-or-later
with an exception permitting linking with Valve's Half-Life engine and game libraries.
`regamedll/dlls/client.cpp` (master, read 2026-11-21), the source of the Counter-Strike user
message table (names, order and payloads, `crates/cs-proto48/src/usermsg.rs`), begins with
`#include "precompiled.h"` and has no licence header, so the repository `LICENSE` is the
notice that applies to it. In phase 2B `regamedll/dlls/cbase.cpp` (`SetObjectCollisionBox`)
and `regamedll/dlls/util.cpp` (`UTIL_EntitiesInBox`) were read for comparison only (master,
read 2026-11-21): the engine's linking and `entities_in_box` were checked against them and no
code was taken from these two files. In phase 2C the files listed below were read and the
behaviour of the game library `cs-game` was ported from them (all master, read 2026-11-22). Files
not listed here were not examined.

The `.h` files listed and `regamedll/dlls/player.cpp` carry a header stating GPL v2-or-later with
the same exception permitting linking with Valve's Half-Life engine and game libraries; the other
`.cpp` files listed begin with `#include "precompiled.h"` and have no licence header, so the
repository `LICENSE` (MIT, notice below) is the notice that applies to them. Each ported
function names its source file, function and the date read in its doc comment.

| File (`regamedll/dlls/` unless noted) | What was taken |
|---|---|
| `player.cpp`, `player.h` | The player's life: `CBasePlayer::Spawn`, `Killed`, `PlayerDeathThink`, `StartDeathCam`, `StartObserver`, `TakeDamage` usage, fall damage in `PlayerPreThink`/`PostThink`, `UpdateClientData` (the radar and status bar calls), `UpdateStatusBar`, `InitStatusBar`, `Radio`, `JoiningThink`, `GetIntoGame`, `HandleMenu_*` team and class choices, `AddAccount`, the HUD hide flags and `SBAR_*`/`MAX_ID_RANGE` values. In the pm_shared port (read 2026-11-22): `WaterMove` (drowning, `air_finished`, the swim and gasp sounds, lava and slime damage), `CheckTimeBasedDamage` and the drown-recovery part of `TakeHealth`, the velocity-modifier block and `fall_velocity` of `PlayerPreThink`, `PlayerPostThink`, `UpdateClientData`'s movement fields (`bInDuck`, `flDuckTime`, `flTimeStepSound`, `flSwimTime`, `waterjumptime`, `pushmsec`, `fuser1` to `fuser3`, `iuser3`), `Killed`'s view offset, and `FixPlayerCrouchStuck`. |
| `client.cpp`, `client.h` | `ClientConnect`, `ClientPutInServer`, `ClientDisconnect`, `ClientKill`, `ClientCommand` (`jointeam`, `joinclass`, `menuselect`, `chooseteam`, `kill`, `nightvision`, `radio1` to `radio3`, `drop`, `ignorerad`, the buy aliases), `Host_Say` (chat formats, sanitising and flood limits), `ClientUserInfoChanged` (name changes), `Radio1` to `Radio3` and `HandleRadioAliasCommands`, `BuyItem`-related command parsing, `ClientSetupVisibility` (the observer part), the usermsg registration table (`LinkUserMessages`) and its order, `InitHUD`-related text. In the pm_shared port: `CmdStart` and `CmdEnd` (the random seed, the duck hull), and `SetupVisibility` for the observer (`UTIL_SetOrigin` to the followed player). |
| `multiplay_gamerules.cpp`, `gamerules.cpp`, `gamerules.h` | `CHalfLifeMultiplay`: `Think`, `RestartRound`, `CheckWinConditions`, `CheckRoundTimeExpired`, `Round_Cts`/`Round_Ts` and `TerminateRound`, the freeze period, the money rewards and the loser bonus ladder, `PlayerKilled`, `DeathNotice`, `IPointsForKill`, `PlayerSpawn`, `GetPlayerSpawnSpot`, `FPlayerCanRespawn`, `InitHUD`, `UpdateTeamScores`, `GoToIntermission`, `ChangeLevel` and the map cycle, `ROUND_RESPAWN_TIME` and the other round constants. In the pm_shared port: the hard locks of `sv_accelerate` (5), `sv_friction` (4) and `sv_stopspeed` (75) in `RestartRound` and `Think`. |
| `game.cpp`, `game.h` | The `mp_*` cvars the rules read and their defaults (`mp_timelimit`, `mp_freezetime`, `mp_roundtime`, `mp_startmoney`, `mp_friendlyfire`, `mp_tkpunish`, `mp_autokick`, `mp_playerid`, `mp_forcecamera`, `mp_forcechasecam`, `mp_fadetoblack`, `mp_limitteams`, `mp_autoteambalance`, `mp_buytime`, `mp_chattime`, `mp_winlimit`, `mp_maxrounds` and others), `PLAYERID_MODE_*`. |
| `world.cpp`, `bmodels.cpp`, `triggers.cpp`, `items.cpp`, `cbase.cpp`, `basemonster.cpp`, `subs.cpp` | `CWorld::Precache`/`Spawn` precache lists and world keys; `func_wall`, `func_illusionary` (`bmodels.cpp`); `func_buyzone`, `func_ladder` and the buy zone touch/`BuyZoneIcon_Set` (`triggers.cpp`); `CItemKevlar`/`CItemAssaultSuit` pickup behaviour (`items.cpp`); `CBaseEntity::TakeDamage` and entity dispatch rules (`cbase.cpp`); the `dmg_save` observation (`basemonster.cpp`); `info_player_*` spawn handling (`subs.cpp`, `gamerules.cpp`). |
| `util.cpp`, `util.h` | `UTIL_ScreenFade`, `UTIL_ClientPrintAll`/`ClientPrint` and their destinations (`HUD_PRINT*`, `HUD_PRINTRADIO`), `DMG_EXPLOSION`, `UTIL_PlayerByIndex`, the `UTIL_*` message helpers. |
| `weapontype.cpp`, `weapontype.h`, `weapons.cpp` | `g_weaponBuyAliasInfo` (the buy aliases), the prices `KEVLAR_PRICE`, `HELMET_PRICE`, `ASSAULTSUIT_PRICE`, `NVG_PRICE`; `W_Precache` (the `radio.spr` precache). |
| `unisignals.h`, `cdll_dll.h`, `observer.cpp`, `observer.h` | `CUnifiedSignals` and `SIGNAL_BUY`; `HIDEHUD_*`, `SCORE_STATUS_*`, `WEAPON_SUIT` and the other `cdll_dll.h` constants (`cdll_dll.h` was read; its header is the GPL v2-or-later one); `CBasePlayer::Observer_*` (`Observer_FindNextPlayer`, `Observer_HandleButtons`, `Observer_SetMode`, `Observer_CheckTarget`, `Observer_CheckProperties`), `CAMERA_MODE_*`, `FADETOBLACK_*`. |
| `regamedll/pm_shared/pm_shared.cpp`, `pm_math.cpp` | The player movement code, ported function by function (vanilla branch: the code not under `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`, `REGAMEDLL_API`; read 2026-11-22; `crates/cs-pm/src/pm/`): `PM_Move` and `PM_PlayerMove`, `PM_CheckParameters`, `PM_CalcRoll`, `PM_DropPunchAngle`, `PM_ReduceTimers`, `PM_CheckWater`, `PM_CategorizePosition`, `PM_CheckFalling`, `PM_Friction`, `PM_Accelerate`, `PM_AirAccelerate`, `PM_AirMove`, `PM_WalkMove`, `PM_FlyMove`, `PM_ClipVelocity`, `PM_AddCorrectGravity`, `PM_FixupGravityVelocity`, `PM_CheckStuck` with the stuck table, `PM_Duck`, `PM_UnDuck`, `PM_FixPlayerCrouchStuck`, the duck spline, `PM_Jump`, `PM_PreventMegaBunnyJumping`, `PM_Ladder`, `PM_LadderMove`, `PM_WaterMove`, `PM_CheckWaterJump`, `PM_WaterJump`, `PM_PlayWaterSounds`, `PM_UpdateStepSound`, `PM_PlayStepSound`, `PM_CatagorizeTextureType`, `PM_MapTextureTypeStepType`, `PM_InitTextureTypes` and `PM_FindTextureType`, `PM_NoClip`, `PM_Physics_Toss`, `PM_SpectatorMove`, `PM_ShouldDoSpectMode`, and the constants of the file; from `pm_math.cpp` the vector helpers (`AngleVectors`, `VectorNormalize` and the like). Both files begin with `#include "precompiled.h"` and have no licence header, so the repository `LICENSE` (MIT) is the notice that applies to them. |
| `regamedll/pm_shared/pm_shared.h`, `pm_defs.h`, `pm_movevars.h`, `pm_materials.h`, `pm_info.h` | Numeric values and names only (`crates/cs-pm/src/pm/consts.rs`, `types.rs`): from `pm_shared.h` the `PM_*` and `OBS_*` constants and the function list; from `pm_defs.h` `MAX_PHYSENTS`, `MAX_MOVEENTS`, `MAX_CLIP_PLANES`, the `PM_*` trace flags (`PM_STUDIO_IGNORE`, `PM_STUDIO_BOX`, `PM_GLASS_IGNORE`, `PM_WORLD_ONLY`) and the field names of `playermove_t`, `physent_t` and `pmtrace_t`; from `pm_movevars.h` the field names of `movevars_t`; from `pm_materials.h` `MAX_TEXTURES`, `MAX_TEXTURENAME_LENGHT` and the `CHAR_TEX_*` values. `pm_info.h` was read and nothing was taken from it (`physinfo` is not sent). All five carry the GPL v2-or-later header with the linking exception. |
| `regamedll/game_shared/bot/nav.h` | One constant: `HalfHumanHeight` (36, line 145), the climb limit of the crouch-stuck fix (`PM_FixPlayerCrouchStuck`, `crates/cs-pm/src/pm/duck.rs`). The file carries the GPL v2-or-later header with the linking exception. |
| `doors.cpp` | `CBaseDoor::Spawn` and `CRotDoor::Spawn` (the spawn part: solidity by `skin` and `SF_DOOR_PASSABLE`, `MOVETYPE_PUSH`), for `func_door`, `func_door_rotating` and `func_water` in `cs-game`'s `world.rs`, read 2026-11-22. It begins with `#include "precompiled.h"` and has no licence header. |
| `regamedll/engine/shake.h` | `FFADE_*` values. |

In phase 2D the files listed below were read and the behaviour of the weapons, bullets, grenades, inventory and economy of `cs-game` was ported from them function by function (all master, read 2026-11-22). The `.h` files carry the GPL v2-or-later header with the linking exception; the `.cpp` files except `player.cpp` have no licence header, so the repository `LICENSE` (MIT, notice below) is the notice that applies to them (details in `docs/LICENSING.md`). Each ported function names its source file, function and the date read in its doc comment; quirks of the source are kept and named in comments, and anything not checked against the source is marked as an assumption.

| File (`regamedll/dlls/` unless noted) | What was taken (phase 2D) |
|---|---|
| `weapons.cpp`, `weapons.h` | `CBasePlayerItem` and `CBasePlayerWeapon`: `ItemPostFrame`, `DefaultDeploy`, `DefaultReload`, `DefaultShotgunReload`, `KickBack`, `PlayEmptySound`, `SendWeaponAnim`, `EjectBrassLate`, `AddToPlayer`, `AddDuplicate`, `ExtractAmmo`, `RetireWeapon`; `CWeaponBox` (`PackWeapon`, `Touch`, `Kill`), `CArmoury` (`armoury_entity`, the item pack enum and the restart), `W_Precache` and the precache lists, the ammo and weapon globals. |
| `weapontype.cpp`, `weapontype.h` | `g_weaponInfo_default` (prices, clip costs, buy amounts, clip sizes, ammo names), the alias tables, `WeaponIdType`, `AmmoType`, the default give and clip tables, `WEAPON_*` flags. |
| `wpn_ak47.cpp`, `wpn_aug.cpp`, `wpn_awp.cpp`, `wpn_deagle.cpp`, `wpn_elite.cpp`, `wpn_famas.cpp`, `wpn_fiveseven.cpp`, `wpn_flashbang.cpp`, `wpn_g3sg1.cpp`, `wpn_galil.cpp`, `wpn_glock18.cpp`, `wpn_hegrenade.cpp`, `wpn_knife.cpp`, `wpn_m249.cpp`, `wpn_m3.cpp`, `wpn_m4a1.cpp`, `wpn_mac10.cpp`, `wpn_mp5navy.cpp`, `wpn_p228.cpp`, `wpn_p90.cpp`, `wpn_scout.cpp`, `wpn_sg550.cpp`, `wpn_sg552.cpp`, `wpn_smokegrenade.cpp`, `wpn_tmp.cpp`, `wpn_ump45.cpp`, `wpn_usp.cpp`, `wpn_xm1014.cpp` | Each weapon's `Spawn`, `Precache`, `GetItemInfo`, `Deploy`, `PrimaryAttack`, `SecondaryAttack`, `Reload`, `WeaponIdle` and the fire function with its spread, accuracy and kick-back numbers, ported weapon by weapon. `wpn_c4.cpp` was read for the survey only and is not ported (phase 2E). |
| `cbase.cpp`, `util.cpp`, `util.h` | `CBaseEntity::FireBullets3` (range falloff, penetration by material, the hit group and the decal and impact effects), `FireBullets` (buckshot), `UTIL_SharedRandomLong` and `UTIL_SharedRandomFloat` (the shared generator, ported bit for bit; it assumes the f32 arithmetic of the original binary), `UTIL_DecalTrace`, `UTIL_ScreenFade` use, `UTIL_MakeVectors` use. |
| `combat.cpp` | `RadiusDamage`, `CBaseMonster::TakeDamage` usage and the blast bookkeeping, `CBasePlayer::TraceAttack` hit group multipliers. `RadiusDamage2` (the bomb's) is not ported. |
| `ggrenade.cpp` | `CGrenade`: `ShootTimed`, `ShootTimed2`, `ShootSmokeGrenade`, `BounceTouch`, `TumbleThink`, `Detonate`, `Explode`, `Explode3`, `Smoke`, `Smoke2`, `Smoke3_C`, `SG_Detonate`, `SG_TumbleThink`, `BounceSound`, the flash blinding in `RadiusFlash`. |
| `player.cpp`, `player.h`, `client.cpp`, `client.h`, `multiplay_gamerules.cpp`, `gamerules.h` | The inventory and the economy: `AddPlayerItem`, `RemovePlayerItem`, `SwitchWeapon`, `SelectItem`, `SelectLastItem`, `GiveNamedItem`, `GiveAmmo`, `GiveDefaultItems`, `DropPlayerItem`, `DropPrimary`, `DropSecondary`, `PackDeadPlayerItems`, `RemoveAllItems`, `TakeDamage` (armor ratios per weapon, momentum, the velocity modifier setter, flinch), `GetKillerWeaponName`, `SetAnimation` and its `m_Activity` handling, `PlayerDeathThink` animation, `ResetMaxSpeed`, `UpdateClientData` weapon fields (`weapons`, `m_iId`, `vuser`, the ammo counters); `BuyWeaponByWeaponID`, `BuyAmmo`, `BuyGunAmmo`, the buy aliases and menus; `FShouldSwitchWeapon`, `DeadPlayerWeapons`. |
| `hintmessage.cpp`, `hintmessage.h` | `CHintMessage` and `CHintMessageQueue`: `Reset`, `Update`, `AddMessage`, the message duration and the `DHF_*` flags, ported in `crates/cs-game/src/hint.rs` (read 2026-11-22). The `.h` carries the GPL v2-or-later header with the linking exception; the `.cpp` has no licence header, so the repository `LICENSE` (MIT) applies to it. |
| `animation.cpp`, `animating.cpp`, `activity.h` | `LookupSequence`, `LookupActivity`, `GetSequenceInfo` (`animation.cpp`) and `CBaseAnimating::StudioFrameAdvance` (`animating.cpp`), ported in `crates/cs-game/src/player_anim.rs` and cited in `crates/cs-assets/src/mdl.rs`; the `ACT_*` values of `activity.h` (read 2026-11-22). `activity.h` carries the GPL v2-or-later header with the linking exception; `animation.cpp` and `animating.cpp` have no licence header, so the repository `LICENSE` (MIT) applies to them. |
| `rehlds/public/rehlds/customentity.h` (ReHLDS) | The beam type values `BEAM_POINTS` 0, `BEAM_ENTPOINT` 1, `BEAM_ENTS` 2 and `BEAM_HOSE` 3, in `crates/cs-game/src/encoders.rs` (read 2026-11-22). Numbers and names only; the file carries Valve LLC's notice (see the Valve Half-Life SDK caveat in `docs/LICENSING.md`). |
| `cdll_dll.h`, `regamedll/common/const.h`, `entity_state.h`, `usercmd.h`, `weaponinfo.h`, `event_flags.h`, `regamedll/engine/studio.h`, `regamedll/dlls/decals.h`, `regamedll_const.h`, `regamedll/pm_shared/pm_defs.h`, `pm_materials.h`, `eiface.h` | Numeric values and names only: the `WEAPON_*`, `HIDEHUD_*` and `FEV_*` values, the field names and layouts of `weapon_data_t`, `entity_state_t`, `usercmd_t`, the studio header and hitbox structs (`studio.h`), the decal list indices, and the texture types. `eiface.h` has no licence header (a struct layout only). |

In phase 2E (master, read 2026-11-23) the files listed below were read and ported function by function into `cs-game` for entity use/damage/targets, map objective triggers and parameters, C4/bomb/defusal, defusal kit, hostages and local navigation, VIP and escape modes, objective round rules/money, and engine-backed bots. Of these, `hostage/hostage.cpp`, `hostage/hostage.h`, `hostage/hostage_localnav.cpp`, `hostage/hostage_localnav.h`, `bot/cs_bot_manager.cpp`, `player.cpp`, `player.h`, `client.h`, `weapons.h`, `weapontype.h`, `items.h`, `mapinfo.h`, `triggers.h` and `unisignals.h` carry the GPL v2-or-later header with the linking exception; `triggers.cpp`, `mapinfo.cpp`, `wpn_c4.cpp`, `ggrenade.cpp`, `multiplay_gamerules.cpp`, `items.cpp`, `client.cpp`, `weapons.cpp`, `weapontype.cpp`, `combat.cpp`, `cbase.cpp`, `subs.cpp`, `basemonster.cpp`, `util.cpp` and `gs/bot.cpp` begin with `#include "precompiled.h"` and have no per-file licence header, so the repository `LICENSE` (MIT, notice below) applies to them (`docs/LICENSING.md`).

| File (`regamedll/dlls/` unless noted) | What was taken (phase 2E) |
|---|---|
| `cbase.cpp`, `combat.cpp`, `basemonster.cpp`, `subs.cpp`, `util.cpp` | `CBaseEntity::TakeDamage`, `SUB_UseTargets` / `FireTargets`, `CBaseDelay::KeyValue` / `DelayThink`, `RadiusDamage2` (C4 spherical/wall-occluded blast damage), entity knockback and target firing, point markers (`info_bomb_target`, `info_hostage_rescue`, `info_vip_start`). |
| `triggers.cpp`, `triggers.h`, `mapinfo.cpp`, `mapinfo.h`, `unisignals.h` | `InitTrigger`, `CBombTarget` (`func_bomb_target`), `CHostageRescue` (`func_hostage_rescue`), `CEscapeZone` (`func_escapezone`), `CVIP_SafetyZone` (`func_vip_safetyzone`), `CMapInfo` (`info_map_parameters`, `buying` and `bombradius`). |
| `wpn_c4.cpp`, `ggrenade.cpp`, `items.cpp`, `items.h` | `CC4` (`Spawn`, `Precache`, `PrimaryAttack`, `WeaponIdle`), `CGrenade::ShootSatchelCharge`, `C4Think`, `DefuseBombStart`, `DefuseBombEnd`, `Explode2`/`Detonate2`, `Use`, `SlideTouch`; `CItemThighPack` (`item_thighpack` defusal kit). |
| `hostage/hostage.cpp`, `hostage.h`, `hostage/hostage_localnav.cpp`, `hostage/hostage_localnav.h` | Classic `!m_improv` `CHostage` (`Spawn`, `Precache`, `IdleThink`, `Use`, `Touch`, `TakeDamage`, `Killed`, `Remove`, `HostagePostFrame`, rescue rewards, `mp_hostagepenalty` and `HostagePos`/`HostageK`) and `CLocalNav` (`PathFind`, `PathTraversable`, `FindPath`, node caching and step climb). `hostage_improv.cpp` was inspected for scope confirmation only and not ported. |
| `player.cpp`, `player.h`, `client.cpp`, `client.h`, `weapons.cpp`, `weapons.h`, `weapontype.cpp`, `weapontype.h` | `PlayerUse`, `DropPlayerItem` (`drop weapon_c4`), `GiveC4`, `CWeaponBox::BombThink`, `MakeVIP`, `become_vip`, `HandleSignals` (`c4`/`rescue`/`escape`/`vipsafety`), `HandleMenu_ChooseTeam` (`jointeam 3` VIP queue), `BuyItem` (`SLOT_ITEM_DEFUSEKIT`, VIP/`as_` restrictions), scoreboard bits (`SCORE_STATUS_BOMB`, `SCORE_STATUS_VIP`), `SendItemStatus` (`ITEM_STATUS_DEFUSER`), and `IsBot` rules branches. |
| `multiplay_gamerules.cpp` | Objective `CheckWinConditions`, `CheckRoundTimeExpired`, `CheckMapConditions`, `CheckVIP` (`StackVIPQueue`, `IsVIPQueueEmpty`, `AddToVIPQueue`, `PickNextVIP`, `ResetCurrentVIP`), escape ratio + 3-round `SwapAllPlayers`, bomb/hostage/VIP/escape money tables, and round-restart entity resets (`GiveC4`, `hostage_entity`, `CArmoury`). |
| `bot/cs_bot_manager.cpp`, `gs/bot.cpp` | `bot_add`, `bot_add_t`, `bot_add_ct`, `bot_kick`, `bot_quota` manager entrypoints and ReHLDS fake-client bot lifecycle (`cs-bot*` ZBot navmesh/AI files were inspected for command scope only and remain Phase 12). |

The `HIDEHUD_*` and `SCORE_STATUS_*` values were read from `regamedll/dlls/cdll_dll.h` (master,
read 2026-11-22). Only the `DMG_*` bit values were taken from memory of the Half-Life SDK
(`regamedll/dlls/damage.h` was not read); they are labelled as assumptions in the code. No SDK
header text was copied.

```
Copyright (c) 2015–2025 ReGameDLL_CS Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Xash3D FWGS

Upstream: https://github.com/FWGS/xash3d-fwgs (GPL-3.0-or-later, as the module documentation of
the files below records). Phase 3 (the client core, protocol additions, asset animation/sprite
helpers, and renderer) was ported from it; the files below were read on 2026-11-23 (with
`engine/client/cl_main.c` `CL_ComputeClientInterpolationAmount` also consulted on 2026-11-24) and
the behaviour was ported function by function, each module naming its source file and function in
its doc comment. `common/const.h` (lines 698–720 `kRenderFx*` numeric values, cited in
`crates/cs-client/src/render/scene.rs`) carries Valve LLC's Half-Life SDK header and was read for
numeric constants only (see the Valve Half-Life SDK caveat in `docs/LICENSING.md`). Code ported
from Xash3D FWGS is distributed in this repository under GPL-3.0-or-later; see `docs/LICENSING.md`.
Files not listed here were not examined.

```text
Copyright (C) 2007–2018 Uncle Mike
Copyright (C) Xash3D FWGS contributors

This program is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation, either version 3 of the
License, or (at your option) any later version.
```

| File (Xash3D FWGS) | Where | What was taken |
|---|---|---|
| `engine/client/cl_main.c` | `crates/cs-clientcore/src/connection.rs`, `commands.rs`, `cvars.rs`, `resources.rs`, `frames.rs`, `interp.rs`; `crates/cs-client/src/input.rs` | The connect sequence (`getchallenge`, `connect`, `new`, `sendres`, `spawn`, `sendents`), the client commands and cvars the connection reads with their flags and defaults, `CL_ComputeClientInterpolationAmount`, `CL_WritePacket` (command rate bounds, backup commands), the console command flags. |
| `engine/client/parse/cl_parse.c`, `engine/client/parse/cl_parse_gs.c` | `crates/cs-proto48/src/messages.rs`; `crates/cs-clientcore/src/resources.rs`, `precache.rs`, `frames.rs`, `hud.rs`, `sound.rs` | `CL_ParseResourceList`, `CL_ParseServerInfo`, the precache tables, `CL_RegisterUserMessage` and `CL_ParseUserMessage` (registration and framing of the game's messages), `CL_ParseParticles`, `CL_ParseSpawnStaticSound`, `CL_ParseCustomization`, `CL_ParseSoundFade`, the sound message parsing, and the `svc_sendcvarvalue` answers. |
| `engine/client/cl_frame.c` | `crates/cs-clientcore/src/frames.rs`, `interp.rs`, `entities.rs` | `CL_ParsePacketEntities`, `CL_DeltaEntity`, the frame ring and the delta base rule, `CL_LinkPlayers`, `CL_LinkPacketEntities`, the interpolation and extrapolation of entities (`ex_interp`, teleport detection, the studio lerp). |
| `engine/client/dll_int/cl_pmove.c` | `crates/cs-clientcore/src/predict.rs` | `CL_PredictMovement`, `CL_CheckPredictionError`, `CL_RunUsercmd`, the `playermove_t` setup and the prediction error smoothing. |
| `engine/client/cl_events.c` | `crates/cs-clientcore/src/events.rs` | `CL_ParseEvent`, `CL_ParseReliableEvent`, `CL_QueueEvent`, `CL_FireEvents`, `CL_FireEvent`, `CL_PlaybackEvent`, `CL_FindEmptyEvent`. |
| `engine/client/cl_tent.c` | `crates/cs-proto48/src/messages.rs`, `tempent.rs`; `crates/cs-clientcore/src/tempents.rs` | `CL_ParseTempEntity` wire decoding, the temp entity pool and the handlers of the `TE_*` messages. The per-frame update of the temp entities and the shell physics are NOT ported: Xash3D leaves them to the client library, so they are our own clean-room design (documented in `tempents.rs`). |
| `engine/client/cl_custom.c`, `engine/common/net_chan.c` | `crates/cs-clientcore/src/resources.rs`, `crates/cs-proto48/src/netchan.rs` | The custom resource list handling (`CL_ResourcePath`, `CL_CheckFile`), `Netchan_CopyFileFragments` file stream header, dual-stream fragmentation, and the file stream size limit (`MAX_NETCHAN_DECOMPRESSED_SIZE`). |
| `engine/client/cl_demo.c` | `crates/cs-proto48/src/demo.rs`, `crates/cs-clientcore/src/demo.rs` | GoldSrc HLDEMO Protocol 5 (`.dem`) structures (`demoheader_t` 544 B, `directory_entry` 92 B, `demo_info_t` 436 B, `usercmd_t` 52 B) and frame codecs (read 2026-11-25). |
| `engine/common/hpak.c` | `crates/cs-assets/src/hpak.rs` | GoldSrc `HPAK` (`.hpk`) custom spray archive parser/writer and WAD3 decal validation (read 2026-11-25). |
| `engine/common/cmd.c`, `engine/common/cvar.c`, `engine/client/cl_main.c` (`cl_filterstuffcmd`) | `crates/cs-clientcore/src/commands.rs`, `cvars.rs`, `connection.rs` | `Cbuf_ExecuteCommandsFromBuffer`, the command tokenizer, the cvar flag set (`FCVAR_PRIVILEGED`, `FCVAR_FILTERABLE`), `Cvar_ShouldSetCvar`, `Cmd_ShouldAllowCommand`, `Cmd_AddRestrictedCommand`, and `cl_filterstuffcmd` slowhack protection. |
| `engine/client/client.h`, `engine/common/netchan.h`, `engine/common/protocol.h` | `crates/cs-clientcore/src/frames.rs`, `interp.rs` | Protocol/client header constants (`MAX_EX_INTERP`, `CL_UPDATE_MASK`, protocol opcode/rate bounds). |
| `public/matrixlib.c`, `public/xash3d_mathlib.c`, `public/xash3d_mathlib.h` | `crates/cs-clientcore/src/interp.rs` | Angle and quaternion helpers. |
| `ref/gl/gl_rmain.c`, `gl_rsurf.c`, `gl_sprite.c`, `gl_studio.c`, `gl_image.c`, `gl_local.h` | `crates/cs-assets/src/{bsp.rs, mdl.rs, spr.rs}`; `crates/cs-client/src/render/{scene.rs, bsp_renderer.rs, mdl_renderer.rs, sprite_renderer.rs, decal_renderer.rs, lighting.rs}` | The entity draw order and transparency sorting, render modes and render FX (pulse, distort, hologram, glow), `R_StudioEstimateFrame` / studio frame and looping rules, `R_StudioLighting` (`SHADE_LAMBERT`), `R_DrawSpriteModel` / sprite drawing, `R_BuildLightMap`, `R_AddDynamicLights`, `R_TextureAnimation` (`-0..-9`), `GL_SetTextureDimensions`, and `GL_BuildMipMap` (read 2026-11-23 and 2026-11-24). |
| `engine/client/gamma.c`, `engine/common/imagelib/img_utils.c` | `crates/cs-assets/src/gamma.rs`; `crates/cs-client/src/render/{gamma.rs, lighting.rs}` | `BuildGammaTable`, `V_ValidateGammaCvars`, `TextureToGamma`, `LightToTexGamma`, `Test_GetGammaTables` verification vectors, `LUMP_TEXGAMMA`, and `Image_Resample32Lerp` (read 2026-11-24). |
| `ref/common/ref_light.c`, `ref/common/ref_context.c` | `crates/cs-assets/src/bsp.rs`; `crates/cs-client/src/render/mdl_renderer.rs` | `CL_RunLightStyles`, `R_RecursiveLightPoint`, `R_EntityDynamicLight`, and `GL_InitRandomTable` (`MOD_FRAMES = 20`) (read 2026-11-24). |
| `common/const.h` (Valve Half-Life SDK header) | `crates/cs-client/src/render/scene.rs` | Numeric values and names only: `kRenderFx*` (lines 698–720, read 2026-11-23). Carries Valve LLC's non-commercial SDK notice; see the Valve Half-Life SDK caveat in `docs/LICENSING.md`. |

## cs16-client

Upstream: https://github.com/FWGS/cs16-client (GPL-2.0-or-later with Valve Half-Life linking
exception; the Counter-Strike 1.6 client library for Xash3D FWGS).

```text
This program is free software; you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation; either version 2 of the
License, or (at your option) any later version.
```

Only the weapon/effect event files `cl_dll/events/event_*.cpp` and `dlls/wpn_shared.h` (read
2026-11-23) were used: `crates/cs-clientcore/src/events.rs` takes its per-weapon and view-model
`*_SHOOT*` animation enum constants from the weapon event files and `dlls/wpn_shared.h` (the
sounds, the view model animation numbers, the shell offsets, the bullet type and the penetration;
the table `WEAPONS`), `event_vehicle.cpp` supplies `EV_Vehicle` (`iparam1` bit-packing and engine
sound table), `event_decal_reset.cpp` and `event_createexplo.cpp` supply reset/explosion event
handling (`events.rs`), and `event_createsmoke.cpp` supplies the smoke grenade clouds
(`crates/cs-clientcore/src/tempents.rs`, `spawn_smoke_clouds`). The helper code those files call
(`EV_GetDefaultShellInfo`, `EV_GetGunPosition`, `HUD_TempEntUpdate` and the rest of the `cl_dll`
code) is in Valve-licensed files and is NOT ported: the shell and gun placement, the shell
physics and the temp entity update are our own clean-room design. The HUD code of `cl_dll/hud*.cpp`
was not read for porting.

## AMX Mod X

Upstream: https://github.com/alliedmodders/amxmodx (GPL-3.0-or-later, read 2026-11-25).

```text
Copyright (C) AMX Mod X Development Team

This program is free software: you can redistribute it and/or modify it under the terms of the
GNU General Public License as published by the Free Software Foundation, either version 3 of the
License, or (at your option) any later version.
```

Pure-Rust Pawn 3.2 bytecode VM, `.amxx` container loader, plugin runtime, and Counter-Strike 1.6
native/forward host in `crates/cs-amxx/` were ported from the following AMX Mod X sources:

| File (AMX Mod X) | Where | What was taken |
|---|---|---|
| `amxxfile.h`, `amxx_loader.cpp` (`CAmxxReader`) | `crates/cs-amxx/src/amxxfile.rs` | `.amxx` v1/v2/v3 container headers (`MAGIC_HEADER` `'AMXB'`, `MAGIC_HEADER2` `'AMXX'`, `MAGIC_VERSION` `0x0300`, `PluginEntry`) and zlib section selection (`cellsize=4`). |
| `amx.h`, `amx.cpp` | `crates/cs-amxx/src/amxxfile.rs`, `crates/cs-amxx/src/vm.rs` | `AMX_HEADER` (`AMX_MAGIC` `0xF1E0`, `AMX_FLAG_*`), `expand()` compact bytecode decompression, `amx_Init`, symbol lookup (`amx_GetPublic`, `amx_GetNative`, `amx_GetPubVar`), Pawn 3.2 opcodes `0..=144` (`OP_NONE..=OP_FLOAT_CMP`), `STKMARGIN`, and packed/unpacked string helpers. |
| `CPlugin.cpp`, `amxmodx.cpp`, `string.cpp`, `float.cpp` | `crates/cs-amxx/src/runtime.rs`, `crates/cs-amxx/src/natives.rs` | Plugin lifecycle/forwards (`plugin_precache`, `plugin_init`, `plugin_cfg`, `plugin_end`, `client_connect`, `client_putinserver`, `client_disconnect`), task scheduler (`set_task`/`remove_task`), command/event/message hooks, and core string/float/cvar/menu/log natives. |
| Modules: `cstrike`, `fun`, `engine`, `fakemeta`, `hamsandwich` | `crates/cs-amxx/src/natives.rs`, `crates/cs-amxx/src/runtime.rs` | Counter-Strike (`cs_get/set_user_money`, `cs_get/set_user_team`, `cs_get/set_user_armor`, `cs_get/set_user_deaths`), fun (`get/set_user_health`, `set_user_godmode`, `give_item`, `strip_user_weapons`, `set_user_rendering`), engine (`entity_get/set_*`), fakemeta (`pev`/`set_pev` `PEV_*` field IDs, `register_forward`), and hamsandwich (`RegisterHam` `Spawn`/`TakeDamage`/`Killed` with `HAM_IGNORED..HAM_SUPERCEDE`). |
