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
Ported code keeps the attribution above, as the MIT licence requires. The protocol crate
`crates/cs-proto48` is ported from ReHLDS and ReGameDLL_CS on this basis.

## Valve Half-Life SDK caveat

Parts of ReHLDS and ReGameDLL_CS originate from Valve's Half-Life SDK. Code that originates
from the Half-Life SDK carries Valve's SDK licence regardless of the licence of the reference
project it was read from. Anything traceable to the Half-Life SDK must be treated with that
in mind before it is ported.

## Valve assets

No Valve assets are stored in this repository. Game assets are mounted read-only from an
existing Steam installation at run time.
