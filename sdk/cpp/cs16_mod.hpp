// cs16-rs ABI v2 Modern C++20 Header-Only WebAssembly Plugin SDK
// Copyright (C) 2026 cs16-rs contributors
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Zero-dependency freestanding C++20 wrapper compiling cleanly with:
//   clang++ --target=wasm32 -std=c++20 -O2 -nostdlib -fno-exceptions \
//           -Wl,--no-entry -Wl,--export-dynamic -I sdk/cpp -o server.wasm main.cpp

#pragma once

#include "cs16_mod.h"

namespace cs16 {

// ---------------------------------------------------------------------------
// Freestanding String View & Stack String Buffer (no <string> / libc++ needed)
// ---------------------------------------------------------------------------

struct StrView {
    const char *ptr = "";
    size_t len = 0;

    constexpr StrView() = default;
    constexpr StrView(const char *p, size_t n) : ptr(p ? p : ""), len(p ? n : 0) {}
    constexpr StrView(const char *cstr) : ptr(cstr ? cstr : ""), len(0) {
        if (cstr) {
            while (cstr[len] != '\0') {
                ++len;
            }
        }
    }

    constexpr const char *data() const { return ptr; }
    constexpr size_t size() const { return len; }
    constexpr int32_t i32_len() const { return static_cast<int32_t>(len); }
    constexpr bool empty() const { return len == 0; }
    constexpr char operator[](size_t i) const { return ptr[i]; }

    static constexpr char ascii_lower(char c) {
        return (c >= 'A' && c <= 'Z') ? static_cast<char>(c - 'A' + 'a') : c;
    }

    constexpr bool equals_ignore_case(StrView other) const {
        if (len != other.len) return false;
        for (size_t i = 0; i < len; ++i) {
            if (ascii_lower(ptr[i]) != ascii_lower(other.ptr[i])) return false;
        }
        return true;
    }

    constexpr StrView trim() const {
        size_t start = 0;
        while (start < len && (ptr[start] == ' ' || ptr[start] == '\t' || ptr[start] == '\r' || ptr[start] == '\n')) {
            ++start;
        }
        size_t end = len;
        while (end > start && (ptr[end - 1] == ' ' || ptr[end - 1] == '\t' || ptr[end - 1] == '\r' || ptr[end - 1] == '\n')) {
            --end;
        }
        return StrView(ptr + start, end - start);
    }

    constexpr StrView first_token() const {
        StrView t = trim();
        size_t i = 0;
        while (i < t.len && t.ptr[i] != ' ' && t.ptr[i] != '\t') {
            ++i;
        }
        return StrView(t.ptr, i);
    }

    constexpr StrView after_first_token() const {
        StrView t = trim();
        size_t i = 0;
        while (i < t.len && t.ptr[i] != ' ' && t.ptr[i] != '\t') {
            ++i;
        }
        return StrView(t.ptr + i, t.len - i).trim();
    }
};

template <size_t Cap>
struct Buf {
    char storage[Cap]{};
    size_t len = 0;

    constexpr Buf() = default;

    char *data() { return storage; }
    const char *c_str() const { return storage; }
    size_t size() const { return len; }
    static constexpr size_t capacity() { return Cap > 0 ? Cap - 1 : 0; }
    bool empty() const { return len == 0; }

    StrView view() const { return StrView(storage, len); }
    operator StrView() const { return view(); }

    void set_len(int32_t raw) {
        if (raw <= 0) {
            len = 0;
        } else {
            size_t n = static_cast<size_t>(raw);
            len = n < capacity() ? n : capacity();
        }
        storage[len] = '\0';
    }

    void clear() {
        len = 0;
        storage[0] = '\0';
    }

    void append(StrView s) {
        for (size_t i = 0; i < s.len && len < capacity(); ++i) {
            storage[len++] = s.ptr[i];
        }
        storage[len] = '\0';
    }

    void append_char(char c) {
        if (len < capacity()) {
            storage[len++] = c;
            storage[len] = '\0';
        }
    }

    void append_i32(int32_t v) {
        if (v == 0) {
            append_char('0');
            return;
        }
        uint32_t u = 0;
        if (v < 0) {
            append_char('-');
            u = static_cast<uint32_t>(-(v + 1)) + 1u;
        } else {
            u = static_cast<uint32_t>(v);
        }
        char tmp[16];
        size_t n = 0;
        while (u > 0 && n < sizeof(tmp)) {
            tmp[n++] = static_cast<char>('0' + (u % 10u));
            u /= 10u;
        }
        while (n > 0) {
            append_char(tmp[--n]);
        }
    }
};

// ---------------------------------------------------------------------------
// Core Value Types & Enums
// ---------------------------------------------------------------------------

struct Vec3 {
    float x = 0.0f;
    float y = 0.0f;
    float z = 0.0f;

