# Licensing

Decision recorded: 2026-11-20.

## Project licence

`cs16-rs` is licensed under the **MIT** licence (`license = "MIT"` in the root `Cargo.toml`).

## Reference projects

The licences of both reference projects were checked on 2026-11-20 by reading their
`LICENSE` files on the GitHub `master` branch:

| Project | Licence | Copyright line |
|---|---|---|
| ReHLDS | MIT | Copyright (c) 2014–2025 ReHLDS Contributors |
| ReGameDLL_CS | MIT | Copyright (c) 2015–2025 ReGameDLL_CS Contributors |

Because both are MIT, their source code may be read and ported into this repository.
The copyright and permission notices are kept in `LICENSE` (this project) and
`THIRD-PARTY-NOTICES.md` (ReHLDS and ReGameDLL_CS) at the repository root. The protocol crate
`crates/cs-proto48` is ported from ReHLDS and ReGameDLL_CS on this basis.

## Open question — file headers in the reference projects

Recorded: 2026-11-21. **Unresolved: this needs the project owner's decision.**

- The repository-level `LICENSE` files of ReHLDS and ReGameDLL_CS read as MIT, as recorded in
  the table above.
- The source file `rehlds/engine/sv_user.cpp` on the ReHLDS `master` branch carries a header
  stating that the file is licensed under the GNU General Public License, version 2 or later,
  with an exception permitting linking with Valve's Half-Life engine and game libraries.
- These two statements have not been reconciled.
- `crates/cs-proto48` describes itself as ported from ReHLDS and ReGameDLL_CS.
- Until the project owner decides, no further code is to be ported from files carrying that
  header. The behaviour of such files may be studied and reimplemented in this project's own
  code.

## Valve Half-Life SDK caveat

Parts of ReHLDS and ReGameDLL_CS originate from Valve's Half-Life SDK. Code that originates
from the Half-Life SDK carries Valve's SDK licence regardless of the licence of the reference
project it was read from. Anything traceable to the Half-Life SDK must be treated with that
in mind before it is ported.

## Valve assets

No Valve assets are stored in this repository. Game assets are mounted read-only from an
existing Steam installation at run time.
