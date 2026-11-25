/*
 * cs16-rs ABI v2 C99 WebAssembly Plugin SDK Header
 * Copyright (C) 2026 cs16-rs contributors
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Standalone zero-dependency C99 header declaring every "cs16_mod" host import
 * provided by cs-mod-host::WasmiRuntime (ABI v1 + v2) and WASM export helpers.
 *
 * Compile freestanding to compact .wasm with Clang:
 *   clang --target=wasm32 -std=c99 -O2 -nostdlib -Wl,--no-entry -Wl,--export-dynamic \
 *         -I sdk/cpp -o server.wasm main.c
 */

#ifndef CS16_MOD_H
#define CS16_MOD_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define CS16_ABI_VERSION 2

#if defined(__wasm__) || defined(__wasm32__)
#define CS16_IMPORT(name) __attribute__((import_module("cs16_mod"), import_name(name)))
#define CS16_EXPORT __attribute__((visibility("default")))
#define CS16_EXPORT_NAME(name) __attribute__((export_name(name)))
#else
#define CS16_IMPORT(name)
#define CS16_EXPORT
#define CS16_EXPORT_NAME(name)
#endif

/* ------------------------------------------------------------------------- */
/* ABI v1 + v2 Constants (matching crates/cs-mod-api/src/{abi,ops,services}.rs) */
/* ------------------------------------------------------------------------- */

#define CS16_SLOT_ALL (-1)
#define CS16_MENU_EXIT (-3)

/* HookPhase */
enum Cs16HookPhase {
    CS16_PHASE_PRE = 0,
    CS16_PHASE_POST = 1
};

/* HookAction */
enum Cs16HookAction {
    CS16_CONTINUE = 0,
    CS16_HANDLED = 1,
    CS16_OVERRIDE = 2,
    CS16_SUPERCEDE = 3
};

/* HookId (exact discriminants from cs-mod-api::HookId) */
enum Cs16HookId {
    /* GameDll hooks (v1) */
    CS16_HOOK_SERVER_ACTIVATE = 1,
    CS16_HOOK_SERVER_DEACTIVATE = 2,
    CS16_HOOK_SPAWN_ENTITY = 3,
    CS16_HOOK_CLIENT_CONNECT = 4,
    CS16_HOOK_CLIENT_PUT_IN_SERVER = 5,
    CS16_HOOK_CLIENT_DISCONNECT = 6,
    CS16_HOOK_CLIENT_COMMAND = 7,
    CS16_HOOK_CLIENT_USERINFO_CHANGED = 8,
    CS16_HOOK_RUN_USERCMD = 9,
    CS16_HOOK_POST_USERCMD = 10,
    CS16_HOOK_START_FRAME = 11,
    CS16_HOOK_THINK = 12,
    CS16_HOOK_TOUCH = 13,
    CS16_HOOK_BLOCKED = 14,
    CS16_HOOK_UPDATE_CLIENT_DATA = 15,
    CS16_HOOK_GET_WEAPON_DATA = 16,
    CS16_HOOK_SERVER_COMMAND = 17,
    CS16_HOOK_ADD_TO_FULL_PACK = 18,
    /* Gameplay & ReAPI hooks (v1 + v2) */
    CS16_HOOK_PLAYER_DEATH = 30,
    CS16_HOOK_ROUND_START = 31,
    CS16_HOOK_PLAYER_SPAWN = 32,
    CS16_HOOK_PLAYER_TAKE_DAMAGE = 33,
    CS16_HOOK_PLAYER_KILLED = 34,
    CS16_HOOK_PLAYER_PRE_THINK = 35,
    CS16_HOOK_PLAYER_POST_THINK = 36,
    CS16_HOOK_PLAYER_JUMP = 37,
    CS16_HOOK_PLAYERS_SHOULD_COLLIDE = 38,
    CS16_HOOK_CAN_BUY_ITEM = 39,
    CS16_HOOK_CAN_HAVE_PLAYER_ITEM = 40,
    CS16_HOOK_WEAPON_DEPLOY = 41,
    CS16_HOOK_WEAPON_PRIMARY_ATTACK = 42,
    CS16_HOOK_WEAPON_RELOAD = 43,
    CS16_HOOK_PLAYER_SAY = 44,
    CS16_HOOK_MENU_SELECT = 45,
    CS16_HOOK_FREEZE_TIME_END = 46,
    CS16_HOOK_ROUND_END = 47,
    CS16_HOOK_BOMB_PLANTING = 48,
    CS16_HOOK_BOMB_PLANTED = 49,
    CS16_HOOK_BOMB_DEFUSING = 50,
    CS16_HOOK_BOMB_DEFUSED = 51,
    CS16_HOOK_BOMB_EXPLODED = 52,
    CS16_HOOK_HOSTAGE_TOUCHED = 53,
    CS16_HOOK_HOSTAGE_RESCUED = 54,
    CS16_HOOK_GRENADE_THROW = 55,
    /* EngineApi hooks (v1) */
    CS16_HOOK_ENG_EMIT_SOUND = 100,
    CS16_HOOK_ENG_SEND_USERMSG = 101,
    CS16_HOOK_ENG_CLIENT_PRINT = 102,
    CS16_HOOK_ENG_CENTER_PRINT = 103,
    CS16_HOOK_ENG_CREATE_ENTITY = 104,
    CS16_HOOK_ENG_REMOVE_ENTITY = 105,
    CS16_HOOK_ENG_KICK = 106,
    CS16_HOOK_ENG_CHANGE_LEVEL = 107
};

