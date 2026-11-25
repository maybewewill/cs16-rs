# cs16-rs Community Plugin: AntiFlood, Rotating Localized AdManager & Async /report Webhook
# SPDX-License-Identifier: GPL-3.0-or-later

import json
from cs16 import mod, HookAction


@mod.info(
    id="antiflood_admanager",
    name="AntiFlood & AdManager",
    version="1.0.0",
    author="cs16-rs",
    description="0.75s chat flood guard, rotating localized EN/RU server tips, and non-blocking /report webhook",
    permissions=["entities", "messages", "timers", "commands", "cvars", "http:discord.com"],
)
def _meta():
    pass


last_chat_time = {}
ad_keys = ["AD_1", "AD_2", "AD_3"]
ad_cursor = 0


@mod.hook("player_say", phase="pre", priority=-5)
def on_say(ev):
    msg = ev.message.strip()
    if msg.startswith("/report"):
        return HookAction.CONTINUE
    now = mod.time
    prev = last_chat_time.get(ev.slot, -999.0)
    if now - prev < 0.75:
        ev.player.chat(ev.player.tr("FLOOD_WARN"))
        return HookAction.SUPERCEDE
    last_chat_time[ev.slot] = now
    return HookAction.CONTINUE


@mod.timer(60.0, repeat=True)
def broadcast_ad():
    global ad_cursor
    key = ad_keys[ad_cursor % len(ad_keys)]
    ad_cursor += 1
    for p in mod.players():
        p.chat(p.tr(key))


@mod.chat_command("/report")
def cmd_report(player, args):
    trimmed = args.strip()
    if not trimmed:
        player.chat(player.tr("REPORT_USAGE"))
        return HookAction.SUPERCEDE
    parts = trimmed.split(" ", 1)
    target = parts[0]
    reason = parts[1].strip() if len(parts) > 1 else "unspecified"
    payload = json.dumps({
        "content": f"[cs16-rs Report] {player.name} reported {target}: {reason} (map={mod.map_name})",
        "reporter": player.name,
        "target": target,
        "reason": reason,
        "map": mod.map_name,
    })
    mod.http_post_json("https://discord.com/api/webhooks/cs16-rs/reports", payload)
    player.chat(player.tr("REPORT_SENT", target=target))
    return HookAction.SUPERCEDE
