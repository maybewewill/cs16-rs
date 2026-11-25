// cs16-rs Standard Weapon Restrict Menu (`std_restmenu`) — C++20 -> WASM
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Features:
// - `amx_restrict` & `/restmenu` paginated GoldSrc menu toggling per-weapon restrictions
// - `CanBuyItem` (Pre) enforcement returning `HookAction::Supercede` for restricted items
// - Dynamic AWP cap: max 1 AWP per team when fewer than 10 players are active
// - Localized `en` / `ru` chat notifications via `LangCatalog`

#include "cs16_mod.hpp"

CS16_PLUGIN_INFO("std_restmenu", "Standard Weapon Restrict Menu", "1.0.0")

namespace {

constexpr int32_t CB_REST_MENU = 1;
constexpr int32_t WEAPON_AWP = 18;
constexpr int32_t WEAPON_SG550 = 13;
constexpr int32_t WEAPON_G3SG1 = 24;
constexpr int32_t WEAPON_SHIELD = 99;

struct WeaponEntry {
    int32_t id;
    const char *alias;
    const char *label;
};

constexpr WeaponEntry kWeapons[] = {
    {WEAPON_AWP, "awp", "AWP Magnum Sniper"},
    {WEAPON_SG550, "sg550", "SG550 Auto-Sniper"},
    {WEAPON_G3SG1, "g3sg1", "G3SG1 Auto-Sniper"},
    {20, "m249", "M249 Para Machinegun"},
    {28, "ak47", "AK-47 Kalashnikov"},
    {22, "m4a1", "M4A1 Carbine"},
    {3, "scout", "Steyr Scout"},
    {26, "deagle", "Desert Eagle"},
    {4, "hegrenade", "HE Grenade"},
    {25, "flashbang", "Flashbang"},
    {WEAPON_SHIELD, "shield", "Tactical Shield"},
};

// Bitmask over `kWeapons` indices (by default SG550, G3SG1, Shield can be toggled live;
// AWP has dynamic cap < 10 players and can also be hard-restricted).
static uint32_t g_restricted_mask = (1u << 1) | (1u << 2) | (1u << 10);
static uint8_t g_team_awp_bought[4] = {0, 0, 0, 0};

int find_weapon_idx_by_id(int32_t wid) {
    for (size_t i = 0; i < sizeof(kWeapons) / sizeof(kWeapons[0]); ++i) {
        if (kWeapons[i].id == wid) return static_cast<int>(i);
    }
    return -1;
}

int find_weapon_idx_by_alias(cs16::StrView alias) {
    for (size_t i = 0; i < sizeof(kWeapons) / sizeof(kWeapons[0]); ++i) {
        if (alias.equals_ignore_case(kWeapons[i].alias)) return static_cast<int>(i);
    }
    return -1;
}

int32_t count_active_players() {
    int32_t max_c = cs16::server::max_clients();
    int32_t n = 0;
    for (int32_t s = 0; s < max_c; ++s) {
        if (cs16::Player(s).is_active()) ++n;
    }
    return n;
}

void open_rest_menu(cs16::Player admin) {
    cs16::Buf<128> title;
    title.append(cs16::Color::MENU_YELLOW);
    auto tr_title = admin.tr("REST_MENU_TITLE");
    title.append(tr_title.view());
    title.append(cs16::Color::MENU_WHITE);

    auto menu = cs16::Menu::create(title.view(), CB_REST_MENU);
    menu.set_per_page(7);
    for (size_t i = 0; i < sizeof(kWeapons) / sizeof(kWeapons[0]); ++i) {
        bool blocked = (g_restricted_mask & (1u << i)) != 0u;
        cs16::Buf<96> line;
        line.append(kWeapons[i].label);
        line.append(" \\R");
        line.append(blocked ? "\\r[OFF]\\w" : "\\w[ON]");
        menu.add_item(line.view(), kWeapons[i].alias);
    }
    menu.display(admin, 0, 20);
}

void toggle_weapon(cs16::Player admin, size_t idx) {
    g_restricted_mask ^= (1u << idx);
    bool now_blocked = (g_restricted_mask & (1u << idx)) != 0u;
    cs16::Buf<96> kv;
    kv.append("weapon=");
    kv.append(kWeapons[idx].label);
    admin.say_tr(now_blocked ? "REST_TOGGLED_ON" : "REST_TOGGLED_OFF", kv.view());
}

} // namespace

