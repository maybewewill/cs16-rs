# cs16-rs Community Plugin: Hit Damager + Smooth +use Parachute
# SPDX-License-Identifier: GPL-3.0-or-later

from cs16 import mod, Button, HookAction


@mod.info(
    id="damager_and_parachute",
    name="Damager & Parachute",
    version="1.0.0",
    author="cs16-rs",
    description="Toggleable crosshair hit-damage indicator (/damager) and smooth +use air parachute",
    permissions=["entities", "messages", "commands"],
)
def _meta():
    pass


damager_enabled = {}
parachute_active = set()


@mod.chat_command("/damager")
def cmd_damager(player, args):
    cur = damager_enabled.get(player.slot, True)
    damager_enabled[player.slot] = not cur
    key = "DAMAGER_ON" if not cur else "DAMAGER_OFF"
    player.chat(player.tr(key))
    return HookAction.SUPERCEDE


@mod.hook("player_take_damage", phase="post")
def on_damage_post(ev):
    att = ev.attacker
    if att is None or att.slot == ev.victim.slot or ev.team_attack:
        return
    if damager_enabled.get(att.slot, True):
        dmg_num = int(round(ev.damage))
        if dmg_num > 0:
            att.hud(
                f"{dmg_num}",
                x=-1.0,
                y=0.55,
                channel=2,
                rgba=(0, 180, 255, 255),
                fadein=0.02,
                hold=0.8,
                fadeout=0.15,
            )


@mod.hook("player_pre_think", phase="pre")
def on_pre_think(ev):
    p = ev.player
    if not p.alive or p.on_ground:
        if p.slot in parachute_active:
            parachute_active.discard(p.slot)
            p.set_gravity(1.0)
        return
    if (ev.buttons & Button.USE) != 0:
        vx, vy, vz = p.velocity
        if vz < -100.0:
            parachute_active.add(p.slot)
            p.set_velocity((vx, vy, -100.0))
            p.set_gravity(0.1)
    elif p.slot in parachute_active:
        parachute_active.discard(p.slot)
        p.set_gravity(1.0)
