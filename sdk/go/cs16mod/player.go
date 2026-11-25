//go:build wasm

package cs16mod

import (
	"strings"
	"unsafe"
)

// Vec3 represents a 3D GoldSrc world coordinate or velocity vector.
type Vec3 struct {
	X, Y, Z float32
}

// Color represents an RGBA color with packing helpers for cs16_mod host calls.
type Color struct {
	R, G, B, A uint8
}

// PackRGBA packs the color into big-endian 0xRRGGBBAA format.
func (c Color) PackRGBA() int32 {
	return int32((uint32(c.R) << 24) | (uint32(c.G) << 16) | (uint32(c.B) << 8) | uint32(c.A))
}

// PackRGB packs the color into 24-bit 0x00RRGGBB format.
func (c Color) PackRGB() int32 {
	return int32((uint32(c.R) << 16) | (uint32(c.G) << 8) | uint32(c.B))
}

// Common CS 1.6 chat color prefixes and GoldSrc menu escape sequences.
const (
	ChatDefault = "\x01"
	ChatTeam    = "\x03"
	ChatGreen   = "\x04"
	MenuYellow  = "\\y"
	MenuWhite   = "\\w"
	MenuRed     = "\\r"
	MenuGrey    = "\\d"
	MenuRight   = "\\R"
)

// TeamKind identifies a Counter-Strike 1.6 team.
type TeamKind int32

const (
	TeamUnassigned TeamKind = 0
	TeamTerrorist  TeamKind = 1
	TeamCT         TeamKind = 2
	TeamSpectator  TeamKind = 3
)

// ArmorKind identifies CS 1.6 armor protection type.
type ArmorKind int32

const (
	ArmorNone     ArmorKind = 0
	ArmorKevlar   ArmorKind = 1
	ArmorVestHelm ArmorKind = 2
)

// GoldSrc rendering FX and mode constants.
const (
	RenderFxNone      int32 = 0
	RenderFxGlowShell int32 = 19

	RenderModeNormal     int32 = 0
	RenderModeTransAlpha int32 = 4
	RenderModeTransAdd   int32 = 5
)

// LangArg is a named or positional substitution passed to Player.Tr / TrServer.
type LangArg struct {
	Key   string
	Value string
}

// Arg constructs a named translation parameter (`{key}`).
func Arg(key, value string) LangArg {
	return LangArg{Key: key, Value: value}
}

func encodeLangArgs(args []LangArg) string {
	if len(args) == 0 {
		return ""
	}
	var b strings.Builder
	for i, a := range args {
		if i > 0 {
			b.WriteByte('\n')
		}
		clean := strings.ReplaceAll(strings.ReplaceAll(a.Value, "\r", " "), "\n", " ")
		if a.Key != "" {
			b.WriteString(a.Key)
			b.WriteByte('=')
		}
		b.WriteString(clean)
	}
	return b.String()
}

func translateSlot(slot int32, key string, args ...LangArg) string {
	kp, kn := strArg(key)
	enc := encodeLangArgs(args)
	ap, an := strArg(enc)
	text, ok := readString(4096, func(out unsafe.Pointer, cap int32) int32 {
		return rawLangTr(slot, kp, kn, ap, an, out, cap)
	})
	if !ok {
		return key
	}
	return text
}

// Player is a lightweight handle for a connected player slot (0..31).
type Player struct {
	Slot int32
}

// NewPlayer creates a handle for a 0-based player slot.
func NewPlayer(slot int32) Player { return Player{Slot: slot} }

// EdictIndex returns the 1-based GoldSrc entity index (`slot + 1`).
func (p Player) EdictIndex() int32 { return p.Slot + 1 }

// IsActive reports whether this player slot is currently connected.
func (p Player) IsActive() bool { return rawPlayerIsActive(p.Slot) != 0 }

// IsAlive reports whether the player is currently alive.
func (p Player) IsAlive() bool { return rawPlayerIsAlive(p.Slot) != 0 }

