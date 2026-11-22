# Parity ledger

`weights.toml` gives each category a weight; the weights sum to 100. Each `<category>.toml` lists `[[item]]` entries with `id`, `status` and an optional `evidence`. Every category file needs a key in `weights.toml`.

The game rules and HUD items of `cs-game` (teams, rounds, money, chat, observer, buying, radio and so on) live in `entities.toml`, in the `entities` category; the weights do not change.

The player movement port (`cs_sim::pm`, the engine's pmove services and the `SV_RunCmd` glue in `cs-game`) lives in `movement.toml`, the `movement` category.

## Statuses

- `missing`: no code for the item.
- `implemented`: code exists, but no test would fail if its behaviour broke. An item that a test only writes or parses on the way through is still `implemented`.
- `tested`: an existing test would fail if the item's behaviour broke. The test asserts on the item's value, either directly or through an equality assertion on a whole decoded message or struct that contains it. `evidence` names the test.
- `verified`: compared with the original game (Steam CS 1.6, protocol 48). `evidence` is required and must point at a golden-capture test or a dated checklist line.

## Running

From the repository root:

```
cargo xtask parity                  # per-category table and TOTAL verified/implemented
cargo xtask parity --min 25         # exit 1 if verified % is below 25
cargo xtask parity import-names <category> <dump-file>   # add names from a cvarlist/cmdlist dump as missing
```
