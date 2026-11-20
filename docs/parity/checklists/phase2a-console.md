# Checklist: server console and engine services with the original client (phase 2A)

Manual checks of the `cs-server --proto48` console (`status`, `kick`, `say`, `changelevel`, `quit`) and of the graceful shutdown, with the original client (Steam CS 1.6, protocol 48) in the game. The automated tests run against `cs-probe` only; this checklist is the comparison with the real client. 👤 These checks are run by the project owner.

Start the server from the repository root; every command below is typed into the console of this server window (the terminal where `cargo run` is running), then Enter:

```
cargo run -p cs-server -- --proto48 --map de_dust2 --port 27015
```

In the original client, open the developer console (`~`) and type:

```
connect 127.0.0.1:27015
```

Fill in the date and the result (pass / fail plus what was seen) of every line. When a line fails, write down what the client showed and what the server window printed.

Items in the registry: `console`, `game_messages`, `map_entity_spawn`, `think`, `changelevel`, `timeouts` in `docs/parity/engine.toml`.

| # | Check | Expected | Date | Result |
|---|-------|----------|------|--------|
| 1 | The client is in the game on `de_dust2`. On the server console type `status` | The server prints the hostname, the map `de_dust2` and one line for the client: a 1-based slot, the state `active`, its name in quotes, its address and its ping. | | |
| 2 | The client is in the game. On the server console type `kick #1 testing the kick` (`#<userid>`; take the userid from `status`) | The client returns to the menu and shows the text `testing the kick` as the reason. The server console prints a `Kicked ...` line and keeps running. | | |
| 3 | Connect again. On the server console type `kick 1 second kick` (`kick <slot> <reason>`; the slot is the number `status` shows) | The same as row 2 with the text `second kick`. | | |
| 4 | Connect again. On the server console type `say hello from the server` | The text `Console: hello from the server` appears in the client's chat or console. | | |
| 5 | The client is in the game on `de_dust2`. On the server console type `changelevel cs_office` | The client loads `cs_office` by itself, without typing anything, ends in the game on the new map and stays connected; no error in the client console. | | |
| 6 | The client is in the game. On the server console type `changelevel de_dust2` and, as soon as the server prints the line, `changelevel cs_office` again (twice in a row, quickly) | The client ends in the game on the last map (`cs_office`) and stays connected; no disconnect, no error in the client console. The server keeps running. | | |
| 7 | The client is in the game. On the server console type `changelevel <a map that exists in the server's `cstrike/maps` but not in the client's>` (for example a custom map copied only to the server) | The client reports that it cannot find the map, or disconnects cleanly to the menu; the server keeps running and accepts new connections. Also try `changelevel no_such_map_anywhere`: the server prints `changelevel failed` and the client stays in the game on the old map. | | |
| 8 | The client is in the game. In the server window press Ctrl+C | The client sees the disconnect at once (the menu appears within a second or two, not after the connection timeout of about 30 seconds) and shows the server's shutdown text. The server prints `shutting down` and exits. | | |

The `kick` forms: `kick #<userid> <reason>` and `kick <slot> <reason>`; a name that matches exactly one client also works (`kick <name> <reason>`). Without a reason the kick uses the default reason text.