/* ArmorKind */
enum Cs16ArmorKind {
    CS16_ARMOR_NONE = 0,
    CS16_ARMOR_KEVLAR = 1,
    CS16_ARMOR_VESTHELM = 2
};

/* TeamKind */
enum Cs16TeamKind {
    CS16_TEAM_UNASSIGNED = 0,
    CS16_TEAM_TERRORIST = 1,
    CS16_TEAM_CT = 2,
    CS16_TEAM_SPECTATOR = 3
};

/* MenuManager properties (MPROP_*) */
enum Cs16MenuProp {
    CS16_MPROP_PERPAGE = 1,
    CS16_MPROP_BACKNAME = 2,
    CS16_MPROP_NEXTNAME = 3,
    CS16_MPROP_EXITNAME = 4,
    CS16_MPROP_TITLE = 5,
    CS16_MPROP_EXIT = 6,
    CS16_MPROP_NOCOLORS = 7,
    CS16_MPROP_NUMBER_COLOR = 8,
    CS16_MPROP_SHOWPAGE = 9,
    CS16_MEXIT_ALL = 1,
    CS16_MEXIT_NEVER = -1
};

/* GoldSrc render FX & modes */
enum Cs16RenderFx {
    CS16_RFX_NONE = 0,
    CS16_RFX_PULSE_SLOW = 1,
    CS16_RFX_PULSE_FAST = 2,
    CS16_RFX_STROBE_SLOW = 9,
    CS16_RFX_GLOW_SHELL = 19
};

enum Cs16RenderMode {
    CS16_RMODE_NORMAL = 0,
    CS16_RMODE_TRANS_COLOR = 1,
    CS16_RMODE_TRANS_TEXTURE = 2,
    CS16_RMODE_GLOW = 3,
    CS16_RMODE_TRANS_ALPHA = 4,
    CS16_RMODE_TRANS_ADD = 5
};

