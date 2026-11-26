# cs16-rs Competitive Gamemode: Multi-1v1 Arena Ladder (`multi_1v1.py`)
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Zero-compile Python 3 competitive 1v1 arena ladder plugin (< 5 ms hot-reload):
# - Arena 1..N ladder pairing (`Arena 1 [Server Champion]`, `Arena 2`, ..., `Arena N`)
#   with Winner-UP (`Arena k -> Arena k-1`) and Loser-DOWN (`Arena k -> Arena k+1`)
# - Authentic GoldSrc `/guns` (`!guns`) preference menu toggling Rifle (`AK-47`/`M4A1`),
#   Pistol (`Deagle`/`USP`/`Glock`), and AWP duel opt-in, resolved cleanly per pair
# - Spatial arena isolation + cross-arena damage guard, full HP/Armor kits (`100 HP + VestHelm`
#   on rifle/AWP, `100 HP + Vest` on pistol), persistent Elo ratings + Arena-1 crown defenses,
#   and `/top1v1` / `!rank` leaderboard commands with full `en` + `ru` localization.

import json
from cs16 import mod, vfx, Color, HookAction, Armor, Team, Menu


@mod.info(
    id="multi_1v1",
    name="Multi-1v1 Arena Ladder",
    version="1.0.0",
    author="cs16-rs",
    description="Competitive Arena 1..N 1v1 duel ladder with /guns preferences, arena isolation, and Elo leaderboard",
    permissions=[
        "entities",
        "messages",
        "cvars",
        "commands",
        "menus",
        "timers",
        "storage",
        "weapons",
        "vfx",
        "sound",
    ],
)
def _meta():
    pass


DEFAULT_ELO = 1000
ELO_K = 32.0
LEADERBOARD_TABLE = "multi_1v1_elo"
ARENA_SPACING = 1600.0
DUEL_HALF_DIST = 320.0

