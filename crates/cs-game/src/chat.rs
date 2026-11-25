//! Chat, names and text messages: `say` and `say_team` (`Host_Say` and `SendSayMessage`), a
//! player changing its name (`ClientUserInfoChanged`, `SetClientUserInfoName`), `ignoremsg`, and
//! the `ClientPrint` family that prints a localization key on the HUD, in the console or in the
//! chat. Ported from ReGameDLL_CS `client.cpp`, `player.cpp`, `util.cpp` and, for
//! `Q_StripUnprintableAndSpace`, ReHLDS `rehlds/engine/unicode_strtools.cpp` (GPL v2 or later
//! with a linking exception; not a ReGameDLL file), read 2026-11-22: the vanilla `mp.dll`
//! behaviour, not the `REGAMEDLL_ADD`/`REGAMEDLL_FIXES` builds (no `allchat`, no `chat_loc_fallback`,
//! no location of the speaker).
//!
//! ## Who hears what
//!
//! The speaker always gets its own line. Another player gets it unless it is a team message to
//! another team, or one player is alive and the other dead (the dead talk among themselves, and so do
//! the living; a spectator or a player who has not joined is "dead"), or the receiver has
//! muted global chat with `ignoremsg`. A speaker with `FL_PROXY` (an HLTV) is heard by all.
//! The voice manager's mute list (`m_VoiceGameMgr.PlayerHasBlockedPlayer`) belongs to the voice
//! task and is not consulted.
//!
//! ## Differences from the original, each deliberate
//!
//! - The console command line reaches the game as `argv`, so the text is `argv[1..]` joined by
//!   single spaces where the original reads the raw rest of the line (`CMD_ARGS`) with its quotes.
//!   The quote stripping of `Host_Say` is ported all the same. The reason is that
//!   `GameDll::client_command` does not carry the raw `CMD_ARGS` line, so unquoted text typed
//!   at the console with repeated spaces differs from the original (a carry: add the raw line
//!   to the boundary later).
//! - A Rust string is always valid UTF-8: `Q_UnicodeValidate` and `Q_UnicodeRepair` have nothing
//!   to repair. Cuts to a byte count land on a character boundary, where the original may cut
//!   a sequence in two.
//! - `iswspace` of the C runtime is `char::is_whitespace` (assumption).
//! - The echo to the server console is a `tracing` line; `UTIL_LogPrintf` (the `logmessages` log
//!   of chat and name changes) is not ported.
//! - `Host_Say`'s branch for a command other than `say`/`say_team` (raw text) is unreachable here
//!   (`Host_Say` is only ever called for those two) and is not ported.

use cs_engine::consts::FL_PROXY;
use cs_engine::{EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;

use crate::GameState;
use crate::player::{DEAD_NO, MAX_PLAYERS, Team};

/// `HUD_PRINTNOTIFY`: the notice line of the console and the chat.
pub const HUD_PRINTNOTIFY: u8 = 1;
/// `HUD_PRINTCONSOLE`: the console only.
pub const HUD_PRINTCONSOLE: u8 = 2;
/// `HUD_PRINTTALK`: the chat.
pub const HUD_PRINTTALK: u8 = 3;
/// `HUD_PRINTCENTER`: the middle of the screen.
pub const HUD_PRINTCENTER: u8 = 4;

/// The most parameters of a `TextMsg` (`ClientPrint`'s `param1` to `param4`).
const MAX_TEXT_PARAMS: usize = 4;
/// Seconds a player must wait between two chat lines (`m_flLastTalk`).
const TALK_INTERVAL: f64 = 0.66;
/// Seconds before the console command `say`/`say_team` is accepted again
/// (`m_flLastCommandTime[CMD_SAY]`, `CMD_SAYTEAM`).
const SAY_COMMAND_INTERVAL: f64 = 0.3;
/// `sizeof(szTemp)` of `Host_Say`: the text is cut to one byte less.
const SAY_BUFFER: usize = 256;
/// `sizeof(text)` of `SendSayMessage`: the line sent holds at most this much, with the format
/// key and the newline in it.
const SAY_LINE_BUDGET: usize = 128;
/// `sizeof(szName)` of `ClientUserInfoChanged`: a name is cut to one byte less.
const NAME_BUFFER: usize = 32;

/// `m_iIgnoreGlobalChat`: which chat the player has muted with `ignoremsg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IgnoreMsg {
    /// `IGNOREMSG_NONE`: hears everyone.
    #[default]
    None,
    /// `IGNOREMSG_ENEMY`: hears its own team only.
    Enemy,
    /// `IGNOREMSG_TEAM`: hears no global chat at all.
    Team,
}