    constexpr float dist_sq(const Vec3 &o) const {
        float dx = x - o.x;
        float dy = y - o.y;
        float dz = z - o.z;
        return dx * dx + dy * dy + dz * dz;
    }
};

struct Color {
    uint8_t r = 255;
    uint8_t g = 255;
    uint8_t b = 255;
    uint8_t a = 255;

    constexpr int32_t pack_rgba() const {
        return static_cast<int32_t>(
            (static_cast<uint32_t>(r) << 24) |
            (static_cast<uint32_t>(g) << 16) |
            (static_cast<uint32_t>(b) << 8) |
            static_cast<uint32_t>(a)
        );
    }

    constexpr int32_t pack_rgb() const {
        return static_cast<int32_t>(
            (static_cast<uint32_t>(r) << 16) |
            (static_cast<uint32_t>(g) << 8) |
            static_cast<uint32_t>(b)
        );
    }

    static constexpr Color White()  { return {255, 255, 255, 255}; }
    static constexpr Color Red()    { return {255, 64, 64, 255}; }
    static constexpr Color Green()  { return {116, 212, 116, 255}; }
    static constexpr Color Cyan()   { return {0, 220, 255, 255}; }
    static constexpr Color Amber()  { return {255, 160, 0, 255}; }
    static constexpr Color Gold()   { return {255, 215, 0, 255}; }
    static constexpr Color CtBlue() { return {153, 204, 255, 255}; }
    static constexpr Color TRed()   { return {255, 64, 64, 255}; }

