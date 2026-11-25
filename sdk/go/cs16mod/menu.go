//go:build wasm

package cs16mod

// Classic AMX Mod X newmenus.inc property keys (MPROP_*).
const (
	MPropPerPage     int32 = 1
	MPropBackName    int32 = 2
	MPropNextName    int32 = 3
	MPropExitName    int32 = 4
	MPropTitle       int32 = 5
	MPropExit        int32 = 6
	MPropNoColors    int32 = 7
	MPropNumberColor int32 = 8
	MPropShowPage    int32 = 9
	MExitAll         int32 = 1
	MExitNever       int32 = -1
)

// MenuCallback handles a player selection on a host-managed paginated menu.
// When the player presses 0. Exit, itemIndex equals MenuExit (-3).
type MenuCallback func(p Player, menuID uint32, itemIndex int32, info string) HookAction

var (
	nextMenuCBID  int32 = 1
	menuCallbacks       = map[int32]MenuCallback{}
)

// Menu wraps a host-managed paginated GoldSrc menu (`1..7`, `8. Back`, `9. More`, `0. Exit`).
type Menu struct {
	ID   int32
	cbID int32
}

// NewMenu creates a host-managed paginated menu and registers cb to receive item selections.
func NewMenu(title string, cb MenuCallback) *Menu {
	cbID := nextMenuCBID
	nextMenuCBID++
	if cb != nil {
		menuCallbacks[cbID] = cb
	}
	tp, tn := strArg(title)
	id := rawMenuCreate(tp, tn, cbID)
	return &Menu{ID: id, cbID: cbID}
}

// AddItem appends a selectable item with an opaque info payload.
func (m *Menu) AddItem(label, info string, enabled ...bool) *Menu {
	if m.ID <= 0 {
		return m
	}
	en := true
	if len(enabled) > 0 {
		en = enabled[0]
	}
	lp, ln := strArg(label)
	ip, in := strArg(info)
	rawMenuAddItem(m.ID, lp, ln, ip, in, boolToI32(en))
	return m
}

// AddBlank inserts a non-selectable blank spacer line.
func (m *Menu) AddBlank() *Menu {
	if m.ID > 0 {
		rawMenuAddBlank(m.ID)
	}
	return m
}

// AddText inserts a non-selectable descriptive text line.
func (m *Menu) AddText(text string) *Menu {
	if m.ID > 0 {
		p, n := strArg(text)
		rawMenuAddText(m.ID, p, n)
	}
	return m
}

// SetPerPage configures selectable items per page (1..7, or 0 to disable pagination).
func (m *Menu) SetPerPage(count int32) *Menu {
	if m.ID > 0 {
		rawMenuSetPropInt(m.ID, MPropPerPage, count)
	}
	return m
}

// SetAllowExit enables or disables the 0. Exit option.
func (m *Menu) SetAllowExit(allow bool) *Menu {
	if m.ID > 0 {
		v := MExitNever
		if allow {
			v = MExitAll
		}
		rawMenuSetPropInt(m.ID, MPropExit, v)
	}
	return m
}

// SetBackLabel sets the localized label for key 8. Back.
func (m *Menu) SetBackLabel(label string) *Menu {
	if m.ID > 0 {
		p, n := strArg(label)
		rawMenuSetPropStr(m.ID, MPropBackName, p, n)
	}
	return m
}

// SetNextLabel sets the localized label for key 9. More.
func (m *Menu) SetNextLabel(label string) *Menu {
	if m.ID > 0 {
		p, n := strArg(label)
		rawMenuSetPropStr(m.ID, MPropNextName, p, n)
	}
	return m
}

// SetExitLabel sets the localized label for key 0. Exit.
func (m *Menu) SetExitLabel(label string) *Menu {
	if m.ID > 0 {
		p, n := strArg(label)
		rawMenuSetPropStr(m.ID, MPropExitName, p, n)
	}
	return m
}

// Display opens this menu for player p at the given 0-based page and timeout (-1 for permanent).
func (m *Menu) Display(p Player, page, timeSec int32) bool {
	if m.ID <= 0 {
		return false
	}
	return rawMenuDisplay(p.Slot, m.ID, page, timeSec) >= 0
}

// Destroy releases this menu inside the host MenuManager.
func (m *Menu) Destroy() {
	if m.ID > 0 {
		rawMenuDestroy(m.ID)
		m.ID = -1
	}
	delete(menuCallbacks, m.cbID)
}
