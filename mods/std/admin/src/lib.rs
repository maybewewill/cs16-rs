//! Standard Admin Suite (`std_admin`) — AMXX+ Rust -> WASM plugin.
//!
//! Covers `admin`, `admincmd`, `adminchat` (`@`/`@@`/`@@@` colored HUD announcements),
//! `adminslots` (reserved VIP/admin slot), `amxmodmenu` (`plmenu` Kick/Ban/Slap/Slay/Team,
//! `mapsmenu` Changelevel/Votemap, `cvarmenu`, `cmdmenu`), `amx_who`, and `amx_help` using
//! authentic CS 1.6 GoldSrc `\y`/`\w`/`\r`/`\d` menus and per-player `LangCatalog` (`en`/`ru`).

use cs_mod_sdk::{
    HookAction, HookId, HookPhase, HostBridge, HudMessageSpec, MENU_EXIT, PaginatedMenu, Player,
    TeamKind, cs_mod_init,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");

const CB_MAIN_MENU: i32 = 1;
const CB_KICK_MENU: i32 = 2;
const CB_BAN_MENU: i32 = 3;
const CB_SLAP_MENU: i32 = 4;
const CB_TEAM_MENU: i32 = 5;
const CB_MAP_MENU: i32 = 6;
const CB_CVAR_MENU: i32 = 7;

const STOCK_MAPS: [&str; 6] = [
    "de_dust2",
    "de_inferno",
    "de_nuke",
    "de_train",
    "de_mirage",
    "cs_office",
];

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);
    host.hook_register(HookId::ClientConnect, HookPhase::Pre, 0);
    host.hook_register(HookId::ClientCommand, HookPhase::Pre, 0);
    host.hook_register(HookId::PlayerSay, HookPhase::Pre, -2);
    for cmd in [
        "amxmodmenu",
        "csmod_admin",
        "amx_kick",
        "amx_ban",
        "amx_slap",
        "amx_slay",
        "amx_map",
        "amx_cvar",
        "amx_who",
        "amx_help",
    ] {
        host.register_command(cmd);
    }
    0
}

fn broadcast_tr(host: &mut impl HostBridge, key: &str, args: &[(&str, &str)]) {
    let max = host.player_max_clients();
    for slot in 0..max {
        if host.player_is_active(slot) {
            Player::new(slot).say_tr(host, key, args);
        }
    }
}

fn find_target_slot(host: &impl HostBridge, query: &str) -> Option<u8> {
    let q = query.trim().trim_matches('"');
    if q.is_empty() {
        return None;
    }
    if let Some(rest) = q.strip_prefix('#')
        && let Ok(uid) = rest.parse::<u8>()
        && uid > 0
    {
        let slot = uid - 1;
        if host.player_is_active(slot) {
            return Some(slot);
        }
    }
    let q_low = q.to_ascii_lowercase();
    for slot in 0..host.player_max_clients() {
        if host.player_is_active(slot)
            && let Some(name) = host.player_name(slot)
            && name.to_ascii_lowercase().contains(&q_low)
        {
            return Some(slot);
        }
    }
    None
}

fn open_main_menu(host: &mut impl HostBridge, admin: Player) {
    let title = format!("\\y{}\\w", admin.tr(host, "ADMIN_MENU_TITLE", &[]));
    let m_kick = admin.tr(host, "ADMIN_MENU_KICK", &[]);
    let m_ban = admin.tr(host, "ADMIN_MENU_BAN", &[]);
    let m_slap = admin.tr(host, "ADMIN_MENU_SLAP", &[]);
    let m_team = admin.tr(host, "ADMIN_MENU_TEAM", &[]);
    let m_map = admin.tr(host, "ADMIN_MENU_MAP", &[]);
    let m_vote = admin.tr(host, "ADMIN_MENU_VOTEMAP", &[]);
    let m_cvar = admin.tr(host, "ADMIN_MENU_CVAR", &[]);
    let m_restart = admin.tr(host, "ADMIN_MENU_RESTART", &[]);

    PaginatedMenu::new(title, CB_MAIN_MENU)
        .item(m_kick, "kick")
        .item(m_ban, "ban")
        .item(m_slap, "slap")
        .item(m_team, "team")
        .item(m_map, "map")
        .item(m_vote, "votemap")
        .item(m_cvar, "cvar")
        .item(m_restart, "restart")
        .per_page(7)
        .display_to(host, admin.slot(), 0, 20);
}