    // GoldSrc chat & menu escape codes
    static constexpr const char *CHAT_DEFAULT = "\x01";
    static constexpr const char *CHAT_TEAM    = "\x03";
    static constexpr const char *CHAT_GREEN   = "\x04";
    static constexpr const char *MENU_YELLOW  = "\\y";
    static constexpr const char *MENU_WHITE   = "\\w";
    static constexpr const char *MENU_RED     = "\\r";
    static constexpr const char *MENU_GREY    = "\\d";
    static constexpr const char *MENU_RIGHT   = "\\R";
};

enum class HookPhase : int32_t {
    Pre = CS16_PHASE_PRE,
    Post = CS16_PHASE_POST
};

enum class HookAction : int32_t {
    Continue = CS16_CONTINUE,
    Handled = CS16_HANDLED,
    Override = CS16_OVERRIDE,
    Supercede = CS16_SUPERCEDE
};

enum class HookId : int32_t {
    ServerActivate = CS16_HOOK_SERVER_ACTIVATE,
    ServerDeactivate = CS16_HOOK_SERVER_DEACTIVATE,
    SpawnEntity = CS16_HOOK_SPAWN_ENTITY,
    ClientConnect = CS16_HOOK_CLIENT_CONNECT,
    ClientPutInServer = CS16_HOOK_CLIENT_PUT_IN_SERVER,
    ClientDisconnect = CS16_HOOK_CLIENT_DISCONNECT,
    ClientCommand = CS16_HOOK_CLIENT_COMMAND,
    ClientUserinfoChanged = CS16_HOOK_CLIENT_USERINFO_CHANGED,
    RunUsercmd = CS16_HOOK_RUN_USERCMD,
    PostUsercmd = CS16_HOOK_POST_USERCMD,
    StartFrame = CS16_HOOK_START_FRAME,
    Think = CS16_HOOK_THINK,
    Touch = CS16_HOOK_TOUCH,
    Blocked = CS16_HOOK_BLOCKED,
    UpdateClientData = CS16_HOOK_UPDATE_CLIENT_DATA,
    GetWeaponData = CS16_HOOK_GET_WEAPON_DATA,
    ServerCommand = CS16_HOOK_SERVER_COMMAND,
    AddToFullPack = CS16_HOOK_ADD_TO_FULL_PACK,
    PlayerDeath = CS16_HOOK_PLAYER_DEATH,
    RoundStart = CS16_HOOK_ROUND_START,
    PlayerSpawn = CS16_HOOK_PLAYER_SPAWN,
    PlayerTakeDamage = CS16_HOOK_PLAYER_TAKE_DAMAGE,
    PlayerKilled = CS16_HOOK_PLAYER_KILLED,
    PlayerPreThink = CS16_HOOK_PLAYER_PRE_THINK,
    PlayerPostThink = CS16_HOOK_PLAYER_POST_THINK,
    PlayerJump = CS16_HOOK_PLAYER_JUMP,
    PlayersShouldCollide = CS16_HOOK_PLAYERS_SHOULD_COLLIDE,
    CanBuyItem = CS16_HOOK_CAN_BUY_ITEM,
    CanHavePlayerItem = CS16_HOOK_CAN_HAVE_PLAYER_ITEM,
    WeaponDeploy = CS16_HOOK_WEAPON_DEPLOY,
    WeaponPrimaryAttack = CS16_HOOK_WEAPON_PRIMARY_ATTACK,
    WeaponReload = CS16_HOOK_WEAPON_RELOAD,
    PlayerSay = CS16_HOOK_PLAYER_SAY,
    MenuSelect = CS16_HOOK_MENU_SELECT,
    FreezeTimeEnd = CS16_HOOK_FREEZE_TIME_END,
    RoundEnd = CS16_HOOK_ROUND_END,
    BombPlanting = CS16_HOOK_BOMB_PLANTING,
    BombPlanted = CS16_HOOK_BOMB_PLANTED,
    BombDefusing = CS16_HOOK_BOMB_DEFUSING,
    BombDefused = CS16_HOOK_BOMB_DEFUSED,
    BombExploded = CS16_HOOK_BOMB_EXPLODED,
    HostageTouched = CS16_HOOK_HOSTAGE_TOUCHED,
    HostageRescued = CS16_HOOK_HOSTAGE_RESCUED,
    GrenadeThrow = CS16_HOOK_GRENADE_THROW,
    EngEmitSound = CS16_HOOK_ENG_EMIT_SOUND,
    EngSendUserMsg = CS16_HOOK_ENG_SEND_USERMSG,
    EngClientPrint = CS16_HOOK_ENG_CLIENT_PRINT,
    EngCenterPrint = CS16_HOOK_ENG_CENTER_PRINT,
    EngCreateEntity = CS16_HOOK_ENG_CREATE_ENTITY,
    EngRemoveEntity = CS16_HOOK_ENG_REMOVE_ENTITY,
    EngKick = CS16_HOOK_ENG_KICK,
    EngChangeLevel = CS16_HOOK_ENG_CHANGE_LEVEL
};

enum class Team : int32_t {
    Unassigned = CS16_TEAM_UNASSIGNED,
    Terrorist = CS16_TEAM_TERRORIST,
    Ct = CS16_TEAM_CT,
    Spectator = CS16_TEAM_SPECTATOR
};

enum class ArmorKind : int32_t {
    None = CS16_ARMOR_NONE,
    Kevlar = CS16_ARMOR_KEVLAR,
    VestHelm = CS16_ARMOR_VESTHELM
};

// ---------------------------------------------------------------------------
// Player Handle Wrapper
// ---------------------------------------------------------------------------

class Player {
public:
    int32_t slot;

    constexpr explicit Player(int32_t slot_idx) : slot(slot_idx) {}

    constexpr int32_t edict_index() const { return slot + 1; }
    bool is_valid() const { return slot >= 0 && slot < 32; }
    bool is_active() const { return cs16_player_is_active(slot) != 0; }
    bool is_alive() const { return cs16_player_is_alive(slot) != 0; }

    Buf<64> name() const {
        Buf<64> b;
        b.set_len(cs16_player_name(slot, b.data(), static_cast<int32_t>(b.capacity())));
        return b;
    }

    Buf<64> ip() const {
        Buf<64> b;
        b.set_len(cs16_player_get_ip(slot, b.data(), static_cast<int32_t>(b.capacity())));
        return b;
    }

    Buf<64> steamid() const {
        Buf<64> b;
        b.set_len(cs16_player_get_steamid(slot, b.data(), static_cast<int32_t>(b.capacity())));
        return b;
    }

    int32_t ping() const { return cs16_player_get_ping(slot); }
    int32_t loss() const { return cs16_player_get_loss(slot); }

    bool origin(Vec3 &out) const {
        float xyz[3] = {0.0f, 0.0f, 0.0f};
        if (cs16_ent_get_origin(edict_index(), xyz) < 0) return false;
        out = {xyz[0], xyz[1], xyz[2]};
        return true;
    }

