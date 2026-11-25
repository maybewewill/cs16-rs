//go:build wasm

package cs16mod

import (
	"strings"
	"unsafe"
)

// HookPhase selects Pre or Post execution relative to the inner game handler.
type HookPhase int32

const (
	PhasePre  HookPhase = 0
	PhasePost HookPhase = 1
)

// HookAction controls hook chain propagation and inner handler execution.
type HookAction int32

const (
	Continue  HookAction = 0
	Handled   HookAction = 1
	Override  HookAction = 2
	Supercede HookAction = 3
)

// HookID matches every ABI v1 + v2 discriminant in cs-mod-api::HookId.
type HookID int32

const (
	HookServerActivate        HookID = 1
	HookServerDeactivate      HookID = 2
	HookSpawnEntity           HookID = 3
	HookClientConnect         HookID = 4
	HookClientPutInServer     HookID = 5
	HookClientDisconnect      HookID = 6
	HookClientCommand         HookID = 7
	HookClientUserinfoChanged HookID = 8
	HookRunUsercmd            HookID = 9
	HookPostUsercmd           HookID = 10
	HookStartFrame            HookID = 11
	HookThink                 HookID = 12
	HookTouch                 HookID = 13
	HookBlocked               HookID = 14
	HookUpdateClientData      HookID = 15
	HookGetWeaponData         HookID = 16
	HookServerCommand         HookID = 17
	HookAddToFullPack         HookID = 18

	HookPlayerDeath          HookID = 30
	HookRoundStart           HookID = 31
	HookPlayerSpawn          HookID = 32
	HookPlayerTakeDamage     HookID = 33
	HookPlayerKilled         HookID = 34
	HookPlayerPreThink       HookID = 35
	HookPlayerPostThink      HookID = 36
	HookPlayerJump           HookID = 37
	HookPlayersShouldCollide HookID = 38
	HookCanBuyItem           HookID = 39
	HookCanHavePlayerItem    HookID = 40
	HookWeaponDeploy         HookID = 41
	HookWeaponPrimaryAttack  HookID = 42
	HookWeaponReload         HookID = 43
	HookPlayerSay            HookID = 44
	HookMenuSelect           HookID = 45
	HookFreezeTimeEnd        HookID = 46
	HookRoundEnd             HookID = 47
	HookBombPlanting         HookID = 48
	HookBombPlanted          HookID = 49
	HookBombDefusing         HookID = 50
	HookBombDefused          HookID = 51
	HookBombExploded         HookID = 52
	HookHostageTouched       HookID = 53
	HookHostageRescued       HookID = 54
	HookGrenadeThrow         HookID = 55

	HookEngEmitSound    HookID = 100
	HookEngSendUserMsg  HookID = 101
	HookEngClientPrint  HookID = 102
	HookEngCenterPrint  HookID = 103
	HookEngCreateEntity HookID = 104
	HookEngRemoveEntity HookID = 105
	HookEngKick         HookID = 106
	HookEngChangeLevel  HookID = 107
)

// DamageEvent carries PlayerTakeDamage hook arguments with mutable Pre overrides.
type DamageEvent struct {
	Victim       Player
	AttackerSlot int32
	Hitgroup     int32
	WeaponID     int32
	DamageBits   int32
	TeamAttack   bool
	Damage       float32
}

// HasAttacker reports whether damage was caused by a player slot.
func (e *DamageEvent) HasAttacker() bool { return e.AttackerSlot >= 0 }

// Attacker returns the attacker Player handle when HasAttacker is true.
func (e *DamageEvent) Attacker() Player { return NewPlayer(e.AttackerSlot) }

// SetDamage overrides the incoming damage amount (return Override from a Pre hook).
func (e *DamageEvent) SetDamage(dmg float32) {
	e.Damage = dmg
	rawHookSetOverrideF32(dmg)
}

// SetHitgroup overrides the damage hitgroup (return Override from a Pre hook).
func (e *DamageEvent) SetHitgroup(hitgroup int32) {
	e.Hitgroup = hitgroup
	rawHookSetOverrideI32(hitgroup)
}

