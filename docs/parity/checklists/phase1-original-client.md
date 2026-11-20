# Checklist: the original client on the Protocol 48 server (phase 1)

Manual checks of `cs-server --proto48` with the original client (Steam CS 1.6, protocol 48). The automated tests run against `cs-probe` only; this checklist is the comparison with the real client. The server runs `WalkGame`: players spawn and walk, there are no weapons, rounds or HUD. 👤 These checks are run by the project owner.

Start the server from the repository root:

```
cargo run -p cs-server -- --proto48 --map de_dust2
```

In the original client, open the developer console (`~`) and type:

```
connect 127.0.0.1:27015
```

Fill in the date and the result (pass / fail plus what was seen) of every line. When a line fails, write down what the client showed, record a capture with `cs-probe --capture` against the reference server on the same scenario (see `docs/dev/reference-stand.md`), and fix the difference.

Items in the registries: `signon`, `delta_frames`, `usercmd_replay`, `msec_budget`, `timeouts`, `changelevel`, `a2s_queries` in `docs/parity/engine.toml`; the `svc_*` / `clc_*` opcodes in `docs/parity/wire.toml`.

| # | Check | Expected | Date | Result |
|---|-------|----------|------|--------|
| 1 | `connect 127.0.0.1:27015` in the original client | The loading bar runs to the end. | | |
| 2 | The map after loading | The map is drawn, the camera is at a spawn point. | | |
| 3 | Walking, jumping, crouching, a ladder | No jerks and no snapping back. | | |
| 4 | A second client, or `cs-probe --connect 127.0.0.1:27015 --walk --seconds 60` | The other player is visible and moves smoothly. | | |
| 5 | 10 minutes of play | No disconnect. | | |
| 6 | `disconnect`, then `connect 127.0.0.1:27015` again | Both work; the client is back in the game. | | |
| 7 | The server browser, LAN tab | The server is listed. | | |