// Name returns the player's current display name.
func (p Player) Name() string {
	s, _ := readString(128, func(out unsafe.Pointer, cap int32) int32 {
		return rawPlayerName(p.Slot, out, cap)
	})
	return s
}

// IP returns the player's IP address string.
func (p Player) IP() string {
	s, _ := readString(64, func(out unsafe.Pointer, cap int32) int32 {
		return rawPlayerGetIP(p.Slot, out, cap)
	})
	return s
}

// SteamID returns the player's SteamID string.
func (p Player) SteamID() string {
	s, _ := readString(64, func(out unsafe.Pointer, cap int32) int32 {
		return rawPlayerGetSteamID(p.Slot, out, cap)
	})
	return s
}

// Ping returns the player's network latency in milliseconds.
func (p Player) Ping() int32 { return rawPlayerGetPing(p.Slot) }

// Loss returns the player's packet loss percentage.
func (p Player) Loss() int32 { return rawPlayerGetLoss(p.Slot) }

// Origin returns the player's world position.
func (p Player) Origin() (Vec3, bool) {
	return readVec3(func(out unsafe.Pointer) int32 { return rawEntGetOrigin(p.EdictIndex(), out) })
}

// AimOrigin returns the world point under the player's crosshair.
func (p Player) AimOrigin() (Vec3, bool) {
	return readVec3(func(out unsafe.Pointer) int32 { return rawPlayerGetAimOrigin(p.Slot, out) })
}

// Health returns the player's current health.
func (p Player) Health() float32 { return rawPlayerGetHealth(p.Slot) }

// SetHealth sets the player's current health.
func (p Player) SetHealth(hp float32) int32 { return rawPlayerSetHealth(p.Slot, hp) }

// SetMaxHealth sets the player's maximum health cap.
func (p Player) SetMaxHealth(hp float32) int32 { return rawPlayerSetMaxHealth(p.Slot, hp) }

// Armor returns the player's current armor value.
func (p Player) Armor() int32 { return rawPlayerGetArmor(p.Slot) }

// ArmorType returns the player's current armor kind.
func (p Player) ArmorType() ArmorKind { return ArmorKind(rawPlayerGetArmorType(p.Slot)) }

// SetArmor sets the player's armor amount and kind.
func (p Player) SetArmor(amount int32, kind ArmorKind) int32 {
	return rawPlayerSetArmor(p.Slot, amount, int32(kind))
}

// Money returns the player's current cash balance.
func (p Player) Money() int32 { return rawPlayerGetMoney(p.Slot) }

// SetMoney sets the player's cash balance.
func (p Player) SetMoney(amount int32, flash bool) int32 {
	return rawPlayerSetMoney(p.Slot, amount, boolToI32(flash))
}

// AddMoney adjusts the player's cash balance by delta.
func (p Player) AddMoney(delta int32, flash bool) int32 {
	return rawPlayerAddMoney(p.Slot, delta, boolToI32(flash))
}

// Frags returns the player's frag count.
func (p Player) Frags() int32 { return rawPlayerGetFrags(p.Slot) }

// Deaths returns the player's death count.
func (p Player) Deaths() int32 { return rawPlayerGetDeaths(p.Slot) }

// SetScore updates the player's scoreboard frags and deaths immediately.
func (p Player) SetScore(frags, deaths int32) int32 { return rawPlayerSetScore(p.Slot, frags, deaths) }

// ResetScore zeroes the player's frags and deaths.
func (p Player) ResetScore() int32 { return rawPlayerSetScore(p.Slot, 0, 0) }

// Team returns the player's current team.
func (p Player) Team() TeamKind { return TeamKind(rawPlayerGetTeam(p.Slot)) }

// SetTeam moves the player to team (pass modelClass < 0 to keep default class).
func (p Player) SetTeam(team TeamKind, modelClass int32) int32 {
	return rawPlayerSetTeam(p.Slot, int32(team), modelClass)
}