/* CS 1.6 Weapon IDs (CSW_*) */
enum Cs16WeaponId {
    CS16_CSW_P228 = 1,
    CS16_CSW_SCOUT = 3,
    CS16_CSW_HEGRENADE = 4,
    CS16_CSW_XM1014 = 5,
    CS16_CSW_C4 = 6,
    CS16_CSW_MAC10 = 7,
    CS16_CSW_AUG = 8,
    CS16_CSW_SMOKEGRENADE = 9,
    CS16_CSW_ELITE = 10,
    CS16_CSW_FIVESEVEN = 11,
    CS16_CSW_UMP45 = 12,
    CS16_CSW_SG550 = 13,
    CS16_CSW_GALIL = 14,
    CS16_CSW_FAMAS = 15,
    CS16_CSW_USP = 16,
    CS16_CSW_GLOCK18 = 17,
    CS16_CSW_AWP = 18,
    CS16_CSW_MP5NAVY = 19,
    CS16_CSW_M249 = 20,
    CS16_CSW_M3 = 21,
    CS16_CSW_M4A1 = 22,
    CS16_CSW_TMP = 23,
    CS16_CSW_G3SG1 = 24,
    CS16_CSW_FLASHBANG = 25,
    CS16_CSW_DEAGLE = 26,
    CS16_CSW_SG552 = 27,
    CS16_CSW_AK47 = 28,
    CS16_CSW_KNIFE = 29,
    CS16_CSW_P90 = 30
};

/* Color packing helpers matching cs-mod-host::runtime::{rgba_u32, rgb_u24} */
static inline int32_t cs16_pack_rgba(uint8_t r, uint8_t g, uint8_t b, uint8_t a) {
    return (int32_t)(((uint32_t)r << 24) | ((uint32_t)g << 16) | ((uint32_t)b << 8) | (uint32_t)a);
}

static inline int32_t cs16_pack_rgb(uint8_t r, uint8_t g, uint8_t b) {
    return (int32_t)(((uint32_t)r << 16) | ((uint32_t)g << 8) | (uint32_t)b);
}

/* ------------------------------------------------------------------------- */
/* Host Imports: Lifecycle & Hook Context                                    */
/* ------------------------------------------------------------------------- */

CS16_IMPORT("hook_register")
int32_t cs16_hook_register(int32_t hook_id, int32_t phase, int32_t priority);

CS16_IMPORT("host_time_ms")
int64_t cs16_host_time_ms(void);

CS16_IMPORT("hook_arg_i32")
int32_t cs16_hook_arg_i32(int32_t idx);

CS16_IMPORT("hook_arg_f32")
float cs16_hook_arg_f32(int32_t idx);

CS16_IMPORT("hook_arg_str")
int32_t cs16_hook_arg_str(int32_t idx, char *out_ptr, int32_t cap);

CS16_IMPORT("hook_arg_bytes")
int32_t cs16_hook_arg_bytes(uint8_t *out_ptr, int32_t cap);

CS16_IMPORT("hook_set_override_i32")
int32_t cs16_hook_set_override_i32(int32_t val);

CS16_IMPORT("hook_set_override_i32_2")
int32_t cs16_hook_set_override_i32_2(int32_t val);

CS16_IMPORT("hook_set_override_f32")
int32_t cs16_hook_set_override_f32(float val);

CS16_IMPORT("hook_set_override_str")
int32_t cs16_hook_set_override_str(const char *ptr, int32_t len);

CS16_IMPORT("rpc_set_return_str")
int32_t cs16_rpc_set_return_str(const char *ptr, int32_t len);

/* ------------------------------------------------------------------------- */
/* Host Imports: Entities & Basic Players (v1)                               */
/* ------------------------------------------------------------------------- */

CS16_IMPORT("ent_count")
int32_t cs16_ent_count(void);

CS16_IMPORT("ent_create")
int32_t cs16_ent_create(const char *cls_ptr, int32_t cls_len);

CS16_IMPORT("ent_remove")
int32_t cs16_ent_remove(int32_t idx);

CS16_IMPORT("ent_get_health")
float cs16_ent_get_health(int32_t idx);

CS16_IMPORT("ent_set_health")
int32_t cs16_ent_set_health(int32_t idx, float health);