fn open_player_picker(host: &mut impl HostBridge, admin: Player, title: &str, cb_id: i32) {
    let mut menu = PaginatedMenu::new(format!("\\y{title}\\w"), cb_id).per_page(7);
    for slot in 0..host.player_max_clients() {
        if host.player_is_active(slot) {
            let p = Player::new(slot);
            let name = p.name(host);
            let tag = match p.team(host) {
                TeamKind::Terrorist => "\\r[T]\\w",
                TeamKind::Ct => "\\w[CT]",
                TeamKind::Spectator => "\\d[SPEC]\\w",
                TeamKind::Unassigned => "\\d[UN]\\w",
            };
            let label = format!("{name} \\R{tag}");
            menu = menu.item(label, slot.to_string());
        }
    }
    menu.display_to(host, admin.slot(), 0, 20);
}

fn open_map_menu(host: &mut impl HostBridge, admin: Player) {
    let title = format!("\\y{}\\w", admin.tr(host, "ADMIN_MENU_MAP", &[]));
    let mut menu = PaginatedMenu::new(title, CB_MAP_MENU).per_page(7);
    for map in STOCK_MAPS {
        menu = menu.item(map, map);
    }
    menu.display_to(host, admin.slot(), 0, 20);
}

fn open_cvar_menu(host: &mut impl HostBridge, admin: Player) {
    let title = format!("\\y{}\\w", admin.tr(host, "ADMIN_MENU_CVAR", &[]));
    let ff = host.cvar_get_f32("mp_friendlyfire");
    let grav = host.cvar_get_f32("sv_gravity");
    let ft = host.cvar_get_f32("mp_freezetime");
    PaginatedMenu::new(title, CB_CVAR_MENU)
        .item(format!("mp_friendlyfire \\R\\y{:.0}\\w", ff), "ff")
        .item(format!("sv_gravity \\R\\y{:.0}\\w", grav), "gravity")
        .item(format!("mp_freezetime \\R\\y{:.0}\\w", ft), "freezetime")
        .item("sv_restart 1 \\R\\rNOW\\w", "restart")
        .display_to(host, admin.slot(), 0, 20);
}

fn handle_admin_hud_chat(host: &mut impl HostBridge, admin: Player, msg: &str) -> HookAction {
    if !admin.has_flag(host, 'i') {
        admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
        return HookAction::Supercede;
    }
    let (y, rest) = if let Some(s) = msg.strip_prefix("@@@") {
        (0.75, s.trim())
    } else if let Some(s) = msg.strip_prefix("@@") {
        (0.28, s.trim())
    } else if let Some(s) = msg.strip_prefix('@') {
        (0.55, s.trim())
    } else {
        return HookAction::Continue;
    };
    if rest.is_empty() {
        return HookAction::Supercede;
    }
    let (rgba, body) = match rest.as_bytes().first().copied() {
        Some(b'r' | b'R') if rest.as_bytes().get(1) == Some(&b' ') => ([255, 64, 64, 255], rest[2..].trim()),
        Some(b'g' | b'G') if rest.as_bytes().get(1) == Some(&b' ') => ([116, 212, 116, 255], rest[2..].trim()),
        Some(b'b' | b'B') if rest.as_bytes().get(1) == Some(&b' ') => ([153, 204, 255, 255], rest[2..].trim()),
        Some(b'y' | b'Y') if rest.as_bytes().get(1) == Some(&b' ') => ([255, 176, 0, 255], rest[2..].trim()),
        _ => ([255, 255, 255, 255], rest),
    };
    let admin_name = admin.name(host);
    let formatted = format!("{admin_name}: {body}");
    host.hud_message(&HudMessageSpec {
        slot: None,
        channel: 1,
        x: -1.0,
        y,
        effect: 0,
        rgba1: rgba,
        rgba2: [255, 160, 0, 255],
        fadein: 0.1,
        hold: 4.5,
        fadeout: 0.4,
        fxtime: 0.0,
        text: formatted,
    });
    broadcast_tr(host, "ADMIN_CHAT_PREFIX", &[("admin", &admin_name), ("msg", body)]);
    HookAction::Supercede
}