/// What the game keeps of a player for chat and names.
#[derive(Debug, Clone, Default)]
pub struct ChatState {
    /// `m_flLastTalk`: server time of the last line (0 for none).
    last_talk: f64,
    /// `m_flLastCommandTime[CMD_SAY]`.
    next_say: f64,
    /// `m_flLastCommandTime[CMD_SAYTEAM]`.
    next_say_team: f64,
    /// `m_iIgnoreGlobalChat`.
    pub ignore: IgnoreMsg,
    /// `pev->netname`: the name the game last accepted for the player (empty before the first).
    pub(crate) netname: String,
    /// `m_bHasChangedName` and `m_szNewName`: a name the player asked for while dead, which it
    /// gets when it respawns.
    pending_name: Option<String>,
}

impl ChatState {
    /// A player that has just entered the server with `name`.
    pub(crate) fn entered(name: &str) -> Self {
        Self {
            netname: name.to_string(),
            ..Self::default()
        }
    }

    /// `Spawn` clears the flood timer (`m_flLastTalk = 0`) and every command timer
    /// (`m_flLastCommandTime[i] = -1`, player.cpp:6144), so a line right after a spawn is heard.
    pub(crate) fn on_spawn(&mut self) {
        self.last_talk = 0.0;
        self.next_say = -1.0;
        self.next_say_team = -1.0;
    }
}

/// `ClientPrint(client, dest, msg, params...)`: the localization key `msg` with up to four
/// parameters on the HUD or in the console of `slot` (`dest` one of the `HUD_PRINT*`). More
/// parameters than four are ignored, as the original has no more to give.
pub(crate) fn client_print(
    eng: &mut EngineApi<'_>,
    slot: u8,
    dest: u8,
    msg: &str,
    params: &[&str],
) {
    eng.send_usermsg(MsgDest::One(slot), &text_msg(dest, msg, params));
}

/// `UTIL_ClientPrintAll(dest, msg, params...)`: like [`client_print`] to everyone.
pub(crate) fn client_print_all(eng: &mut EngineApi<'_>, dest: u8, msg: &str, params: &[&str]) {
    eng.send_usermsg(MsgDest::All, &text_msg(dest, msg, params));
}

fn text_msg(dest: u8, msg: &str, params: &[&str]) -> CsUserMsg {
    CsUserMsg::TextMsg {
        dest,
        message: msg.to_string(),
        params: params
            .iter()
            .take(MAX_TEXT_PARAMS)
            .map(|p| (*p).to_string())
            .collect(),
    }
}

/// `Q_IsMeanSpaceW`: spaces and joiners that disturb a chat line when they stand at its end.
fn is_mean_space(c: char) -> bool {
    matches!(
        c as u32,
        0x0082
            | 0x0083
            | 0x00A0
            | 0x034F
            | 0x2000..=0x200D
            | 0x2028
            | 0x2029
            | 0x202F
            | 0x2060
            | 0x3000
            | 0xFEFF
            | 0xFFFC
    )
}

/// `Q_IsUnprintableW` (without `UNICODE_FIXES`): the bidirectional overrides.
fn is_unprintable(c: char) -> bool {
    matches!(c as u32, 0x202A..=0x202E | 0x206A..=0x206F)
}

/// `Q_StripUnprintableAndSpace`: drops the control characters (below 0x20), the bidirectional
/// overrides and the ellipsis character, then the white space and "mean" spaces at the end
/// and the white space at the start.
pub(crate) fn strip_unprintable_and_space(s: &str) -> String {
    let kept: Vec<char> = s
        .chars()
        .filter(|&c| c as u32 >= 0x20 && !is_unprintable(c) && c != '\u{2026}')
        .collect();
    let mut end = kept.len();
    while end > 0 && (kept[end - 1].is_whitespace() || is_mean_space(kept[end - 1])) {
        end -= 1;
    }
    let mut start = 0;
    while start < end && kept[start].is_whitespace() {
        start += 1;
    }
    kept[start..end].iter().collect()
}

