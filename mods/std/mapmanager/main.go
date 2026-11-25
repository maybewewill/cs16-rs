//go:build wasm

// Standard Map Manager (`std_mapmanager`) — Go -> WASI/WASM plugin.
// Provides `mapchooser`, `nextmap`, `timeleft`, `thetime`, `rtv`, `nominate`/`/maps`,
// end-of-map countdown vote with "Extend <map> +15m" and "Finish current round before changelevel".
package main

import (
	_ "embed"
	"fmt"
	"strings"

	"github.com/cs16-rs/cs16-rs/sdk/go/cs16mod"
)

//go:embed lang.toml
var langTOML string

const (
	timerVoteFinish int32 = 101
	timerEndOfMap   int32 = 102
)

var (
	mapPool = []string{
		"de_dust2",
		"de_inferno",
		"de_nuke",
		"de_train",
		"de_mirage",
		"de_cbble",
		"de_aztec",
		"cs_office",
		"cs_italy",
	}
	nominations       []string
	rtvVoters         = map[int32]bool{}
	voteActive        = false
	voteCandidates    []string
	voteTally         = map[string]int{}
	voteCastBySlot    = map[int32]bool{}
	nextMap           = "de_inferno"
	pendingChange     = false
	extendedCount     = 0
	currentMapCached  = "de_dust2"
)

func activePlayers() []cs16mod.Player {
	max := cs16mod.MaxClients()
	out := make([]cs16mod.Player, 0, max)
	for s := int32(0); s < max; s++ {
		p := cs16mod.NewPlayer(s)
		if p.IsActive() {
			out = append(out, p)
		}
	}
	return out
}

func broadcastTr(key string, args ...cs16mod.LangArg) {
	for _, p := range activePlayers() {
		p.SayTr(key, args...)
	}
}

func formatSeconds(secs int64) string {
	if secs < 0 {
		secs = 0
	}
	m := secs / 60
	s := secs % 60
	return fmt.Sprintf("%02d:%02d", m, s)
}

func startMapVote() {
	if voteActive {
		return
	}
	voteActive = true
	voteTally = map[string]int{}
	voteCastBySlot = map[int32]bool{}
	voteCandidates = nil

	seen := map[string]bool{currentMapCached: true}
	for _, nom := range nominations {
		if !seen[nom] && len(voteCandidates) < 4 {
			seen[nom] = true
			voteCandidates = append(voteCandidates, nom)
		}
	}
	for _, m := range mapPool {
		if !seen[m] && len(voteCandidates) < 4 {
			seen[m] = true
			voteCandidates = append(voteCandidates, m)
		}
	}
	extendKey := "__extend__"
	for _, c := range voteCandidates {
		voteTally[c] = 0
	}
	voteTally[extendKey] = 0

	for _, p := range activePlayers() {
		title := cs16mod.MenuYellow + p.Tr("MAP_VOTE_TITLE") + cs16mod.MenuWhite
		menu := cs16mod.NewMenu(title, onVoteSelect).SetPerPage(7).SetAllowExit(true)
		for _, m := range voteCandidates {
			menu.AddItem(m, m)
		}
		if extendedCount < 2 {
			menu.AddItem(p.Tr("MAP_VOTE_EXTEND", cs16mod.Arg("map", currentMapCached)), extendKey)
		}
		menu.Display(p, 0, 15)
		p.ScreenFade(0.3, 0.2, 0, cs16mod.Color{R: 0, G: 0, B: 0, A: 90})
	}
	cs16mod.TimerAfter(15000, timerVoteFinish, 0)
}

func onVoteSelect(p cs16mod.Player, menuID uint32, itemIndex int32, info string) cs16mod.HookAction {
	if itemIndex == cs16mod.MenuExit || !voteActive || voteCastBySlot[p.Slot] {
		return cs16mod.Handled
	}
	voteCastBySlot[p.Slot] = true
	voteTally[info]++
	label := info
	if info == "__extend__" {
		label = p.Tr("MAP_VOTE_EXTEND", cs16mod.Arg("map", currentMapCached))
	}
	broadcastTr("MAP_VOTE_CAST", cs16mod.Arg("player", p.Name()), cs16mod.Arg("choice", label))
	return cs16mod.Handled
}

func finishMapVote(_ int32) cs16mod.HookAction {
	if !voteActive {
		return cs16mod.Continue
	}
	voteActive = false
	bestKey := ""
	bestVotes := -1
	for _, c := range voteCandidates {
		if voteTally[c] > bestVotes {
			bestVotes = voteTally[c]
			bestKey = c
		}
	}
	if extendedCount < 2 && voteTally["__extend__"] > bestVotes {
		bestKey = "__extend__"
	}
	if bestKey == "__extend__" {
		extendedCount++
		tl := cs16mod.CvarFloat("mp_timelimit")
		if tl <= 0 {
			tl = 20
		}
		cs16mod.SetCvarFloat("mp_timelimit", tl+15)
		broadcastTr("MAP_VOTE_EXTENDED", cs16mod.Arg("map", currentMapCached))
	} else if bestKey != "" {
		nextMap = bestKey
		pendingChange = true
		cs16mod.StorageSet("nextmap", nextMap)
		broadcastTr("MAP_VOTE_WON", cs16mod.Arg("map", nextMap))
	}
	nominations = nil
	rtvVoters = map[int32]bool{}
	return cs16mod.Continue
}