    bool aim_origin(Vec3 &out) const {
        float xyz[3] = {0.0f, 0.0f, 0.0f};
        if (cs16_player_get_aim_origin(slot, xyz) < 0) return false;
        out = {xyz[0], xyz[1], xyz[2]};
        return true;
    }

    float health() const { return cs16_player_get_health(slot); }
    int32_t set_health(float hp) const { return cs16_player_set_health(slot, hp); }
    int32_t set_max_health(float hp) const { return cs16_player_set_max_health(slot, hp); }

    int32_t armor() const { return cs16_player_get_armor(slot); }
    ArmorKind armor_type() const { return static_cast<ArmorKind>(cs16_player_get_armor_type(slot)); }
    int32_t set_armor(int32_t value, ArmorKind kind = ArmorKind::VestHelm) const {
        return cs16_player_set_armor(slot, value, static_cast<int32_t>(kind));
    }

    int32_t money() const { return cs16_player_get_money(slot); }
    int32_t set_money(int32_t amount, bool flash = true) const {
        return cs16_player_set_money(slot, amount, flash ? 1 : 0);
    }
    int32_t add_money(int32_t delta, bool flash = true) const {
        return cs16_player_add_money(slot, delta, flash ? 1 : 0);
    }

    int32_t frags() const { return cs16_player_get_frags(slot); }
    int32_t deaths() const { return cs16_player_get_deaths(slot); }
    int32_t set_score(int32_t f, int32_t d) const { return cs16_player_set_score(slot, f, d); }
    int32_t reset_score() const { return cs16_player_set_score(slot, 0, 0); }

    Team team() const { return static_cast<Team>(cs16_player_get_team(slot)); }
    int32_t set_team(Team t, int32_t model_class = -1) const {
        return cs16_player_set_team(slot, static_cast<int32_t>(t), model_class);
    }

    int32_t respawn() const { return cs16_player_respawn(slot); }
    int32_t slap(int32_t power, bool random_dir = true) const {
        return cs16_player_slap(slot, power, random_dir ? 1 : 0);
    }
    int32_t slay(bool preserve_frags = false) const {
        return cs16_player_slay(slot, preserve_frags ? 1 : 0);
    }
    int32_t damage(int32_t attacker_slot, float dmg, int32_t bits = 0) const {
        return cs16_player_damage(slot, attacker_slot, dmg, bits);
    }
    int32_t kick(StrView reason) const {
        return cs16_player_kick(slot, reason.data(), reason.i32_len());
    }

    int32_t set_godmode(bool enabled) const { return cs16_player_set_godmode(slot, enabled ? 1 : 0); }
    int32_t set_noclip(bool enabled) const { return cs16_player_set_noclip(slot, enabled ? 1 : 0); }
    int32_t set_maxspeed(float speed) const { return cs16_player_set_maxspeed(slot, speed); }
    int32_t set_gravity(float grav) const { return cs16_player_set_gravity(slot, grav); }
    int32_t set_footsteps(bool silent) const { return cs16_player_set_footsteps(slot, silent ? 1 : 0); }

    int32_t set_rendering(int32_t fx, Color col, int32_t mode, uint8_t amount) const {
        return cs16_player_set_rendering(slot, fx, col.pack_rgb(), mode, static_cast<int32_t>(amount));
    }
    int32_t set_glow(Color col, uint8_t thickness = 16) const {
        return set_rendering(CS16_RFX_GLOW_SHELL, col, CS16_RMODE_NORMAL, thickness);
    }
    int32_t set_trans_alpha(uint8_t alpha) const {
        return set_rendering(CS16_RFX_NONE, Color::White(), CS16_RMODE_TRANS_ALPHA, alpha);
    }
    int32_t clear_rendering() const {
        return set_rendering(CS16_RFX_NONE, Color::White(), CS16_RMODE_NORMAL, 255);
    }

    int32_t set_model(StrView model) const {
        return cs16_player_set_model(slot, model.data(), model.i32_len());
    }
    int32_t reset_model() const { return cs16_player_reset_model(slot); }