FALLBACK_LANG_TOML = """
[en]
M1V1_ARENA_TAG = "[Arena {arena}]"
M1V1_CHAMP_TAG = "[Arena 1 - Server Champion]"
M1V1_MATCH_START = "\\x04{tag}\\x01 {p1} vs {p2} — Mode: {mode}"
M1V1_BANNER_CHAMP = "ARENA 1 [SERVER CHAMPION] — {p1} vs {p2} ({mode})"
M1V1_BANNER_ARENA = "ARENA {arena} — {p1} vs {p2} ({mode})"
M1V1_WAIT_QUEUE = "\\x04[Arena Wait]\\x01 Waiting 1 round for the next 1v1 arena."
M1V1_DUEL_WIN = "\\x04{tag}\\x01 {winner} defeated {loser}! (+{delta} Elo -> {elo})"
M1V1_CHAMP_DEFEND = "\\x04[Arena 1]\\x01 {winner} defended the Server Champion crown ({defenses} defenses)!"
M1V1_PROMOTED = "\\x04[Arena {arena}]\\x01 Promoted UP to Arena {arena}!"
M1V1_RELEGATED = "\\x04[Arena {arena}]\\x01 Moved DOWN to Arena {arena}."
M1V1_GUNS_TITLE = "Multi-1v1 Arena Weapons"
M1V1_PREF_SAVED = "\\x04[Multi-1v1]\\x01 Preferences saved: Rifle {rifle} | Pistol {pistol} | Modes: {modes}."
M1V1_TOP_HEADER = "\\x04[Multi-1v1]\\x01 Top 1v1 Arena Elo Leaderboard:"
M1V1_TOP_ROW = "#{rank} {name} — {elo} Elo ({wins}W/{losses}L, Arena 1 defenses: {defenses})"
M1V1_TOP_EMPTY = "\\x04[Multi-1v1]\\x01 No ranked 1v1 duels played yet."
M1V1_RANK_LINE = "\\x04[Multi-1v1]\\x01 Rank #{rank}/{total} — {elo} Elo ({wins}W/{losses}L, Arena 1 defenses: {defenses}, Arena {arena})."
M1V1_MODE_RIFLE = "RIFLE"
M1V1_MODE_PISTOL = "PISTOL"
M1V1_MODE_AWP = "AWP"

[ru]
M1V1_ARENA_TAG = "[Арена {arena}]"
M1V1_CHAMP_TAG = "[Арена 1 - Чемпион сервера]"
M1V1_MATCH_START = "\\x04{tag}\\x01 {p1} против {p2} — Режим: {mode}"
M1V1_BANNER_CHAMP = "АРЕНА 1 [ЧЕМПИОН СЕРВЕРА] — {p1} против {p2} ({mode})"
M1V1_BANNER_ARENA = "АРЕНА {arena} — {p1} против {p2} ({mode})"
M1V1_WAIT_QUEUE = "\\x04[Арена Ожидание]\\x01 Ожидание 1 раунд до следующей дуэли 1v1."
M1V1_DUEL_WIN = "\\x04{tag}\\x01 {winner} победил {loser}! (+{delta} Elo -> {elo})"
M1V1_CHAMP_DEFEND = "\\x04[Арена 1]\\x01 {winner} защитил корону Чемпиона сервера (защит: {defenses})!"
M1V1_PROMOTED = "\\x04[Арена {arena}]\\x01 Повышение на Арену {arena}!"
M1V1_RELEGATED = "\\x04[Арена {arena}]\\x01 Понижение на Арену {arena}."
M1V1_GUNS_TITLE = "Настройка оружия Multi-1v1"
M1V1_PREF_SAVED = "\\x04[Multi-1v1]\\x01 Настройки сохранены: Винтовка {rifle} | Пистолет {pistol} | Режимы: {modes}."
M1V1_TOP_HEADER = "\\x04[Multi-1v1]\\x01 Топ игроков Multi-1v1 (рейтинг Elo):"
M1V1_TOP_ROW = "#{rank} {name} — {elo} Elo ({wins}П/{losses}Пор, защит Арены 1: {defenses})"
M1V1_TOP_EMPTY = "\\x04[Multi-1v1]\\x01 Пока нет сыгранных дуэлей 1v1."
M1V1_RANK_LINE = "\\x04[Multi-1v1]\\x01 Ранг #{rank}/{total} — {elo} Elo ({wins}П/{losses}Пор, защит Арены 1: {defenses}, Арена {arena})."
M1V1_MODE_RIFLE = "ВИНТОВКИ"
M1V1_MODE_PISTOL = "ПИСТОЛЕТЫ"
M1V1_MODE_AWP = "AWP"
"""

_lang_loaded = False
ladder = []
active_arenas = []
slot_to_arena = {}
waiting_slot = None
round_counter = 0
round_promoted = False
pref_cache = {}
stats_cache = {}


def ensure_lang():
    global _lang_loaded
    if not _lang_loaded:
        # Ensure built-in bilingual keys are always present even in isolated unit runs.
        if mod.tr("M1V1_ARENA_TAG") == "M1V1_ARENA_TAG":
            mod.load_lang_toml(FALLBACK_LANG_TOML)
        _lang_loaded = True


def _sanitize_key(raw):
    out = []
    for ch in raw:
        if ch.isalnum() or ch in ("_", "-", "."):
            out.append(ch)
        else:
            out.append("_")
    s = "".join(out)
    return s[:40] if s else ""


def player_identity(player):
    sid = _sanitize_key(player.steamid)
    if sid:
        return sid
    nm = _sanitize_key(player.name)
    if nm:
        return f"name_{nm}"
    return f"slot_{player.slot}"


def default_pref():
    return {
        "rifle": "ak47",
        "pistol": "deagle",
        "allow_rifle": True,
        "allow_pistol": True,
        "allow_awp": False,
        "prefer": "rifle",
    }


