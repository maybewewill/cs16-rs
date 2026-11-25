# cs16-rs Community Plugin: VIP System (`/vip`, `/vipmenu`)
# SPDX-License-Identifier: GPL-3.0-or-later

from cs16 import mod, vfx, Color, HookAction, Armor, Team, Menu


@mod.info(
    id="vip_system",
    name="VIP System",
    version="1.0.0",
    author="cs16-rs",
    description="Round 2+ VIP weapon menu, spawn armor/nades, kill/HS vampire bonus, [VIP] chat tag & RPC",
    permissions=["entities", "messages", "menus", "vfx", "weapons", "admins", "rpc", "storage", "commands"],
)
def _meta():
    pass


round_num = 1
used_this_round = set()

VIP_MOTD_HTML = (
    "<html><body style='background:#000000;color:#FFB000;font-family:Tahoma,Arial,sans-serif;font-size:12px;margin:10px;'>"
    "<div style='border:1px solid #FFB000;background:#0D0D0D;padding:8px;'>"
    "<h2 style='margin:0 0 8px 0;color:#FFB000;border-bottom:1px solid #554400;padding-bottom:4px;'>VIP PRIVILEGES (FLAG 't')</h2>"
    "<table style='width:100%;border-collapse:collapse;color:#E8D8A0;'>"
    "<tr><td style='border:1px solid #3A3218;padding:4px;'><b>Spawn Kit</b></td><td style='border:1px solid #3A3218;padding:4px;'>100 Kevlar + Helmet, HE/Flash/Smoke, Defuse Kit (CT)</td></tr>"
    "<tr><td style='border:1px solid #3A3218;padding:4px;'><b>Kill Bonus</b></td><td style='border:1px solid #3A3218;padding:4px;'>+$300 / +10 HP (+$500 / +20 HP on Headshot) + Golden Aura</td></tr>"
    "<tr><td style='border:1px solid #3A3218;padding:4px;'><b>VIP Arsenal</b></td><td style='border:1px solid #3A3218;padding:4px;'>Type /vipmenu from Round 2 for AK-47 / M4A1 / AWP + Deagle</td></tr>"
    "<tr><td style='border:1px solid #3A3218;padding:4px;'><b>Chat &amp; Slot</b></td><td style='border:1px solid #3A3218;padding:4px;'>Green [VIP] chat prefix &amp; reserved slot</td></tr>"
    "</table></div></body></html>"
)


@mod.export_fn("vip.is_vip")
def rpc_is_vip(slot):
    p = mod.player(int(slot))
    if p is None:
        return 0
    return 1 if p.has_flag("t") else 0


@mod.export_fn("vip.get_points")
def rpc_get_points(slot):
    p = mod.player(int(slot))
    if p is None:
        return 0
    key = f"pts_{p.slot}"
    return int(mod.storage.get(key, "0"))


@mod.hook("round_start", phase="post")
def on_round_start(ev):
    global round_num
    round_num = ev.round_num
    used_this_round.clear()


@mod.hook("player_spawn", phase="post")
def on_spawn(ev):
    p = ev.player
    if not p.has_flag("t"):
        return
    p.set_armor(100, Armor.VESTHELM)
    p.give_item("weapon_hegrenade")
    p.give_item("weapon_flashbang")
    p.give_item("weapon_smokegrenade")
    if p.team == Team.CT:
        p.set_defuser(True)


@mod.hook("player_killed", phase="post")
def on_kill(ev):
    k = ev.killer
    if k is None or not k.has_flag("t") or ev.team_kill:
        return
    bonus_money = 500 if ev.headshot else 300
    bonus_hp = 20 if ev.headshot else 10
    k.add_money(bonus_money, flash=True)
    k.set_health(min(k.health + bonus_hp, k.max_health))
    pts_key = f"pts_{k.slot}"
    pts = int(mod.storage.get(pts_key, "0")) + (2 if ev.headshot else 1)
    mod.storage.set(pts_key, str(pts))
    vfx.aura_light(k.origin, radius=18, rgb=(255, 215, 0), life=8, decay=15)
    k.screen_fade(duration=0.25, hold=0.05, flags=0, rgba=(0, 80, 220, 45))


def on_vip_pick(player, info):
    if not player.alive or player.slot in used_this_round:
        return
    used_this_round.add(player.slot)
    player.give_item(info)
    player.give_item("weapon_deagle")
    player.give_item("weapon_hegrenade")
    player.give_item("weapon_flashbang")
    player.set_armor(100, Armor.VESTHELM)
    if player.team == Team.CT:
        player.set_defuser(True)
    label = info.replace("weapon_", "").upper()
    player.chat(f"{Color.GREEN}[VIP]{Color.DEFAULT} {player.tr('VIP_BOUGHT', weapon=label)}")
    mod.emit_event("vip.purchased", {"slot": player.slot, "weapon": info})


@mod.chat_command("/vipmenu")
def cmd_vipmenu(player, args):
    if not player.has_flag("t"):
        player.chat(player.tr("VIP_NOT_ALLOWED"))
        return HookAction.SUPERCEDE
    if round_num < 2:
        player.chat(player.tr("VIP_ROUND_LOCK"))
        return HookAction.SUPERCEDE
    if player.slot in used_this_round:
        player.chat(player.tr("VIP_ALREADY_USED"))
        return HookAction.SUPERCEDE
    menu = Menu(player.tr("VIP_MENU_TITLE"), callback=on_vip_pick)
    menu.add_item("AK-47 + Deagle + Full Nades", "weapon_ak47")
    menu.add_item("M4A1 + Deagle + Full Nades", "weapon_m4a1")
    menu.add_item("AWP + Deagle + Full Nades", "weapon_awp")
    menu.show(player)
    return HookAction.SUPERCEDE


@mod.chat_command("/vip")
def cmd_vip_info(player, args):
    player.show_motd(player.tr("VIP_MOTD_TITLE"), VIP_MOTD_HTML)
    return HookAction.SUPERCEDE


@mod.hook("player_say", phase="pre", priority=5)
def on_vip_chat(ev):
    msg = ev.message.strip()
    if ev.player.has_flag("t") and not msg.startswith("/") and not msg.startswith("!"):
        ev.message = f"{Color.GREEN}[VIP] {Color.TEAM}{ev.player.name}{Color.DEFAULT}: {msg}"
        return HookAction.OVERRIDE
    return HookAction.CONTINUE
