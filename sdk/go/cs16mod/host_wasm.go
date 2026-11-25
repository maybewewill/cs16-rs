//go:build wasm

// Package cs16mod provides the idiomatic Go WebAssembly SDK for cs16-rs ABI v2 mods.
//
// Copyright (C) 2026 cs16-rs contributors
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Build a plugin as a WASI reactor module with Go 1.24+:
//
//	GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -o server.wasm .
//
// Or with TinyGo:
//
//	tinygo build -target=wasm-unknown -o server.wasm .
package cs16mod

import (
	"encoding/binary"
	"math"
	"strconv"
	"strings"
	"unsafe"
)

const (
	// ABIVersion is the cs16-rs host ABI version targeted by this SDK.
	ABIVersion = 2
	// SlotAll targets all connected players in host calls accepting a nullable slot.
	SlotAll int32 = -1
	// MenuExit is passed as itemIndex to menu callbacks when a player selects 0. Exit.
	MenuExit int32 = -3
)

// ---------------------------------------------------------------------------
// Raw "cs16_mod" WASM Host Imports (exact signatures from WasmiRuntime)
// ---------------------------------------------------------------------------

// Lifecycle & hook context

//go:wasmimport cs16_mod hook_register
func rawHookRegister(hookID, phase, priority int32) int32

//go:wasmimport cs16_mod host_time_ms
func rawHostTimeMs() int64

//go:wasmimport cs16_mod hook_arg_i32
func rawHookArgI32(idx int32) int32

//go:wasmimport cs16_mod hook_arg_f32
func rawHookArgF32(idx int32) float32

