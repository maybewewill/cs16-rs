# Reference stand: original HLDS for golden captures

The reference stand is an original Counter-Strike 1.6 dedicated server (HLDS) that `cs-probe`
connects to in order to record golden captures. Captures are never committed: they live in the
directory named by the environment variable `CS16_GOLDEN_DIR`.

> **Tested on the development machine (2026-11-22)** with the `hlds.exe` of the Steam Half-Life
> install (build 10210, section 1). The SteamCMD install (section 1b), the Linux command and the
> ReHLDS / ReGameDLL_CS variant (section 3) have not been run.

## 1. Run the HLDS of the Steam Half-Life install

The Steam install of Half-Life (`...\steamapps\common\Half-Life`) holds `hlds.exe` and the
`cstrike` mod with `maps/de_dust2.bsp`. Nothing else has to be installed.

1. Put a file `steam_appid.txt` with the single line `10` next to `hlds.exe`. Without it the
   server stops with `Unable to initialize Steam`. First check whether a `steam_appid.txt` is
   already there: if one is, back it up (copy it to `steam_appid.txt.bak`) before writing the
   new one, and put it back afterwards. If it was overwritten without a backup, restore it with
   Steam's "Verify integrity of game files" for Half-Life.
2. Start it from that directory:

   ```sh
   hlds.exe -console -game cstrike -port 27016 +sv_lan 1 +mp_consistency 0 +maxplayers 8 +map de_dust2
   ```

- Port `27016` keeps the stand clear of a server of ours on the default `27015`.
- `sv_lan 1` lets the probe in with its LAN auth ticket (no Steam backend check).
- `mp_consistency 0` stops the server from requesting file-consistency checks, which the probe
  does not answer (with a consistency list it stops with
  `consistency required: set mp_consistency 0`).

### 1b. Install HLDS with SteamCMD instead

Not run on this machine. App 90 often needs the install command more than once before it
reports success:

```sh
steamcmd +force_install_dir ./hlds +login anonymous \
         +app_set_config 90 mod cstrike \
         +app_update 90 validate +quit
```

Then start `hlds.exe` (Windows) or `./hlds_run` (Linux, same arguments, also not run) from
`hlds` with the command line of section 1 (the `steam_appid.txt` file is the same).

## 2. Record a capture

```sh
export CS16_GOLDEN_DIR=/path/to/golden          # PowerShell: $env:CS16_GOLDEN_DIR = "C:\golden"
cs-probe --connect 127.0.0.1:27016 --capture "$CS16_GOLDEN_DIR/hlds_join.cs48cap" --seconds 15 \
         --cmd "jointeam 2" --cmd "joinclass 5"
```

This is the join capture of the first comparison: the probe connects, signs on, and chooses
the CT team and a class once in game. It exits with status 0 when it reached `InGame`; the
capture holds every datagram in both directions. Without `--cmd` the probe only connects.
`--walk` makes it walk forward as well.

`cargo xtask golden <ip:port> <name>` records a 30 s walking capture into `CS16_GOLDEN_DIR`
(not run on this machine).

Then check that the decoder understands everything the server sent:

```sh
cargo test -p cs-probe --test golden_replay
```

The test replays every `*.cs48cap` in `CS16_GOLDEN_DIR` and fails on any parse error or
`SvcMessage::Ignored` message. Without `CS16_GOLDEN_DIR` it passes without checking anything.

## 3. Read a capture: dump and diff

```sh
cs-probe --dump CAPTURE [--from S] [--to S] [--no-entities]
cs-probe --diff A B [--no-entities]
```

`--dump` decodes a capture offline with the decoder the probe uses live and prints one line per
message, in arrival order, prefixed with the time of the datagram and `S` (server to client) or
`C` (client to server):

- the `svc_*` name; a user message is `usermsg:<registered name>` with its fields, as hex where
  `cs_proto48::usermsg` has no typed decoder;
- the text of `svc_stufftext`, `svc_print` and `svc_centerprint`, sound and event arguments;
- for `svc_clientdata`, the fields that changed against the previous one (`weaponN.field` for
  the weapon data), and for the entity updates the changed fields per entity;
- one summary line per client datagram: `clc_move` (new commands, newest `msec`, `buttons`,
  `weaponselect`), `clc_stringcmd`, `clc_delta`.

`--from` and `--to` cut by time (seconds on the probe's clock), `--no-entities` leaves out the
entity updates. The padding `svc_nop` is not shown.

`--diff` aligns two captures and prints what differs:

- The captures are cut into phases at each string command the probe sent (`new`, `sendres`,
  `spawn`, `sendents`, `jointeam`, ...); phases are matched by command and occurrence.
- In a phase, the messages that are not sent every frame are aligned by a longest common
  subsequence over their kind (`svc_*` name, `usermsg:<name>`); a matched pair is compared field
  by field. The report lists the messages in one capture only, the messages in a different place,
  and the fields that differ.
- The per-frame messages (`svc_time`, `svc_clientdata`, entity updates, pings) are counted per
  phase, and the first and the last `svc_clientdata` of the phase are compared. This is also how
  the update rates differ.
- Normalised, so they do not show: player names, `svc_time` values, `random_seed`, connectionless
  text, the arguments of string commands and, with `--no-entities`, everything about entities.
  Without it, the entity numbers above the players are not compared one by one: the report
  compares how many non-player entities an update has and which player fields changed.

Both modes only read files. A dump or a report holds Valve-server output: do not commit it.

## 4. Optional: ReHLDS + ReGameDLL_CS

ReHLDS (<https://github.com/dreamstalker/rehlds>) and ReGameDLL_CS
(<https://github.com/s1lentq/ReGameDLL_CS>) are reimplementations of the engine and the game
library; their sources are what `cs-proto48` is ported from (licensing: see
[`docs/LICENSING.md`](../LICENSING.md)). To compare against them:

1. Copy the ReHLDS release binaries over the HLDS ones (`swds.dll`/`engine_i486.so` and friends).
2. Copy the ReGameDLL_CS release `mp.dll`/`cs.so` over `cstrike/dlls/`, plus its
   `game.cfg` into `cstrike/`.
3. Start the server with the same command line as above. *(untested)*

Record captures from the original HLDS first; ReHLDS captures are a secondary reference.

## Working assumptions still to check

The first join capture (`jointeam 2`, `joinclass 5`) confirmed the probe's sign-on sequence
(`new` → `sendres` → empty `clc_resourcelist` → `spawn <count> <crc>` → `sendents` → first
entity update) against the HLDS of build 10210: it reaches `InGame` with 0 parse errors. Other
paths of the sign-on (a map change, a server that asks for consistency checks) have not been
recorded.