def get_pref(player):
    ident = player_identity(player)
    if ident in pref_cache:
        return pref_cache[ident]
    raw = mod.storage.get(f"m1v1_pref_{ident}", "")
    pref = default_pref()
    if raw:
        try:
            data = json.loads(raw)
            if isinstance(data, dict):
                for k in pref:
                    if k in data:
                        pref[k] = data[k]
        except Exception:
            pass
    pref_cache[ident] = pref
    return pref


def save_pref(player, pref):
    ident = player_identity(player)
    pref_cache[ident] = pref
    mod.storage.set(f"m1v1_pref_{ident}", json.dumps(pref))


def get_stats(player):
    ident = player_identity(player)
    if ident in stats_cache:
        st = stats_cache[ident]
        if player.name:
            st["name"] = player.name
        return st
    raw = mod.storage.get(f"m1v1_stat_{ident}", "")
    st = {
        "name": player.name or f"Player{player.slot}",
        "elo": DEFAULT_ELO,
        "wins": 0,
        "losses": 0,
        "defenses": 0,
    }
    if raw:
        try:
            data = json.loads(raw)
            if isinstance(data, dict):
                st["name"] = data.get("name", st["name"]) or st["name"]
                st["elo"] = int(data.get("elo", DEFAULT_ELO))
                st["wins"] = int(data.get("wins", 0))
                st["losses"] = int(data.get("losses", 0))
                st["defenses"] = int(data.get("defenses", 0))
        except Exception:
            pass
    if player.name:
        st["name"] = player.name
    stats_cache[ident] = st
    return st


def save_stats(player, st):
    ident = player_identity(player)
    stats_cache[ident] = st
    payload = json.dumps(st)
    mod.storage.set(f"m1v1_stat_{ident}", payload)
    mod.leaderboard.upsert(LEADERBOARD_TABLE, ident, float(st["elo"]), payload)


def arena_tag(player, arena_num):
    if arena_num == 1:
        return player.tr("M1V1_CHAMP_TAG")
    return player.tr("M1V1_ARENA_TAG", arena=arena_num)


def mode_label(player, mode):
    if mode == "pistol":
        return player.tr("M1V1_MODE_PISTOL")
    if mode == "awp":
        return player.tr("M1V1_MODE_AWP")
    return player.tr("M1V1_MODE_RIFLE")


def resolve_duel_mode(pref_a, pref_b, arena_num, round_num):
    common = []
    if pref_a.get("allow_rifle", True) and pref_b.get("allow_rifle", True):
        common.append("rifle")
    if pref_a.get("allow_pistol", True) and pref_b.get("allow_pistol", True):
        common.append("pistol")
    if pref_a.get("allow_awp", False) and pref_b.get("allow_awp", False):
        common.append("awp")

    if not common:
        return "rifle"
    if len(common) == 1:
        return common[0]

    prefer_a = pref_a.get("prefer", "rifle")
    prefer_b = pref_b.get("prefer", "rifle")

    # If both duelists explicitly opted into AWP (off by default), award AWP duel
    # unless both specifically locked preference to pistol.
    if "awp" in common and not (prefer_a == "pistol" and prefer_b == "pistol"):
        return "awp"

    if prefer_a == prefer_b and prefer_a in common:
        return prefer_a

    idx = (round_num + arena_num - 1) % len(common)
    return common[idx]


def arena_spawns(arena_num):
    base_x = float(arena_num - 1) * ARENA_SPACING
    return (
        (base_x - DUEL_HALF_DIST, 0.0, 64.0),
        (base_x + DUEL_HALF_DIST, 0.0, 64.0),
    )


def equip_duelist(player, pref, mode):
    player.strip_weapons(False)
    player.give_item("weapon_knife")
    player.set_health(100.0)
    player.set_defuser(False)

    pistol = pref.get("pistol", "deagle")
    if pistol not in ("usp", "glock18", "deagle"):
        pistol = "deagle"
    rifle = pref.get("rifle", "ak47")
    if rifle not in ("ak47", "m4a1"):
        rifle = "ak47"

    if mode == "pistol":
        player.set_armor(100, Armor.KEVLAR)
        player.give_item(f"weapon_{pistol}")
    elif mode == "awp":
        player.set_armor(100, Armor.VESTHELM)
        player.give_item("weapon_awp")
        player.give_item(f"weapon_{pistol}")
    else:
        player.set_armor(100, Armor.VESTHELM)
        player.give_item(f"weapon_{rifle}")
        player.give_item(f"weapon_{pistol}")