//go:wasmimport cs16_mod hook_arg_str
func rawHookArgStr(idx int32, ptr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod hook_arg_bytes
func rawHookArgBytes(ptr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod hook_set_override_i32
func rawHookSetOverrideI32(val int32) int32

//go:wasmimport cs16_mod hook_set_override_i32_2
func rawHookSetOverrideI32_2(val int32) int32

//go:wasmimport cs16_mod hook_set_override_f32
func rawHookSetOverrideF32(val float32) int32

//go:wasmimport cs16_mod hook_set_override_str
func rawHookSetOverrideStr(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod rpc_set_return_str
func rawRPCSetReturnStr(ptr unsafe.Pointer, length int32) int32

// Entities & basic players (v1)

//go:wasmimport cs16_mod ent_count
func rawEntCount() int32

//go:wasmimport cs16_mod ent_create
func rawEntCreate(clsPtr unsafe.Pointer, clsLen int32) int32

//go:wasmimport cs16_mod ent_remove
func rawEntRemove(idx int32) int32

//go:wasmimport cs16_mod ent_get_health
func rawEntGetHealth(idx int32) float32

//go:wasmimport cs16_mod ent_set_health
func rawEntSetHealth(idx int32, health float32) int32

//go:wasmimport cs16_mod ent_get_origin
func rawEntGetOrigin(idx int32, outPtr unsafe.Pointer) int32

//go:wasmimport cs16_mod ent_set_origin
func rawEntSetOrigin(idx int32, x, y, z float32) int32

//go:wasmimport cs16_mod ent_get_velocity
func rawEntGetVelocity(idx int32, outPtr unsafe.Pointer) int32

//go:wasmimport cs16_mod ent_set_velocity
func rawEntSetVelocity(idx int32, x, y, z float32) int32

//go:wasmimport cs16_mod ent_get_flags
func rawEntGetFlags(idx int32) int32

//go:wasmimport cs16_mod ent_set_flags
func rawEntSetFlags(idx, flags int32) int32

//go:wasmimport cs16_mod ent_classname
func rawEntClassname(idx int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod player_max_clients
func rawPlayerMaxClients() int32

//go:wasmimport cs16_mod player_is_active
func rawPlayerIsActive(slot int32) int32

//go:wasmimport cs16_mod player_name
func rawPlayerName(slot int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod player_kick
func rawPlayerKick(slot int32, ptr unsafe.Pointer, length int32) int32

// Messages, CVars, Commands, Timers, Traces, Sound, Storage, Client HUD (v1)

//go:wasmimport cs16_mod print_client
func rawPrintClient(slot int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod print_all
func rawPrintAll(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod center_print
func rawCenterPrint(slot int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod say_text
func rawSayText(slot int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod send_custom_usermsg
func rawSendCustomUsermsg(slot int32, namePtr unsafe.Pointer, nameLen int32, dataPtr unsafe.Pointer, dataLen int32) int32

//go:wasmimport cs16_mod cvar_get_f32
func rawCvarGetF32(ptr unsafe.Pointer, length int32) float32

//go:wasmimport cs16_mod cvar_set_f32
func rawCvarSetF32(ptr unsafe.Pointer, length int32, val float32) int32

//go:wasmimport cs16_mod cvar_get_str
func rawCvarGetStr(ptr unsafe.Pointer, length int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod cvar_set_str
func rawCvarSetStr(ptr unsafe.Pointer, length int32, valPtr unsafe.Pointer, valLen int32) int32

//go:wasmimport cs16_mod register_command
func rawRegisterCommand(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod server_exec
func rawServerExec(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod change_level
func rawChangeLevel(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod show_menu
func rawShowMenu(slot, keys, timeSec int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod timer_after
func rawTimerAfter(delayMs, cbID, arg int32) int32

//go:wasmimport cs16_mod timer_repeat
func rawTimerRepeat(intervalMs, cbID, arg int32) int32

//go:wasmimport cs16_mod timer_cancel
func rawTimerCancel(timerID int32) int32

//go:wasmimport cs16_mod trace_line
func rawTraceLine(x1, y1, z1, x2, y2, z2 float32) float32

//go:wasmimport cs16_mod precache_sound
func rawPrecacheSound(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod emit_sound
func rawEmitSound(entIdx, channel int32, ptr unsafe.Pointer, length int32, volume float32) int32

//go:wasmimport cs16_mod storage_get
func rawStorageGet(keyPtr unsafe.Pointer, keyLen int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod storage_set
func rawStorageSet(keyPtr unsafe.Pointer, keyLen int32, valPtr unsafe.Pointer, valLen int32) int32

//go:wasmimport cs16_mod storage_remove
func rawStorageRemove(keyPtr unsafe.Pointer, keyLen int32) int32

//go:wasmimport cs16_mod hud_draw_rect
func rawHUDDrawRect(x, y, w, h float32, rgba int32) int32

//go:wasmimport cs16_mod hud_draw_text
func rawHUDDrawText(x, y, size float32, rgba int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod hud_draw_bar
func rawHUDDrawBar(x, y, w, h, frac float32, rgba int32) int32

//go:wasmimport cs16_mod hud_panel
func rawHUDPanel(x, y, w, h float32, rgba int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod client_exec
func rawClientExec(ptr unsafe.Pointer, length int32) int32

// ABI v2 player & weapon natives

//go:wasmimport cs16_mod player_get_health
func rawPlayerGetHealth(slot int32) float32

//go:wasmimport cs16_mod player_set_health
func rawPlayerSetHealth(slot int32, health float32) int32

//go:wasmimport cs16_mod player_set_max_health
func rawPlayerSetMaxHealth(slot int32, maxHealth float32) int32

//go:wasmimport cs16_mod player_get_armor
func rawPlayerGetArmor(slot int32) int32

//go:wasmimport cs16_mod player_get_armor_type
func rawPlayerGetArmorType(slot int32) int32

//go:wasmimport cs16_mod player_set_armor
func rawPlayerSetArmor(slot, armor, kind int32) int32

//go:wasmimport cs16_mod player_get_money
func rawPlayerGetMoney(slot int32) int32

//go:wasmimport cs16_mod player_set_money
func rawPlayerSetMoney(slot, amount, flash int32) int32

//go:wasmimport cs16_mod player_add_money
func rawPlayerAddMoney(slot, delta, flash int32) int32

//go:wasmimport cs16_mod player_get_frags
func rawPlayerGetFrags(slot int32) int32

//go:wasmimport cs16_mod player_get_deaths
func rawPlayerGetDeaths(slot int32) int32

//go:wasmimport cs16_mod player_set_score
func rawPlayerSetScore(slot, frags, deaths int32) int32

//go:wasmimport cs16_mod player_get_team
func rawPlayerGetTeam(slot int32) int32

//go:wasmimport cs16_mod player_set_team
func rawPlayerSetTeam(slot, team, modelClass int32) int32

//go:wasmimport cs16_mod player_is_alive
func rawPlayerIsAlive(slot int32) int32

//go:wasmimport cs16_mod player_respawn
func rawPlayerRespawn(slot int32) int32

//go:wasmimport cs16_mod player_slap
func rawPlayerSlap(slot, power, randomDir int32) int32

//go:wasmimport cs16_mod player_slay
func rawPlayerSlay(slot, preserveFrags int32) int32

//go:wasmimport cs16_mod player_damage
func rawPlayerDamage(victim, attacker int32, damage float32, bits int32) int32

//go:wasmimport cs16_mod player_set_godmode
func rawPlayerSetGodmode(slot, enabled int32) int32

//go:wasmimport cs16_mod player_set_noclip
func rawPlayerSetNoclip(slot, enabled int32) int32

//go:wasmimport cs16_mod player_set_maxspeed
func rawPlayerSetMaxspeed(slot int32, maxspeed float32) int32

//go:wasmimport cs16_mod player_set_gravity
func rawPlayerSetGravity(slot int32, gravity float32) int32

//go:wasmimport cs16_mod player_set_footsteps
func rawPlayerSetFootsteps(slot, silent int32) int32

//go:wasmimport cs16_mod player_set_rendering
func rawPlayerSetRendering(slot, fx, rgbPacked, mode, amount int32) int32

//go:wasmimport cs16_mod player_set_model
func rawPlayerSetModel(slot int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod player_reset_model
func rawPlayerResetModel(slot int32) int32

//go:wasmimport cs16_mod player_get_ip
func rawPlayerGetIP(slot int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod player_get_steamid
func rawPlayerGetSteamID(slot int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod player_get_ping
func rawPlayerGetPing(slot int32) int32

//go:wasmimport cs16_mod player_get_loss
func rawPlayerGetLoss(slot int32) int32

//go:wasmimport cs16_mod player_get_aim_origin
func rawPlayerGetAimOrigin(slot int32, outPtr unsafe.Pointer) int32

//go:wasmimport cs16_mod player_give_item
func rawPlayerGiveItem(slot int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod player_strip_weapons
func rawPlayerStripWeapons(slot int32) int32

//go:wasmimport cs16_mod player_get_weapon_clip
func rawPlayerGetWeaponClip(slot, weaponID int32) int32

//go:wasmimport cs16_mod player_set_weapon_clip
func rawPlayerSetWeaponClip(slot, weaponID, clip int32) int32

//go:wasmimport cs16_mod player_get_bpammo
func rawPlayerGetBpAmmo(slot, weaponID int32) int32

//go:wasmimport cs16_mod player_set_bpammo
func rawPlayerSetBpAmmo(slot, weaponID, amount int32) int32

//go:wasmimport cs16_mod player_set_defuser
func rawPlayerSetDefuser(slot, has int32) int32

//go:wasmimport cs16_mod player_set_nvg
func rawPlayerSetNVG(slot, has int32) int32

//go:wasmimport cs16_mod player_drop_slot
func rawPlayerDropSlot(slot, invSlot int32) int32

//go:wasmimport cs16_mod register_custom_weapon
func rawRegisterCustomWeapon(specPtr unsafe.Pointer, specLen int32) int32

//go:wasmimport cs16_mod give_custom_weapon
func rawGiveCustomWeapon(slot int32, idPtr unsafe.Pointer, idLen int32) int32

// ABI v2 VFX primitives

//go:wasmimport cs16_mod vfx_shockwave_ring
func rawVFXShockwaveRing(x, y, z, radius float32, rgba, life, width, slot int32) int32

//go:wasmimport cs16_mod vfx_laser_beam
func rawVFXLaserBeam(x1, y1, z1, x2, y2, z2 float32, rgba, life, width, noise, slot int32) int32

//go:wasmimport cs16_mod vfx_follow_trail
func rawVFXFollowTrail(entIdx, rgba, life, width, slot int32) int32

//go:wasmimport cs16_mod vfx_kill_beam
func rawVFXKillBeam(entIdx, slot int32) int32

//go:wasmimport cs16_mod vfx_aura_light
func rawVFXAuraLight(x, y, z float32, radius, rgb, life, decay, slot int32) int32

//go:wasmimport cs16_mod vfx_sparks
func rawVFXSparks(x, y, z float32, slot int32) int32

//go:wasmimport cs16_mod vfx_implosion
func rawVFXImplosion(x, y, z float32, radius, count, life, slot int32) int32

//go:wasmimport cs16_mod vfx_teleport_splash
func rawVFXTeleportSplash(x, y, z float32, slot int32) int32

//go:wasmimport cs16_mod vfx_lava_splash
func rawVFXLavaSplash(x, y, z float32, slot int32) int32

//go:wasmimport cs16_mod vfx_explosion
func rawVFXExplosion(x, y, z float32, scale, flags, slot int32) int32

//go:wasmimport cs16_mod vfx_blood_stream
func rawVFXBloodStream(x, y, z, dx, dy, dz float32, color, speed, slot int32) int32

//go:wasmimport cs16_mod vfx_world_decal
func rawVFXWorldDecal(x, y, z float32, namePtr unsafe.Pointer, nameLen, slot int32) int32

// ABI v2 Paginated Menus, HUD/DHUD/MOTD/Screen Effects

//go:wasmimport cs16_mod menu_create
func rawMenuCreate(titlePtr unsafe.Pointer, titleLen, cbID int32) int32

//go:wasmimport cs16_mod menu_add_item
func rawMenuAddItem(menuID int32, labelPtr unsafe.Pointer, labelLen int32, infoPtr unsafe.Pointer, infoLen, enabled int32) int32

//go:wasmimport cs16_mod menu_add_blank
func rawMenuAddBlank(menuID int32) int32

//go:wasmimport cs16_mod menu_add_text
func rawMenuAddText(menuID int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod menu_set_prop_int
func rawMenuSetPropInt(menuID, prop, val int32) int32

//go:wasmimport cs16_mod menu_set_prop_str
func rawMenuSetPropStr(menuID, prop int32, valPtr unsafe.Pointer, valLen int32) int32

//go:wasmimport cs16_mod menu_display
func rawMenuDisplay(slot, menuID, page, timeSec int32) int32

//go:wasmimport cs16_mod menu_destroy
func rawMenuDestroy(menuID int32) int32

//go:wasmimport cs16_mod hud_message
func rawHUDMessage(slot, channelEffect int32, x, y float32, rgba1, rgba2 int32, fadein, hold, fadeout, fxtime float32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod dhud_message
func rawDHUDMessage(slot, rgbEffect int32, x, y, fadein, hold, fadeout, fxtime float32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod show_motd
func rawShowMOTD(slot int32, titlePtr unsafe.Pointer, titleLen int32, bodyPtr unsafe.Pointer, bodyLen int32) int32

//go:wasmimport cs16_mod screen_fade
func rawScreenFade(slot int32, durationSec, holdSec float32, flags, rgba int32) int32

//go:wasmimport cs16_mod screen_shake
func rawScreenShake(slot int32, amplitude, durationSec, frequency float32) int32

//go:wasmimport cs16_mod bar_time
func rawBarTime(slot, seconds int32) int32

// ABI v2 i18n, Admins, RPC, NVault, Leaderboards, Webhooks

//go:wasmimport cs16_mod lang_load_toml
func rawLangLoadTOML(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod lang_load_amxx
func rawLangLoadAMXX(ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod lang_tr
func rawLangTr(slot int32, keyPtr unsafe.Pointer, keyLen int32, argsPtr unsafe.Pointer, argsLen int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod lang_player_code
func rawLangPlayerCode(slot int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod lang_set_player_code
func rawLangSetPlayerCode(slot int32, ptr unsafe.Pointer, length int32) int32

//go:wasmimport cs16_mod admin_player_flags
func rawAdminPlayerFlags(slot int32) int32

//go:wasmimport cs16_mod admin_has_flag
func rawAdminHasFlag(slot, flagChar int32) int32

//go:wasmimport cs16_mod rpc_export
func rawRPCExport(namePtr unsafe.Pointer, nameLen, cbID int32) int32

//go:wasmimport cs16_mod rpc_call
func rawRPCCall(namePtr unsafe.Pointer, nameLen, argI32 int32, argPtr unsafe.Pointer, argLen int32, outPtr unsafe.Pointer, outCap int32) int32

//go:wasmimport cs16_mod rpc_subscribe
func rawRPCSubscribe(evPtr unsafe.Pointer, evLen, cbID int32) int32

//go:wasmimport cs16_mod rpc_emit_event
func rawRPCEmitEvent(evPtr unsafe.Pointer, evLen int32, payloadPtr unsafe.Pointer, payloadLen int32) int32

//go:wasmimport cs16_mod nvault_open
func rawNVaultOpen(namePtr unsafe.Pointer, nameLen int32) int32

//go:wasmimport cs16_mod nvault_get
func rawNVaultGet(handle int32, keyPtr unsafe.Pointer, keyLen int32, outPtr unsafe.Pointer, cap int32) int32

//go:wasmimport cs16_mod nvault_set
func rawNVaultSet(handle int32, keyPtr unsafe.Pointer, keyLen int32, valPtr unsafe.Pointer, valLen int32) int32

//go:wasmimport cs16_mod nvault_remove
func rawNVaultRemove(handle int32, keyPtr unsafe.Pointer, keyLen int32) int32

//go:wasmimport cs16_mod leaderboard_upsert
func rawLeaderboardUpsert(tablePtr unsafe.Pointer, tableLen int32, keyPtr unsafe.Pointer, keyLen int32, score float64, payloadPtr unsafe.Pointer, payloadLen int32) int32

//go:wasmimport cs16_mod leaderboard_rank
func rawLeaderboardRank(tablePtr unsafe.Pointer, tableLen int32, keyPtr unsafe.Pointer, keyLen int32, outPtr unsafe.Pointer, outCap int32) int32

//go:wasmimport cs16_mod leaderboard_top
func rawLeaderboardTop(tablePtr unsafe.Pointer, tableLen, offset, limit int32, outPtr unsafe.Pointer, outCap int32) int32

//go:wasmimport cs16_mod http_post_json
func rawHTTPPostJSON(urlPtr unsafe.Pointer, urlLen int32, bodyPtr unsafe.Pointer, bodyLen, cbID int32) int32

// ---------------------------------------------------------------------------
// Internal Memory & Encoding Helpers
// ---------------------------------------------------------------------------

func strArg(s string) (unsafe.Pointer, int32) {
	if len(s) == 0 {
		return nil, 0
	}
	return unsafe.Pointer(unsafe.StringData(s)), int32(len(s))
}

func bytesArg(b []byte) (unsafe.Pointer, int32) {
	if len(b) == 0 {
		return nil, 0
	}
	return unsafe.Pointer(unsafe.SliceData(b)), int32(len(b))
}

func boolToI32(b bool) int32 {
	if b {
		return 1
	}
	return 0
}

func readString(bufCap int, fn func(ptr unsafe.Pointer, cap int32) int32) (string, bool) {
	buf := make([]byte, bufCap)
	n := fn(unsafe.Pointer(unsafe.SliceData(buf)), int32(len(buf)))
	if n < 0 {
		return "", false
	}
	if int(n) < len(buf) {
		buf = buf[:n]
	}
	return string(buf), true
}

func readVec3(fn func(outPtr unsafe.Pointer) int32) (Vec3, bool) {
	var raw [12]byte
	if fn(unsafe.Pointer(&raw[0])) < 0 {
		return Vec3{}, false
	}
	return Vec3{
		X: math.Float32frombits(binary.LittleEndian.Uint32(raw[0:4])),
		Y: math.Float32frombits(binary.LittleEndian.Uint32(raw[4:8])),
		Z: math.Float32frombits(binary.LittleEndian.Uint32(raw[8:12])),
	}, true
}

// ---------------------------------------------------------------------------
// High-Level Server & Platform Functions
// ---------------------------------------------------------------------------

// TimeMs returns current server time in milliseconds.
func TimeMs() int64 { return rawHostTimeMs() }

// MaxClients returns the server's max player slots (typically 1..32).
func MaxClients() int32 { return rawPlayerMaxClients() }

// PrintAll prints a line to every client's console.
func PrintAll(text string) int32 {
	p, n := strArg(text)
	return rawPrintAll(p, n)
}

// SayAll broadcasts a colored chat message to all players.
func SayAll(text string) int32 {
	p, n := strArg(text)
	return rawSayText(SlotAll, p, n)
}

// CenterPrintAll displays a centered text banner to all players.
func CenterPrintAll(text string) int32 {
	p, n := strArg(text)
	return rawCenterPrint(SlotAll, p, n)
}

// RegisterCommand declares a console command handled by this plugin.
func RegisterCommand(name string) int32 {
	p, n := strArg(name)
	return rawRegisterCommand(p, n)
}

// ServerExec executes a server console line (e.g. "mp_timelimit 20").
func ServerExec(line string) int32 {
	p, n := strArg(line)
	return rawServerExec(p, n)
}

// ChangeLevel transitions the server to the given map name.
func ChangeLevel(mapName string) int32 {
	p, n := strArg(mapName)
	return rawChangeLevel(p, n)
}

// CvarFloat reads a numeric CVar value.
func CvarFloat(name string) float32 {
	p, n := strArg(name)
	return rawCvarGetF32(p, n)
}

// SetCvarFloat updates a numeric CVar value.
func SetCvarFloat(name string, val float32) int32 {
	p, n := strArg(name)
	return rawCvarSetF32(p, n, val)
}

// CvarString reads a string CVar value.
func CvarString(name string) (string, bool) {
	p, n := strArg(name)
	return readString(1024, func(out unsafe.Pointer, cap int32) int32 {
		return rawCvarGetStr(p, n, out, cap)
	})
}

// SetCvarString updates a string CVar value.
func SetCvarString(name, val string) int32 {
	np, nn := strArg(name)
	vp, vn := strArg(val)
	return rawCvarSetStr(np, nn, vp, vn)
}

// TimerAfter schedules a one-shot callback after delayMs milliseconds.
func TimerAfter(delayMs, cbID, arg int32) int32 { return rawTimerAfter(delayMs, cbID, arg) }

// TimerRepeat schedules a repeating callback every intervalMs milliseconds.
func TimerRepeat(intervalMs, cbID, arg int32) int32 { return rawTimerRepeat(intervalMs, cbID, arg) }

// TimerCancel cancels a previously scheduled timer by ID.
func TimerCancel(timerID int32) int32 { return rawTimerCancel(timerID) }

// TraceLine traces a world ray and returns the hit fraction in [0, 1].
func TraceLine(start, end Vec3) float32 {
	return rawTraceLine(start.X, start.Y, start.Z, end.X, end.Y, end.Z)
}

// PrecacheSound precaches a sound file during server activation.
func PrecacheSound(sample string) int32 {
	p, n := strArg(sample)
	return rawPrecacheSound(p, n)
}

// EmitSound plays a sound from entIdx on channel at volume [0..1].
func EmitSound(entIdx, channel int32, sample string, volume float32) int32 {
	p, n := strArg(sample)
	return rawEmitSound(entIdx, channel, p, n, volume)
}

// StorageGet fetches a persistent per-mod string value.
func StorageGet(key string) (string, bool) {
	kp, kn := strArg(key)
	return readString(4096, func(out unsafe.Pointer, cap int32) int32 {
		return rawStorageGet(kp, kn, out, cap)
	})
}

// StorageSet persists a per-mod string value.
func StorageSet(key, val string) int32 {
	kp, kn := strArg(key)
	vp, vn := strArg(val)
	return rawStorageSet(kp, kn, vp, vn)
}

// StorageRemove deletes a per-mod storage key.
func StorageRemove(key string) int32 {
	kp, kn := strArg(key)
	return rawStorageRemove(kp, kn)
}

// LangLoadTOML registers an embedded `[en]` / `[ru]` TOML dictionary in the shared LangCatalog.
func LangLoadTOML(src string) int32 {
	p, n := strArg(src)
	return rawLangLoadTOML(p, n)
}

// LangLoadAMXX registers an AMX Mod X `data/lang/*.txt` dictionary in the shared LangCatalog.
func LangLoadAMXX(src string) int32 {
	p, n := strArg(src)
	return rawLangLoadAMXX(p, n)
}

// TrServer translates key using the server default language (`amx_language`).
func TrServer(key string, args ...LangArg) string {
	return translateSlot(SlotAll, key, args...)
}

// NVaultOpen opens or creates a shared AMXX-compatible NVault store.
func NVaultOpen(name string) int32 {
	p, n := strArg(name)
	return rawNVaultOpen(p, n)
}

// NVaultGet reads a key from an open NVault handle.
func NVaultGet(handle int32, key string) (string, bool) {
	kp, kn := strArg(key)
	return readString(4096, func(out unsafe.Pointer, cap int32) int32 {
		return rawNVaultGet(handle, kp, kn, out, cap)
	})
}

// NVaultSet writes a key/value pair into an open NVault handle.
func NVaultSet(handle int32, key, val string) int32 {
	kp, kn := strArg(key)
	vp, vn := strArg(val)
	return rawNVaultSet(handle, kp, kn, vp, vn)
}

// NVaultRemove removes a key from an open NVault handle.
func NVaultRemove(handle int32, key string) int32 {
	kp, kn := strArg(key)
	return rawNVaultRemove(handle, kp, kn)
}

// LeaderboardEntry describes one ranked row from the host O(log N) LeaderboardStore.
type LeaderboardEntry struct {
	Rank        int
	Key         string
	Score       float64
	PayloadJSON string
}

// LeaderboardUpsert inserts or updates a ranked record in table.
func LeaderboardUpsert(table, key string, score float64, payloadJSON string) int32 {
	tp, tn := strArg(table)
	kp, kn := strArg(key)
	pp, pn := strArg(payloadJSON)
	return rawLeaderboardUpsert(tp, tn, kp, kn, score, pp, pn)
}

// LeaderboardRank looks up key's 1-based rank, entry, and total row count in table.
func LeaderboardRank(table, key string) (LeaderboardEntry, int, bool) {
	tp, tn := strArg(table)
	kp, kn := strArg(key)
	buf := make([]byte, 4096)
	rank := rawLeaderboardRank(tp, tn, kp, kn, unsafe.Pointer(unsafe.SliceData(buf)), int32(len(buf)))
	if rank <= 0 {
		return LeaderboardEntry{}, 0, false
	}
	end := len(buf)
	for i, b := range buf {
		if b == 0 {
			end = i
			break
		}
	}
	parts := strings.SplitN(string(buf[:end]), "\t", 4)
	if len(parts) < 3 {
		return LeaderboardEntry{Rank: int(rank), Key: key}, 0, true
	}
	total, _ := strconv.Atoi(parts[1])
	score, _ := strconv.ParseFloat(parts[2], 64)
	payload := ""
	if len(parts) >= 4 {
		payload = parts[3]
	}
	return LeaderboardEntry{Rank: int(rank), Key: key, Score: score, PayloadJSON: payload}, total, true
}

// LeaderboardTop fetches up to limit ranked rows from table starting at offset.
func LeaderboardTop(table string, offset, limit int32) []LeaderboardEntry {
	tp, tn := strArg(table)
	raw, ok := readString(16384, func(out unsafe.Pointer, cap int32) int32 {
		return rawLeaderboardTop(tp, tn, offset, limit, out, cap)
	})
	if !ok || raw == "" {
		return nil
	}
	lines := strings.Split(raw, "\n")
	out := make([]LeaderboardEntry, 0, len(lines))
	for _, line := range lines {
		if line == "" {
			continue
		}
		parts := strings.SplitN(line, "\t", 4)
		if len(parts) < 3 {
			continue
		}
		rank, err1 := strconv.Atoi(parts[0])
		score, err2 := strconv.ParseFloat(parts[2], 64)
		if err1 != nil || err2 != nil {
			continue
		}
		payload := ""
		if len(parts) >= 4 {
			payload = parts[3]
		}
		out = append(out, LeaderboardEntry{
			Rank:        rank,
			Key:         parts[1],
			Score:       score,
			PayloadJSON: payload,
		})
	}
	return out
}

// RPCCall synchronously invokes an exported polyglot RPC function on any loaded plugin.
func RPCCall(fnName string, argInt int32, argStr string) (int32, string) {
	np, nn := strArg(fnName)
	ap, an := strArg(argStr)
	buf := make([]byte, 4096)
	rc := rawRPCCall(np, nn, argInt, ap, an, unsafe.Pointer(unsafe.SliceData(buf)), int32(len(buf)))
	if rc <= 0 {
		return rc, ""
	}
	end := int(rc)
	if end > len(buf) {
		end = len(buf)
	}
	for i := 0; i < end; i++ {
		if buf[i] == 0 {
			end = i
			break
		}
	}
	return rc, string(buf[:end])
}

// RPCEmitEvent broadcasts a named cross-plugin event with a JSON payload.
func RPCEmitEvent(eventName, payloadJSON string) int32 {
	ep, en := strArg(eventName)
	pp, pn := strArg(payloadJSON)
	return rawRPCEmitEvent(ep, en, pp, pn)
}

// PostWebhookJSON enqueues a non-blocking, host-allowlisted HTTP POST JSON webhook.
// Results are delivered later to the callback registered with OnWebhook(cbID, ...).
func PostWebhookJSON(url, bodyJSON string, cbID int32) int32 {
	up, un := strArg(url)
	bp, bn := strArg(bodyJSON)
	return rawHTTPPostJSON(up, un, bp, bn, cbID)
}

// HUDMessage displays a classic GoldSrc TE_TEXTMESSAGE HUD message on channel 0..4.
func HUDMessage(slot, channel, effect int32, x, y float32, c1, c2 Color, fadein, hold, fadeout, fxtime float32, text string) int32 {
	p, n := strArg(text)
	ce := (channel & 0xFF) | ((effect & 0xFF) << 8)
	return rawHUDMessage(slot, ce, x, y, c1.PackRGBA(), c2.PackRGBA(), fadein, hold, fadeout, fxtime, p, n)
}

// DHUDMessage displays a large director HUD message.
func DHUDMessage(slot, effect int32, x, y float32, col Color, fadein, hold, fadeout, fxtime float32, text string) int32 {
	p, n := strArg(text)
	re := int32((uint32(effect&0xFF) << 24) | uint32(col.PackRGB()))
	return rawDHUDMessage(slot, re, x, y, fadein, hold, fadeout, fxtime, p, n)
}