/// `s` cut to at most `max` bytes, on a character boundary.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The format key of a chat line, as `Host_Say` picks it (no place name: vanilla only has one in
/// Condition Zero).
fn chat_format(team: Team, dead: bool, team_only: bool) -> &'static str {
    if team_only {
        match (team, dead) {
            (Team::Ct, true) => "#Cstrike_Chat_CT_Dead",
            (Team::Ct, false) => "#Cstrike_Chat_CT",
            (Team::Terrorist, true) => "#Cstrike_Chat_T_Dead",
            (Team::Terrorist, false) => "#Cstrike_Chat_T",
            _ => "#Cstrike_Chat_Spec",
        }
    } else if dead {
        if team == Team::Spectator {
            "#Cstrike_Chat_AllSpec"
        } else {
            "#Cstrike_Chat_AllDead"
        }
    } else {
        "#Cstrike_Chat_All"
    }
}

/// What the server console shows of a chat line (`pszConsoleFormat`), up to the name.
fn console_prefix(team: Team, dead: bool, team_only: bool) -> &'static str {
    if team_only {
        match (team, dead) {
            (Team::Ct, true) => "*DEAD*(Counter-Terrorist) ",
            (Team::Ct, false) => "(Counter-Terrorist) ",
            (Team::Terrorist, true) => "*DEAD*(Terrorist) ",
            (Team::Terrorist, false) => "(Terrorist) ",
            _ => "(Spectator) ",
        }
    } else if dead {
        if team == Team::Spectator {
            "*SPEC* "
        } else {
            "*DEAD* "
        }
    } else {
        ""
    }
}

/// What `SendSayMessage` knows of one end of a chat line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Party {
    pub team: Team,
    pub dead: bool,
}

/// `SendSayMessage`'s test of whether `receiver` gets a line from `sender`: a team line stays in
/// the team, the living and the dead do not hear each other (an HLTV is heard by both), and
/// `ignoremsg` mutes the other team (`Enemy`) or all (`Team`).
pub(crate) fn hears(
    sender: Party,
    sender_is_proxy: bool,
    team_only: bool,
    receiver: Party,
    ignore: IgnoreMsg,
) -> bool {
    if team_only && receiver.team != sender.team {
        return false;
    }
    if receiver.dead != sender.dead && !sender_is_proxy {
        return false;
    }
    match ignore {
        IgnoreMsg::None => true,
        IgnoreMsg::Enemy => receiver.team == sender.team,
        IgnoreMsg::Team => false,
    }
}

/// `deadflag != DEAD_NO` of the player of `slot`; a player with no edict counts as dead.
fn is_dead(eng: &EngineApi<'_>, slot: u8) -> bool {
    let id = eng.client_edict(slot);
    eng.ent(id).is_none_or(|v| v.deadflag != DEAD_NO)
}

/// The `say` and `say_team` console commands (`InternalCommand`): at most one every 0.3 s, then
/// [`host_say`].
pub(crate) fn say_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
    team_only: bool,
) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let next = if team_only {
        &mut p.chat.next_say_team
    } else {
        &mut p.chat.next_say
    };
    if now < *next {
        return;
    }
    *next = now + SAY_COMMAND_INTERVAL;
    host_say(g, eng, slot, argv, team_only);
}

/// `Host_Say`: the player of `slot` says the rest of `argv` to everyone, or to its team with
/// `team_only`. Sends nothing for a line spoken within 0.66 s of the last, an empty one, one with
/// nothing printable in it, or a slot with no player.
pub(crate) fn host_say(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
    team_only: bool,
) {
    let now = eng.time();
    let sender_dead = is_dead(eng, slot);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if p.chat.last_talk != 0.0 && now - p.chat.last_talk < TALK_INTERVAL {
        return;
    }
    p.chat.last_talk = now;
    let team = p.team;

    // `say` with a blank message: nothing to do.
    if argv.len() < 2 {
        return;
    }
    let joined = argv[1..].join(" ");
    let mut text = truncate_bytes(&joined, SAY_BUFFER - 1);
    // Remove the quotes (a leading one, and then the last character, whatever it is).
    if let Some(rest) = text.strip_prefix('"') {
        text = match rest.char_indices().next_back() {
            Some((i, _)) => &rest[..i],
            None => rest,
        };
    }
    if text.is_empty() {
        return;
    }
    let text = strip_unprintable_and_space(text);
    if text.is_empty() {
        return;
    }

    if crate::rules::handle_knife_round_chat(g, eng, slot, &text) {
        return;
    }

    let format = chat_format(team, sender_dead, team_only);
    let console = console_prefix(team, sender_dead, team_only);

    // A `%` that starts anything but `%l` (a localization key) becomes a space.
    let mut chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        if chars[i] == '%'
            && let Some(&next) = chars.get(i + 1)
            && next != 'l'
            && next != ' '
        {
            chars[i] = ' ';
        }
    }
    let text: String = chars.into_iter().collect();

    send_say_message(g, eng, slot, team_only, &text, format, console, sender_dead);
}