CS16_IMPORT("ent_get_origin")
int32_t cs16_ent_get_origin(int32_t idx, float *out_xyz);

CS16_IMPORT("ent_set_origin")
int32_t cs16_ent_set_origin(int32_t idx, float x, float y, float z);

CS16_IMPORT("ent_get_velocity")
int32_t cs16_ent_get_velocity(int32_t idx, float *out_xyz);

CS16_IMPORT("ent_set_velocity")
int32_t cs16_ent_set_velocity(int32_t idx, float x, float y, float z);

CS16_IMPORT("ent_get_flags")
int32_t cs16_ent_get_flags(int32_t idx);

CS16_IMPORT("ent_set_flags")
int32_t cs16_ent_set_flags(int32_t idx, int32_t flags);

CS16_IMPORT("ent_classname")
int32_t cs16_ent_classname(int32_t idx, char *out_ptr, int32_t cap);

CS16_IMPORT("player_max_clients")
int32_t cs16_player_max_clients(void);

CS16_IMPORT("player_is_active")
int32_t cs16_player_is_active(int32_t slot);

CS16_IMPORT("player_name")
int32_t cs16_player_name(int32_t slot, char *out_ptr, int32_t cap);

CS16_IMPORT("player_kick")
int32_t cs16_player_kick(int32_t slot, const char *ptr, int32_t len);

/* ------------------------------------------------------------------------- */
/* Host Imports: Messages, CVars, Commands, Timers, Traces, Sound, Storage   */
/* ------------------------------------------------------------------------- */

CS16_IMPORT("print_client")
int32_t cs16_print_client(int32_t slot, const char *ptr, int32_t len);

CS16_IMPORT("print_all")
int32_t cs16_print_all(const char *ptr, int32_t len);

CS16_IMPORT("center_print")
int32_t cs16_center_print(int32_t slot, const char *ptr, int32_t len);

CS16_IMPORT("say_text")
int32_t cs16_say_text(int32_t slot, const char *ptr, int32_t len);

CS16_IMPORT("send_custom_usermsg")
int32_t cs16_send_custom_usermsg(
    int32_t slot,
    const char *name_ptr,
    int32_t name_len,
    const uint8_t *data_ptr,
    int32_t data_len
);

CS16_IMPORT("cvar_get_f32")
float cs16_cvar_get_f32(const char *ptr, int32_t len);

CS16_IMPORT("cvar_set_f32")
int32_t cs16_cvar_set_f32(const char *ptr, int32_t len, float val);

CS16_IMPORT("cvar_get_str")
int32_t cs16_cvar_get_str(const char *ptr, int32_t len, char *out_ptr, int32_t cap);

CS16_IMPORT("cvar_set_str")
int32_t cs16_cvar_set_str(const char *ptr, int32_t len, const char *val_ptr, int32_t val_len);

CS16_IMPORT("register_command")
int32_t cs16_register_command(const char *ptr, int32_t len);

CS16_IMPORT("server_exec")
int32_t cs16_server_exec(const char *ptr, int32_t len);

CS16_IMPORT("change_level")
int32_t cs16_change_level(const char *ptr, int32_t len);

CS16_IMPORT("show_menu")
int32_t cs16_show_menu(int32_t slot, int32_t keys, int32_t time_sec, const char *ptr, int32_t len);

CS16_IMPORT("timer_after")
int32_t cs16_timer_after(int32_t delay_ms, int32_t cb_id, int32_t arg);

CS16_IMPORT("timer_repeat")
int32_t cs16_timer_repeat(int32_t interval_ms, int32_t cb_id, int32_t arg);

CS16_IMPORT("timer_cancel")
int32_t cs16_timer_cancel(int32_t timer_id);

CS16_IMPORT("trace_line")
float cs16_trace_line(float x1, float y1, float z1, float x2, float y2, float z2);

CS16_IMPORT("precache_sound")
int32_t cs16_precache_sound(const char *ptr, int32_t len);