func openNominationMenu(p cs16mod.Player) {
	title := cs16mod.MenuYellow + p.Tr("MAP_NOM_TITLE") + cs16mod.MenuWhite
	menu := cs16mod.NewMenu(title, func(pl cs16mod.Player, _ uint32, itemIndex int32, info string) cs16mod.HookAction {
		if itemIndex == cs16mod.MenuExit || info == "" {
			return cs16mod.Handled
		}
		nominateMap(pl, info)
		return cs16mod.Handled
	}).SetPerPage(7)
	for _, m := range mapPool {
		if m != currentMapCached {
			menu.AddItem(m, m)
		}
	}
	menu.Display(p, 0, 20)
}

func nominateMap(p cs16mod.Player, mapName string) {
	clean := strings.TrimSpace(strings.ToLower(mapName))
	if clean == "" {
		openNominationMenu(p)
		return
	}
	if len(nominations) >= 4 {
		p.SayTr("MAP_NOM_FULL")
		return
	}
	for _, existing := range nominations {
		if existing == clean {
			return
		}
	}
	nominations = append(nominations, clean)
	broadcastTr("MAP_NOM_ADDED", cs16mod.Arg("player", p.Name()), cs16mod.Arg("map", clean))
}

func handleRtv(p cs16mod.Player, _ string) cs16mod.HookAction {
	if voteActive {
		return cs16mod.Handled
	}
	if rtvVoters[p.Slot] {
		p.SayTr("MAP_RTV_ALREADY")
		return cs16mod.Handled
	}
	rtvVoters[p.Slot] = true
	total := len(activePlayers())
	if total < 1 {
		total = 1
	}
	needed := (total*60 + 99) / 100
	if needed < 1 {
		needed = 1
	}
	votes := len(rtvVoters)
	broadcastTr("MAP_RTV_VOTED",
		cs16mod.Arg("player", p.Name()),
		cs16mod.Arg("votes", fmt.Sprintf("%d", votes)),
		cs16mod.Arg("needed", fmt.Sprintf("%d", needed)),
	)
	if votes >= needed {
		broadcastTr("MAP_RTV_TRIGGERED")
		startMapVote()
	}
	return cs16mod.Handled
}

func init() {
	cs16mod.OnInit(func() int32 {
		cs16mod.LangLoadTOML(langTOML)
		if saved, ok := cs16mod.StorageGet("nextmap"); ok && saved != "" {
			nextMap = saved
		}
		cs16mod.TimerRepeat(30000, timerEndOfMap, 0)
		return 0
	})

	cs16mod.OnTimer(timerVoteFinish, finishMapVote)
	cs16mod.OnTimer(timerEndOfMap, func(_ int32) cs16mod.HookAction {
		tl := cs16mod.CvarFloat("mp_timelimit")
		if tl <= 0 || voteActive || pendingChange {
			return cs16mod.Continue
		}
		elapsedMin := float32(cs16mod.TimeMs()) / 60000.0
		if tl-elapsedMin <= 2.0 {
			startMapVote()
		}
		return cs16mod.Continue
	})

	cs16mod.OnRoundEnd(func(_, _, _ int32) cs16mod.HookAction {
		if pendingChange && nextMap != "" {
			pendingChange = false
			cs16mod.ChangeLevel(nextMap)
		}
		return cs16mod.Continue
	})

	cs16mod.OnChat("nextmap", func(p cs16mod.Player, _ string) cs16mod.HookAction {
		p.SayTr("MAP_NEXTMAP", cs16mod.Arg("map", nextMap))
		return cs16mod.Handled
	})
	cs16mod.OnChat("/nextmap", func(p cs16mod.Player, _ string) cs16mod.HookAction {
		p.SayTr("MAP_NEXTMAP", cs16mod.Arg("map", nextMap))
		return cs16mod.Handled
	})
	cs16mod.OnChat("timeleft", func(p cs16mod.Player, _ string) cs16mod.HookAction {
		tl := int64(cs16mod.CvarFloat("mp_timelimit") * 60.0)
		rem := tl - (cs16mod.TimeMs() / 1000)
		if tl <= 0 {
			p.SayTr("MAP_TIMELEFT", cs16mod.Arg("time", "No Time Limit"))
		} else {
			p.SayTr("MAP_TIMELEFT", cs16mod.Arg("time", formatSeconds(rem)))
		}
		return cs16mod.Handled
	})
	cs16mod.OnChat("thetime", func(p cs16mod.Player, _ string) cs16mod.HookAction {
		p.SayTr("MAP_THETIME", cs16mod.Arg("time", formatSeconds(cs16mod.TimeMs()/1000)))
		return cs16mod.Handled
	})
	cs16mod.OnChat("rtv", handleRtv)
	cs16mod.OnChat("/rtv", handleRtv)
	cs16mod.OnChat("nominate", func(p cs16mod.Player, args string) cs16mod.HookAction {
		nominateMap(p, args)
		return cs16mod.Handled
	})
	cs16mod.OnChat("/nominate", func(p cs16mod.Player, args string) cs16mod.HookAction {
		nominateMap(p, args)
		return cs16mod.Handled
	})
	cs16mod.OnChat("/maps", func(p cs16mod.Player, _ string) cs16mod.HookAction {
		openNominationMenu(p)
		return cs16mod.Supercede
	})

	cs16mod.OnCommand("amx_votemap", func(p cs16mod.Player, _ string) cs16mod.HookAction {
		if p.HasFlag('j') {
			startMapVote()
		}
		return cs16mod.Handled
	})

	cs16mod.OnRPC("mapmanager.start_vote", func(_ int32, _ string) (int32, string) {
		startMapVote()
		return 1, nextMap
	})
	cs16mod.OnRPC("mapmanager.nextmap", func(_ int32, _ string) (int32, string) {
		return 1, nextMap
	})
}

func main() {}
