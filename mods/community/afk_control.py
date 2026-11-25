# cs16-rs Community Plugin: AFK Control (15s C4 Transfer, 45s Spec Move, Full-Server Spec Kick)
# SPDX-License-Identifier: GPL-3.0-or-later

import math
from cs16 import mod, Team, WeaponId


@mod.info(
    id="afk_control",
    name="AFK Control",
    version="1.0.0",
    author="cs16-rs",
    description="Transfers C4 after 15s idle, moves 45s AFK players to Spectator, kicks idle specs when full",
    permissions=["entities", "messages", "timers", "weapons", "admins"],
)
def _meta():
    pass


last_pos = {}
afk_secs = {}
spec_idle_secs = {}


@mod.hook("client_disconnect", phase="post")
def on_disconnect(ev):
    last_pos.pop(ev.slot, None)
    afk_secs.pop(ev.slot, None)
    spec_idle_secs.pop(ev.slot, None)


@mod.timer(1.0, repeat=True)
def check_afk():
    players = mod.players()
    active_count = len(players)
    server_full = active_count >= max(mod.max_clients - 1, 1)

    for p in players:
        if p.team == Team.SPECTATOR:
            afk_secs[p.slot] = 0.0
            spec_idle_secs[p.slot] = spec_idle_secs.get(p.slot, 0.0) + 1.0
            if server_full and spec_idle_secs[p.slot] >= 30.0 and not p.has_flag("a"):
                p.kick(p.tr("AFK_KICK_REASON"))
            continue

        spec_idle_secs[p.slot] = 0.0
        if not p.alive:
            afk_secs[p.slot] = 0.0
            continue

        pos = p.origin
        prev = last_pos.get(p.slot)
        if prev is not None and math.dist(pos, prev) < 4.0:
            afk_secs[p.slot] = afk_secs.get(p.slot, 0.0) + 1.0
        else:
            afk_secs[p.slot] = 0.0
            last_pos[p.slot] = pos

        idle = afk_secs[p.slot]
        if idle >= 15.0 and p.team == Team.TERRORIST and p.has_weapon(WeaponId.C4):
            mates = [
                m
                for m in players
                if m.slot != p.slot
                and m.alive
                and m.team == Team.TERRORIST
                and afk_secs.get(m.slot, 0.0) < 5.0
            ]
            if mates:
                nearest = min(mates, key=lambda m: math.dist(m.origin, pos))
                p.drop_slot(5)
                nearest.give_item("weapon_c4")
                for pl in players:
                    pl.chat(pl.tr("AFK_C4_TRANSFER", **{"from": p.name, "to": nearest.name}))

        if idle >= 45.0 and not p.has_flag("a"):
            afk_secs[p.slot] = 0.0
            p.slay()
            p.set_team(Team.SPECTATOR)
            for pl in players:
                pl.chat(pl.tr("AFK_MOVED_SPEC", name=p.name))