CS16_IMPORT("emit_sound")
int32_t cs16_emit_sound(int32_t ent_idx, int32_t channel, const char *ptr, int32_t len, float volume);

CS16_IMPORT("storage_get")
int32_t cs16_storage_get(const char *key_ptr, int32_t key_len, char *out_ptr, int32_t cap);

CS16_IMPORT("storage_set")
int32_t cs16_storage_set(const char *key_ptr, int32_t key_len, const char *val_ptr, int32_t val_len);

CS16_IMPORT("storage_remove")
int32_t cs16_storage_remove(const char *key_ptr, int32_t key_len);

CS16_IMPORT("hud_draw_rect")
int32_t cs16_hud_draw_rect(float x, float y, float w, float h, int32_t rgba);

CS16_IMPORT("hud_draw_text")
int32_t cs16_hud_draw_text(float x, float y, float size, int32_t rgba, const char *ptr, int32_t len);

CS16_IMPORT("hud_draw_bar")
int32_t cs16_hud_draw_bar(float x, float y, float w, float h, float frac, int32_t rgba);

CS16_IMPORT("hud_panel")
int32_t cs16_hud_panel(float x, float y, float w, float h, int32_t rgba, const char *ptr, int32_t len);

CS16_IMPORT("client_exec")
int32_t cs16_client_exec(const char *ptr, int32_t len);

/* ------------------------------------------------------------------------- */
/* Host Imports: ABI v2 Player & Weapon Natives                              */
/* ------------------------------------------------------------------------- */

CS16_IMPORT("player_get_health")
float cs16_player_get_health(int32_t slot);

CS16_IMPORT("player_set_health")
int32_t cs16_player_set_health(int32_t slot, float health);

CS16_IMPORT("player_set_max_health")
int32_t cs16_player_set_max_health(int32_t slot, float max_health);

CS16_IMPORT("player_get_armor")
int32_t cs16_player_get_armor(int32_t slot);

CS16_IMPORT("player_get_armor_type")
int32_t cs16_player_get_armor_type(int32_t slot);

CS16_IMPORT("player_set_armor")
int32_t cs16_player_set_armor(int32_t slot, int32_t armor, int32_t kind);

CS16_IMPORT("player_get_money")
int32_t cs16_player_get_money(int32_t slot);

CS16_IMPORT("player_set_money")
int32_t cs16_player_set_money(int32_t slot, int32_t amount, int32_t flash);

CS16_IMPORT("player_add_money")
int32_t cs16_player_add_money(int32_t slot, int32_t delta, int32_t flash);

CS16_IMPORT("player_get_frags")
int32_t cs16_player_get_frags(int32_t slot);

CS16_IMPORT("player_get_deaths")
int32_t cs16_player_get_deaths(int32_t slot);

CS16_IMPORT("player_set_score")
int32_t cs16_player_set_score(int32_t slot, int32_t frags, int32_t deaths);

CS16_IMPORT("player_get_team")
int32_t cs16_player_get_team(int32_t slot);

CS16_IMPORT("player_set_team")
int32_t cs16_player_set_team(int32_t slot, int32_t team, int32_t model_class);

CS16_IMPORT("player_is_alive")
int32_t cs16_player_is_alive(int32_t slot);

CS16_IMPORT("player_respawn")
int32_t cs16_player_respawn(int32_t slot);

CS16_IMPORT("player_slap")
int32_t cs16_player_slap(int32_t slot, int32_t power, int32_t random_dir);

CS16_IMPORT("player_slay")
int32_t cs16_player_slay(int32_t slot, int32_t preserve_frags);

CS16_IMPORT("player_damage")
int32_t cs16_player_damage(int32_t victim, int32_t attacker, float damage, int32_t bits);

CS16_IMPORT("player_set_godmode")
int32_t cs16_player_set_godmode(int32_t slot, int32_t enabled);

CS16_IMPORT("player_set_noclip")
int32_t cs16_player_set_noclip(int32_t slot, int32_t enabled);