// Respawn immediately respawns the player.
func (p Player) Respawn() int32 { return rawPlayerRespawn(p.Slot) }

// Slap deals slap damage and optional random velocity impulse.
func (p Player) Slap(power int32, randomDir bool) int32 {
	return rawPlayerSlap(p.Slot, power, boolToI32(randomDir))
}

// Slay kills the player immediately.
func (p Player) Slay(preserveFrags bool) int32 {
	return rawPlayerSlay(p.Slot, boolToI32(preserveFrags))
}

// Damage applies damage from attackerSlot (or SlotAll for world) with damage bits.
func (p Player) Damage(attackerSlot int32, damage float32, bits int32) int32 {
	return rawPlayerDamage(p.Slot, attackerSlot, damage, bits)
}

// Kick disconnects the player with reason.
func (p Player) Kick(reason string) int32 {
	ptr, n := strArg(reason)
	return rawPlayerKick(p.Slot, ptr, n)
}

// SetGodmode toggles invulnerability.
func (p Player) SetGodmode(enabled bool) int32 { return rawPlayerSetGodmode(p.Slot, boolToI32(enabled)) }

// SetNoclip toggles noclip movement.
func (p Player) SetNoclip(enabled bool) int32 { return rawPlayerSetNoclip(p.Slot, boolToI32(enabled)) }

// SetMaxspeed overrides the player's maximum movement speed.
func (p Player) SetMaxspeed(speed float32) int32 { return rawPlayerSetMaxspeed(p.Slot, speed) }

// SetGravity sets the player's gravity multiplier (1.0 = normal).
func (p Player) SetGravity(gravity float32) int32 { return rawPlayerSetGravity(p.Slot, gravity) }

// SetFootsteps toggles silent footsteps.
func (p Player) SetFootsteps(silent bool) int32 { return rawPlayerSetFootsteps(p.Slot, boolToI32(silent)) }

// SetRendering configures GoldSrc render FX, RGB color, mode, and amount.
func (p Player) SetRendering(fx int32, col Color, mode, amount int32) int32 {
	return rawPlayerSetRendering(p.Slot, fx, col.PackRGB(), mode, amount)
}

// SetGlow applies a GlowShell outline around the player.
func (p Player) SetGlow(col Color, thickness int32) int32 {
	return p.SetRendering(RenderFxGlowShell, col, RenderModeNormal, thickness)
}

// SetModel overrides the player's character model.
func (p Player) SetModel(model string) int32 {
	ptr, n := strArg(model)
	return rawPlayerSetModel(p.Slot, ptr, n)
}

// ResetModel restores the player's default team model.
func (p Player) ResetModel() int32 { return rawPlayerResetModel(p.Slot) }

// GiveItem grants an item or weapon entity by full classname (e.g. "weapon_ak47").
func (p Player) GiveItem(item string) int32 {
	ptr, n := strArg(item)
	return rawPlayerGiveItem(p.Slot, ptr, n)
}

// GiveWeapon grants a weapon, auto-prefixing "weapon_" when omitted (e.g. "ak47" -> "weapon_ak47").
func (p Player) GiveWeapon(name string) int32 {
	if !strings.HasPrefix(name, "weapon_") && !strings.HasPrefix(name, "item_") && !strings.HasPrefix(name, "ammo_") {
		name = "weapon_" + name
	}
	return p.GiveItem(name)
}

// GiveCustomWeapon grants a registered declarative custom weapon by its ID.
func (p Player) GiveCustomWeapon(idName string) int32 {
	ptr, n := strArg(idName)
	return rawGiveCustomWeapon(p.Slot, ptr, n)
}

// StripWeapons strips all weapons from the player.
func (p Player) StripWeapons() int32 { return rawPlayerStripWeapons(p.Slot) }

// WeaponClip returns the magazine count for weaponID (<= 0 for active weapon).
func (p Player) WeaponClip(weaponID int32) int32 { return rawPlayerGetWeaponClip(p.Slot, weaponID) }