CS16_ON_HOOK(cs16::HookId::RoundStart, cs16::HookPhase::Post, on_round_start) {
    g_team_awp_bought[0] = 0;
    g_team_awp_bought[1] = 0;
    g_team_awp_bought[2] = 0;
    g_team_awp_bought[3] = 0;
    return cs16::HookAction::Continue;
}

CS16_ON_HOOK(cs16::HookId::CanBuyItem, cs16::HookPhase::Pre, on_can_buy) {
    int32_t slot = cs16_hook_arg_i32(0);
    int32_t item_id = cs16_hook_arg_i32(1);
    cs16::Player p(slot);

    int idx = find_weapon_idx_by_id(item_id);
    if (idx >= 0 && (g_restricted_mask & (1u << idx)) != 0u) {
        cs16::Buf<96> kv;
        kv.append("weapon=");
        kv.append(kWeapons[idx].label);
        p.say_tr("REST_BLOCKED", kv.view());
        return cs16::HookAction::Supercede;
    }

    if (item_id == WEAPON_AWP && count_active_players() < 10) {
        int32_t team = static_cast<int32_t>(p.team());
        if (team >= 1 && team <= 2) {
            if (g_team_awp_bought[team] >= 1) {
                p.say_tr("REST_AWP_LIMIT");
                return cs16::HookAction::Supercede;
            }
            g_team_awp_bought[team] += 1;
        }
    }
    return cs16::HookAction::Continue;
}

CS16_ON_HOOK(cs16::HookId::ClientCommand, cs16::HookPhase::Pre, on_client_cmd) {
    int32_t slot = cs16_hook_arg_i32(0);
    cs16::Buf<64> cmd;
    cmd.set_len(cs16_hook_arg_str(0, cmd.data(), static_cast<int32_t>(cmd.capacity())));
    if (!cmd.view().equals_ignore_case("amx_restrict") &&
        !cmd.view().equals_ignore_case("amx_restmenu")) {
        return cs16::HookAction::Continue;
    }
    cs16::Player admin(slot);
    if (!admin.has_flag('g')) {
        admin.say_tr("REST_NO_ACCESS");
        return cs16::HookAction::Handled;
    }
    cs16::Buf<128> arg1;
    arg1.set_len(cs16_hook_arg_str(1, arg1.data(), static_cast<int32_t>(arg1.capacity())));
    int idx = find_weapon_idx_by_alias(arg1.view());
    if (idx >= 0) {
        toggle_weapon(admin, static_cast<size_t>(idx));
    } else {
        open_rest_menu(admin);
    }
    return cs16::HookAction::Handled;
}

CS16_ON_CHAT("/restmenu", on_restmenu_chat) {
    (void)args;
    if (!player.has_flag('g')) {
        player.say_tr("REST_NO_ACCESS");
        return cs16::HookAction::Supercede;
    }
    open_rest_menu(player);
    return cs16::HookAction::Supercede;
}

extern "C" CS16_EXPORT int32_t cs_mod_menu(
    int32_t cb_id, int32_t slot, int32_t menu_id, int32_t item_index
) {
    (void)menu_id;
    if (cb_id != CB_REST_MENU || item_index == CS16_MENU_EXIT) {
        return CS16_HANDLED;
    }
    cs16::Buf<64> info;
    info.set_len(cs16_hook_arg_str(0, info.data(), static_cast<int32_t>(info.capacity())));
    int idx = find_weapon_idx_by_alias(info.view());
    if (idx >= 0) {
        cs16::Player admin(slot);
        toggle_weapon(admin, static_cast<size_t>(idx));
        open_rest_menu(admin);
    }
    return CS16_HANDLED;
}

CS16_EXPORT_PLUGIN()