// KilledEvent carries PlayerKilled hook arguments.
type KilledEvent struct {
	Victim     Player
	KillerSlot int32
	WeaponID   int32
	Hitgroup   int32
	Headshot   bool
	TeamKill   bool
	WeaponName string
}

type hookHandlerEntry struct {
	hook     HookID
	phase    HookPhase
	priority int32
	fn       func() HookAction
}

type chatHandlerEntry struct {
	trigger string
	fn      func(p Player, args string) HookAction
}

type cmdHandlerEntry struct {
	cmd string
	fn  func(p Player, args string) HookAction
}

type rpcExportEntry struct {
	name string
	cbID int32
	fn   func(argInt int32, argStr string) (int32, string)
}

type rpcEventEntry struct {
	eventName string
	cbID      int32
	fn        func(payloadJSON string) HookAction
}

var (
	initCallbacks    []func() int32
	hookHandlers     []hookHandlerEntry
	chatHandlers     []chatHandlerEntry
	cmdHandlers      []cmdHandlerEntry
	timerCallbacks   = map[int32]func(arg int32) HookAction{}
	webhookCallbacks = map[int32]func(status int32, body, errMsg string) HookAction{}
	rpcExports       []rpcExportEntry
	rpcEvents        []rpcEventEntry
	nextRPCCBID      int32 = 1
	nextEventCBID    int32 = 1
)

// OnInit registers a callback executed during cs_mod_init after hooks are bound.
func OnInit(fn func() int32) {
	initCallbacks = append(initCallbacks, fn)
}

// OnHook registers a low-level handler for any ABI v1/v2 HookID and HookPhase.
func OnHook(hook HookID, phase HookPhase, priority int32, fn func() HookAction) {
	hookHandlers = append(hookHandlers, hookHandlerEntry{
		hook:     hook,
		phase:    phase,
		priority: priority,
		fn:       fn,
	})
}

// OnChat registers a chat trigger (e.g. "/rtv", "rtv", "/nominate") matched case-insensitively
// against the first token of PlayerSay Pre messages.
func OnChat(trigger string, fn func(p Player, args string) HookAction) {
	chatHandlers = append(chatHandlers, chatHandlerEntry{trigger: trigger, fn: fn})
}

// OnCommand registers a client console command handler (e.g. "amx_votemap").
func OnCommand(cmd string, fn func(p Player, args string) HookAction) {
	cmdHandlers = append(cmdHandlers, cmdHandlerEntry{cmd: cmd, fn: fn})
}

// OnPlayerTakeDamage registers a typed PlayerTakeDamage handler.
func OnPlayerTakeDamage(phase HookPhase, fn func(ev *DamageEvent) HookAction) {
	OnHook(HookPlayerTakeDamage, phase, 0, func() HookAction {
		ev := DamageEvent{
			Victim:       NewPlayer(rawHookArgI32(0)),
			AttackerSlot: rawHookArgI32(1),
			Hitgroup:     rawHookArgI32(2),
			WeaponID:     rawHookArgI32(3),
			DamageBits:   rawHookArgI32(4),
			TeamAttack:   rawHookArgI32(5) != 0,
			Damage:       rawHookArgF32(0),
		}
		return fn(&ev)
	})
}

// OnPlayerSpawn registers a typed PlayerSpawn handler.
func OnPlayerSpawn(phase HookPhase, fn func(p Player) HookAction) {
	OnHook(HookPlayerSpawn, phase, 0, func() HookAction {
		return fn(NewPlayer(rawHookArgI32(0)))
	})
}