    int32_t give_item(StrView item) const {
        return cs16_player_give_item(slot, item.data(), item.i32_len());
    }
    int32_t give_custom_weapon(StrView id_name) const {
        return cs16_give_custom_weapon(slot, id_name.data(), id_name.i32_len());
    }
    int32_t strip_weapons() const { return cs16_player_strip_weapons(slot); }
    int32_t weapon_clip(int32_t weapon_id) const { return cs16_player_get_weapon_clip(slot, weapon_id); }
    int32_t set_weapon_clip(int32_t weapon_id, int32_t clip) const {
        return cs16_player_set_weapon_clip(slot, weapon_id, clip);
    }
    int32_t bpammo(int32_t weapon_id) const { return cs16_player_get_bpammo(slot, weapon_id); }
    int32_t set_bpammo(int32_t weapon_id, int32_t amount) const {
        return cs16_player_set_bpammo(slot, weapon_id, amount);
    }
    int32_t set_defuser(bool has) const { return cs16_player_set_defuser(slot, has ? 1 : 0); }
    int32_t set_nvg(bool has) const { return cs16_player_set_nvg(slot, has ? 1 : 0); }
    int32_t drop_slot(int32_t inv_slot) const { return cs16_player_drop_slot(slot, inv_slot); }

    int32_t admin_flags() const { return cs16_admin_player_flags(slot); }
    bool has_flag(char flag_char) const { return cs16_admin_has_flag(slot, static_cast<int32_t>(flag_char)) > 0; }

    Buf<16> lang() const {
        Buf<16> b;
        b.set_len(cs16_lang_player_code(slot, b.data(), static_cast<int32_t>(b.capacity())));
        return b;
    }
    int32_t set_lang(StrView code) const {
        return cs16_lang_set_player_code(slot, code.data(), code.i32_len());
    }

    template <size_t Cap = 256>
    Buf<Cap> tr(StrView key, StrView args_kv = "") const {
        Buf<Cap> b;
        b.set_len(cs16_lang_tr(
            slot,
            key.data(), key.i32_len(),
            args_kv.data(), args_kv.i32_len(),
            b.data(), static_cast<int32_t>(b.capacity())
        ));
        return b;
    }

    int32_t say(StrView text) const { return cs16_say_text(slot, text.data(), text.i32_len()); }
    int32_t say_tr(StrView key, StrView args_kv = "") const {
        auto text = tr<256>(key, args_kv);
        return say(text.view());
    }
    int32_t print(StrView text) const { return cs16_print_client(slot, text.data(), text.i32_len()); }
    int32_t center_print(StrView text) const { return cs16_center_print(slot, text.data(), text.i32_len()); }
    int32_t show_motd(StrView title, StrView body) const {
        return cs16_show_motd(slot, title.data(), title.i32_len(), body.data(), body.i32_len());
    }
    int32_t screen_fade(float duration_sec, float hold_sec, int32_t flags, Color col) const {
        return cs16_screen_fade(slot, duration_sec, hold_sec, flags, col.pack_rgba());
    }
    int32_t screen_shake(float amplitude, float duration_sec, float frequency) const {
        return cs16_screen_shake(slot, amplitude, duration_sec, frequency);
    }
    int32_t bar_time(int32_t seconds) const { return cs16_bar_time(slot, seconds); }
};

// ---------------------------------------------------------------------------
// Strongly-Typed Hook Event Context Helpers
// ---------------------------------------------------------------------------

struct DamageEvent {
    Player victim;
    int32_t attacker_slot;
    int32_t hitgroup;
    int32_t weapon_id;
    int32_t damage_bits;
    bool team_attack;
    float damage;

    static DamageEvent current() {
        return DamageEvent{
            Player(cs16_hook_arg_i32(0)),
            cs16_hook_arg_i32(1),
            cs16_hook_arg_i32(2),
            cs16_hook_arg_i32(3),
            cs16_hook_arg_i32(4),
            cs16_hook_arg_i32(5) != 0,
            cs16_hook_arg_f32(0),
        };
    }

    bool has_attacker() const { return attacker_slot >= 0; }
    Player attacker() const { return Player(attacker_slot); }

    void set_damage(float new_damage) {
        damage = new_damage;
        cs16_hook_set_override_f32(new_damage);
    }

    void set_hitgroup(int32_t new_hitgroup) {
        hitgroup = new_hitgroup;
        cs16_hook_set_override_i32(new_hitgroup);
    }
};

struct CollideEvent {
    Player a;
    Player b;
    bool should_collide;

    static CollideEvent current() {
        return CollideEvent{
            Player(cs16_hook_arg_i32(0)),
            Player(cs16_hook_arg_i32(1)),
            cs16_hook_arg_i32(2) != 0,
        };
    }

    HookAction allow(bool collide) const {
        cs16_hook_set_override_i32(collide ? 1 : 0);
        return HookAction::Override;
    }
};

struct BuyItemEvent {
    Player player;
    int32_t item_id;
    int32_t price;