CS16_IMPORT("player_set_maxspeed")
int32_t cs16_player_set_maxspeed(int32_t slot, float maxspeed);

CS16_IMPORT("player_set_gravity")
int32_t cs16_player_set_gravity(int32_t slot, float gravity);

CS16_IMPORT("player_set_footsteps")
int32_t cs16_player_set_footsteps(int32_t slot, int32_t silent);

CS16_IMPORT("player_set_rendering")
int32_t cs16_player_set_rendering(int32_t slot, int32_t fx, int32_t rgb_packed, int32_t mode, int32_t amount);

CS16_IMPORT("player_set_model")
int32_t cs16_player_set_model(int32_t slot, const char *ptr, int32_t len);

CS16_IMPORT("player_reset_model")
int32_t cs16_player_reset_model(int32_t slot);

CS16_IMPORT("player_get_ip")
int32_t cs16_player_get_ip(int32_t slot, char *out_ptr, int32_t cap);

CS16_IMPORT("player_get_steamid")
int32_t cs16_player_get_steamid(int32_t slot, char *out_ptr, int32_t cap);

CS16_IMPORT("player_get_ping")
int32_t cs16_player_get_ping(int32_t slot);

CS16_IMPORT("player_get_loss")
int32_t cs16_player_get_loss(int32_t slot);

CS16_IMPORT("player_get_aim_origin")
int32_t cs16_player_get_aim_origin(int32_t slot, float *out_xyz);

CS16_IMPORT("player_give_item")
int32_t cs16_player_give_item(int32_t slot, const char *ptr, int32_t len);

CS16_IMPORT("player_strip_weapons")
int32_t cs16_player_strip_weapons(int32_t slot);

CS16_IMPORT("player_get_weapon_clip")
int32_t cs16_player_get_weapon_clip(int32_t slot, int32_t weapon_id);

CS16_IMPORT("player_set_weapon_clip")
int32_t cs16_player_set_weapon_clip(int32_t slot, int32_t weapon_id, int32_t clip);

CS16_IMPORT("player_get_bpammo")
int32_t cs16_player_get_bpammo(int32_t slot, int32_t weapon_id);

CS16_IMPORT("player_set_bpammo")
int32_t cs16_player_set_bpammo(int32_t slot, int32_t weapon_id, int32_t amount);

CS16_IMPORT("player_set_defuser")
int32_t cs16_player_set_defuser(int32_t slot, int32_t has);

CS16_IMPORT("player_set_nvg")
int32_t cs16_player_set_nvg(int32_t slot, int32_t has);

CS16_IMPORT("player_drop_slot")
int32_t cs16_player_drop_slot(int32_t slot, int32_t inv_slot);

CS16_IMPORT("register_custom_weapon")
int32_t cs16_register_custom_weapon(const char *spec_ptr, int32_t spec_len);

CS16_IMPORT("give_custom_weapon")
int32_t cs16_give_custom_weapon(int32_t slot, const char *id_ptr, int32_t id_len);

/* ------------------------------------------------------------------------- */
/* Host Imports: ABI v2 VFX Primitives (`TE_*`)                              */
/* ------------------------------------------------------------------------- */

CS16_IMPORT("vfx_shockwave_ring")
int32_t cs16_vfx_shockwave_ring(
    float x, float y, float z,
    float radius, int32_t rgba, int32_t life, int32_t width, int32_t slot
);

CS16_IMPORT("vfx_laser_beam")
int32_t cs16_vfx_laser_beam(
    float x1, float y1, float z1,
    float x2, float y2, float z2,
    int32_t rgba, int32_t life, int32_t width, int32_t noise, int32_t slot
);

CS16_IMPORT("vfx_follow_trail")
int32_t cs16_vfx_follow_trail(int32_t ent_idx, int32_t rgba, int32_t life, int32_t width, int32_t slot);