def sync_ladder_with_active():
    global ladder
    active_players = [p for p in mod.players() if p.team != Team.SPECTATOR]
    active_slots = [p.slot for p in active_players]
    active_set = set(active_slots)

    kept = [s for s in ladder if s in active_set]
    kept_set = set(kept)
    for s in active_slots:
        if s not in kept_set:
            kept.append(s)
    ladder = kept
    return {p.slot: p for p in active_players}


def start_arena_round():
    global active_arenas, slot_to_arena, waiting_slot, round_counter, round_promoted
    ensure_lang()
    round_counter += 1
    round_promoted = False

    by_slot = sync_ladder_with_active()
    active_arenas = []
    slot_to_arena = {}
    waiting_slot = None

    num_pairs = len(ladder) // 2
    for idx in range(num_pairs):
        arena_num = idx + 1
        slot_a = ladder[idx * 2]
        slot_b = ladder[idx * 2 + 1]
        pa = by_slot.get(slot_a)
        pb = by_slot.get(slot_b)
        if pa is None or pb is None:
            continue

        # Ensure stats & leaderboard entries exist for both duelists
        sa = get_stats(pa)
        sb = get_stats(pb)
        save_stats(pa, sa)
        save_stats(pb, sb)

        pref_a = get_pref(pa)
        pref_b = get_pref(pb)
        mode = resolve_duel_mode(pref_a, pref_b, arena_num, round_counter)

        pa.set_team(Team.TERRORIST)
        pb.set_team(Team.CT)

        spawn_a, spawn_b = arena_spawns(arena_num)
        pa.set_origin(spawn_a)
        pa.set_velocity((0.0, 0.0, 0.0))
        pb.set_origin(spawn_b)
        pb.set_velocity((0.0, 0.0, 0.0))

        equip_duelist(pa, pref_a, mode)
        equip_duelist(pb, pref_b, mode)

        arena_rec = {
            "arena": arena_num,
            "slot_a": slot_a,
            "slot_b": slot_b,
            "mode": mode,
            "winner": None,
            "loser": None,
        }
        active_arenas.append(arena_rec)
        slot_to_arena[slot_a] = arena_rec
        slot_to_arena[slot_b] = arena_rec

        rgb = (255, 196, 0) if arena_num == 1 else (110, 210, 255)
        for p in (pa, pb):
            tag = arena_tag(p, arena_num)
            m_lbl = mode_label(p, mode)
            p.chat(p.tr("M1V1_MATCH_START", tag=tag, p1=pa.name, p2=pb.name, mode=m_lbl))
            if arena_num == 1:
                banner = p.tr("M1V1_BANNER_CHAMP", p1=pa.name, p2=pb.name, mode=m_lbl)
            else:
                banner = p.tr("M1V1_BANNER_ARENA", arena=arena_num, p1=pa.name, p2=pb.name, mode=m_lbl)
            p.dhud(banner, x=-1.0, y=0.16, rgb=rgb, hold=3.5)

    if len(ladder) % 2 == 1:
        waiting_slot = ladder[-1]
        pw = by_slot.get(waiting_slot)
        if pw is not None:
            pw.chat(pw.tr("M1V1_WAIT_QUEUE"))