    static BuyItemEvent current() {
        return BuyItemEvent{
            Player(cs16_hook_arg_i32(0)),
            cs16_hook_arg_i32(1),
            cs16_hook_arg_i32(2),
        };
    }

    HookAction set_price(int32_t new_price) const {
        cs16_hook_set_override_i32(new_price);
        return HookAction::Override;
    }
};

// ---------------------------------------------------------------------------
// 1-Line VFX Builder (`TE_*`)
// ---------------------------------------------------------------------------

namespace vfx {

inline int32_t shockwave_ring(
    Vec3 origin, float radius, Color col,
    int32_t life_decisec = 8, int32_t width = 10, int32_t slot = CS16_SLOT_ALL
) {
    return cs16_vfx_shockwave_ring(origin.x, origin.y, origin.z, radius, col.pack_rgba(), life_decisec, width, slot);
}

inline int32_t laser_beam(
    Vec3 start, Vec3 end, Color col,
    int32_t life_decisec = 5, int32_t width = 4, int32_t noise = 0, int32_t slot = CS16_SLOT_ALL
) {
    return cs16_vfx_laser_beam(start.x, start.y, start.z, end.x, end.y, end.z, col.pack_rgba(), life_decisec, width, noise, slot);
}

inline int32_t follow_trail(
    int32_t ent_idx, Color col,
    int32_t life_decisec = 10, int32_t width = 5, int32_t slot = CS16_SLOT_ALL
) {
    return cs16_vfx_follow_trail(ent_idx, col.pack_rgba(), life_decisec, width, slot);
}

inline int32_t kill_beam(int32_t ent_idx, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_kill_beam(ent_idx, slot);
}

inline int32_t aura_light(
    Vec3 origin, int32_t radius, Color col,
    int32_t life_decisec = 10, int32_t decay = 5, int32_t slot = CS16_SLOT_ALL
) {
    return cs16_vfx_aura_light(origin.x, origin.y, origin.z, radius, col.pack_rgb(), life_decisec, decay, slot);
}

inline int32_t sparks(Vec3 origin, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_sparks(origin.x, origin.y, origin.z, slot);
}

inline int32_t implosion(Vec3 origin, int32_t radius, int32_t count, int32_t life_decisec, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_implosion(origin.x, origin.y, origin.z, radius, count, life_decisec, slot);
}

inline int32_t teleport_splash(Vec3 origin, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_teleport_splash(origin.x, origin.y, origin.z, slot);
}

inline int32_t lava_splash(Vec3 origin, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_lava_splash(origin.x, origin.y, origin.z, slot);
}

inline int32_t explosion(Vec3 origin, int32_t scale, int32_t flags = 0, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_explosion(origin.x, origin.y, origin.z, scale, flags, slot);
}

inline int32_t blood_stream(Vec3 origin, Vec3 dir, int32_t color, int32_t speed, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_blood_stream(origin.x, origin.y, origin.z, dir.x, dir.y, dir.z, color, speed, slot);
}

inline int32_t world_decal(Vec3 origin, StrView decal_name, int32_t slot = CS16_SLOT_ALL) {
    return cs16_vfx_world_decal(origin.x, origin.y, origin.z, decal_name.data(), decal_name.i32_len(), slot);
}

} // namespace vfx

// ---------------------------------------------------------------------------
// Host-Managed Paginated Menu Wrapper
// ---------------------------------------------------------------------------

class Menu {
public:
    int32_t id = -1;

    constexpr Menu() = default;
    constexpr explicit Menu(int32_t menu_id) : id(menu_id) {}

    static Menu create(StrView title, int32_t callback_id) {
        return Menu(cs16_menu_create(title.data(), title.i32_len(), callback_id));
    }

    bool is_valid() const { return id > 0; }

    Menu &add_item(StrView label, StrView info = "", bool enabled = true) {
        if (is_valid()) {
            cs16_menu_add_item(id, label.data(), label.i32_len(), info.data(), info.i32_len(), enabled ? 1 : 0);
        }
        return *this;
    }

    Menu &add_blank() {
        if (is_valid()) cs16_menu_add_blank(id);
        return *this;
    }

    Menu &add_text(StrView text) {
        if (is_valid()) cs16_menu_add_text(id, text.data(), text.i32_len());
        return *this;
    }

    Menu &set_per_page(int32_t per_page) {
        if (is_valid()) cs16_menu_set_prop_int(id, CS16_MPROP_PERPAGE, per_page);
        return *this;
    }

