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

The copyright and permission notices are kept in `LICENSE` (this project) and
`THIRD-PARTY-NOTICES.md` (ReHLDS and ReGameDLL_CS) at the repository root. The protocol crate
`crates/cs-proto48` is ported from ReHLDS and ReGameDLL_CS.

## Resolved — file headers in the reference projects

Recorded: 2026-11-21.

What was found:

- The repository-level `LICENSE` files of ReHLDS and ReGameDLL_CS read as MIT, as recorded in
  the table above.
- Source files in these projects (for example `rehlds/engine/sv_user.cpp` on the ReHLDS
  `master` branch) carry a header stating that the file is licensed under the GNU General
  Public License, version 2 or later, with an exception permitting linking with Valve's
  Half-Life engine and game libraries.

Decision: the project owner chose GPL-3.0-or-later because it is compatible with both readings
of the reference projects' licensing.

Porting code from ReHLDS and ReGameDLL_CS is allowed again, with the source of ported code
named in `THIRD-PARTY-NOTICES.md`.

## Valve Half-Life SDK caveat

Parts of ReHLDS and ReGameDLL_CS originate from Valve's Half-Life SDK. Code that originates
from the Half-Life SDK carries Valve's SDK licence regardless of the licence of the reference
project it was read from. Anything traceable to the Half-Life SDK must be treated with that
in mind before it is ported.

## Valve assets

No Valve assets are stored in this repository. Game assets are mounted read-only from an
existing Steam installation at run time.
