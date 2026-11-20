# Checklist: map change with connected clients (phase 1)

Manual checks of `Server::change_map` with the original client (Steam CS 1.6, protocol 48) connected to this server. The loopback tests run against `cs-probe` only, and the probe is more forgiving than the real client in the two cases below. Each check is done in a separate session. Fill in the date and the result (pass / fail plus what was seen); a failing line needs a note of what the client did.

Item in the registry: `changelevel` in `docs/parity/engine.toml`.

| # | Case | Steps | Expected | Date | Result |
|---|------|-------|----------|------|--------|
| 1 | Duplicated `reconnect` | Join the server and wait until in game. Have the server send a reliable message (for example a name change by another player) and change the map within the same frame, before the client's acknowledgement arrives; repeat until the server log shows a resend of the `reconnect` (it is delivered twice when the client had received the abandoned message). | The client loads the new map once, ends in game on it and stays connected; the console shows no second "Connection accepted"/load loop and no error. | | |
| 2 | Half-received fragment stream from the old map | Join the server. While the client is still loading (sign-on data of the first map arriving as fragments, e.g. right after "Sending resource list"), change the map, then change it again while it loads the second map. | The client drops the old map's partial stream, loads the last map, ends in game on it and stays connected; no "illegal fragment" / overflow / disconnect. | | |