// OnPlayerKilled registers a typed PlayerKilled handler.
func OnPlayerKilled(phase HookPhase, fn func(ev KilledEvent) HookAction) {
	OnHook(HookPlayerKilled, phase, 0, func() HookAction {
		weaponName, _ := readString(64, func(out unsafe.Pointer, cap int32) int32 {
			return rawHookArgStr(0, out, cap)
		})
		ev := KilledEvent{
			Victim:     NewPlayer(rawHookArgI32(0)),
			KillerSlot: rawHookArgI32(1),
			WeaponID:   rawHookArgI32(2),
			Hitgroup:   rawHookArgI32(3),
			Headshot:   rawHookArgI32(4) != 0,
			TeamKill:   rawHookArgI32(5) != 0,
			WeaponName: weaponName,
		}
		return fn(ev)
	})
}

// OnRoundStart registers a Post RoundStart handler receiving the 1-based round number.
func OnRoundStart(fn func(roundNum int32) HookAction) {
	OnHook(HookRoundStart, PhasePost, 0, func() HookAction {
		return fn(rawHookArgI32(0))
	})
}

// OnRoundEnd registers a Post RoundEnd handler receiving (winner, reason, bonus).
func OnRoundEnd(fn func(winner, reason, bonus int32) HookAction) {
	OnHook(HookRoundEnd, PhasePost, 0, func() HookAction {
		return fn(rawHookArgI32(0), rawHookArgI32(1), rawHookArgI32(2))
	})
}

// OnTimer registers a timer callback for cbID (scheduled via TimerAfter / TimerRepeat).
func OnTimer(cbID int32, fn func(arg int32) HookAction) {
	timerCallbacks[cbID] = fn
}

// OnWebhook registers a callback for non-blocking PostWebhookJSON responses matching cbID.
func OnWebhook(cbID int32, fn func(status int32, body, errMsg string) HookAction) {
	webhookCallbacks[cbID] = fn
}

// OnRPC exports a named synchronous polyglot RPC function callable from Rust/C++/Go/Python/AMXX.
func OnRPC(fnName string, fn func(argInt int32, argStr string) (int32, string)) {
	id := nextRPCCBID
	nextRPCCBID++
	rpcExports = append(rpcExports, rpcExportEntry{name: fnName, cbID: id, fn: fn})
}

// OnEvent subscribes to a named cross-plugin event emitted via RPCEmitEvent.
func OnEvent(eventName string, fn func(payloadJSON string) HookAction) {
	id := nextEventCBID
	nextEventCBID++
	rpcEvents = append(rpcEvents, rpcEventEntry{eventName: eventName, cbID: id, fn: fn})
}

// ---------------------------------------------------------------------------
// Exported WASM Guest Entrypoints invoked by cs-mod-host::WasmiRuntime
// ---------------------------------------------------------------------------

//go:wasmexport cs_mod_init
func csModInit() int32 {
	registered := map[uint64]bool{}
	regOnce := func(hook HookID, phase HookPhase, priority int32) {
		key := (uint64(uint32(hook)) << 32) | uint64(uint32(phase))
		if !registered[key] {
			registered[key] = true
			rawHookRegister(int32(hook), int32(phase), priority)
		}
	}

	for _, h := range hookHandlers {
		regOnce(h.hook, h.phase, h.priority)
	}
	if len(chatHandlers) > 0 {
		regOnce(HookPlayerSay, PhasePre, 0)
	}
	if len(cmdHandlers) > 0 {
		regOnce(HookClientCommand, PhasePre, 0)
		for _, c := range cmdHandlers {
			RegisterCommand(c.cmd)
		}
	}
	for _, r := range rpcExports {
		p, n := strArg(r.name)
		rawRPCExport(p, n, r.cbID)
	}
	for _, ev := range rpcEvents {
		p, n := strArg(ev.eventName)
		rawRPCSubscribe(p, n, ev.cbID)
	}
	for _, fn := range initCallbacks {
		if rc := fn(); rc < 0 {
			return rc
		}
	}
	return 0
}