def promote_and_relegate_ladder():
    global ladder, round_promoted
    if round_promoted or not active_arenas:
        return
    round_promoted = True

    winners = []
    losers = []
    for arena in active_arenas:
        w = arena["winner"]
        l = arena["loser"]
        if w is None or l is None:
            # Unfinished arena: higher-seeded duelist holds spot
            w = arena["slot_a"]
            l = arena["slot_b"]
        winners.append(w)
        losers.append(l)

    m = len(active_arenas)
    new_ladder = []
    if m == 1:
        new_ladder.append(winners[0])
        if waiting_slot is not None:
            new_ladder.append(waiting_slot)
            new_ladder.append(losers[0])
        else:
            new_ladder.append(losers[0])
    else:
        # Arena 1: winner(1) defends vs winner(2) promoted UP
        new_ladder.append(winners[0])
        new_ladder.append(winners[1])
        # Middle arenas k (2..m-1): loser(k-1) dropped DOWN vs winner(k+1) promoted UP
        for k in range(1, m - 1):
            new_ladder.append(losers[k - 1])
            new_ladder.append(winners[k + 1])
        # Bottom arena m: loser(m-1) dropped DOWN vs waiting player (or loser(m))
        new_ladder.append(losers[m - 2])
        if waiting_slot is not None:
            new_ladder.append(waiting_slot)
            new_ladder.append(losers[m - 1])
        else:
            new_ladder.append(losers[m - 1])

    ladder = new_ladder

    # Announce promotions and relegations to participating duelists
    for idx, arena in enumerate(active_arenas):
        arena_num = arena["arena"]
        w_slot = winners[idx]
        l_slot = losers[idx]
        pw = mod.player(w_slot)
        pl = mod.player(l_slot)
        if pw is not None and arena_num > 1:
            pw.chat(pw.tr("M1V1_PROMOTED", arena=arena_num - 1))
        if pl is not None and arena_num < m:
            pl.chat(pl.tr("M1V1_RELEGATED", arena=arena_num + 1))


def format_enabled_modes(pref):
    enabled = []
    if pref.get("allow_rifle", True):
        enabled.append("Rifle")
    if pref.get("allow_pistol", True):
        enabled.append("Pistol")
    if pref.get("allow_awp", False):
        enabled.append("AWP")
    return "/".join(enabled) if enabled else "Rifle"


def open_guns_menu(player):
    ensure_lang()
    pref = get_pref(player)
    title = f"{Color.MENU_YELLOW}{player.tr('M1V1_GUNS_TITLE')}{Color.MENU_WHITE}"
    rifle_lbl = pref["rifle"].upper()
    pistol_lbl = pref["pistol"].upper()
    r_tag = f"{Color.MENU_YELLOW}[ON]{Color.MENU_WHITE}" if pref["allow_rifle"] else f"{Color.MENU_GREY}[OFF]{Color.MENU_WHITE}"
    p_tag = f"{Color.MENU_YELLOW}[ON]{Color.MENU_WHITE}" if pref["allow_pistol"] else f"{Color.MENU_GREY}[OFF]{Color.MENU_WHITE}"
    a_tag = f"{Color.MENU_YELLOW}[ON]{Color.MENU_WHITE}" if pref["allow_awp"] else f"{Color.MENU_GREY}[OFF]{Color.MENU_WHITE}"
    pref_lbl = pref["prefer"].upper()

    menu = Menu(title, callback=on_guns_select, per_page=7)
    menu.add_item(f"Rifle Choice (AK-47 / M4A1) {Color.MENU_RIGHT}{Color.MENU_YELLOW}[{rifle_lbl}]{Color.MENU_WHITE}", "toggle:rifle")
    menu.add_item(f"Pistol Choice (Deagle / USP / Glock) {Color.MENU_RIGHT}{Color.MENU_YELLOW}[{pistol_lbl}]{Color.MENU_WHITE}", "toggle:pistol")
    menu.add_item(f"Rifle Rounds {Color.MENU_RIGHT}{r_tag}", "toggle:allow_rifle")
    menu.add_item(f"Pistol Rounds {Color.MENU_RIGHT}{p_tag}", "toggle:allow_pistol")
    menu.add_item(f"AWP Rounds Opt-In {Color.MENU_RIGHT}{a_tag}", "toggle:allow_awp")
    menu.add_item(f"Preferred Round Type {Color.MENU_RIGHT}{Color.MENU_YELLOW}[{pref_lbl}]{Color.MENU_WHITE}", "toggle:prefer")
    menu.show(player, 0, 25)