    Menu &set_allow_exit(bool allow) {
        if (is_valid()) cs16_menu_set_prop_int(id, CS16_MPROP_EXIT, allow ? CS16_MEXIT_ALL : CS16_MEXIT_NEVER);
        return *this;
    }

    Menu &set_back_label(StrView label) {
        if (is_valid()) cs16_menu_set_prop_str(id, CS16_MPROP_BACKNAME, label.data(), label.i32_len());
        return *this;
    }

    Menu &set_next_label(StrView label) {
        if (is_valid()) cs16_menu_set_prop_str(id, CS16_MPROP_NEXTNAME, label.data(), label.i32_len());
        return *this;
    }

    Menu &set_exit_label(StrView label) {
        if (is_valid()) cs16_menu_set_prop_str(id, CS16_MPROP_EXITNAME, label.data(), label.i32_len());
        return *this;
    }

    int32_t display(Player p, int32_t page = 0, int32_t time_sec = 15) const {
        return is_valid() ? cs16_menu_display(p.slot, id, page, time_sec) : -1;
    }

    int32_t destroy() {
        int32_t rc = is_valid() ? cs16_menu_destroy(id) : -1;
        id = -1;
        return rc;
    }
};

// ---------------------------------------------------------------------------
// Server Utility Wrappers
// ---------------------------------------------------------------------------

namespace server {

inline int64_t time_ms() { return cs16_host_time_ms(); }
inline int32_t max_clients() { return cs16_player_max_clients(); }
inline int32_t print_all(StrView text) { return cs16_print_all(text.data(), text.i32_len()); }
inline int32_t say_all(StrView text) { return cs16_say_text(CS16_SLOT_ALL, text.data(), text.i32_len()); }
inline int32_t center_print_all(StrView text) { return cs16_center_print(CS16_SLOT_ALL, text.data(), text.i32_len()); }
inline int32_t register_command(StrView name) { return cs16_register_command(name.data(), name.i32_len()); }
inline int32_t exec(StrView line) { return cs16_server_exec(line.data(), line.i32_len()); }
inline int32_t change_level(StrView map) { return cs16_change_level(map.data(), map.i32_len()); }
inline int32_t lang_load_toml(StrView src) { return cs16_lang_load_toml(src.data(), src.i32_len()); }
inline int32_t timer_after(int32_t delay_ms, int32_t cb_id, int32_t arg = 0) {
    return cs16_timer_after(delay_ms, cb_id, arg);
}
inline int32_t timer_repeat(int32_t interval_ms, int32_t cb_id, int32_t arg = 0) {
    return cs16_timer_repeat(interval_ms, cb_id, arg);
}
inline int32_t http_post_json(StrView url, StrView body_json, int32_t cb_id = 0) {
    return cs16_http_post_json(url.data(), url.i32_len(), body_json.data(), body_json.i32_len(), cb_id);
}

} // namespace server

// ---------------------------------------------------------------------------
// Declarative Registration Macros (`CS16_PLUGIN_INFO`, `CS16_ON_HOOK`, `CS16_ON_CHAT`)
// ---------------------------------------------------------------------------

namespace detail {

using HookFn = HookAction (*)();
using ChatFn = HookAction (*)(Player, StrView);

struct HookEntry {
    int32_t hook_id;
    int32_t phase;
    int32_t priority;
    HookFn fn;
};

struct ChatEntry {
    const char *trigger;
    ChatFn fn;
};

inline constexpr size_t MAX_HOOKS = 32;
inline constexpr size_t MAX_CHATS = 32;

struct Registry {
    HookEntry hooks[MAX_HOOKS]{};
    size_t hook_count = 0;
    ChatEntry chats[MAX_CHATS]{};
    size_t chat_count = 0;
    bool initialized = false;

    void add_hook(HookId id, HookPhase phase, int32_t priority, HookFn fn) {
        if (hook_count < MAX_HOOKS) {
            hooks[hook_count++] = HookEntry{
                static_cast<int32_t>(id),
                static_cast<int32_t>(phase),
                priority,
                fn,
            };
        }
    }

    void add_chat(const char *trigger, ChatFn fn) {
        if (chat_count < MAX_CHATS) {
            chats[chat_count++] = ChatEntry{trigger, fn};
        }
    }