// SetWeaponClip sets the magazine count for weaponID.
func (p Player) SetWeaponClip(weaponID, clip int32) int32 {
	return rawPlayerSetWeaponClip(p.Slot, weaponID, clip)
}

// BpAmmo returns the reserve backpack ammo for weaponID.
func (p Player) BpAmmo(weaponID int32) int32 { return rawPlayerGetBpAmmo(p.Slot, weaponID) }

// SetBpAmmo sets the reserve backpack ammo for weaponID.
func (p Player) SetBpAmmo(weaponID, amount int32) int32 {
	return rawPlayerSetBpAmmo(p.Slot, weaponID, amount)
}

// SetDefuser grants or removes a CT defusal kit.
func (p Player) SetDefuser(has bool) int32 { return rawPlayerSetDefuser(p.Slot, boolToI32(has)) }

// SetNVG grants or removes night vision goggles.
func (p Player) SetNVG(has bool) int32 { return rawPlayerSetNVG(p.Slot, boolToI32(has)) }

// DropSlot drops the weapon in inventory slot invSlot (1..5).
func (p Player) DropSlot(invSlot int32) int32 { return rawPlayerDropSlot(p.Slot, invSlot) }

// AdminFlags returns the player's AMXX RBAC bitmask.
func (p Player) AdminFlags() int32 { return rawAdminPlayerFlags(p.Slot) }

// HasFlag reports whether the player holds the given AMXX access flag letter (e.g. 't' for VIP).
func (p Player) HasFlag(flag rune) bool { return rawAdminHasFlag(p.Slot, int32(flag)) > 0 }

// Lang returns the player's active ISO language code (e.g. "en", "ru").
func (p Player) Lang() string {
	s, ok := readString(32, func(out unsafe.Pointer, cap int32) int32 {
		return rawLangPlayerCode(p.Slot, out, cap)
	})
	if !ok {
		return "en"
	}
	return s
}

// SetLang updates the player's active language code.
func (p Player) SetLang(code string) int32 {
	ptr, n := strArg(code)
	return rawLangSetPlayerCode(p.Slot, ptr, n)
}

// Tr translates key in this player's active language using the shared LangCatalog.
func (p Player) Tr(key string, args ...LangArg) string {
	return translateSlot(p.Slot, key, args...)
}

// Say sends a colored chat line to this player.
func (p Player) Say(text string) int32 {
	ptr, n := strArg(text)
	return rawSayText(p.Slot, ptr, n)
}

// SayTr translates key for this player and sends the result as chat text.
func (p Player) SayTr(key string, args ...LangArg) int32 {
	return p.Say(p.Tr(key, args...))
}

// Print sends a console message to this player.
func (p Player) Print(text string) int32 {
	ptr, n := strArg(text)
	return rawPrintClient(p.Slot, ptr, n)
}

// CenterPrint sends a center-screen banner to this player.
func (p Player) CenterPrint(text string) int32 {
	ptr, n := strArg(text)
	return rawCenterPrint(p.Slot, ptr, n)
}

// ShowMOTD opens the CS 1.6 VGUI2 MOTD window for this player.
func (p Player) ShowMOTD(title, body string) int32 {
	tp, tn := strArg(title)
	bp, bn := strArg(body)
	return rawShowMOTD(p.Slot, tp, tn, bp, bn)
}

// ScreenFade applies a ScreenFade effect to this player.
func (p Player) ScreenFade(durationSec, holdSec float32, flags int32, col Color) int32 {
	return rawScreenFade(p.Slot, durationSec, holdSec, flags, col.PackRGBA())
}

// ScreenShake shakes this player's camera.
func (p Player) ScreenShake(amplitude, durationSec, frequency float32) int32 {
	return rawScreenShake(p.Slot, amplitude, durationSec, frequency)
}

// BarTime shows a progress bar for seconds (0 cancels).
func (p Player) BarTime(seconds int32) int32 { return rawBarTime(p.Slot, seconds) }