def on_guns_select(player, item_index, info):
    ensure_lang()
    if item_index < 0 or not info:
        return
    pref = get_pref(player)
    if info == "toggle:rifle":
        pref["rifle"] = "m4a1" if pref["rifle"] == "ak47" else "ak47"
    elif info == "toggle:pistol":
        order = ["deagle", "usp", "glock18"]
        cur = pref.get("pistol", "deagle")
        idx = order.index(cur) if cur in order else 0
        pref["pistol"] = order[(idx + 1) % len(order)]
    elif info == "toggle:allow_rifle":
        pref["allow_rifle"] = not pref.get("allow_rifle", True)
    elif info == "toggle:allow_pistol":
        pref["allow_pistol"] = not pref.get("allow_pistol", True)
    elif info == "toggle:allow_awp":
        pref["allow_awp"] = not pref.get("allow_awp", False)
    elif info == "toggle:prefer":
        order = ["rifle", "pistol", "awp"]
        cur = pref.get("prefer", "rifle")
        idx = order.index(cur) if cur in order else 0
        nxt = order[(idx + 1) % len(order)]
        pref["prefer"] = nxt
        pref[f"allow_{nxt}"] = True
    else:
        return

    # Always keep at least one round category enabled.
    if not (pref["allow_rifle"] or pref["allow_pistol"] or pref["allow_awp"]):
        pref["allow_rifle"] = True

    save_pref(player, pref)
    player.chat(
        player.tr(
            "M1V1_PREF_SAVED",
            rifle=pref["rifle"].upper(),
            pistol=pref["pistol"].upper(),
            modes=format_enabled_modes(pref),
        )
    )
    open_guns_menu(player)


def show_top_leaderboard(player):
    ensure_lang()
    rows = mod.leaderboard.top(LEADERBOARD_TABLE, 0, 10)
    if not rows:
        player.chat(player.tr("M1V1_TOP_EMPTY"))
        return
    player.chat(player.tr("M1V1_TOP_HEADER"))
    for row in rows:
        name = row["key"]
        wins = 0
        losses = 0
        defenses = 0
        payload_raw = row.get("payload", "")
        if payload_raw:
            try:
                meta = json.loads(payload_raw)
                if isinstance(meta, dict):
                    name = meta.get("name", name) or name
                    wins = int(meta.get("wins", 0))
                    losses = int(meta.get("losses", 0))
                    defenses = int(meta.get("defenses", 0))
            except Exception:
                pass
        elo_int = int(row["score"] + 0.5)
        player.chat(
            player.tr(
                "M1V1_TOP_ROW",
                rank=row["rank"],
                name=name,
                elo=elo_int,
                wins=wins,
                losses=losses,
                defenses=defenses,
            )
        )


def show_player_rank(player):
    ensure_lang()
    st = get_stats(player)
    save_stats(player, st)
    ident = player_identity(player)
    info = mod.leaderboard.rank(LEADERBOARD_TABLE, ident)
    rank = info["rank"] if info is not None else 1
    total = info["total"] if info is not None else 1
    cur_arena = slot_to_arena[player.slot]["arena"] if player.slot in slot_to_arena else 1
    player.chat(
        player.tr(
            "M1V1_RANK_LINE",
            rank=rank,
            total=total,
            elo=st["elo"],
            wins=st["wins"],
            losses=st["losses"],
            defenses=st["defenses"],
            arena=cur_arena,
        )
    )


@mod.hook("client_put_in_server", phase="post")
def on_client_join(ev):
    ensure_lang()
    p = ev.player
    if p.slot not in ladder:
        ladder.append(p.slot)
    st = get_stats(p)
    save_stats(p, st)
    get_pref(p)


@mod.hook("client_disconnect", phase="post")
def on_client_leave(ev):
    global waiting_slot
    slot = ev.slot
    if slot in ladder:
        ladder.remove(slot)
    if waiting_slot == slot:
        waiting_slot = None
    slot_to_arena.pop(slot, None)


