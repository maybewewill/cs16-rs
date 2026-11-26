# cs16-rs Polyglot Modding Guide (ABI v2)

> **Interactive Online Resources:**
> - **[Docs & API Portal (`/docs/`)](https://maybewewill.github.io/cs16-rs/docs/)** — Complete 52-hook reference (5-language tabs), 92 SDK API methods, 736 CVars & commands, 138 AMXX natives + live `.sma` converter, and interactive Menu/HUD/VFX studio.
> - **[Live Browser Python Playground (`/playground/`)](https://maybewewill.github.io/cs16-rs/playground/)** — Write & hot-reload Python 3 plugins in `< 5 ms` directly inside a playable WebGPU/WebGL2 match with live ResMon fuel/calls telemetry.
> - **[Plugin Hub (`/mods/` — 15 Built-In Mods)](https://maybewewill.github.io/cs16-rs/mods/)** — Searchable catalog with source viewer, `.py`/`.csmod` downloads, and 1-click *"Open in Playground"*.

`cs16-rs` ships a sandboxed, zero-`unsafe` modding platform where **Python 3**, **Rust**, **C/C++20**, **Go**, and legacy **AMX Mod X (`.amxx`)** plugins run side-by-side on the same 100 Hz Protocol 48 server (and inside the browser WebGPU/WebGL2 `cs-web` client!), sharing paginated `ShowMenu`, `TE_TEXTMESSAGE` / `DRC_CMD_MESSAGE` HUD & DHUD banners, per-player `i18n`, RBAC admins, `nvault` + $O(\log N)$ leaderboards, and synchronous cross-language RPC.

Every plugin runs inside strict fuel & memory caps: an infinite loop or runtime error faults only that plugin, keeps the server ticking at 100 Hz, and captures a deterministic black-box crash dump in `logs/mod_faults/` for offline `< 10 ms` replay.

---

## Built-In 15-Plugin Showcase Catalog (`configs/plugins.toml`)

All 15 built-in plugins compile/validate cleanly via `cargo xtask mod build` and are indexed in the online **[Plugin Hub (`/mods/`)](https://maybewewill.github.io/cs16-rs/mods/)**:

| Category | Plugin | Language | Highlights |
|---|---|---|---|
| **Standard** *(enabled)* | `mods/std/admin` | Rust WASM | RBAC admin commands (`amx_kick`, `amx_ban`, `amx_slay`, `amx_slap`, `amx_map`, `amx_csay`) |
| **Standard** *(enabled)* | `mods/std/mapmanager` | Go / WASI | `rtv`, `nominate`, `timeleft`, `nextmap`, paginated end-of-map vote |
| **Standard** *(enabled)* | `mods/std/statsx` | Rust WASM | Persistent K/D/HS `/rank`, `/top15`, `/me`, `/hp` with $O(\log N)$ leaderboard |
| **Standard** *(enabled)* | `mods/std/restmenu` | C++20 WASM | Weapon restriction menu (`CanBuyItem` hook) |
| **Community** *(enabled)* | `mods/community/resetscore.py` | Python 3 | `/rs` / `/resetscore` with VFX aura & EN/RU localization |
| **Community** *(enabled)* | `mods/community/vip_system.py` | Python 3 | VIP flags, bonus armor/money, round menus, golden weapons |
| **Community** *(enabled)* | `mods/community/afk_control.py` | Python 3 | Idle detection, bomb transfer & spectator/kick management |
| **Community** *(enabled)* | `mods/community/damager_and_parachute.py` | Python 3 | Live HUD hit-damage indicator + `+use` parachute glide |
| **Community** *(enabled)* | `mods/community/antiflood_admanager.py` | Python 3 | Chat flood protection & scheduled server announcements |
| **Gamemodes** *(opt-in)* | `mods/gamemodes/semiclip` | Rust WASM | Teammate collision pass-through + translucent fade |
| **Gamemodes** *(opt-in)* | `mods/gamemodes/csdm` | Rust WASM | Counter-Strike Deathmatch instant respawn, weapon menu & spawn protection |
| **Gamemodes** *(opt-in)* | `mods/gamemodes/gungame` | Rust WASM | 24-level weapon progression, knife steal & turbo mode |
| **Gamemodes** *(opt-in)* | `mods/gamemodes/zombie_plague` | Rust WASM | Infection rounds, Nemesis/Survivor bosses, laser mines & extra items |
| **Gamemodes** *(opt-in)* | `mods/gamemodes/retakes` | Rust WASM | **Competitive Retakes:** 3T vs 4CT Bombsite A/B scenarios (`de_dust2`/`de_inferno`/`de_nuke`/`de_mirage`/`de_train` + fallback), Pistol/Force-Buy/Full-Buy rounds, `/guns` allocator, max-1 AWP lottery, win-streak team scramble & EN/RU DHUD banners |
| **Gamemodes** *(opt-in)* | `mods/gamemodes/multi_1v1.py` | Python 3 | **Competitive Multi-1v1 Arena Ladder:** Arena `1..N` winner-up / loser-down promotion, Server Champion crown, spatial arena isolation + cross-arena damage guard, `/guns` Rifle/Pistol/AWP preferences, persistent Elo & EN/RU `/top1v1` / `!rank` |

---

## 1. Python 3 Zero-Compile Plugins (`mods/*.py`)

Drop any `.py` file into `mods/` (or `mods/community/`). No external Python runtime is needed — `cs-mod-py` runs a pure-Rust, fuel-metered Python 3 interpreter built directly into `cs-server` and `cs-local`. Edit the file on disk (or in `/admin` → **Mods & ResMon Studio**) and it hot-reloads automatically when `sv_mod_autoreload 1` is enabled while preserving `mod.storage`.

Type stubs for VS Code / Pyright / Pylance live in [`sdk/python/cs16.pyi`](../sdk/python/cs16.pyi).

```python
# mods/community/hello_vip.py
from cs16 import mod, vfx, Color, HookAction, Armor, Menu

@mod.info(
    id="hello_vip",
    version="1.0.0",
    permissions=["entities", "messages", "menus", "vfx", "admins", "storage", "commands"],
)
def _meta():
    pass

@mod.chat_command("/rs", "/resetscore")
def cmd_rs(player, args):
    player.reset_score()
    player.chat(f"{Color.GREEN}[RS]{Color.DEFAULT} {player.tr('RS_DONE', name=player.name)}")
    return HookAction.SUPERCEDE

@mod.hook("player_take_damage", phase="pre")
def on_damage(ev):
    if ev.attacker and ev.attacker.has_flag("t"):
        ev.damage = ev.damage * 1.15
        return HookAction.OVERRIDE
    return HookAction.CONTINUE

@mod.hook("player_killed", phase="post")
def on_kill(ev):
    if ev.killer:
        vfx.aura_light(ev.killer.origin, radius=18, rgb=(255, 215, 0), life=8, decay=15)
        ev.killer.add_money(300, flash=True)
```

---

## 2. Rust WASM SDK (`cs-mod-sdk` + `cargo xtask mod`)

Scaffold, build, and pack a Rust mod into `dist/mods/<id>.csmod`:

```bash
cargo xtask mod new my_gamemode --lang rust
cargo xtask mod build
```

Minimal server mod using `cs-mod-sdk` (`#![forbid(unsafe_code)]`, `wasm32-unknown-unknown`):

```rust
use cs_mod_sdk::{
    CustomWeaponBuilder, HookAction, HookId, HookPhase, HostBridge, Player, cs_mod_hook,
    cs_mod_init, vfx, weapon_id,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    CustomWeaponBuilder::new("golden_ak47", weapon_id::AK47)
        .display_name("Golden AK-47")
        .damage_mult(1.5)
        .clip_size(35)
        .knockback(1.8)
        .register(host);
    host.hook_register(HookId::PlayerSpawn, HookPhase::Post, 0);
    0
}

#[cs_mod_hook(PlayerSpawn, post)]
pub fn on_player_spawn(host: &mut impl HostBridge) -> HookAction {
    let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
    let p = Player::new(slot);
    p.give_custom_weapon(host, "golden_ak47");
    if let Some(pos) = p.origin(host) {
        vfx::shockwave_ring(host, pos, 180.0, [255, 200, 0, 220], 8, 12, None);
    }
    HookAction::Continue
}

cs_mod_sdk::cs16_wasm_exports! {
    init: init_mod,
    hooks: [(HookId::PlayerSpawn, HookPhase::Post) => on_player_spawn],
}
```

Unit-test your Rust mod natively without launching the engine using `cs_mod_sdk::MockHostBridge`.

---

## 3. C / C++20 WASM SDK (`sdk/cpp/cs16_mod.hpp`)

Single header-only C++20 SDK with zero RTTI/exceptions and zero heap allocations required:

```bash
cargo xtask mod new my_cpp_mod --lang cpp
clang++ --target=wasm32-unknown-unknown -O2 -std=c++20 -nostdlib \
  -Wl,--no-entry -Wl,--allow-undefined -Wl,--export-dynamic \
  -I sdk/cpp mods/my_cpp_mod/main.cpp -o mods/my_cpp_mod/server.wasm
```

```cpp
#include "cs16_mod.hpp"

CS16_PLUGIN_INFO("no_awp", "Restrict AWP", "1.0.0")

extern "C" CS16_EXPORT int32_t cs_mod_init() {
    cs16::hooks::register_hook(cs16::HookId::CanBuyItem, cs16::HookPhase::Pre, 0);
    return 0;
}

extern "C" CS16_EXPORT int32_t cs_mod_hook(int32_t hook_id, int32_t) {
    if (hook_id == static_cast<int32_t>(cs16::HookId::CanBuyItem)) {
        auto item_id = cs16::hooks::arg_i32(1);
        if (item_id == 18 /* AWP */) {
            cs16::Player(cs16::hooks::arg_i32(0)).say_text("\x04[Restrict]\x01 AWP is disabled.");
            return static_cast<int32_t>(cs16::HookAction::Supercede);
        }
    }
    return static_cast<int32_t>(cs16::HookAction::Continue);
}
```

---

## 4. Go WASM SDK (`sdk/go/cs16mod`)

Build standard Go (`>= 1.24`, `GOOS=wasip1 GOARCH=wasm -buildmode=c-shared`) plugins — the host provides minimal safe `wasi_snapshot_preview1` shims (`fd_write` captured to host logs, `clock_time_get`, `random_get`, `proc_exit` isolating only the plugin):

```bash
cargo xtask mod new my_go_mod --lang go
cd mods/my_go_mod
GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -o server.wasm .
```

```go
//go:build wasm
package main

import "github.com/cs16-rs/cs16-rs/sdk/go/cs16mod"

//export cs_mod_init
func cs_mod_init() int32 {
    cs16mod.RegisterHook(cs16mod.HookPlayerSay, cs16mod.PhasePre, 0)
    return 0
}

//export cs_mod_hook
func cs_mod_hook(hookID, _ int32) int32 {
    if hookID == cs16mod.HookPlayerSay && cs16mod.HookArgStr(0) == "nextmap" {
        p := cs16mod.NewPlayer(cs16mod.HookArgI32(0))
        p.SayText("\x04[MAPS]\x01 Next map: de_inferno")
        return cs16mod.ActionSupercede
    }
    return cs16mod.ActionContinue
}

func main() {}
```

---

## 5. Platform Primitives: VFX, Custom Weapons, i18n, RBAC, Menus & RPC

- **1-Line VFX Builder (`vfx_*`):** Emits authentic Protocol 48 `SVC_TEMPENTITY` messages (`vfx_laser_beam`, `vfx_shockwave_ring`, `vfx_explosion`, `vfx_sparks`, `vfx_blood_spray`, `vfx_aura_light`, `vfx_tracer`, `vfx_smoke_puff`, `vfx_glow_sprite`, `vfx_beam_follow`, `vfx_implosion`, `vfx_screen_fade`) — renders identically on vanilla `hl.exe`, `cs-client`, and `cs-web`.
- **Declarative Custom Weapons:** Register weapons (`CustomWeaponSpec`: base weapon, `v_`/`p_`/`w_` models, fire sound, `damage_mult`, `clip_size`, `deploy_speed_mult`, `recoil_mult`, `knockback`, `cost`) in `cs_mod_init`; the host precaches assets and applies stats automatically in `cs-game`.
- **Per-Player `i18n` (`configs/lang/*.toml` & `data/lang/*.txt`):** Players change language live via `/lang en` or `/lang ru` (persisted via `ConfigStorage`; server default controlled by `amx_language`). Both native mods (`player.tr("KEY", ...)`) and `.amxx` plugins (`%L`) share the same `LangCatalog`.
- **RBAC Admins (`configs/admins.toml` & `users.ini`):** Unified `a..z` AMXX access flags verified by SteamID, IP, or Name + `_pw` password.
- **Paginated GoldSrc Menus (`MenuManager`):** Automatic `1..7`, `8. Back`, `9. More`, `0. Exit` pagination with authentic `\y` / `\w` / `\r` / `\d` / `\R` CS 1.6 text styling and zero `menuselect` collisions across `.wasm`, `.py`, and `.amxx` plugins.
- **Polyglot RPC (`RpcBus`):** Synchronous `rpc_export` / `rpc_call` and `rpc_emit_event` / `rpc_subscribe` work bidirectionally across Rust, C++, Go, Python, and AMXX (`register_native` / `CreateMultiForward`).
- **Allowlisted Async Webhooks:** Declare `permissions = ["http:discord.com"]` (or `http_hosts = ["discord.com"]` in `mod.toml`) to send non-blocking HTTP POST JSON requests off the 100 Hz tick thread.

---

## 6. 5-Second Legacy AMX Mod X Server Migration

Convert an existing `cstrike/addons/amxmodx/` installation (`users.ini`, `plugins.ini`, `plugins-*.ini`, `maps.ini`, `amxx.cfg`, `data/lang/*.txt`) in one command:

```bash
cargo run -p cs-server -- --import-amxx /path/to/cstrike/addons/amxmodx --out-dir .
# or via xtask:
cargo xtask import-amxx /path/to/cstrike/addons/amxmodx --out-dir .
```

- Automatically upgrades enabled stock AMXX plugins (`admin.amxx`, `admincmd.amxx`, `mapchooser.amxx`, `statsx.amxx`, `restmenu.amxx`, `antiflood.amxx`, etc.) to their native `cs16-rs` equivalents (`std_admin`, `std_mapmanager`, `std_statsx`, `std_restmenu`, `antiflood_admanager.py`).
- Mounts custom third-party `<custom>.amxx` files directly into `configs/plugins.toml` on the built-in `cs-amxx` Pawn 3.2 VM.
- Boot the unified server with `configs/plugins.toml`:

```bash
cargo run -p cs-server -- --map de_dust2 --plugins-toml configs/plugins.toml --admin
```

---

## 7. Time-Travel Crash Debugging (`cargo xtask mod replay`)

When any `.wasm`, `.py`, or `.amxx` plugin traps, panics, or hits `OutOfFuel`, `cs-server` isolates that plugin, keeps running at 100 Hz, and writes a bounded JSON snapshot (`<= 256 KiB`, ring-buffered to 32 dumps) to `logs/mod_faults/<timestamp>_<mod_id>_<trigger>.json`.

Download the dump from `/admin` → **Mods & ResMon Studio** (or grab it from `logs/mod_faults/`) and replay the exact failing invocation locally in `< 10 ms`:

```bash
# Reproduce the exact crash offline:
cargo xtask mod replay logs/mod_faults/1732560000_my_mod_player_take_damage_pre.json

# Verify your fix against the exact same production snapshot:
cargo xtask mod replay logs/mod_faults/1732560000_my_mod_player_take_damage_pre.json \
  --mod-path mods/community/my_mod.py
```

---

## 8. Live Resource Monitor (`resmon`) & Web Admin Plugin Studio

Server / RCON console commands:
- `mod_list` / `amx_plugins` — unified table across Rust/WASM, C++/WASM, Go/WASM, Python, and AMXX (`ID`, `LANGUAGE`, `VERSION`, `STATUS`, `AVG us/tick`, `MEM KB`).
- `mod_stats` — live `resmon` view sorted descending by CPU time (`AVG us`, `P99 us`, `MAX us`, `FUEL`, `MEM KB`, `CALLS`, `FAULTS`).
- `mod_pause <id>` / `mod_unpause <id>` — suspend/resume any `.wasm`, `.py`, or `.amxx` plugin live.
- `mod_reload <id|all>` — hot-reload one plugin or all plugins from disk.

Web Admin (`/admin`, authenticated by `rcon_password` Bearer token):
- **Mods & ResMon Studio** tab shows live color-coded CPU bars (`< 50 us` green, `< 250 us` yellow, `>= 250 us` red), 1-click Pause/Resume/Reload, black-box fault dump downloads (`GET /api/mods/faults/<id>.json`), and a dark-mode source editor (`GET`/`PUT /api/mods/source/<file>`) strictly sandboxed to `mods/*.py` and `configs/*.{toml,cfg,txt}` with instant `< 5 ms` **Save & Hot-Reload**.
