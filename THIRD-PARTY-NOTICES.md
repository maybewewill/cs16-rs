# Third-party notices

`cs16-rs` contains code ported from the following projects. Code ported from them is
distributed in this repository under GPL-3.0-or-later, the licence of this project; see
`docs/LICENSING.md`.

## ReHLDS

Upstream: https://github.com/rehlds/ReHLDS

The repository-level `LICENSE` of ReHLDS is MIT (notice below); the engine source files listed
and the engine headers `net.h`, `server.h` and `common/mathlib.h` (master, read 2026-11-21)
carry a header stating GPL v2-or-later with an exception permitting linking with Valve's
Half-Life engine and game libraries. The remaining headers listed (`common/const.h`,
`common/usercmd.h`, `common/entity_state.h`, `common/qlimits.h`, `public/rehlds/edict.h`) carry
Valve's own notices instead; see the last rows. Files not listed here were not examined.

Files read and ported from (the behaviour was ported function by function; each ported
function names its source file, function and the date read in its doc comment):

| File (`rehlds/engine/` unless noted) | What was taken |
|---|---|
| `sv_user.cpp` | `SV_AddLinksToPM_`, `SV_IsCollidableInPM`, `SV_CopyEdictToPhysent` (which entities block a player's movement trace), `SV_EstablishTimeBase` and `SV_RunCmd` (the time base of a move), `SV_FinishPMove` and `SV_ProcessImpacts` (impacts after a move), and lag compensation: `SV_IsLagCompensationActive`, `SV_GetUnlagTargetTime`, `SV_FindUnlagFrames`, `SV_FlagNonInterpolatedPlayers`, `SV_CalcUnlagFraction`, `SV_UnlagCheckTeleport`, `SV_SaveTruePositions`, `SV_ApplyAdjustedPositions`, `SV_SetupMove`, `SV_RestoreMove`, `SV_GetTrueOrigin`, `SV_GetTrueMinMax`, plus the ping bookkeeping of `SV_ExecuteClientMessage`, `SV_CalcClientTime` and `SV_ParseMove`. |
| `world.cpp` | `SV_ClearWorld`, `SV_CreateAreaNode`, `SV_UnlinkEdict`, `SV_FindTouchedLeafs`, `SV_LinkEdict`, `SV_TouchLinks` (entity linking and trigger touches); `SV_Move`, `SV_ClipToLinks`, `SV_ClipMoveToEntity`, `SV_SingleClipMoveToEntity`, `SV_HullForEntity`, `SV_HullForBsp`, `SV_HullForBox`, `SV_InitBoxHull`, `SV_MoveBounds`, `SV_LinkContents` (entity traces). |
| `sv_phys.cpp` | `SV_Physics` and the movetype functions (`SV_Physics_None`, `_Follow`, `_Noclip`, `_Toss`, `_Bounce`, `_Step`, `_Pusher`), `SV_RunThink`, `SV_CheckVelocity`, `SV_AddGravity`, `SV_ClipVelocity`, `SV_FlyMove`, `SV_PushEntity`, `SV_PushMove`, `SV_PushRotate`, `SV_CheckMovingGround`, `SV_CheckWater`, `SV_CheckWaterTransition`, `SV_Submerged`, `SV_Impact`. |
| `sv_move.cpp` | `SV_CheckBottom`, `SV_movetest`, `SV_movestep`, `SV_StepDirection`, `SV_FlyDirection`, `SV_FixCheckBottom`, `SV_NewChaseDir2`, `SV_MoveToOrigin_I` (entity walking). |
| `sv_main.cpp` | `SV_WriteEntitiesToClient` and `SV_SendClientDatagram` (the overflow rules), `SV_Frame_Internal` (frame order), `SV_ActivateServer` (settle passes), `SV_SpawnServer` (world edict, PAS at activation), `SV_SetMoveVars`, `SV_StartSound_internal`, `SV_Multicast`, `SV_ValidClientMulticast`, the `cl_lw` / `cl_lc` userinfo use. |
| `pr_cmds.cpp` | `PF_setorigin_I`, `PF_setsize_I`, `PF_setmodel_I`, `PF_traceline_DLL`, `TraceHull`, `TraceModel`, `PF_walkmove_I`, `PF_droptofloor_I`, `PF_lightstyle_I`. |
| `pr_edict.cpp` | `ED_Alloc_internal` (the edict reuse rule). |
| `host.cpp` | `Host_FilterTime` (the 0.25 s frame clamp). |
| `net_chan.cpp`, `net_ws.cpp` | The netchan of `crates/cs-proto48/src/netchan.rs` (sequence numbers, fragmentation, `Netchan_Transmit` and its overflow rule for unreliable data next to reliable data, the packet header). |
| `delta.cpp` | The delta compression of `crates/cs-proto48/src/delta.rs`: `DELTA_MarkSendFields` equality, `DELTA_WriteDelta`, `DELTA_ParseDelta`, the built-in `g_MetaDelta`. |
| `common.cpp` | The bit and byte stream reader and writer of `crates/cs-proto48/src/bitbuf.rs` (`MSG_*`, including the overflow clamping) and `COM_Munge`, `COM_Munge2`, `COM_Munge3` and their inverses (`crypto.rs`). |
| `crc.cpp` | `CRC32`, `COM_BlockSequenceCRCByte`, `CRC_MapFile` (`crypto.rs`). |
| `info.cpp` | The `\key\value` info string parser and serializer (`crates/cs-proto48/src/info.rs`). |
| `cmodel.cpp` | `CM_CalcPAS` (the PAS built from the PVS rows). |
| `mathlib.cpp` | `AngleVectors`, `AngleVectorsTranspose`. |
| `r_studio.cpp` | Read for documentation only: `SV_HullForStudioModel` and `SV_CheckSphereIntersection` are described in the module docs as not ported; no code was taken. |
| `net.h`, `server.h` (engine headers, GPL header) | Buffer size constants only (`NET_MAX_PAYLOAD`, `MAX_DATAGRAM`, the size of `signon_data`). |
| `common/mathlib.h` | Read for the value of `M_PI` (a double literal) in comments only; no constant or code was taken (the engine uses `std::f64::consts::PI`). The file carries the GPL v2-or-later header with the linking exception, like the engine sources; it is not a Valve-noticed header. |
| `common/const.h`, `common/usercmd.h`, `common/entity_state.h` | Numeric values and names only: from `const.h` the `SOLID_*`, `MOVETYPE_*`, `FL_*`, `CONTENTS_*` and `EF_*` constants (the engine's `world.rs`, `collision.rs`, `unlag.rs`, `physics_water.rs`); from `usercmd.h` and `entity_state.h` the field names and types of `usercmd_t`, `clientdata_t` and `entity_state_t` that the delta tables and the protocol need (`cs-proto48/src/delta.rs`). These three files carry Valve LLC's copyright notice (with Id Software's) and restrict use, distribution and modification of the source and of the resulting object code to non-commercial enhancements to products from Valve LLC, and prohibit all other use without Valve's written permission. They do not carry the GPL header of the engine sources. |
| `common/qlimits.h`, `public/rehlds/edict.h` | Numeric values and names only: `MAX_PACKET_ENTITIES` and `MAX_LIGHTSTYLE_SIZE` from `qlimits.h`, `MAX_ENT_LEAFS` from `edict.h` (`world.rs`). Both carry a plain copyright line of Valve LLC (1996-2002, all rights reserved) and no terms of use. |

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
| `player.cpp`, `player.h` | The player's life: `CBasePlayer::Spawn`, `Killed`, `PlayerDeathThink`, `StartDeathCam`, `StartObserver`, `TakeDamage` usage, fall damage in `PlayerPreThink`/`PostThink`, `UpdateClientData` (the radar and status bar calls), `UpdateStatusBar`, `InitStatusBar`, `Radio`, `JoiningThink`, `GetIntoGame`, `HandleMenu_*` team and class choices, `AddAccount`, the HUD hide flags and `SBAR_*`/`MAX_ID_RANGE` values. |
| `client.cpp`, `client.h` | `ClientConnect`, `ClientPutInServer`, `ClientDisconnect`, `ClientKill`, `ClientCommand` (`jointeam`, `joinclass`, `menuselect`, `chooseteam`, `kill`, `nightvision`, `radio1` to `radio3`, `drop`, `ignorerad`, the buy aliases), `Host_Say` (chat formats, sanitising and flood limits), `ClientUserInfoChanged` (name changes), `Radio1` to `Radio3` and `HandleRadioAliasCommands`, `BuyItem`-related command parsing, `ClientSetupVisibility` (the observer part), the usermsg registration table (`LinkUserMessages`) and its order, `InitHUD`-related text. |
| `multiplay_gamerules.cpp`, `gamerules.cpp`, `gamerules.h` | `CHalfLifeMultiplay`: `Think`, `RestartRound`, `CheckWinConditions`, `CheckRoundTimeExpired`, `Round_Cts`/`Round_Ts` and `TerminateRound`, the freeze period, the money rewards and the loser bonus ladder, `PlayerKilled`, `DeathNotice`, `IPointsForKill`, `PlayerSpawn`, `GetPlayerSpawnSpot`, `FPlayerCanRespawn`, `InitHUD`, `UpdateTeamScores`, `GoToIntermission`, `ChangeLevel` and the map cycle, `ROUND_RESPAWN_TIME` and the other round constants. |
| `game.cpp`, `game.h` | The `mp_*` cvars the rules read and their defaults (`mp_timelimit`, `mp_freezetime`, `mp_roundtime`, `mp_startmoney`, `mp_friendlyfire`, `mp_tkpunish`, `mp_autokick`, `mp_playerid`, `mp_forcecamera`, `mp_forcechasecam`, `mp_fadetoblack`, `mp_limitteams`, `mp_autoteambalance`, `mp_buytime`, `mp_chattime`, `mp_winlimit`, `mp_maxrounds` and others), `PLAYERID_MODE_*`. |
| `world.cpp`, `bmodels.cpp`, `triggers.cpp`, `items.cpp`, `cbase.cpp`, `basemonster.cpp`, `subs.cpp` | `CWorld::Precache`/`Spawn` precache lists and world keys; `func_wall`, `func_illusionary` (`bmodels.cpp`); `func_buyzone`, `func_ladder` and the buy zone touch/`BuyZoneIcon_Set` (`triggers.cpp`); `CItemKevlar`/`CItemAssaultSuit` pickup behaviour (`items.cpp`); `CBaseEntity::TakeDamage` and entity dispatch rules (`cbase.cpp`); the `dmg_save` observation (`basemonster.cpp`); `info_player_*` spawn handling (`subs.cpp`, `gamerules.cpp`). |
| `util.cpp`, `util.h` | `UTIL_ScreenFade`, `UTIL_ClientPrintAll`/`ClientPrint` and their destinations (`HUD_PRINT*`, `HUD_PRINTRADIO`), `DMG_EXPLOSION`, `UTIL_PlayerByIndex`, the `UTIL_*` message helpers. |
| `weapontype.cpp`, `weapontype.h`, `weapons.cpp` | `g_weaponBuyAliasInfo` (the buy aliases), the prices `KEVLAR_PRICE`, `HELMET_PRICE`, `ASSAULTSUIT_PRICE`, `NVG_PRICE`; `W_Precache` (the `radio.spr` precache). |
| `unisignals.h`, `cdll_dll.h`, `observer.cpp`, `observer.h` | `CUnifiedSignals` and `SIGNAL_BUY`; `HIDEHUD_*`, `WEAPON_SUIT` and the other `cdll_dll.h` constants; `CBasePlayer::Observer_*` (`Observer_FindNextPlayer`, `Observer_HandleButtons`, `Observer_SetMode`, `Observer_CheckTarget`, `Observer_CheckProperties`), `CAMERA_MODE_*`, `FADETOBLACK_*`. |
| `regamedll/pm_shared/pm_shared.h`, `pm_shared.cpp` | `OBS_*` mode values; `PM_SpectatorMove` read for the observer's angle assumption (no movement code taken; the movement is `cs_sim::pmove`). |
| `regamedll/engine/shake.h` | `FFADE_*` values. |
| `regamedll/game_shared/unicode_strtools.cpp` (engine-side string helper) | `Q_StripUnprintableAndSpace` behaviour for chat text. |

Values that are named after the Half-Life SDK (`DMG_*` bit values, `SCORE_STATUS_DEAD`,
`HIDEHUD_*`) were taken from memory in places and are labelled as assumptions in the code; no
SDK header text was copied. `regamedll/dlls/damage.h` was not read.

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