CS16_IMPORT("vfx_kill_beam")
int32_t cs16_vfx_kill_beam(int32_t ent_idx, int32_t slot);

CS16_IMPORT("vfx_aura_light")
int32_t cs16_vfx_aura_light(
    float x, float y, float z,
    int32_t radius, int32_t rgb, int32_t life, int32_t decay, int32_t slot
);

CS16_IMPORT("vfx_sparks")
int32_t cs16_vfx_sparks(float x, float y, float z, int32_t slot);

CS16_IMPORT("vfx_implosion")
int32_t cs16_vfx_implosion(
    float x, float y, float z,
    int32_t radius, int32_t count, int32_t life, int32_t slot
);

CS16_IMPORT("vfx_teleport_splash")
int32_t cs16_vfx_teleport_splash(float x, float y, float z, int32_t slot);

CS16_IMPORT("vfx_lava_splash")
int32_t cs16_vfx_lava_splash(float x, float y, float z, int32_t slot);

CS16_IMPORT("vfx_explosion")
int32_t cs16_vfx_explosion(float x, float y, float z, int32_t scale, int32_t flags, int32_t slot);

CS16_IMPORT("vfx_blood_stream")
int32_t cs16_vfx_blood_stream(
    float x, float y, float z,
    float dx, float dy, float dz,
    int32_t color, int32_t speed, int32_t slot
);

CS16_IMPORT("vfx_world_decal")
int32_t cs16_vfx_world_decal(
    float x, float y, float z,
    const char *name_ptr, int32_t name_len, int32_t slot
);

/* ------------------------------------------------------------------------- */
/* Host Imports: ABI v2 Paginated MenuManager & HUD/DHUD/MOTD/Screen Effects */
/* ------------------------------------------------------------------------- */

CS16_IMPORT("menu_create")
int32_t cs16_menu_create(const char *title_ptr, int32_t title_len, int32_t cb_id);

CS16_IMPORT("menu_add_item")
int32_t cs16_menu_add_item(
    int32_t menu_id,
    const char *label_ptr, int32_t label_len,
    const char *info_ptr, int32_t info_len,
    int32_t enabled
);

CS16_IMPORT("menu_add_blank")
int32_t cs16_menu_add_blank(int32_t menu_id);

CS16_IMPORT("menu_add_text")
int32_t cs16_menu_add_text(int32_t menu_id, const char *ptr, int32_t len);

CS16_IMPORT("menu_set_prop_int")
int32_t cs16_menu_set_prop_int(int32_t menu_id, int32_t prop, int32_t val);

CS16_IMPORT("menu_set_prop_str")
int32_t cs16_menu_set_prop_str(int32_t menu_id, int32_t prop, const char *val_ptr, int32_t val_len);

CS16_IMPORT("menu_display")
int32_t cs16_menu_display(int32_t slot, int32_t menu_id, int32_t page, int32_t time_sec);

CS16_IMPORT("menu_destroy")
int32_t cs16_menu_destroy(int32_t menu_id);

CS16_IMPORT("hud_message")
int32_t cs16_hud_message(
    int32_t slot, int32_t channel_effect,
    float x, float y,
    int32_t rgba1, int32_t rgba2,
    float fadein, float hold, float fadeout, float fxtime,
    const char *ptr, int32_t len
);

CS16_IMPORT("dhud_message")
int32_t cs16_dhud_message(
    int32_t slot, int32_t rgb_effect,
    float x, float y,
    float fadein, float hold, float fadeout, float fxtime,
    const char *ptr, int32_t len
);

CS16_IMPORT("show_motd")
int32_t cs16_show_motd(
    int32_t slot,
    const char *title_ptr, int32_t title_len,
    const char *body_ptr, int32_t body_len
);

CS16_IMPORT("screen_fade")
int32_t cs16_screen_fade(int32_t slot, float duration_sec, float hold_sec, int32_t flags, int32_t rgba);