//go:wasmexport cs_mod_hook
func csModHook(hookID, phase int32) int32 {
	best := Continue
	h := HookID(hookID)
	ph := HookPhase(phase)

	if h == HookPlayerSay && ph == PhasePre && len(chatHandlers) > 0 {
		slot := rawHookArgI32(0)
		msg, _ := readString(512, func(out unsafe.Pointer, cap int32) int32 {
			return rawHookArgStr(0, out, cap)
		})
		trimmed := strings.TrimSpace(msg)
		tok, rest, _ := strings.Cut(trimmed, " ")
		rest = strings.TrimSpace(rest)
		for _, ch := range chatHandlers {
			if strings.EqualFold(tok, ch.trigger) {
				act := ch.fn(NewPlayer(slot), rest)
				if act > best {
					best = act
				}
				if act == Supercede {
					return int32(Supercede)
				}
			}
		}
	}

	if h == HookClientCommand && ph == PhasePre && len(cmdHandlers) > 0 {
		slot := rawHookArgI32(0)
		cmd, _ := readString(128, func(out unsafe.Pointer, cap int32) int32 {
			return rawHookArgStr(0, out, cap)
		})
		fullLine, _ := readString(512, func(out unsafe.Pointer, cap int32) int32 {
			return rawHookArgStr(2, out, cap)
		})
		_, rest, _ := strings.Cut(strings.TrimSpace(fullLine), " ")
		rest = strings.TrimSpace(rest)
		for _, ch := range cmdHandlers {
			if strings.EqualFold(cmd, ch.cmd) {
				act := ch.fn(NewPlayer(slot), rest)
				if act > best {
					best = act
				}
				if act == Supercede {
					return int32(Supercede)
				}
			}
		}
	}

	for _, entry := range hookHandlers {
		if entry.hook == h && entry.phase == ph {
			act := entry.fn()
			if act > best {
				best = act
			}
			if act == Supercede {
				return int32(Supercede)
			}
		}
	}
	return int32(best)
}

//go:wasmexport cs_mod_timer
func csModTimer(cbID, arg int32) int32 {
	if fn, ok := timerCallbacks[cbID]; ok && fn != nil {
		return int32(fn(arg))
	}
	return int32(Continue)
}

//go:wasmexport cs_mod_menu
func csModMenu(cbID, slot, menuID, itemIndex int32) int32 {
	fn, ok := menuCallbacks[cbID]
	if !ok || fn == nil {
		return int32(Continue)
	}
	info, _ := readString(256, func(out unsafe.Pointer, cap int32) int32 {
		return rawHookArgStr(0, out, cap)
	})
	return int32(fn(NewPlayer(slot), uint32(menuID), itemIndex, info))
}

//go:wasmexport cs_mod_rpc
func csModRPC(cbID int32) int32 {
	for _, r := range rpcExports {
		if r.cbID == cbID && r.fn != nil {
			argInt := rawHookArgI32(0)
			argStr, _ := readString(4096, func(out unsafe.Pointer, cap int32) int32 {
				return rawHookArgStr(0, out, cap)
			})
			retInt, retStr := r.fn(argInt, argStr)
			if retStr != "" {
				p, n := strArg(retStr)
				rawRPCSetReturnStr(p, n)
			}
			return retInt
		}
	}
	return 0
}

//go:wasmexport cs_mod_event
func csModEvent(cbID int32) int32 {
	for _, ev := range rpcEvents {
		if ev.cbID == cbID && ev.fn != nil {
			payload, _ := readString(4096, func(out unsafe.Pointer, cap int32) int32 {
				return rawHookArgStr(1, out, cap)
			})
			return int32(ev.fn(payload))
		}
	}
	return int32(Continue)
}

//go:wasmexport cs_mod_webhook
func csModWebhook(cbID, status int32) int32 {
	fn, ok := webhookCallbacks[cbID]
	if !ok || fn == nil {
		return int32(Continue)
	}
	body, _ := readString(16384, func(out unsafe.Pointer, cap int32) int32 {
		return rawHookArgStr(0, out, cap)
	})
	errMsg, _ := readString(1024, func(out unsafe.Pointer, cap int32) int32 {
		return rawHookArgStr(1, out, cap)
	})
	return int32(fn(status, body, errMsg))
}