@mod.hook("round_start", phase="post")
def on_round_start(ev):
    start_arena_round()


@mod.hook("can_buy_item", phase="pre")
def on_can_buy(ev):
    return HookAction.SUPERCEDE


@mod.hook("player_take_damage", phase="pre")
def on_take_damage(ev):
    attacker = ev.attacker
    if attacker is None or attacker.slot == ev.victim.slot:
        return HookAction.CONTINUE
    a_arena = slot_to_arena.get(attacker.slot)
    v_arena = slot_to_arena.get(ev.victim.slot)
    if a_arena is None or v_arena is None or a_arena["arena"] != v_arena["arena"]:
        return HookAction.SUPERCEDE
    return HookAction.CONTINUE


@mod.hook("player_killed", phase="post")
def on_player_killed(ev):
    ensure_lang()
    victim_slot = ev.victim.slot
    arena = slot_to_arena.get(victim_slot)
    if arena is None or arena["winner"] is not None:
        return

    if victim_slot == arena["slot_a"]:
        winner_slot = arena["slot_b"]
    else:
        winner_slot = arena["slot_a"]

    arena["winner"] = winner_slot
    arena["loser"] = victim_slot

    pw = mod.player(winner_slot)
    pl = mod.player(victim_slot)
    if pw is not None and pl is not None:
        sw = get_stats(pw)
        sl = get_stats(pl)
        expected_w = 1.0 / (1.0 + (10.0 ** ((sl["elo"] - sw["elo"]) / 400.0)))
        delta = int(ELO_K * (1.0 - expected_w) + 0.5)
        if delta < 1:
            delta = 1
        sw["elo"] = sw["elo"] + delta
        sw["wins"] = sw["wins"] + 1
        sl["elo"] = max(100, sl["elo"] - delta)
        sl["losses"] = sl["losses"] + 1

        if arena["arena"] == 1:
            sw["defenses"] = sw["defenses"] + 1
            vfx.aura_light(pw.origin, radius=20, rgb=(255, 200, 0), life=12, decay=10)

        save_stats(pw, sw)
        save_stats(pl, sl)

        for p in (pw, pl):
            tag = arena_tag(p, arena["arena"])
            p.chat(
                p.tr(
                    "M1V1_DUEL_WIN",
                    tag=tag,
                    winner=pw.name,
                    loser=pl.name,
                    delta=delta,
                    elo=sw["elo"],
                )
            )
        if arena["arena"] == 1:
            for p in mod.players():
                p.chat(p.tr("M1V1_CHAMP_DEFEND", winner=pw.name, defenses=sw["defenses"]))

    if all(a["winner"] is not None for a in active_arenas):
        promote_and_relegate_ladder()


@mod.hook("round_end", phase="post")
def on_round_end(ev):
    promote_and_relegate_ladder()


@mod.chat_command("/guns", "!guns", "guns")
def cmd_guns_chat(player, args):
    open_guns_menu(player)
    return HookAction.SUPERCEDE


@mod.console_command("guns", "arena_guns")
def cmd_guns_console(player, args):
    if player is not None:
        open_guns_menu(player)
    return HookAction.HANDLED


@mod.chat_command("/top1v1", "!top1v1", "/toparena")
def cmd_top1v1_chat(player, args):
    show_top_leaderboard(player)
    return HookAction.SUPERCEDE


@mod.console_command("top1v1")
def cmd_top1v1_console(player, args):
    if player is not None:
        show_top_leaderboard(player)
    return HookAction.HANDLED


@mod.chat_command("!rank", "/rank1v1", "!rank1v1")
def cmd_rank_chat(player, args):
    show_player_rank(player)
    return HookAction.SUPERCEDE


@mod.console_command("rank1v1")
def cmd_rank_console(player, args):
    if player is not None:
        show_player_rank(player)
    return HookAction.HANDLED


@mod.console_command("arena_start")
def cmd_arena_start(player, args):
    start_arena_round()
    return HookAction.HANDLED