    int32_t init_all() {
        bool say_registered = false;
        for (size_t i = 0; i < hook_count; ++i) {
            cs16_hook_register(hooks[i].hook_id, hooks[i].phase, hooks[i].priority);
            if (hooks[i].hook_id == CS16_HOOK_PLAYER_SAY && hooks[i].phase == CS16_PHASE_PRE) {
                say_registered = true;
            }
        }
        if (chat_count > 0 && !say_registered) {
            cs16_hook_register(CS16_HOOK_PLAYER_SAY, CS16_PHASE_PRE, 0);
        }
        initialized = true;
        return 0;
    }

    int32_t dispatch(int32_t hook_id, int32_t phase) {
        int32_t best = CS16_CONTINUE;
        if (hook_id == CS16_HOOK_PLAYER_SAY && phase == CS16_PHASE_PRE && chat_count > 0) {
            int32_t slot = cs16_hook_arg_i32(0);
            Buf<512> msg;
            msg.set_len(cs16_hook_arg_str(0, msg.data(), static_cast<int32_t>(msg.capacity())));
            StrView tok = msg.view().first_token();
            StrView rest = msg.view().after_first_token();
            for (size_t i = 0; i < chat_count; ++i) {
                if (tok.equals_ignore_case(StrView(chats[i].trigger))) {
                    int32_t act = static_cast<int32_t>(chats[i].fn(Player(slot), rest));
                    if (act > best) best = act;
                    if (act == CS16_SUPERCEDE) return act;
                }
            }
        }
        for (size_t i = 0; i < hook_count; ++i) {
            if (hooks[i].hook_id == hook_id && hooks[i].phase == phase) {
                int32_t act = static_cast<int32_t>(hooks[i].fn());
                if (act > best) best = act;
                if (act == CS16_SUPERCEDE) return act;
            }
        }
        return best;
    }
};

inline Registry g_registry{};

inline Registry &registry() {
    return g_registry;
}

struct HookRegistrar {
    HookRegistrar(HookId id, HookPhase phase, int32_t priority, HookFn fn) {
        registry().add_hook(id, phase, priority, fn);
    }
};

struct ChatRegistrar {
    ChatRegistrar(const char *trigger, ChatFn fn) {
        registry().add_chat(trigger, fn);
    }
};

} // namespace detail
} // namespace cs16

#define CS16_CONCAT_INNER(a, b) a##b
#define CS16_CONCAT(a, b) CS16_CONCAT_INNER(a, b)

#define CS16_PLUGIN_INFO(mod_id_lit, mod_name_lit, mod_ver_lit) \
    namespace cs16_plugin_meta {                                \
        inline constexpr const char *ID = (mod_id_lit);         \
        inline constexpr const char *NAME = (mod_name_lit);     \
        inline constexpr const char *VERSION = (mod_ver_lit);   \
    }

#define CS16_ON_HOOK(hook_enum, phase_enum, fn_name)                                           \
    static ::cs16::HookAction fn_name();                                                       \
    static ::cs16::detail::HookRegistrar CS16_CONCAT(_cs16_hook_reg_, fn_name)(                \
        (hook_enum), (phase_enum), 0, &(fn_name)                                               \
    );                                                                                         \
    static ::cs16::HookAction fn_name()

#define CS16_ON_CHAT(trigger_lit, fn_name)                                                     \
    static ::cs16::HookAction fn_name(::cs16::Player player, ::cs16::StrView args);            \
    static ::cs16::detail::ChatRegistrar CS16_CONCAT(_cs16_chat_reg_, fn_name)(                \
        (trigger_lit), &(fn_name)                                                              \
    );                                                                                         \
    static ::cs16::HookAction fn_name(::cs16::Player player, ::cs16::StrView args)

#if defined(__wasm__) && !defined(__wasi__)
extern "C" void __wasm_call_ctors(void);
#define CS16_CALL_CTORS() __wasm_call_ctors()
#else
#define CS16_CALL_CTORS() ((void)0)
#endif

// Emits `cs_mod_init` and `cs_mod_hook` wired to all `CS16_ON_HOOK` / `CS16_ON_CHAT` declarations.
#define CS16_EXPORT_PLUGIN()                                   \
    extern "C" CS16_EXPORT int32_t cs_mod_init(void) {         \
        CS16_CALL_CTORS();                                     \
        return ::cs16::detail::registry().init_all();          \
    }                                                          \
    extern "C" CS16_EXPORT int32_t cs_mod_hook(                \
        int32_t hook_id, int32_t phase                         \
    ) {                                                        \
        return ::cs16::detail::registry().dispatch(            \
            hook_id, phase                                     \
        );                                                     \
    }
