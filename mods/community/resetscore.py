# cs16-rs Community Plugin: ResetScore (`/rs`, `/resetscore`, `!rs`)
# SPDX-License-Identifier: GPL-3.0-or-later

from cs16 import mod, Color, HookAction


@mod.info(
    id="resetscore",
    name="ResetScore",
    version="1.0.0",
    author="cs16-rs",
    description="Zeroes frags and deaths via /rs, /resetscore, or !rs with per-player EN/RU chat confirmation",
    permissions=["entities", "messages", "commands"],
)
def _meta():
    pass


@mod.chat_command("/rs", "/resetscore", "!rs")
def cmd_resetscore(player, args):
    player.reset_score()
    msg = player.tr("RS_DONE", name=player.name)
    player.chat(f"{Color.GREEN}[RS]{Color.DEFAULT} {msg}")
    return HookAction.SUPERCEDE
