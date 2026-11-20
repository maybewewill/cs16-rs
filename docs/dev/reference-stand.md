# Reference stand: original HLDS for golden captures

The reference stand is an original Counter-Strike 1.6 dedicated server (HLDS) that `cs-probe`
connects to in order to record golden captures. Captures are never committed: they live in the
directory named by the environment variable `CS16_GOLDEN_DIR`.

> **Untested.** None of the commands below has been run on this machine yet. They describe the
> intended setup; correct them when the stand is first brought up.

## 1. Install HLDS with SteamCMD

1. Download SteamCMD from Valve (<https://developer.valvesoftware.com/wiki/SteamCMD>) and unpack
   it, for example to `C:\steamcmd` (Windows) or `~/steamcmd` (Linux).
2. Install the dedicated server (app 90) with the `cstrike` mod. App 90 often needs the install
   command more than once before it reports success:

   ```sh
   # untested
   steamcmd +force_install_dir ./hlds +login anonymous \
            +app_set_config 90 mod cstrike \
            +app_update 90 validate +quit
   ```

3. Check that `hlds/cstrike/` exists and contains `maps/de_dust2.bsp`.

## 2. Start the server

From the `hlds` directory:

```sh
# untested — Windows
hlds.exe -console -game cstrike -port 27015 +sv_lan 1 +mp_consistency 0 +maxplayers 8 +map de_dust2

# untested — Linux
./hlds_run -game cstrike -port 27015 +sv_lan 1 +mp_consistency 0 +maxplayers 8 +map de_dust2
```

- `sv_lan 1` lets the probe in with its LAN auth ticket (no Steam backend check).
- `mp_consistency 0` stops the server from requesting file-consistency checks, which the probe
  does not answer (with a consistency list it stops with
  `consistency required: set mp_consistency 0`).

## 3. Optional: ReHLDS + ReGameDLL_CS

ReHLDS (<https://github.com/dreamstalker/rehlds>) and ReGameDLL_CS
(<https://github.com/s1lentq/ReGameDLL_CS>) are MIT-licensed reimplementations of the engine and
the game library; their sources are what `cs-proto48` is ported from. To compare against them:

1. Copy the ReHLDS release binaries over the HLDS ones (`swds.dll`/`engine_i486.so` and friends).
2. Copy the ReGameDLL_CS release `mp.dll`/`cs.so` over `cstrike/dlls/`, plus its
   `game.cfg` into `cstrike/`.
3. Start the server with the same command line as above. *(untested)*

Record captures from the original HLDS first; ReHLDS captures are a secondary reference.

## 4. Record a capture

```sh
# untested
export CS16_GOLDEN_DIR=/path/to/golden          # PowerShell: $env:CS16_GOLDEN_DIR = "C:\golden"
cargo xtask golden 127.0.0.1:27015 hlds_dust2_walk
```

This runs `cs-probe --connect 127.0.0.1:27015 --capture $CS16_GOLDEN_DIR/hlds_dust2_walk.cs48cap
--seconds 30 --walk`. It succeeds when the probe reached `InGame`; the capture holds every
datagram in both directions.

Then check that the decoder understands everything the server sent:

```sh
# untested
cargo test -p cs-probe --test golden_replay
```

The test replays every `*.cs48cap` in `CS16_GOLDEN_DIR` and fails on any parse error or
`SvcMessage::Ignored` message. Without `CS16_GOLDEN_DIR` it passes without checking anything.

## Unverified working assumptions

The probe's sign-on sequence (`new` → `sendres` → empty `clc_resourcelist` → `spawn <count> <crc>`
→ `sendents` → first entity update) and the format of the `spawn` arguments are recalled HLDS
behaviour. They stay `implemented`, not `verified`, until a capture from this stand confirms them.