/// `SendSayMessage`: cuts the line to what fits the message, sends it to the speaker and to
/// everyone who hears it, and echoes it to the server console.
#[allow(clippy::too_many_arguments)]
fn send_say_message(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    team_only: bool,
    text: &str,
    format: &str,
    console: &str,
    sender_dead: bool,
) {
    // Room for the newline and the terminator, and for the format key.
    let room = SAY_LINE_BUDGET.saturating_sub(3 + format.len());
    let mut msg = truncate_bytes(text, room);
    if let Some(stripped) = msg.strip_suffix('\n') {
        msg = stripped;
    }
    let line = format!("{msg}\n");
    let sender_index = slot.wrapping_add(1);

    let Some(sender) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let sender_party = Party {
        team: sender.team,
        dead: sender_dead,
    };
    let sender_name = sender.chat.netname.clone();
    let proxy = eng
        .ent(eng.client_edict(slot))
        .is_some_and(|v| v.flags & FL_PROXY != 0);

    let say_text = |line: &str| CsUserMsg::SayText {
        sender_id: sender_index,
        text: format.to_string(),
        params: vec![String::new(), line.to_string()],
    };
    for (other, rcv) in g.players.iter().enumerate().take(MAX_PLAYERS) {
        let other_slot = other as u8;
        let Some(rcv) = rcv.as_ref() else {
            continue;
        };
        if other == usize::from(slot) {
            continue;
        }
        let receiver = Party {
            team: rcv.team,
            dead: is_dead(eng, other_slot),
        };
        if hears(sender_party, proxy, team_only, receiver, rcv.chat.ignore) {
            eng.send_usermsg(MsgDest::One(other_slot), &say_text(&line));
        }
    }
    // The speaker hears itself, whatever the rules.
    eng.send_usermsg(MsgDest::One(slot), &say_text(&line));

    tracing::info!("{console}{sender_name} : {line}", line = line.trim_end());
}

/// `ignoremsg`: cycles the player through hearing all, its own team only and nothing.
pub(crate) fn ignore_msg_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let (next, text) = match p.chat.ignore {
        IgnoreMsg::None => (IgnoreMsg::Enemy, "#Ignore_Broadcast_Messages"),
        IgnoreMsg::Enemy => (IgnoreMsg::Team, "#Ignore_Broadcast_Team_Messages"),
        IgnoreMsg::Team => (IgnoreMsg::None, "#Accept_All_Messages"),
    };
    p.chat.ignore = next;
    client_print(eng, slot, HUD_PRINTCENTER, text, &[]);
}

/// The first part of `ClientUserInfoChanged`: the player of `slot` may have asked for a new
/// name. A name other than the one the game last accepted (the engine has already stored it in
/// the userinfo, and has applied its own rate limit and uniqueness) loses its `%` and `&`
/// and cannot start with `#`; a living player takes it and everyone is told
/// (`#Cstrike_Name_Change`); a dead one is told it must wait (`#Name_change_at_respawn`), keeps
/// its old name in the userinfo and gets the new one when it spawns (see [`apply_pending_name`]).
pub(crate) fn name_changed(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(new_name) = eng.client_key(slot, "name").map(str::to_string) else {
        return;
    };
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if p.chat.netname.is_empty() {
        // The first name, set when the player came in: nothing to announce.
        p.chat.netname = new_name;
        return;
    }
    if p.chat.netname == new_name {
        return;
    }
    let mut name: String = truncate_bytes(&new_name, NAME_BUFFER - 1)
        .chars()
        .map(|c| if c == '%' || c == '&' { ' ' } else { c })
        .collect();
    if name.starts_with('#') {
        name.replace_range(0..1, "*");
    }
    let old = p.chat.netname.clone();
    if is_dead(eng, slot) {
        // `SetClientUserInfoName`: the name waits for the respawn.
        if let Some(p) = g
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.chat.pending_name = Some(name);
        }
        eng.set_client_key(slot, "name", &old);
        client_print(eng, slot, HUD_PRINTTALK, "#Name_change_at_respawn", &[]);
        return;
    }
    p.chat.netname = name.clone();
    eng.set_client_key(slot, "name", &name);
    eng.send_usermsg(
        MsgDest::Broadcast,
        &CsUserMsg::SayText {
            sender_id: slot.wrapping_add(1),
            text: "#Cstrike_Name_Change".to_string(),
            params: vec![old, name],
        },
    );
}

