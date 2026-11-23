# Licensing

Decision recorded: 2026-11-21 (project owner's decision).

## Project licence

`cs16-rs` is licensed under the **GNU General Public License, version 3 or later**
(`license = "GPL-3.0-or-later"` in the root `Cargo.toml`). The full licence text is in
`LICENSE` at the repository root.

Until 2026-11-21 the project was licensed under MIT.

## Reference projects

The repository-level licences of both reference projects were checked on 2026-11-20 by reading
their `LICENSE` files on the GitHub `master` branch:

| Project | Repository `LICENSE` | Copyright line |
|---|---|---|
| ReHLDS | MIT | Copyright (c) 2014–2025 ReHLDS Contributors |
| ReGameDLL_CS | MIT | Copyright (c) 2015–2025 ReGameDLL_CS Contributors |

`LICENSE` at the repository root holds the GPL v3 text; `THIRD-PARTY-NOTICES.md` holds the
upstream copyright and permission notices of ReHLDS and ReGameDLL_CS. The protocol crate
`crates/cs-proto48` is ported from ReHLDS and ReGameDLL_CS.

## Resolved — file headers in the reference projects

Recorded: 2026-11-21.

What was found:

- The repository-level `LICENSE` files of ReHLDS and ReGameDLL_CS read as MIT, as recorded in
  the table above.
- The ReHLDS engine sources read (`sv_user.cpp`, `sv_main.cpp`, `sv_phys.cpp`, `world.cpp`,
  `delta.cpp`, `common.cpp`, `crc.cpp`, `net_ws.cpp`, `net_chan.cpp`, `info.cpp` and the others
  listed in `THIRD-PARTY-NOTICES.md`, all master, read 2026-11-21), the engine headers `net.h`
  and `server.h` and `rehlds/common/mathlib.h`, and ReGameDLL_CS `regamedll/dlls/player.cpp`
  (master, read 2026-11-21), carry a header stating that the file is licensed under the GNU
  General Public License, version 2 or later, with an exception permitting linking with Valve's
  Half-Life engine and game libraries. ReGameDLL_CS `regamedll/dlls/client.cpp` has no licence
  header. Files not listed in `THIRD-PARTY-NOTICES.md` were not examined.
- Phase 2C (read 2026-11-22): ReGameDLL_CS `regamedll/dlls/player.cpp` and the headers
  `player.h`, `client.h`, `gamerules.h`, `game.h`, `util.h`, `cdll_dll.h`, `weapontype.h`,
  `unisignals.h`, `observer.h`, `pm_shared/pm_shared.h` and `engine/shake.h` carry the same
  GPL v2-or-later header with the linking exception. The other `.cpp` files read (`client.cpp`,
  `multiplay_gamerules.cpp`, `gamerules.cpp`, `game.cpp`, `util.cpp`, `triggers.cpp`, `world.cpp`,
  `bmodels.cpp`, `items.cpp`, `cbase.cpp`, `basemonster.cpp`, `subs.cpp`, `weapontype.cpp`,
  `weapons.cpp`, `observer.cpp`, `pm_shared/pm_shared.cpp`) begin with
  `#include "precompiled.h"` and have no licence header, so the repository `LICENSE` (MIT) is
  the notice that applies to them. `THIRD-PARTY-NOTICES.md` says what was taken from each.
- ReHLDS `rehlds/engine/unicode_strtools.cpp` (master, read 2026-11-22; ported for the chat text
  clean-up of `cs-game`) carries the same GPL v2-or-later header with the linking exception as
  the other engine sources. Earlier notes placed it in ReGameDLL_CS (`game_shared`); that was a
  mistake.
- pm_shared port (read 2026-11-22; ReGameDLL_CS and ReHLDS master, the headers of the fetched
  copies checked): ReGameDLL_CS `regamedll/pm_shared/pm_shared.h`, `pm_defs.h`, `pm_movevars.h`,
  `pm_materials.h`, `pm_info.h`, `regamedll/game_shared/bot/nav.h` and `regamedll/dlls/player.cpp`
  carry the GPL v2-or-later header with the linking exception. `regamedll/pm_shared/pm_shared.cpp`
  (the ported movement code), `pm_math.cpp` and, among the `dlls` files, `client.cpp`,
  `multiplay_gamerules.cpp` and `doors.cpp` begin with `#include "precompiled.h"` and have no
  licence header, so the repository `LICENSE` (MIT) is the notice that applies to them. The ReHLDS
  engine files `pmove.cpp`, `pmovetst.cpp`, `sv_user.cpp`, `sv_pmove.cpp`, `sv_main.cpp`,
  `pr_cmds.cpp`, `model.cpp` and `tmessage.cpp` carry the GPL v2-or-later header with the linking
  exception. `THIRD-PARTY-NOTICES.md` says what was taken from each. The `FL_*`,
  `MOVETYPE_*`, `IN_*` and `CONTENTS_*` values in `crates/cs-sim/src/pm/consts.rs` are numeric
  values only, of the kind already listed under the Valve Half-Life SDK caveat; the ones marked
  there as `(HL SDK)` come from the standard SDK and are an assumption until checked.
- Phase 2D weapons port (read 2026-11-22; ReGameDLL_CS and ReHLDS master, the first 40 lines of each fetched copy checked): ReGameDLL_CS `regamedll/dlls/player.cpp`, `player.h`, `cbase.h`, `client.h`, `gamerules.h`, `util.h`, `weapons.h`, `weapontype.h`, `cdll_dll.h`, `decals.h`, `regamedll/common/const.h`, `entity_state.h`, `usercmd.h`, `weaponinfo.h`, `event_flags.h`, `regamedll/engine/studio.h`, `regamedll/pm_shared/pm_defs.h` and `pm_materials.h` carry the GPL v2-or-later header with the linking exception. These `dlls` files begin with `#include "precompiled.h"` (or with declarations) and have no licence header, so the repository `LICENSE` (MIT) is the notice that applies to them: `weapons.cpp`, `weapontype.cpp`, `cbase.cpp`, `combat.cpp`, `ggrenade.cpp`, `util.cpp`, `client.cpp`, `gamerules.cpp`, `multiplay_gamerules.cpp` and every `wpn_*.cpp` (`wpn_ak47.cpp` to `wpn_xm1014.cpp`, `wpn_c4.cpp` read for the survey only and not ported). `regamedll/dlls/eiface.h` has no licence header either. The ReHLDS engine files `sv_main.cpp`, `sv_user.cpp`, `sv_phys.cpp`, `pr_cmds.cpp`, `world.cpp` and `r_studio.cpp` carry the GPL v2-or-later header with the linking exception; of ReHLDS `decals.cpp`, `decals.h`, `wad.cpp`, and the `common/` headers `event_flags.h`, `event_args.h`, `weaponinfo.h`, `resource.h`, and `engine/delta.h` read for numeric values and layouts, the licence headers were NOT checked in this task: treat them like `common/const.h` (Valve's notice, numbers and names only) until they are. `THIRD-PARTY-NOTICES.md` says what was taken from each. `r_studio.cpp` was before listed as read for documentation only; since phase 2D `SV_HullForStudioModel`, `SV_StudioSetupBones` and `R_StudioHull` are ported (`crates/cs-engine/src/studio.rs`), so that row is changed.
- Phase 2D final review additions (read 2026-11-22; the first 40 lines of each fetched copy checked, one URL per fetch): ReGameDLL_CS `regamedll/dlls/hintmessage.h` and `activity.h` carry the GPL v2-or-later header with the linking exception. `regamedll/dlls/hintmessage.cpp`, `animation.cpp` and `animating.cpp` begin with `#include "precompiled.h"` and have no licence header, so the repository `LICENSE` (MIT) is the notice that applies to them. ReHLDS `rehlds/public/rehlds/customentity.h` carries Valve LLC's notice (use restricted to non-commercial enhancements to products from Valve LLC), so only the `BEAM_POINTS`, `BEAM_ENTPOINT`, `BEAM_ENTS` and `BEAM_HOSE` values (0 to 3) were taken from it, as numbers and names, like `common/const.h`. `THIRD-PARTY-NOTICES.md` says what was taken from each.
- Phase 2E objectives & bots port (read 2026-11-23; ReGameDLL_CS and ReHLDS master, file headers checked): ReGameDLL_CS `regamedll/dlls/player.cpp`, `player.h`, `client.h`, `weapons.h`, `weapontype.h`, `items.h`, `mapinfo.h`, `triggers.h`, `unisignals.h`, `hostage/hostage.cpp`, `hostage/hostage.h`, `hostage/hostage_localnav.cpp`, `hostage/hostage_localnav.h`, `bot/cs_bot_manager.cpp` (and `bot_manager.cpp` / `bot/cs_bot*.cpp` read for scope/command reference), and ReHLDS `pr_cmds.cpp`, `sv_user.cpp`, `sv_main.cpp` carry the GPL v2-or-later header with the linking exception. These ReGameDLL_CS `.cpp` files begin with `#include "precompiled.h"` and have no per-file licence header, so the repository `LICENSE` (MIT) applies to them: `triggers.cpp`, `mapinfo.cpp`, `wpn_c4.cpp`, `ggrenade.cpp`, `multiplay_gamerules.cpp`, `items.cpp`, `client.cpp`, `weapons.cpp`, `weapontype.cpp`, `combat.cpp`, `cbase.cpp`, `subs.cpp`, `basemonster.cpp`, `util.cpp` and `gs/bot.cpp`. `THIRD-PARTY-NOTICES.md` says what was taken from each.
- ReHLDS `rehlds/common/const.h`, `usercmd.h` and `entity_state.h` carry Valve LLC's notice,
  which restricts use, distribution and modification to non-commercial enhancements to products
  from Valve LLC; `rehlds/common/qlimits.h` and `rehlds/public/rehlds/edict.h` carry a plain
  Valve copyright line without terms of use.

Decision: the project owner chose GPL-3.0-or-later because it is compatible with both readings
of the reference projects' licensing.

Porting code from ReHLDS and ReGameDLL_CS is allowed again, with the source of ported code
named in `THIRD-PARTY-NOTICES.md`. Code copied from the Valve Half-Life SDK headers stays
excluded; see [Valve Half-Life SDK caveat](#valve-half-life-sdk-caveat) for what was taken.

## Copyright notice

```
Copyright (C) 2026 cs16-rs contributors

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
```

## Valve Half-Life SDK caveat

Parts of ReHLDS and ReGameDLL_CS originate from Valve's Half-Life SDK. Code that originates
from the Half-Life SDK carries Valve's SDK licence regardless of the licence of the reference
project it was read from, and no such code was copied.

What was taken from the headers that carry Valve's notice (`const.h`, `usercmd.h`,
`entity_state.h`, `qlimits.h`, `edict.h`) is the numeric constants and names the original
client, the game libraries and the wire protocol require to match: the `SOLID_*`, `MOVETYPE_*`,
`FL_*`, `CONTENTS_*` and `EF_*` constants, `MAX_PACKET_ENTITIES`, `MAX_ENT_LEAFS`,
`MAX_LIGHTSTYLE_SIZE`, and the field names and types of `usercmd_t`, `clientdata_t` and
`entity_state_t`. `THIRD-PARTY-NOTICES.md` lists them file by file. Whether taking these
numbers and names is acceptable under GPL-3.0-or-later is an open question for the owner.

## Valve assets

No Valve assets are stored in this repository. Game assets are mounted read-only from an
existing Steam installation at run time.
