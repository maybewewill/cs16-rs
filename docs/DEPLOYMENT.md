# Deploying & Operating `cs16-rs`

This guide covers running `cs-server` as a standalone binary or Docker container, managing matches via the Web Admin Panel, locating Half-Life / Counter-Strike 1.6 assets on Linux, macOS, and Steam Deck, downloading per-round `.dem` replays, and sharing lobby invite links.

> **Assets notice:** No Valve game assets are bundled with `cs16-rs` or its container image. Mount your own Half-Life / CS 1.6 directory read-only at runtime.

---

## 1. Single-Binary `cs-server`

Build and run the dedicated 100 Hz Protocol 48 server:

```bash
cargo build --release -p cs-server --bin cs-server
./target/release/cs-server \
  --proto48 \
  --gamedir /path/to/Half-Life \
  --port 27015 \
  --web-port 27016 \
  --web-dir ./web \
  --map de_dust2 \
  --maxplayers 16 \
  --bots 4 \
  --hostname "CS 1.6 Rust Dedicated"
```

Key flags:
- `--gamedir <path>` (or `CS16_GAMEDIR` env var): Half-Life root containing `cstrike/` and `valve/`.
- `--port <udp>`: GoldSrc UDP game & RCON port (default `27015`).
- `--web-port <tcp>`: Enables the browser WebSocket game host, static asset server (`--web-dir`), and `/admin` HTTP API.
- `--relay-allow <ip:port>`: Optional WebSocket-to-UDP relay target allowlist (requires `--web-port`).
- `--mods-dir <dir>` / `--amxx-dir <dir>`: Load WASM mods or AMX Mod X `.amxx` plugins.
- `--master <ip:port>`: Register heartbeats with one or more master servers (repeatable).

Set `rcon_password` in your server console or startup config before exposing the Web Admin Panel:

```text
rcon_password your-strong-secret
```

---

## 2. Docker & Docker Compose

The root `Dockerfile` performs a reproducible multi-stage `--locked` Rust build and copies only the stripped `cs-server` binary and `web/` assets into `debian:bookworm-slim` running as an unprivileged `cs16` user.

### Build & Run with `docker compose`

```bash
export CS16_GAMEDIR=/srv/steam/Half-Life
export CS16_MAP=de_dust2
export CS16_MAXPLAYERS=16
export CS16_HOSTNAME="My CS 1.6 Server"

docker compose up -d --build
```

### Direct `docker run`

```bash
docker build -t cs16-rs/cs-server:latest .
docker run -d \
  --name cs16-server \
  -p 27015:27015/udp \
  -p 27016:27016/tcp \
  -v /srv/steam/Half-Life:/gamedir:ro \
  cs16-rs/cs-server:latest \
  --proto48 --gamedir /gamedir --port 27015 --web-port 27016 --web-dir /app/web --map de_dust2
```

Ports:
- **`27015/udp`**: Native GoldSrc Protocol 48 game traffic, A2S queries, and UDP RCON.
- **`27016/tcp`**: Web client (`/`), WebSocket transport (`/ws`), Web Admin UI (`/admin`), and Admin API (`/api/*`).

---

## 3. Web Admin Panel & HTTP API

Open `http://<server-host>:27016/admin` in any browser and authenticate using the server's configured `rcon_password`.

Capabilities:
- **Server Status & Live Players**: View hostname, map, round score, uptime, ping, team, frags/deaths, and Kick/Ban players via RCON (`kick #userid`, `banid <mins> #userid kick`).
- **Map Switcher & Bot Controls**: Change maps (`changelevel <map>`), adjust `bot_quota` and `bot_difficulty` (`0..=3`), or add/kick bots.
- **Interactive RCON Terminal**: Run arbitrary server console commands with Up/Down history.
- **Match Stats Leaderboard**: Live kills, deaths, K/D, HS%, assists, damage, ADR, plants, defuses, hostage rescues, and MVPs.

API endpoints (authenticated via `Authorization: Bearer <rcon_password>` or `X-Admin-Token`, verified in constant time):
- `GET /admin` — Admin HTML shell
- `GET /api/status` — Server status & available maps JSON
- `GET /api/players` — Connected players & bots JSON
- `GET /api/stats` — Match leaderboard & round history JSON
- `GET /api/replays` — Recorded round `.dem` index JSON
- `GET /api/replays/<id>.dem` — Download HLDEMO v5 round replay file
- `POST /api/rcon` — Execute `{"command":"..."}` (max 1024-byte command)
- `GET /api/invite` — Generate `?join=<host:port>&room=<code>` invite payload

---

## 4. Round `.dem` Replays & Match Stats

`cs-server` records per-round **HLDEMO Protocol 5** (`.dem`) files into a bounded in-memory ring buffer (`RoundReplayStore`, default 32 rounds / 128 MiB cap) using `DemoRecorder`.

- Download any completed round's `.dem` file directly from the **Round `.dem` Replays** table in `/admin` or via `GET /api/replays/<id>.dem`.
- Downloaded `.dem` files are standard GoldSrc Protocol 5 demos playable in `cs-client` or retail CS 1.6 (`viewdemo <file>` / `playdemo <file>`).
- `MatchStatsTracker` tracks per-round and match-wide ADR (`damage / rounds_played`), HS%, automatic damage-threshold assists (`>= 40` damage), bomb plants/defuses, hostage rescues, and round MVPs.

---

## 5. Lobby Invite Links

Use the **One-Click Lobby Invite Builder** in `/admin` (or `GET /api/invite?host=<host>&port=27016&room=<code>`) to generate shareable web join links:

```text
https://play.example.com/?join=play.example.com%3A27016&room=dust-01
```

Native GoldSrc and `cs-client` players can join directly from console with:

```text
connect play.example.com:27015
```

---

## 6. Linux, macOS & Steam Deck Paths

`cs_platform::xdg` locates configuration, user data, and Half-Life installations without `unsafe` code:

- **Linux (XDG)**:
  - Config: `$XDG_CONFIG_HOME/cs16-rs` (fallback `~/.config/cs16-rs`)
  - Data: `$XDG_DATA_HOME/cs16-rs` (fallback `~/.local/share/cs16-rs`)
  - Steam libraries: `~/.local/share/Steam`, `~/.steam/steam`, `~/.steam/debian-installation`, and Flatpak Steam (`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`), parsing `steamapps/libraryfolders.vdf`.
- **Steam Deck / SteamOS**:
  - Detected via `SteamDeck=1`, `XDG_CURRENT_DESKTOP=gamescope`, `/etc/steamos-release`, or `/etc/os-release` (`ID=steamos` / `VARIANT_ID=steamdeck`).
  - Checks `libraryfolders.vdf` plus microSD mounts (`/run/media/mmcblk0p1/SteamLibrary/steamapps/common/Half-Life`).
- **macOS**:
  - Config & Data: `~/Library/Application Support/cs16-rs`
  - Steam library: `~/Library/Application Support/Steam/steamapps/libraryfolders.vdf`
- **Override anywhere**: Set `CS16_GAMEDIR=/path/to/Half-Life` or pass `--gamedir /path/to/Half-Life`.