/// What `Spawn` does with a name asked for while dead: the player gets it, with no
/// announcement.
pub(crate) fn apply_pending_name(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let Some(name) = p.chat.pending_name.take() else {
        return;
    };
    if eng.client_key(slot, "name") != Some(name.as_str()) {
        eng.set_client_key(slot, "name", &name);
    }
    p.chat.netname = name;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_controls_and_the_ends() {
        assert_eq!(strip_unprintable_and_space("  hi\tthere\n "), "hithere");
        assert_eq!(strip_unprintable_and_space("a  b"), "a  b");
        assert_eq!(strip_unprintable_and_space("\x01\x02\x7f"), "\x7f");
        assert_eq!(strip_unprintable_and_space("hi\u{202e}x\u{2026}"), "hix");
        assert_eq!(strip_unprintable_and_space("hi\u{200b}"), "hi");
        assert_eq!(strip_unprintable_and_space(" \u{a0}x"), "x");
        assert_eq!(strip_unprintable_and_space(" \t "), "");
    }

    #[test]
    fn truncation_keeps_characters_whole() {
        assert_eq!(truncate_bytes("abc", 5), "abc");
        assert_eq!(truncate_bytes("abc", 2), "ab");
        assert_eq!(truncate_bytes("aé", 2), "a");
        assert_eq!(truncate_bytes("é", 0), "");
    }

    #[test]
    fn format_key_follows_host_say() {
        use Team::*;
        assert_eq!(chat_format(Ct, false, false), "#Cstrike_Chat_All");
        assert_eq!(chat_format(Ct, true, false), "#Cstrike_Chat_AllDead");
        assert_eq!(chat_format(Spectator, true, false), "#Cstrike_Chat_AllSpec");
        assert_eq!(chat_format(Unassigned, true, false), "#Cstrike_Chat_AllDead");
        assert_eq!(chat_format(Ct, false, true), "#Cstrike_Chat_CT");
        assert_eq!(chat_format(Ct, true, true), "#Cstrike_Chat_CT_Dead");
        assert_eq!(chat_format(Terrorist, false, true), "#Cstrike_Chat_T");
        assert_eq!(chat_format(Terrorist, true, true), "#Cstrike_Chat_T_Dead");
        assert_eq!(chat_format(Spectator, false, true), "#Cstrike_Chat_Spec");
        assert_eq!(chat_format(Unassigned, true, true), "#Cstrike_Chat_Spec");
    }

    fn party(team: Team, dead: bool) -> Party {
        Party { team, dead }
    }

    #[test]
    fn hearing_rules() {
        let alive_ct = party(Team::Ct, false);
        let dead_ct = party(Team::Ct, true);
        let alive_t = party(Team::Terrorist, false);
        let none = IgnoreMsg::None;
        // Everyone alive hears everyone alive; a team line stays in the team.
        assert!(hears(alive_ct, false, false, alive_t, none));
        assert!(!hears(alive_ct, false, true, alive_t, none));
        assert!(hears(alive_ct, false, true, alive_ct, none));
        // The living and the dead do not hear each other, except an HLTV.
        assert!(!hears(alive_ct, false, false, dead_ct, none));
        assert!(!hears(dead_ct, false, false, alive_ct, none));
        assert!(hears(dead_ct, true, false, alive_ct, none));
        assert!(hears(dead_ct, false, true, dead_ct, none));
        // `ignoremsg`.
        assert!(!hears(alive_ct, false, false, alive_t, IgnoreMsg::Enemy));
        assert!(hears(alive_ct, false, false, alive_ct, IgnoreMsg::Enemy));
        assert!(!hears(alive_ct, false, false, alive_ct, IgnoreMsg::Team));
    }

    #[test]
    fn text_msg_takes_four_parameters() {
        match text_msg(HUD_PRINTTALK, "#k", &["1", "2", "3", "4", "5"]) {
            CsUserMsg::TextMsg {
                dest,
                message,
                params,
            } => {
                assert_eq!(dest, 3);
                assert_eq!(message, "#k");
                assert_eq!(params, ["1", "2", "3", "4"]);
            }
            other => panic!("not a TextMsg: {other:?}"),
        }
    }
}
