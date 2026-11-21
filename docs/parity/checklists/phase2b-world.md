# Checklist: players, brushes and movement in the world with the original client (phase 2B)

Manual checks of the world and physics of `cs-server --proto48` with the original client (Steam CS 1.6, protocol 48) in the game: players that collide with each other, solid brush entities, movement along walls, and smooth movement with a bad connection. The automated tests run against `cs-probe` only; this checklist is the comparison with the real client. 👤 These checks are run by the project owner.

Start the server from the repository root:

```
cargo run -p cs-server -- --proto48 --map de_dust2 --port 27015
```

In the original client, open the developer console (`~`) and type:

```
connect 127.0.0.1:27015
```

Rows 1, 2 and 6 need a second client: a second copy of the original client on the same machine or another machine (for the second client use the same `connect` command with the server's address), or `cargo run -p cs-probe -- --connect 127.0.0.1:27015 --seconds 120 --walk` as a walking stand-in for the second player where a row says so.

Fill in the date and the result (pass / fail plus what was seen) of every line. When a line fails, write down what the client showed and what the server window printed.

What is not checked here because it does not exist yet: doors, buttons, lifts and trains do not move (no map game logic before phase 7), there are no weapons (a grenade or a bullet cannot be fired), so the lag compensation cannot be shot at. Items in the registry: `entity_links`, `entity_traces`, `physics_movetypes`, `touch`, `pushers`, `player_world`, `lag_compensation`, `pas`, `player_movement_trace`, `player_impacts` in `docs/parity/engine.toml`.

| # | Check | Expected | Date | Result |
|---|-------|----------|------|--------|
| 1 | Two original clients are connected to the server and both are in the game on `de_dust2`. Walk one player straight into the other from the side, then jump onto the other player's head | The players do not pass through each other: the walking player is stopped at the other's body and can slide along it; there is no overlap of the two models. Standing on the head of the other player is possible or the player slides off, but never sinks into it. | | |
| 2 | The same two clients stand still next to each other, then one of them walks away and back several times | Neither client sees the other jump, vanish or flicker; the other player is visible all the time while in line of sight, including right after turning a corner. | | |
| 3 | One client on `de_dust2`. Walk into a solid wall of the map (any wall of the map) with `+forward` held and then strafe left and right along it | The player stops at the wall and slides along it; no jitter, no shaking of the view, no passing through. | | |
| 4 | One client on `de_dust2`. Find a `func_wall` or `func_breakable` brush entity (wooden crates and breakable boxes; if unsure which one is a brush entity, try several) and walk into it from every side; jump on top of it and stand there for 10 seconds | The brush is solid from every side (the player does not pass through it, as with a world wall); standing on it does not sink or jitter. The crate does not break and does not move yet (no game logic). | | |
| 5 | One client on `de_dust2`. Walk along a long wall at an angle, hold `+moveright` while pressing into the wall corner of a stair or a crate, and step up a stair and a low crate (18 units) | No jitter along the wall corners; stairs are climbed; no getting stuck in corners (the brushes of the map are now separate entities, the world walls are not). | | |
| 6 | Two original clients. On the first client open the console and type `fakelag 100` (the GoldSrc name of the command; an assumption, not checked against the Steam build) and, if the client does not know it, `net_fakelag 100`; if neither works, use `net_graph 3` on a connection with a real delay and write down what was used and walk around the other player on the second client. Shooting is not possible yet, so only the movement of the other player is checked | The other player moves without rubber-banding: no steps back, no jumps forward, no teleporting when it stops or turns. With a 100 ms delay the other player lags behind in time but moves smoothly. | | |
| 7 | One original client and `cargo run -p cs-probe -- --connect 127.0.0.1:27015 --seconds 120 --walk` as the second player. Watch the probe's player run around | The walking probe's player is visible and moves smoothly and is not stuck inside walls or crates; the client's own movement near it has no jitter. | | |
| 8 | One client on `de_dust2`. Walk through a big open area and back, and look at the far end of long corridors and through windows | The map's other player and the entities in the line of sight are visible; nothing visible pops in or out at the edges of a room, and nothing hides behind walls that should show it (the PVS now uses the full leaf list of each entity). | | |

Row 6 note: with `fakelag` (or `net_fakelag`) the packet delay is added by the original client on its side. If neither `fakelag` nor `net_fakelag` is available, write `not available` as the result and skip the row; do not fail the item for that.
