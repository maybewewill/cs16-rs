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
- The source files examined — ReHLDS `rehlds/engine/sv_user.cpp` (master, read 2026-11-21) and
  ReGameDLL_CS `regamedll/dlls/player.cpp` (master, read 2026-11-21) — carry a header stating
  that the file is licensed under the GNU General Public License, version 2 or later, with an
  exception permitting linking with Valve's Half-Life engine and game libraries. Other files of
  these projects were not examined.

Decision: the project owner chose GPL-3.0-or-later because it is compatible with both readings
of the reference projects' licensing.

Porting code from ReHLDS and ReGameDLL_CS is allowed again, with the source of ported code
named in `THIRD-PARTY-NOTICES.md`. Code derived from the Valve Half-Life SDK stays excluded; see
[Valve Half-Life SDK caveat](#valve-half-life-sdk-caveat).

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
project it was read from. Anything traceable to the Half-Life SDK must be treated with that
in mind before it is ported.

## Valve assets

No Valve assets are stored in this repository. Game assets are mounted read-only from an
existing Steam installation at run time.