fn handle_client_command(host: &mut impl HostBridge) -> HookAction {
    let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
    let admin = Player::new(slot);
    let cmd = host.hook_arg_str(0).unwrap_or_default().to_ascii_lowercase();
    let full = host.hook_arg_str(2).unwrap_or_default();
    let mut parts = full.split_whitespace();
    let _ = parts.next();

    match cmd.as_str() {
        "amxmodmenu" | "csmod_admin" => {
            if !admin.has_flag(host, 'u') {
                admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                return HookAction::Handled;
            }
            open_main_menu(host, admin);
            HookAction::Handled
        }
        "amx_kick" => {
            if !admin.has_flag(host, 'c') {
                admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                return HookAction::Handled;
            }
            if let Some(q) = parts.next()
                && let Some(tslot) = find_target_slot(host, q)
            {
                let target = Player::new(tslot);
                if target.has_flag(host, 'a') && tslot != slot {
                    admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                    return HookAction::Handled;
                }
                let reason: String = parts.collect::<Vec<_>>().join(" ");
                let reason_ref = if reason.is_empty() { "Kicked by admin" } else { reason.as_str() };
                let tname = target.name(host);
                let aname = admin.name(host);
                target.kick(host, reason_ref);
                broadcast_tr(host, "ADMIN_KICKED", &[("admin", &aname), ("target", &tname), ("reason", reason_ref)]);
            }
            HookAction::Handled
        }
        "amx_ban" => {
            if !admin.has_flag(host, 'd') {
                admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                return HookAction::Handled;
            }
            if let Some(q) = parts.next()
                && let Some(tslot) = find_target_slot(host, q)
            {
                let target = Player::new(tslot);
                let reason = parts.collect::<Vec<_>>().join(" ");
                let reason_ref = if reason.is_empty() { "Banned by admin" } else { reason.as_str() };
                let tname = target.name(host);
                let aname = admin.name(host);
                let sid = target.steamid(host);
                if !sid.is_empty() {
                    host.server_exec(&format!("banid 0 {sid} kick"));
                } else {
                    target.kick(host, reason_ref);
                }
                broadcast_tr(host, "ADMIN_BANNED", &[("admin", &aname), ("target", &tname), ("reason", reason_ref)]);
            }
            HookAction::Handled
        }
        "amx_slap" => {
            if !admin.has_flag(host, 'e') {
                admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                return HookAction::Handled;
            }
            if let Some(q) = parts.next()
                && let Some(tslot) = find_target_slot(host, q)
            {
                let dmg = parts.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(5).clamp(0, 99);
                let target = Player::new(tslot);
                let tname = target.name(host);
                let aname = admin.name(host);
                target.slap(host, dmg, true);
                let dmg_s = dmg.to_string();
                broadcast_tr(host, "ADMIN_SLAPPED", &[("admin", &aname), ("target", &tname), ("dmg", &dmg_s)]);
            }
            HookAction::Handled
        }
        "amx_slay" => {
            if !admin.has_flag(host, 'e') {
                admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                return HookAction::Handled;
            }
            if let Some(q) = parts.next()
                && let Some(tslot) = find_target_slot(host, q)
            {
                let target = Player::new(tslot);
                let tname = target.name(host);
                let aname = admin.name(host);
                target.slay(host, false);
                broadcast_tr(host, "ADMIN_SLAYED", &[("admin", &aname), ("target", &tname)]);
            }
            HookAction::Handled
        }
        "amx_map" => {
            if !admin.has_flag(host, 'f') {
                admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                return HookAction::Handled;
            }
            if let Some(map) = parts.next() {
                let aname = admin.name(host);
                broadcast_tr(host, "ADMIN_MAP_CHANGED", &[("admin", &aname), ("map", map)]);
                host.change_level(map);
            }
            HookAction::Handled
        }
        "amx_cvar" => {
            if !admin.has_flag(host, 'g') {
                admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                return HookAction::Handled;
            }
            if let (Some(cvar), Some(val)) = (parts.next(), parts.next()) {
                host.cvar_set_str(cvar, val);
                let aname = admin.name(host);
                broadcast_tr(host, "ADMIN_CVAR_SET", &[("admin", &aname), ("cvar", cvar), ("value", val)]);
            }
            HookAction::Handled
        }
        "amx_who" => {
            let hdr = admin.tr(host, "ADMIN_WHO_HEADER", &[]);
            admin.print(host, &format!("{hdr}\n"));
            for s in 0..host.player_max_clients() {
                if host.player_is_active(s) {
                    let p = Player::new(s);
                    let flags = p.admin_flags(host);
                    admin.print(
                        host,
                        &format!(
                            "{:<2} {:<20} {:<20} 0x{:08x}\n",
                            s + 1,
                            p.name(host),
                            p.steamid(host),
                            flags.bits()
                        ),
                    );
                }
            }
            HookAction::Handled
        }
        "amx_help" => {
            let help = admin.tr(host, "ADMIN_HELP_LINE", &[]);
            admin.print(host, &format!("{help}\n"));
            HookAction::Handled
        }
        _ => HookAction::Continue,
    }
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::ClientConnect, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let max = host.player_max_clients();
            if max >= 4 {
                let mut active = 0u8;
                for s in 0..max {
                    if host.player_is_active(s) {
                        active += 1;
                    }
                }
                if active >= max - 1 && !Player::new(slot).has_flag(host, 'b') {
                    return HookAction::Supercede;
                }
            }
            HookAction::Continue
        }
        (HookId::ClientCommand, HookPhase::Pre) => handle_client_command(host),
        (HookId::PlayerSay, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let msg = host.hook_arg_str(0).unwrap_or_default();
            let trimmed = msg.trim();
            if trimmed.starts_with('@') {
                return handle_admin_hud_chat(host, Player::new(slot), trimmed);
            }
            if trimmed.eq_ignore_ascii_case("/admin") || trimmed.eq_ignore_ascii_case("/amxmodmenu") {
                let admin = Player::new(slot);
                if admin.has_flag(host, 'u') {
                    open_main_menu(host, admin);
                } else {
                    admin.say_tr(host, "ADMIN_NO_ACCESS", &[]);
                }
                return HookAction::Supercede;
            }
            HookAction::Continue
        }
        _ => HookAction::Continue,
    }
}