CS16_IMPORT("screen_shake")
int32_t cs16_screen_shake(int32_t slot, float amplitude, float duration_sec, float frequency);

CS16_IMPORT("bar_time")
int32_t cs16_bar_time(int32_t slot, int32_t seconds);

/* ------------------------------------------------------------------------- */
/* Host Imports: ABI v2 i18n, Admins, RPC, NVault, Leaderboards, Webhooks    */
/* ------------------------------------------------------------------------- */

CS16_IMPORT("lang_load_toml")
int32_t cs16_lang_load_toml(const char *ptr, int32_t len);

CS16_IMPORT("lang_load_amxx")
int32_t cs16_lang_load_amxx(const char *ptr, int32_t len);

CS16_IMPORT("lang_tr")
int32_t cs16_lang_tr(
    int32_t slot,
    const char *key_ptr, int32_t key_len,
    const char *args_ptr, int32_t args_len,
    char *out_ptr, int32_t cap
);

CS16_IMPORT("lang_player_code")
int32_t cs16_lang_player_code(int32_t slot, char *out_ptr, int32_t cap);

CS16_IMPORT("lang_set_player_code")
int32_t cs16_lang_set_player_code(int32_t slot, const char *ptr, int32_t len);

CS16_IMPORT("admin_player_flags")
int32_t cs16_admin_player_flags(int32_t slot);

CS16_IMPORT("admin_has_flag")
int32_t cs16_admin_has_flag(int32_t slot, int32_t flag_char);

CS16_IMPORT("rpc_export")
int32_t cs16_rpc_export(const char *name_ptr, int32_t name_len, int32_t cb_id);

CS16_IMPORT("rpc_call")
int32_t cs16_rpc_call(
    const char *name_ptr, int32_t name_len,
    int32_t arg_i32,
    const char *arg_ptr, int32_t arg_len,
    char *out_ptr, int32_t out_cap
);

CS16_IMPORT("rpc_subscribe")
int32_t cs16_rpc_subscribe(const char *ev_ptr, int32_t ev_len, int32_t cb_id);

CS16_IMPORT("rpc_emit_event")
int32_t cs16_rpc_emit_event(
    const char *ev_ptr, int32_t ev_len,
    const char *payload_ptr, int32_t payload_len
);

CS16_IMPORT("nvault_open")
int32_t cs16_nvault_open(const char *name_ptr, int32_t name_len);

CS16_IMPORT("nvault_get")
int32_t cs16_nvault_get(int32_t handle, const char *key_ptr, int32_t key_len, char *out_ptr, int32_t cap);

CS16_IMPORT("nvault_set")
int32_t cs16_nvault_set(
    int32_t handle,
    const char *key_ptr, int32_t key_len,
    const char *val_ptr, int32_t val_len
);

CS16_IMPORT("nvault_remove")
int32_t cs16_nvault_remove(int32_t handle, const char *key_ptr, int32_t key_len);

CS16_IMPORT("leaderboard_upsert")
int32_t cs16_leaderboard_upsert(
    const char *table_ptr, int32_t table_len,
    const char *key_ptr, int32_t key_len,
    double score,
    const char *payload_ptr, int32_t payload_len
);

CS16_IMPORT("leaderboard_rank")
int32_t cs16_leaderboard_rank(
    const char *table_ptr, int32_t table_len,
    const char *key_ptr, int32_t key_len,
    char *out_ptr, int32_t out_cap
);

CS16_IMPORT("leaderboard_top")
int32_t cs16_leaderboard_top(
    const char *table_ptr, int32_t table_len,
    int32_t offset, int32_t limit,
    char *out_ptr, int32_t out_cap
);

CS16_IMPORT("http_post_json")
int32_t cs16_http_post_json(
    const char *url_ptr, int32_t url_len,
    const char *body_ptr, int32_t body_len,
    int32_t cb_id
);

#ifdef __cplusplus
}
#endif

#endif /* CS16_MOD_H */
