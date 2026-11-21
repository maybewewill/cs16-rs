# Third-party notices

`cs16-rs` contains code ported from the following projects. Code ported from them is
distributed in this repository under GPL-3.0-or-later, the licence of this project; see
`docs/LICENSING.md`.

## ReHLDS

Upstream: https://github.com/rehlds/ReHLDS

The repository-level `LICENSE` of ReHLDS is MIT (notice below); the engine source files listed
below (master, read 2026-11-21) carry a header stating GPL v2-or-later with an exception
permitting linking with Valve's Half-Life engine and game libraries. Files not listed here
were not examined.

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
| `net_chan.cpp` | `Netchan_Transmit` (the overflow rule for unreliable data next to reliable data). |
| `cmodel.cpp` | `CM_CalcPAS` (the PAS built from the PVS rows). |
| `mathlib.cpp` | `AngleVectors`, `AngleVectorsTranspose`. |
| `r_studio.cpp` | Read for documentation only: `SV_HullForStudioModel` and `SV_CheckSphereIntersection` are described in the module docs as not ported; no code was taken. |
| `net.h`, `server.h` (engine headers, GPL header) | Buffer size constants only (`NET_MAX_PAYLOAD`, `MAX_DATAGRAM`, the size of `signon_data`). |
| `common/const.h`, `common/qlimits.h`, `public/rehlds/edict.h`, `mathlib.h` | Numeric values and names only: the `SOLID_*`, `MOVETYPE_*` and `FL_*` constants, `MAX_PACKET_ENTITIES`, `MAX_LIGHTSTYLE_SIZE`, `MAX_ENT_LEAFS`, and the double-precision `M_PI`. These headers do not carry the GPL header of the engine sources: `const.h`, `qlimits.h` and `edict.h` carry Valve's (and Id's) copyright notice for the Half-Life SDK, `mathlib.h` carries none in its first lines. Only these interface constants, which the original client and game libraries require to match, were used; no other text of these files was taken. |

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

The repository-level `LICENSE` of ReGameDLL_CS is MIT (notice below); the source file examined,
`regamedll/dlls/player.cpp` (master, read 2026-11-21), carries a header stating GPL v2-or-later
with an exception permitting linking with Valve's Half-Life engine and game libraries. In phase 2B `regamedll/dlls/cbase.cpp`
(`SetObjectCollisionBox`) and `regamedll/dlls/util.cpp` (`UTIL_EntitiesInBox`) were read
for comparison only (master, read 2026-11-21): the engine's linking and `entities_in_box`
were checked against them and no code was taken from these two files. Other files of this
project were not examined.

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