pub fn dispatch_menu(
    host: &mut impl HostBridge,
    cb_id: i32,
    slot: u8,
    _menu_id: u32,
    item_index: i32,
) -> HookAction {
    if item_index == MENU_EXIT {
        return HookAction::Handled;
    }
    let admin = Player::new(slot);
    let info = host.hook_arg_str(0).unwrap_or_default();
    let aname = admin.name(host);

    match cb_id {
        CB_MAIN_MENU => match info.as_str() {
            "kick" => {
                let title = admin.tr(host, "ADMIN_MENU_KICK", &[]);
                open_player_picker(host, admin, &title, CB_KICK_MENU);
            }
            "ban" => {
                let title = admin.tr(host, "ADMIN_MENU_BAN", &[]);
                open_player_picker(host, admin, &title, CB_BAN_MENU);
            }
            "slap" => {
                let title = admin.tr(host, "ADMIN_MENU_SLAP", &[]);
                open_player_picker(host, admin, &title, CB_SLAP_MENU);
            }
            "team" => {
                let title = admin.tr(host, "ADMIN_MENU_TEAM", &[]);
                open_player_picker(host, admin, &title, CB_TEAM_MENU);
            }
            "map" => open_map_menu(host, admin),
            "votemap" => {
                let _ = host.rpc_call("mapmanager.start_vote", i32::from(slot), "");
            }
            "cvar" => open_cvar_menu(host, admin),
            "restart" => {
                host.server_exec("sv_restart 1");
                broadcast_tr(host, "ADMIN_RESTART", &[("admin", &aname)]);
            }
            _ => {}
        },
        CB_KICK_MENU => {
            if let Ok(tslot) = info.parse::<u8>() {
                let target = Player::new(tslot);
                let tname = target.name(host);
                target.kick(host, "Kicked via amxmodmenu");
                broadcast_tr(host, "ADMIN_KICKED", &[("admin", &aname), ("target", &tname), ("reason", "amxmodmenu")]);
            }
        }
        CB_BAN_MENU => {
            if let Ok(tslot) = info.parse::<u8>() {
                let target = Player::new(tslot);
                let tname = target.name(host);
                target.kick(host, "Banned via amxmodmenu");
                broadcast_tr(host, "ADMIN_BANNED", &[("admin", &aname), ("target", &tname), ("reason", "amxmodmenu")]);
            }
        }
        CB_SLAP_MENU => {
            if let Ok(tslot) = info.parse::<u8>() {
                let target = Player::new(tslot);
                let tname = target.name(host);
                target.slap(host, 5, true);
                broadcast_tr(host, "ADMIN_SLAPPED", &[("admin", &aname), ("target", &tname), ("dmg", "5")]);
            }
        }
        CB_TEAM_MENU => {
            if let Ok(tslot) = info.parse::<u8>() {
                let target = Player::new(tslot);
                let next = match target.team(host) {
                    TeamKind::Terrorist => TeamKind::Ct,
                    TeamKind::Ct => TeamKind::Spectator,
                    _ => TeamKind::Terrorist,
                };
                let label = match next {
                    TeamKind::Terrorist => "Terrorists",
                    TeamKind::Ct => "Counter-Terrorists",
                    TeamKind::Spectator => "Spectators",
                    TeamKind::Unassigned => "Unassigned",
                };
                let tname = target.name(host);
                target.set_team(host, next, None);
                broadcast_tr(host, "ADMIN_TEAM_MOVED", &[("admin", &aname), ("target", &tname), ("team", label)]);
            }
        }
        CB_MAP_MENU => {
            broadcast_tr(host, "ADMIN_MAP_CHANGED", &[("admin", &aname), ("map", &info)]);
            host.change_level(&info);
        }
        CB_CVAR_MENU => match info.as_str() {
            "ff" => {
                let next = if host.cvar_get_f32("mp_friendlyfire") > 0.5 { "0" } else { "1" };
                host.cvar_set_str("mp_friendlyfire", next);
                broadcast_tr(host, "ADMIN_CVAR_SET", &[("admin", &aname), ("cvar", "mp_friendlyfire"), ("value", next)]);
            }
            "gravity" => {
                let next = if host.cvar_get_f32("sv_gravity") < 600.0 { "800" } else { "400" };
                host.cvar_set_str("sv_gravity", next);
                broadcast_tr(host, "ADMIN_CVAR_SET", &[("admin", &aname), ("cvar", "sv_gravity"), ("value", next)]);
            }
            "freezetime" => {
                let next = if host.cvar_get_f32("mp_freezetime") > 0.5 { "0" } else { "6" };
                host.cvar_set_str("mp_freezetime", next);
                broadcast_tr(host, "ADMIN_CVAR_SET", &[("admin", &aname), ("cvar", "mp_freezetime"), ("value", next)]);
            }
            "restart" => {
                host.server_exec("sv_restart 1");
                broadcast_tr(host, "ADMIN_RESTART", &[("admin", &aname)]);
            }
            _ => {}
        },
        _ => {}
    }
    HookAction::Handled
}

cs_mod_sdk::cs16_wasm_exports! {
    init: init_mod,
    hook: dispatch_hook,
    menu: dispatch_menu,
}
