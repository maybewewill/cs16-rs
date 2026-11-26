//! Zero-`unsafe` macro generators emitting the `wasm32` `"cs16_mod"` host import bindings,
//! `WasmHostBridge` implementation, and ABI v2 `cs_mod_*` exports inside standalone guest `.wasm`
//! mod crates.
//!
//! Why macros? In Rust edition 2024, `extern "C"` import blocks and `#[unsafe(no_mangle)]`
//! require explicit `unsafe` syntax which triggers `#![forbid(unsafe_code)]` on workspace crates.
//! Defining the bindings and exports in `macro_rules!` keeps `cs-mod-sdk` 100%
//! `#![forbid(unsafe_code)]` across all workspace targets (including `wasm32-unknown-unknown`)
//! while allowing external `cdylib` guest mod crates (`mods/*`) to wire the exact `cs16_mod` ABI
//! v2 imports and exports cleanly via `cs_mod_sdk::cs16_wasm_host_bridge!();` and
//! `cs_mod_sdk::cs16_wasm_exports! { ... }`.

/// Materializes `mod raw` (all `"cs16_mod"` ABI v1 + v2 WASM imports matching `WasmiRuntime`)
/// and `pub struct WasmHostBridge` implementing [`crate::HostBridge`] inside a guest `wasm32` mod.
#[macro_export]
macro_rules! cs16_wasm_host_bridge {
    () => {
        #[cfg(target_arch = "wasm32")]
        #[allow(dead_code, clippy::too_many_arguments)]
        pub mod raw {
            #[link(wasm_import_module = "cs16_mod")]
            unsafe extern "C" {
                // Lifecycle & hook context
                pub safe fn hook_register(hook_id: i32, phase: i32, priority: i32) -> i32;
                pub safe fn host_time_ms() -> i64;
                pub safe fn hook_arg_i32(idx: i32) -> i32;
                pub safe fn hook_arg_f32(idx: i32) -> f32;
                pub safe fn hook_arg_str(idx: i32, ptr: i32, cap: i32) -> i32;
                pub safe fn hook_arg_bytes(ptr: i32, cap: i32) -> i32;
                pub safe fn hook_set_override_i32(val: i32) -> i32;
                pub safe fn hook_set_override_i32_2(val: i32) -> i32;
                pub safe fn hook_set_override_f32(val: f32) -> i32;
                pub safe fn hook_set_override_str(ptr: i32, len: i32) -> i32;
                pub safe fn rpc_set_return_str(ptr: i32, len: i32) -> i32;

                // Entities & basic players (v1)
                pub safe fn ent_count() -> i32;
                pub safe fn ent_create(cls_ptr: i32, cls_len: i32) -> i32;
                pub safe fn ent_remove(idx: i32) -> i32;
                pub safe fn ent_get_health(idx: i32) -> f32;
                pub safe fn ent_set_health(idx: i32, health: f32) -> i32;
                pub safe fn ent_get_origin(idx: i32, out_ptr: i32) -> i32;
                pub safe fn ent_set_origin(idx: i32, x: f32, y: f32, z: f32) -> i32;
                pub safe fn ent_get_velocity(idx: i32, out_ptr: i32) -> i32;
                pub safe fn ent_set_velocity(idx: i32, x: f32, y: f32, z: f32) -> i32;
                pub safe fn ent_get_flags(idx: i32) -> i32;
                pub safe fn ent_set_flags(idx: i32, flags: i32) -> i32;
                pub safe fn ent_classname(idx: i32, out_ptr: i32, cap: i32) -> i32;
                pub safe fn player_max_clients() -> i32;
                pub safe fn player_is_active(slot: i32) -> i32;
                pub safe fn player_name(slot: i32, out_ptr: i32, cap: i32) -> i32;
                pub safe fn player_kick(slot: i32, ptr: i32, len: i32) -> i32;

                // Messages (v1)
                pub safe fn print_client(slot: i32, ptr: i32, len: i32) -> i32;
                pub safe fn print_all(ptr: i32, len: i32) -> i32;
                pub safe fn center_print(slot: i32, ptr: i32, len: i32) -> i32;
                pub safe fn say_text(slot: i32, ptr: i32, len: i32) -> i32;
                pub safe fn send_custom_usermsg(
                    slot: i32,
                    name_ptr: i32,
                    name_len: i32,
                    data_ptr: i32,
                    data_len: i32,
                ) -> i32;

                // CVars & commands (v1)
                pub safe fn cvar_get_f32(ptr: i32, len: i32) -> f32;
                pub safe fn cvar_set_f32(ptr: i32, len: i32, val: f32) -> i32;
                pub safe fn cvar_get_str(ptr: i32, len: i32, out_ptr: i32, cap: i32) -> i32;
                pub safe fn cvar_set_str(ptr: i32, len: i32, val_ptr: i32, val_len: i32) -> i32;
                pub safe fn register_command(ptr: i32, len: i32) -> i32;
                pub safe fn server_exec(ptr: i32, len: i32) -> i32;
                pub safe fn change_level(ptr: i32, len: i32) -> i32;

                // Raw menu, timers, traces, sound, storage, client HUD (v1)
                pub safe fn show_menu(slot: i32, keys: i32, time_sec: i32, ptr: i32, len: i32) -> i32;
                pub safe fn timer_after(delay_ms: i32, cb_id: i32, arg: i32) -> i32;
                pub safe fn timer_repeat(interval_ms: i32, cb_id: i32, arg: i32) -> i32;
                pub safe fn timer_cancel(timer_id: i32) -> i32;
                pub safe fn trace_line(x1: f32, y1: f32, z1: f32, x2: f32, y2: f32, z2: f32) -> f32;
                pub safe fn precache_sound(ptr: i32, len: i32) -> i32;
                pub safe fn emit_sound(ent_idx: i32, channel: i32, ptr: i32, len: i32, volume: f32) -> i32;
                pub safe fn storage_get(key_ptr: i32, key_len: i32, out_ptr: i32, cap: i32) -> i32;
                pub safe fn storage_set(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32;
                pub safe fn storage_remove(key_ptr: i32, key_len: i32) -> i32;
                pub safe fn hud_draw_rect(x: f32, y: f32, w: f32, h: f32, rgba: i32) -> i32;
                pub safe fn hud_draw_text(x: f32, y: f32, size: f32, rgba: i32, ptr: i32, len: i32) -> i32;
                pub safe fn hud_draw_bar(x: f32, y: f32, w: f32, h: f32, frac: f32, rgba: i32) -> i32;
                pub safe fn hud_panel(x: f32, y: f32, w: f32, h: f32, rgba: i32, ptr: i32, len: i32) -> i32;
                pub safe fn client_exec(ptr: i32, len: i32) -> i32;

                // ABI v2 player & weapon natives
                pub safe fn player_get_health(slot: i32) -> f32;
                pub safe fn player_set_health(slot: i32, health: f32) -> i32;
                pub safe fn player_set_max_health(slot: i32, max_health: f32) -> i32;
                pub safe fn player_get_armor(slot: i32) -> i32;
                pub safe fn player_get_armor_type(slot: i32) -> i32;
                pub safe fn player_set_armor(slot: i32, armor: i32, kind: i32) -> i32;
                pub safe fn player_get_money(slot: i32) -> i32;
                pub safe fn player_set_money(slot: i32, amount: i32, flash: i32) -> i32;
                pub safe fn player_add_money(slot: i32, delta: i32, flash: i32) -> i32;
                pub safe fn player_get_frags(slot: i32) -> i32;
                pub safe fn player_get_deaths(slot: i32) -> i32;
                pub safe fn player_set_score(slot: i32, frags: i32, deaths: i32) -> i32;
                pub safe fn player_get_team(slot: i32) -> i32;
                pub safe fn player_set_team(slot: i32, team: i32, model_class: i32) -> i32;
                pub safe fn player_is_alive(slot: i32) -> i32;
                pub safe fn player_respawn(slot: i32) -> i32;
                pub safe fn player_slap(slot: i32, power: i32, random_dir: i32) -> i32;
                pub safe fn player_slay(slot: i32, preserve_frags: i32) -> i32;
                pub safe fn player_damage(victim: i32, attacker: i32, damage: f32, bits: i32) -> i32;
                pub safe fn player_set_godmode(slot: i32, enabled: i32) -> i32;
                pub safe fn player_set_noclip(slot: i32, enabled: i32) -> i32;
                pub safe fn player_set_maxspeed(slot: i32, maxspeed: f32) -> i32;
                pub safe fn player_set_gravity(slot: i32, gravity: f32) -> i32;
                pub safe fn player_set_footsteps(slot: i32, silent: i32) -> i32;
                pub safe fn player_set_rendering(
                    slot: i32,
                    fx: i32,
                    rgb_packed: i32,
                    mode: i32,
                    amount: i32,
                ) -> i32;
                pub safe fn player_set_model(slot: i32, ptr: i32, len: i32) -> i32;
                pub safe fn player_reset_model(slot: i32) -> i32;
                pub safe fn player_get_ip(slot: i32, out_ptr: i32, cap: i32) -> i32;
                pub safe fn player_get_steamid(slot: i32, out_ptr: i32, cap: i32) -> i32;
                pub safe fn player_get_ping(slot: i32) -> i32;
                pub safe fn player_get_loss(slot: i32) -> i32;
                pub safe fn player_get_aim_origin(slot: i32, out_ptr: i32) -> i32;
                pub safe fn player_give_item(slot: i32, ptr: i32, len: i32) -> i32;
                pub safe fn player_strip_weapons(slot: i32) -> i32;
                pub safe fn player_get_weapon_clip(slot: i32, weapon_id: i32) -> i32;
                pub safe fn player_set_weapon_clip(slot: i32, weapon_id: i32, clip: i32) -> i32;
                pub safe fn player_get_bpammo(slot: i32, weapon_id: i32) -> i32;
                pub safe fn player_set_bpammo(slot: i32, weapon_id: i32, amount: i32) -> i32;
                pub safe fn player_set_defuser(slot: i32, has: i32) -> i32;
                pub safe fn player_set_nvg(slot: i32, has: i32) -> i32;
                pub safe fn player_drop_slot(slot: i32, inv_slot: i32) -> i32;
                pub safe fn register_custom_weapon(spec_ptr: i32, spec_len: i32) -> i32;
                pub safe fn give_custom_weapon(slot: i32, id_ptr: i32, id_len: i32) -> i32;

                // ABI v2 VFX (`TE_*`)
                pub safe fn vfx_shockwave_ring(
                    x: f32,
                    y: f32,
                    z: f32,
                    radius: f32,
                    rgba: i32,
                    life: i32,
                    width: i32,
                    slot: i32,
                ) -> i32;
                pub safe fn vfx_laser_beam(
                    x1: f32,
                    y1: f32,
                    z1: f32,
                    x2: f32,
                    y2: f32,
                    z2: f32,
                    rgba: i32,
                    life: i32,
                    width: i32,
                    noise: i32,
                    slot: i32,
                ) -> i32;
                pub safe fn vfx_follow_trail(
                    ent_idx: i32,
                    rgba: i32,
                    life: i32,
                    width: i32,
                    slot: i32,
                ) -> i32;
                pub safe fn vfx_kill_beam(ent_idx: i32, slot: i32) -> i32;
                pub safe fn vfx_aura_light(
                    x: f32,
                    y: f32,
                    z: f32,
                    radius: i32,
                    rgb: i32,
                    life: i32,
                    decay: i32,
                    slot: i32,
                ) -> i32;
                pub safe fn vfx_sparks(x: f32, y: f32, z: f32, slot: i32) -> i32;
                pub safe fn vfx_implosion(
                    x: f32,
                    y: f32,
                    z: f32,
                    radius: i32,
                    count: i32,
                    life: i32,
                    slot: i32,
                ) -> i32;
                pub safe fn vfx_teleport_splash(x: f32, y: f32, z: f32, slot: i32) -> i32;
                pub safe fn vfx_lava_splash(x: f32, y: f32, z: f32, slot: i32) -> i32;
                pub safe fn vfx_explosion(
                    x: f32,
                    y: f32,
                    z: f32,
                    scale: i32,
                    flags: i32,
                    slot: i32,
                ) -> i32;
                pub safe fn vfx_blood_stream(
                    x: f32,
                    y: f32,
                    z: f32,
                    dx: f32,
                    dy: f32,
                    dz: f32,
                    color: i32,
                    speed: i32,
                    slot: i32,
                ) -> i32;
                pub safe fn vfx_world_decal(
                    x: f32,
                    y: f32,
                    z: f32,
                    name_ptr: i32,
                    name_len: i32,
                    slot: i32,
                ) -> i32;

                // ABI v2 Paginated menus
                pub safe fn menu_create(title_ptr: i32, title_len: i32, cb_id: i32) -> i32;
                pub safe fn menu_add_item(
                    menu_id: i32,
                    label_ptr: i32,
                    label_len: i32,
                    info_ptr: i32,
                    info_len: i32,
                    enabled: i32,
                ) -> i32;
                pub safe fn menu_add_blank(menu_id: i32) -> i32;
                pub safe fn menu_add_text(menu_id: i32, ptr: i32, len: i32) -> i32;
                pub safe fn menu_set_prop_int(menu_id: i32, prop: i32, val: i32) -> i32;
                pub safe fn menu_set_prop_str(
                    menu_id: i32,
                    prop: i32,
                    val_ptr: i32,
                    val_len: i32,
                ) -> i32;
                pub safe fn menu_display(slot: i32, menu_id: i32, page: i32, time_sec: i32) -> i32;
                pub safe fn menu_destroy(menu_id: i32) -> i32;

                // ABI v2 HUD / DHUD / MOTD / Screen Effects
                pub safe fn hud_message(
                    slot: i32,
                    channel_effect: i32,
                    x: f32,
                    y: f32,
                    rgba1: i32,
                    rgba2: i32,
                    fadein: f32,
                    hold: f32,
                    fadeout: f32,
                    fxtime: f32,
                    ptr: i32,
                    len: i32,
                ) -> i32;
                pub safe fn dhud_message(
                    slot: i32,
                    rgb_effect: i32,
                    x: f32,
                    y: f32,
                    fadein: f32,
                    hold: f32,
                    fadeout: f32,
                    fxtime: f32,
                    ptr: i32,
                    len: i32,
                ) -> i32;
                pub safe fn show_motd(
                    slot: i32,
                    title_ptr: i32,
                    title_len: i32,
                    body_ptr: i32,
                    body_len: i32,
                ) -> i32;
                pub safe fn screen_fade(
                    slot: i32,
                    duration_sec: f32,
                    hold_sec: f32,
                    flags: i32,
                    rgba: i32,
                ) -> i32;
                pub safe fn screen_shake(
                    slot: i32,
                    amplitude: f32,
                    duration_sec: f32,
                    frequency: f32,
                ) -> i32;
                pub safe fn bar_time(slot: i32, seconds: i32) -> i32;

                // ABI v2 Platform Services (i18n, Admins, RPC, NVault, Leaderboards, Webhooks)
                pub safe fn lang_load_toml(ptr: i32, len: i32) -> i32;
                pub safe fn lang_load_amxx(ptr: i32, len: i32) -> i32;
                pub safe fn lang_tr(
                    slot: i32,
                    key_ptr: i32,
                    key_len: i32,
                    args_ptr: i32,
                    args_len: i32,
                    out_ptr: i32,
                    cap: i32,
                ) -> i32;
                pub safe fn lang_player_code(slot: i32, out_ptr: i32, cap: i32) -> i32;
                pub safe fn lang_set_player_code(slot: i32, ptr: i32, len: i32) -> i32;
                pub safe fn admin_player_flags(slot: i32) -> i32;
                pub safe fn admin_has_flag(slot: i32, flag_char: i32) -> i32;
                pub safe fn rpc_export(name_ptr: i32, name_len: i32, cb_id: i32) -> i32;
                pub safe fn rpc_call(
                    name_ptr: i32,
                    name_len: i32,
                    arg_i32: i32,
                    arg_ptr: i32,
                    arg_len: i32,
                    out_ptr: i32,
                    out_cap: i32,
                ) -> i32;
                pub safe fn rpc_subscribe(ev_ptr: i32, ev_len: i32, cb_id: i32) -> i32;
                pub safe fn rpc_emit_event(
                    ev_ptr: i32,
                    ev_len: i32,
                    payload_ptr: i32,
                    payload_len: i32,
                ) -> i32;
                pub safe fn nvault_open(name_ptr: i32, name_len: i32) -> i32;
                pub safe fn nvault_get(
                    handle: i32,
                    key_ptr: i32,
                    key_len: i32,
                    out_ptr: i32,
                    cap: i32,
                ) -> i32;
                pub safe fn nvault_set(
                    handle: i32,
                    key_ptr: i32,
                    key_len: i32,
                    val_ptr: i32,
                    val_len: i32,
                ) -> i32;
                pub safe fn nvault_remove(handle: i32, key_ptr: i32, key_len: i32) -> i32;
                pub safe fn leaderboard_upsert(
                    table_ptr: i32,
                    table_len: i32,
                    key_ptr: i32,
                    key_len: i32,
                    score: f64,
                    payload_ptr: i32,
                    payload_len: i32,
                ) -> i32;
                pub safe fn leaderboard_rank(
                    table_ptr: i32,
                    table_len: i32,
                    key_ptr: i32,
                    key_len: i32,
                    out_ptr: i32,
                    out_cap: i32,
                ) -> i32;
                pub safe fn leaderboard_top(
                    table_ptr: i32,
                    table_len: i32,
                    offset: i32,
                    limit: i32,
                    out_ptr: i32,
                    out_cap: i32,
                ) -> i32;
                pub safe fn http_post_json(
                    url_ptr: i32,
                    url_len: i32,
                    body_ptr: i32,
                    body_len: i32,
                    cb_id: i32,
                ) -> i32;
            }
        }

        /// Real `wasm32` [`$crate::HostBridge`] calling directly into the `"cs16_mod"` imports.
        #[cfg(target_arch = "wasm32")]
        #[derive(Debug, Default, Clone, Copy)]
        pub struct WasmHostBridge;

        #[cfg(target_arch = "wasm32")]
        impl WasmHostBridge {
            #[inline]
            fn opt_slot(slot: Option<u8>) -> i32 {
                slot.map_or(-1, i32::from)
            }

            fn read_str_with(cap: usize, f: impl FnOnce(i32, i32) -> i32) -> Option<String> {
                let mut buf = vec![0u8; cap];
                let n = f(buf.as_mut_ptr() as i32, buf.len() as i32);
                if n < 0 {
                    return None;
                }
                let len = (n as usize).min(buf.len());
                buf.truncate(len);
                String::from_utf8(buf).ok()
            }

            fn read_vec3_with(f: impl FnOnce(i32) -> i32) -> Option<[f32; 3]> {
                let mut raw = [0u8; 12];
                if f(raw.as_mut_ptr() as i32) < 0 {
                    return None;
                }
                Some([
                    f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
                    f32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]),
                    f32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]),
                ])
            }
        }

        #[cfg(target_arch = "wasm32")]
        impl $crate::HostBridge for WasmHostBridge {
            fn time_ms(&self) -> i64 {
                raw::host_time_ms()
            }

            fn print_all(&mut self, text: &str) {
                let b = text.as_bytes();
                let _ = raw::print_all(b.as_ptr() as i32, b.len() as i32);
            }

            fn center_print(&mut self, slot: Option<u8>, text: &str) {
                let b = text.as_bytes();
                let _ = raw::center_print(Self::opt_slot(slot), b.as_ptr() as i32, b.len() as i32);
            }

            fn say_text(&mut self, slot: Option<u8>, text: &str) {
                let b = text.as_bytes();
                let _ = raw::say_text(Self::opt_slot(slot), b.as_ptr() as i32, b.len() as i32);
            }

            fn show_menu(&mut self, slot: u8, keys_mask: u16, time_sec: i8, text: &str) {
                let b = text.as_bytes();
                let _ = raw::show_menu(
                    i32::from(slot),
                    i32::from(keys_mask),
                    i32::from(time_sec),
                    b.as_ptr() as i32,
                    b.len() as i32,
                );
            }

            fn set_health(&mut self, ent_index: u16, health: f32) {
                let _ = raw::ent_set_health(i32::from(ent_index), health);
            }

            fn storage_get(&self, key: &str) -> Option<String> {
                let kb = key.as_bytes();
                Self::read_str_with(4096, |p, c| {
                    raw::storage_get(kb.as_ptr() as i32, kb.len() as i32, p, c)
                })
            }

            fn storage_set(&mut self, key: &str, value: &str) {
                let kb = key.as_bytes();
                let vb = value.as_bytes();
                let _ = raw::storage_set(
                    kb.as_ptr() as i32,
                    kb.len() as i32,
                    vb.as_ptr() as i32,
                    vb.len() as i32,
                );
            }

            fn send_custom_usermsg(&mut self, slot: Option<u8>, name: &str, data: &[u8]) {
                let nb = name.as_bytes();
                let _ = raw::send_custom_usermsg(
                    Self::opt_slot(slot),
                    nb.as_ptr() as i32,
                    nb.len() as i32,
                    data.as_ptr() as i32,
                    data.len() as i32,
                );
            }

            fn hook_register(
                &mut self,
                hook: $crate::HookId,
                phase: $crate::HookPhase,
                priority: i32,
            ) -> i32 {
                raw::hook_register(hook as i32, phase as i32, priority)
            }

            fn hook_arg_i32(&self, idx: i32) -> i32 {
                raw::hook_arg_i32(idx)
            }

            fn hook_arg_f32(&self, idx: i32) -> f32 {
                raw::hook_arg_f32(idx)
            }

            fn hook_arg_str(&self, idx: i32) -> Option<String> {
                Self::read_str_with(4096, |p, c| raw::hook_arg_str(idx, p, c))
            }

            fn hook_arg_bytes(&self) -> Vec<u8> {
                let mut buf = vec![0u8; 4096];
                let n = raw::hook_arg_bytes(buf.as_mut_ptr() as i32, buf.len() as i32);
                if n <= 0 {
                    return Vec::new();
                }
                buf.truncate((n as usize).min(buf.len()));
                buf
            }

            fn hook_set_override_i32(&mut self, val: i32) -> i32 {
                raw::hook_set_override_i32(val)
            }

            fn hook_set_override_i32_2(&mut self, val: i32) -> i32 {
                raw::hook_set_override_i32_2(val)
            }

            fn hook_set_override_f32(&mut self, val: f32) -> i32 {
                raw::hook_set_override_f32(val)
            }

            fn hook_set_override_str(&mut self, val: &str) -> i32 {
                let b = val.as_bytes();
                raw::hook_set_override_str(b.as_ptr() as i32, b.len() as i32)
            }

            fn rpc_set_return_str(&mut self, val: &str) -> i32 {
                let b = val.as_bytes();
                raw::rpc_set_return_str(b.as_ptr() as i32, b.len() as i32)
            }

            fn ent_count(&self) -> i32 {
                raw::ent_count()
            }

            fn ent_create(&mut self, classname: &str) -> i32 {
                let b = classname.as_bytes();
                raw::ent_create(b.as_ptr() as i32, b.len() as i32)
            }

            fn ent_remove(&mut self, index: u16) -> i32 {
                raw::ent_remove(i32::from(index))
            }

            fn ent_get_health(&self, index: u16) -> f32 {
                raw::ent_get_health(i32::from(index))
            }

            fn ent_get_origin(&self, index: u16) -> Option<[f32; 3]> {
                Self::read_vec3_with(|p| raw::ent_get_origin(i32::from(index), p))
            }

            fn ent_set_origin(&mut self, index: u16, origin: [f32; 3]) -> i32 {
                raw::ent_set_origin(i32::from(index), origin[0], origin[1], origin[2])
            }

            fn ent_get_velocity(&self, index: u16) -> Option<[f32; 3]> {
                Self::read_vec3_with(|p| raw::ent_get_velocity(i32::from(index), p))
            }

            fn ent_set_velocity(&mut self, index: u16, velocity: [f32; 3]) -> i32 {
                raw::ent_set_velocity(i32::from(index), velocity[0], velocity[1], velocity[2])
            }

            fn ent_get_flags(&self, index: u16) -> i32 {
                raw::ent_get_flags(i32::from(index))
            }

            fn ent_set_flags(&mut self, index: u16, flags: u32) -> i32 {
                raw::ent_set_flags(i32::from(index), flags as i32)
            }

            fn ent_classname(&self, index: u16) -> Option<String> {
                Self::read_str_with(256, |p, c| raw::ent_classname(i32::from(index), p, c))
            }

            fn player_max_clients(&self) -> u8 {
                raw::player_max_clients().clamp(0, 32) as u8
            }

            fn player_is_active(&self, slot: u8) -> bool {
                raw::player_is_active(i32::from(slot)) != 0
            }

            fn player_name(&self, slot: u8) -> Option<String> {
                Self::read_str_with(128, |p, c| raw::player_name(i32::from(slot), p, c))
            }

            fn player_kick(&mut self, slot: u8, reason: &str) -> i32 {
                let b = reason.as_bytes();
                raw::player_kick(i32::from(slot), b.as_ptr() as i32, b.len() as i32)
            }

            fn print_client(&mut self, slot: u8, text: &str) -> i32 {
                let b = text.as_bytes();
                raw::print_client(i32::from(slot), b.as_ptr() as i32, b.len() as i32)
            }

            fn player_get_health(&self, slot: u8) -> f32 {
                raw::player_get_health(i32::from(slot))
            }

            fn player_set_health(&mut self, slot: u8, health: f32) -> i32 {
                raw::player_set_health(i32::from(slot), health)
            }

            fn player_set_max_health(&mut self, slot: u8, max_health: f32) -> i32 {
                raw::player_set_max_health(i32::from(slot), max_health)
            }

            fn player_get_armor(&self, slot: u8) -> i32 {
                raw::player_get_armor(i32::from(slot))
            }

            fn player_get_armor_type(&self, slot: u8) -> $crate::ArmorKind {
                $crate::ArmorKind::from_i32(raw::player_get_armor_type(i32::from(slot)))
            }

            fn player_set_armor(
                &mut self,
                slot: u8,
                armor: i32,
                kind: $crate::ArmorKind,
            ) -> i32 {
                raw::player_set_armor(i32::from(slot), armor, kind as i32)
            }

            fn player_get_money(&self, slot: u8) -> i32 {
                raw::player_get_money(i32::from(slot))
            }

            fn player_set_money(&mut self, slot: u8, amount: i32, flash: bool) -> i32 {
                raw::player_set_money(i32::from(slot), amount, i32::from(flash))
            }

            fn player_add_money(&mut self, slot: u8, delta: i32, flash: bool) -> i32 {
                raw::player_add_money(i32::from(slot), delta, i32::from(flash))
            }

            fn player_get_frags(&self, slot: u8) -> i32 {
                raw::player_get_frags(i32::from(slot))
            }

            fn player_get_deaths(&self, slot: u8) -> i32 {
                raw::player_get_deaths(i32::from(slot))
            }

            fn player_set_score(&mut self, slot: u8, frags: i32, deaths: i32) -> i32 {
                raw::player_set_score(i32::from(slot), frags, deaths)
            }

            fn player_get_team(&self, slot: u8) -> $crate::TeamKind {
                $crate::TeamKind::from_i32(raw::player_get_team(i32::from(slot)))
            }

            fn player_set_team(
                &mut self,
                slot: u8,
                team: $crate::TeamKind,
                model_class: Option<u8>,
            ) -> i32 {
                raw::player_set_team(
                    i32::from(slot),
                    team as i32,
                    model_class.map_or(-1, i32::from),
                )
            }

            fn player_is_alive(&self, slot: u8) -> bool {
                raw::player_is_alive(i32::from(slot)) != 0
            }

            fn player_respawn(&mut self, slot: u8) -> i32 {
                raw::player_respawn(i32::from(slot))
            }

            fn player_slap(&mut self, slot: u8, power: i32, random_dir: bool) -> i32 {
                raw::player_slap(i32::from(slot), power, i32::from(random_dir))
            }

            fn player_slay(&mut self, slot: u8, preserve_frags: bool) -> i32 {
                raw::player_slay(i32::from(slot), i32::from(preserve_frags))
            }

            fn player_damage(
                &mut self,
                victim: u8,
                attacker: Option<u8>,
                damage: f32,
                bits: u32,
            ) -> i32 {
                raw::player_damage(
                    i32::from(victim),
                    Self::opt_slot(attacker),
                    damage,
                    bits as i32,
                )
            }

            fn player_set_godmode(&mut self, slot: u8, enabled: bool) -> i32 {
                raw::player_set_godmode(i32::from(slot), i32::from(enabled))
            }

            fn player_set_noclip(&mut self, slot: u8, enabled: bool) -> i32 {
                raw::player_set_noclip(i32::from(slot), i32::from(enabled))
            }

            fn player_set_maxspeed(&mut self, slot: u8, maxspeed: f32) -> i32 {
                raw::player_set_maxspeed(i32::from(slot), maxspeed)
            }

            fn player_set_gravity(&mut self, slot: u8, gravity: f32) -> i32 {
                raw::player_set_gravity(i32::from(slot), gravity)
            }

            fn player_set_footsteps(&mut self, slot: u8, silent: bool) -> i32 {
                raw::player_set_footsteps(i32::from(slot), i32::from(silent))
            }

            fn player_set_rendering(
                &mut self,
                slot: u8,
                fx: u8,
                rgb: [u8; 3],
                mode: u8,
                amount: u8,
            ) -> i32 {
                raw::player_set_rendering(
                    i32::from(slot),
                    i32::from(fx),
                    $crate::pack_rgb(rgb),
                    i32::from(mode),
                    i32::from(amount),
                )
            }

            fn player_set_model(&mut self, slot: u8, model: &str) -> i32 {
                let b = model.as_bytes();
                raw::player_set_model(i32::from(slot), b.as_ptr() as i32, b.len() as i32)
            }

            fn player_reset_model(&mut self, slot: u8) -> i32 {
                raw::player_reset_model(i32::from(slot))
            }

            fn player_get_ip(&self, slot: u8) -> Option<String> {
                Self::read_str_with(64, |p, c| raw::player_get_ip(i32::from(slot), p, c))
            }

            fn player_get_steamid(&self, slot: u8) -> Option<String> {
                Self::read_str_with(64, |p, c| raw::player_get_steamid(i32::from(slot), p, c))
            }

            fn player_get_ping(&self, slot: u8) -> i32 {
                raw::player_get_ping(i32::from(slot))
            }

            fn player_get_loss(&self, slot: u8) -> i32 {
                raw::player_get_loss(i32::from(slot))
            }

            fn player_get_aim_origin(&self, slot: u8) -> Option<[f32; 3]> {
                Self::read_vec3_with(|p| raw::player_get_aim_origin(i32::from(slot), p))
            }

            fn player_give_item(&mut self, slot: u8, item: &str) -> i32 {
                let b = item.as_bytes();
                raw::player_give_item(i32::from(slot), b.as_ptr() as i32, b.len() as i32)
            }

            fn player_strip_weapons(&mut self, slot: u8) -> i32 {
                raw::player_strip_weapons(i32::from(slot))
            }

            fn player_get_weapon_clip(&self, slot: u8, weapon_id: u8) -> i32 {
                raw::player_get_weapon_clip(i32::from(slot), i32::from(weapon_id))
            }

            fn player_set_weapon_clip(&mut self, slot: u8, weapon_id: u8, clip: i32) -> i32 {
                raw::player_set_weapon_clip(i32::from(slot), i32::from(weapon_id), clip)
            }

            fn player_get_bpammo(&self, slot: u8, weapon_id: u8) -> i32 {
                raw::player_get_bpammo(i32::from(slot), i32::from(weapon_id))
            }

            fn player_set_bpammo(&mut self, slot: u8, weapon_id: u8, amount: i32) -> i32 {
                raw::player_set_bpammo(i32::from(slot), i32::from(weapon_id), amount)
            }

            fn player_set_defuser(&mut self, slot: u8, has: bool) -> i32 {
                raw::player_set_defuser(i32::from(slot), i32::from(has))
            }

            fn player_set_nvg(&mut self, slot: u8, has: bool) -> i32 {
                raw::player_set_nvg(i32::from(slot), i32::from(has))
            }

            fn player_drop_slot(&mut self, slot: u8, inv_slot: u8) -> i32 {
                raw::player_drop_slot(i32::from(slot), i32::from(inv_slot))
            }

            fn register_custom_weapon(&mut self, spec: &$crate::CustomWeaponSpec) -> i32 {
                let encoded = $crate::encode_custom_weapon_spec(spec);
                let b = encoded.as_bytes();
                raw::register_custom_weapon(b.as_ptr() as i32, b.len() as i32)
            }

            fn give_custom_weapon(&mut self, slot: u8, id_name: &str) -> i32 {
                let b = id_name.as_bytes();
                raw::give_custom_weapon(i32::from(slot), b.as_ptr() as i32, b.len() as i32)
            }

            fn cvar_get_f32(&self, name: &str) -> f32 {
                let b = name.as_bytes();
                raw::cvar_get_f32(b.as_ptr() as i32, b.len() as i32)
            }

            fn cvar_set_f32(&mut self, name: &str, val: f32) -> i32 {
                let b = name.as_bytes();
                raw::cvar_set_f32(b.as_ptr() as i32, b.len() as i32, val)
            }

            fn cvar_get_str(&self, name: &str) -> Option<String> {
                let b = name.as_bytes();
                Self::read_str_with(1024, |p, c| {
                    raw::cvar_get_str(b.as_ptr() as i32, b.len() as i32, p, c)
                })
            }

            fn cvar_set_str(&mut self, name: &str, val: &str) -> i32 {
                let nb = name.as_bytes();
                let vb = val.as_bytes();
                raw::cvar_set_str(
                    nb.as_ptr() as i32,
                    nb.len() as i32,
                    vb.as_ptr() as i32,
                    vb.len() as i32,
                )
            }

            fn register_command(&mut self, name: &str) -> i32 {
                let b = name.as_bytes();
                raw::register_command(b.as_ptr() as i32, b.len() as i32)
            }

            fn server_exec(&mut self, line: &str) -> i32 {
                let b = line.as_bytes();
                raw::server_exec(b.as_ptr() as i32, b.len() as i32)
            }

            fn change_level(&mut self, map: &str) -> i32 {
                let b = map.as_bytes();
                raw::change_level(b.as_ptr() as i32, b.len() as i32)
            }

            fn timer_after(&mut self, delay_ms: u32, cb_id: i32, arg: i32) -> i32 {
                raw::timer_after(delay_ms.min(i32::MAX as u32) as i32, cb_id, arg)
            }

            fn timer_repeat(&mut self, interval_ms: u32, cb_id: i32, arg: i32) -> i32 {
                raw::timer_repeat(interval_ms.min(i32::MAX as u32) as i32, cb_id, arg)
            }

            fn timer_cancel(&mut self, timer_id: u32) -> i32 {
                raw::timer_cancel(timer_id as i32)
            }

            fn trace_line(&self, start: [f32; 3], end: [f32; 3]) -> f32 {
                raw::trace_line(start[0], start[1], start[2], end[0], end[1], end[2])
            }

            fn precache_sound(&mut self, sample: &str) -> i32 {
                let b = sample.as_bytes();
                raw::precache_sound(b.as_ptr() as i32, b.len() as i32)
            }

            fn emit_sound(&mut self, ent_idx: u16, channel: u8, sample: &str, volume: f32) -> i32 {
                let b = sample.as_bytes();
                raw::emit_sound(
                    i32::from(ent_idx),
                    i32::from(channel),
                    b.as_ptr() as i32,
                    b.len() as i32,
                    volume,
                )
            }

            fn storage_remove(&mut self, key: &str) -> i32 {
                let b = key.as_bytes();
                raw::storage_remove(b.as_ptr() as i32, b.len() as i32)
            }

            fn emit_vfx(&mut self, vfx: $crate::VfxOp) -> i32 {
                match vfx {
                    $crate::VfxOp::ShockwaveRing {
                        origin,
                        radius,
                        rgba,
                        life_decisec,
                        width,
                        slot,
                    } => raw::vfx_shockwave_ring(
                        origin[0],
                        origin[1],
                        origin[2],
                        radius,
                        $crate::pack_rgba(rgba),
                        i32::from(life_decisec),
                        i32::from(width),
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::LaserBeam {
                        start,
                        end,
                        rgba,
                        life_decisec,
                        width,
                        noise,
                        slot,
                    } => raw::vfx_laser_beam(
                        start[0],
                        start[1],
                        start[2],
                        end[0],
                        end[1],
                        end[2],
                        $crate::pack_rgba(rgba),
                        i32::from(life_decisec),
                        i32::from(width),
                        i32::from(noise),
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::FollowTrail {
                        ent_index,
                        rgba,
                        life_decisec,
                        width,
                        slot,
                    } => raw::vfx_follow_trail(
                        i32::from(ent_index),
                        $crate::pack_rgba(rgba),
                        i32::from(life_decisec),
                        i32::from(width),
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::KillBeam { ent_index, slot } => {
                        raw::vfx_kill_beam(i32::from(ent_index), Self::opt_slot(slot))
                    }
                    $crate::VfxOp::AuraLight {
                        origin,
                        radius,
                        rgb,
                        life_decisec,
                        decay,
                        slot,
                    } => raw::vfx_aura_light(
                        origin[0],
                        origin[1],
                        origin[2],
                        i32::from(radius),
                        $crate::pack_rgb(rgb),
                        i32::from(life_decisec),
                        i32::from(decay),
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::Sparks { origin, slot } => {
                        raw::vfx_sparks(origin[0], origin[1], origin[2], Self::opt_slot(slot))
                    }
                    $crate::VfxOp::Implosion {
                        origin,
                        radius,
                        count,
                        life_decisec,
                        slot,
                    } => raw::vfx_implosion(
                        origin[0],
                        origin[1],
                        origin[2],
                        i32::from(radius),
                        i32::from(count),
                        i32::from(life_decisec),
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::TeleportSplash { origin, slot } => raw::vfx_teleport_splash(
                        origin[0],
                        origin[1],
                        origin[2],
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::LavaSplash { origin, slot } => {
                        raw::vfx_lava_splash(origin[0], origin[1], origin[2], Self::opt_slot(slot))
                    }
                    $crate::VfxOp::Explosion {
                        origin,
                        scale,
                        flags,
                        slot,
                    } => raw::vfx_explosion(
                        origin[0],
                        origin[1],
                        origin[2],
                        i32::from(scale),
                        i32::from(flags),
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::BloodStream {
                        origin,
                        dir,
                        color,
                        speed,
                        slot,
                    } => raw::vfx_blood_stream(
                        origin[0],
                        origin[1],
                        origin[2],
                        dir[0],
                        dir[1],
                        dir[2],
                        i32::from(color),
                        i32::from(speed),
                        Self::opt_slot(slot),
                    ),
                    $crate::VfxOp::WorldDecal {
                        origin,
                        decal_name,
                        slot,
                    } => {
                        let b = decal_name.as_bytes();
                        raw::vfx_world_decal(
                            origin[0],
                            origin[1],
                            origin[2],
                            b.as_ptr() as i32,
                            b.len() as i32,
                            Self::opt_slot(slot),
                        )
                    }
                }
            }

            fn menu_create(&mut self, title: &str, cb_id: i32) -> i32 {
                let b = title.as_bytes();
                raw::menu_create(b.as_ptr() as i32, b.len() as i32, cb_id)
            }

            fn menu_add_item(&mut self, menu_id: u32, label: &str, info: &str, enabled: bool) -> i32 {
                let lb = label.as_bytes();
                let ib = info.as_bytes();
                raw::menu_add_item(
                    menu_id as i32,
                    lb.as_ptr() as i32,
                    lb.len() as i32,
                    ib.as_ptr() as i32,
                    ib.len() as i32,
                    i32::from(enabled),
                )
            }

            fn menu_add_blank(&mut self, menu_id: u32) -> i32 {
                raw::menu_add_blank(menu_id as i32)
            }

            fn menu_add_text(&mut self, menu_id: u32, text: &str) -> i32 {
                let b = text.as_bytes();
                raw::menu_add_text(menu_id as i32, b.as_ptr() as i32, b.len() as i32)
            }

            fn menu_set_prop_int(&mut self, menu_id: u32, prop: i32, val: i32) -> i32 {
                raw::menu_set_prop_int(menu_id as i32, prop, val)
            }

            fn menu_set_prop_str(&mut self, menu_id: u32, prop: i32, val: &str) -> i32 {
                let b = val.as_bytes();
                raw::menu_set_prop_str(menu_id as i32, prop, b.as_ptr() as i32, b.len() as i32)
            }

            fn menu_display(&mut self, slot: u8, menu_id: u32, page: u16, time_sec: i8) -> i32 {
                raw::menu_display(
                    i32::from(slot),
                    menu_id as i32,
                    i32::from(page),
                    i32::from(time_sec),
                )
            }

            fn menu_destroy(&mut self, menu_id: u32) -> i32 {
                raw::menu_destroy(menu_id as i32)
            }

            fn hud_message(&mut self, spec: &$crate::HudMessageSpec) -> i32 {
                let b = spec.text.as_bytes();
                let channel_effect =
                    i32::from(spec.channel) | (i32::from(spec.effect) << 8);
                raw::hud_message(
                    Self::opt_slot(spec.slot),
                    channel_effect,
                    spec.x,
                    spec.y,
                    $crate::pack_rgba(spec.rgba1),
                    $crate::pack_rgba(spec.rgba2),
                    spec.fadein,
                    spec.hold,
                    spec.fadeout,
                    spec.fxtime,
                    b.as_ptr() as i32,
                    b.len() as i32,
                )
            }

            fn dhud_message(&mut self, spec: &$crate::DhudMessageSpec) -> i32 {
                let b = spec.text.as_bytes();
                let rgb_effect = ($crate::pack_rgb(spec.rgb) as u32
                    | ((spec.effect as u32) << 24)) as i32;
                raw::dhud_message(
                    Self::opt_slot(spec.slot),
                    rgb_effect,
                    spec.x,
                    spec.y,
                    spec.fadein,
                    spec.hold,
                    spec.fadeout,
                    spec.fxtime,
                    b.as_ptr() as i32,
                    b.len() as i32,
                )
            }

            fn show_motd(&mut self, slot: u8, title: &str, body: &str) -> i32 {
                let tb = title.as_bytes();
                let bb = body.as_bytes();
                raw::show_motd(
                    i32::from(slot),
                    tb.as_ptr() as i32,
                    tb.len() as i32,
                    bb.as_ptr() as i32,
                    bb.len() as i32,
                )
            }

            fn screen_fade(&mut self, spec: &$crate::ScreenFadeSpec) -> i32 {
                raw::screen_fade(
                    Self::opt_slot(spec.slot),
                    spec.duration_sec,
                    spec.hold_sec,
                    i32::from(spec.flags),
                    $crate::pack_rgba(spec.rgba),
                )
            }

            fn screen_shake(&mut self, spec: &$crate::ScreenShakeSpec) -> i32 {
                raw::screen_shake(
                    Self::opt_slot(spec.slot),
                    spec.amplitude,
                    spec.duration_sec,
                    spec.frequency,
                )
            }

            fn bar_time(&mut self, slot: u8, seconds: u16) -> i32 {
                raw::bar_time(i32::from(slot), i32::from(seconds))
            }

            fn hud_draw_rect(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) -> i32 {
                raw::hud_draw_rect(x, y, w, h, $crate::pack_rgba(rgba))
            }

            fn hud_draw_text(
                &mut self,
                x: f32,
                y: f32,
                size: f32,
                rgba: [u8; 4],
                text: &str,
            ) -> i32 {
                let b = text.as_bytes();
                raw::hud_draw_text(x, y, size, $crate::pack_rgba(rgba), b.as_ptr() as i32, b.len() as i32)
            }

            fn hud_draw_bar(
                &mut self,
                x: f32,
                y: f32,
                w: f32,
                h: f32,
                frac: f32,
                rgba: [u8; 4],
            ) -> i32 {
                raw::hud_draw_bar(x, y, w, h, frac, $crate::pack_rgba(rgba))
            }

            fn hud_panel(
                &mut self,
                x: f32,
                y: f32,
                w: f32,
                h: f32,
                rgba: [u8; 4],
                title: &str,
            ) -> i32 {
                let b = title.as_bytes();
                raw::hud_panel(
                    x,
                    y,
                    w,
                    h,
                    $crate::pack_rgba(rgba),
                    b.as_ptr() as i32,
                    b.len() as i32,
                )
            }

            fn client_exec(&mut self, line: &str) -> i32 {
                let b = line.as_bytes();
                raw::client_exec(b.as_ptr() as i32, b.len() as i32)
            }

            fn lang_load_toml(&mut self, src: &str) -> i32 {
                let b = src.as_bytes();
                raw::lang_load_toml(b.as_ptr() as i32, b.len() as i32)
            }

            fn lang_load_amxx(&mut self, src: &str) -> i32 {
                let b = src.as_bytes();
                raw::lang_load_amxx(b.as_ptr() as i32, b.len() as i32)
            }

            fn lang_tr(&self, slot: Option<u8>, key: &str, args: &[(&str, &str)]) -> String {
                let kb = key.as_bytes();
                let encoded = $crate::encode_lang_args(args);
                let ab = encoded.as_bytes();
                Self::read_str_with(4096, |p, c| {
                    raw::lang_tr(
                        Self::opt_slot(slot),
                        kb.as_ptr() as i32,
                        kb.len() as i32,
                        ab.as_ptr() as i32,
                        ab.len() as i32,
                        p,
                        c,
                    )
                })
                .unwrap_or_else(|| key.to_string())
            }

            fn lang_player_code(&self, slot: Option<u8>) -> String {
                Self::read_str_with(32, |p, c| raw::lang_player_code(Self::opt_slot(slot), p, c))
                    .unwrap_or_else(|| "en".to_string())
            }

            fn lang_set_player_code(&mut self, slot: u8, code: &str) -> i32 {
                let b = code.as_bytes();
                raw::lang_set_player_code(i32::from(slot), b.as_ptr() as i32, b.len() as i32)
            }

            fn admin_player_flags(&self, slot: u8) -> $crate::AdminFlags {
                let bits = raw::admin_player_flags(i32::from(slot));
                if bits < 0 {
                    $crate::AdminFlags::ALL
                } else {
                    $crate::AdminFlags::from_bits(bits as u32)
                }
            }

            fn admin_has_flag(&self, slot: u8, flag: char) -> bool {
                raw::admin_has_flag(i32::from(slot), flag as i32) > 0
            }

            fn rpc_export(&mut self, fn_name: &str, cb_id: i32) -> i32 {
                let b = fn_name.as_bytes();
                raw::rpc_export(b.as_ptr() as i32, b.len() as i32, cb_id)
            }

            fn rpc_call(
                &mut self,
                fn_name: &str,
                arg_i32: i32,
                arg_str: &str,
            ) -> (i32, Option<String>) {
                let nb = fn_name.as_bytes();
                let ab = arg_str.as_bytes();
                let mut out = vec![0u8; 4096];
                let rc = raw::rpc_call(
                    nb.as_ptr() as i32,
                    nb.len() as i32,
                    arg_i32,
                    ab.as_ptr() as i32,
                    ab.len() as i32,
                    out.as_mut_ptr() as i32,
                    out.len() as i32,
                );
                if rc <= 0 {
                    return (rc, None);
                }
                let len = (rc as usize).min(out.len());
                let text = std::str::from_utf8(&out[..len])
                    .ok()
                    .filter(|s| !s.is_empty() && !s.bytes().all(|x| x == 0))
                    .map(str::to_string);
                (rc, text)
            }

            fn rpc_subscribe(&mut self, event_name: &str, cb_id: i32) -> i32 {
                let b = event_name.as_bytes();
                raw::rpc_subscribe(b.as_ptr() as i32, b.len() as i32, cb_id)
            }

            fn rpc_emit_event(&mut self, event_name: &str, payload_json: &str) -> i32 {
                let eb = event_name.as_bytes();
                let pb = payload_json.as_bytes();
                raw::rpc_emit_event(
                    eb.as_ptr() as i32,
                    eb.len() as i32,
                    pb.as_ptr() as i32,
                    pb.len() as i32,
                )
            }

            fn nvault_open(&mut self, name: &str) -> i32 {
                let b = name.as_bytes();
                raw::nvault_open(b.as_ptr() as i32, b.len() as i32)
            }

            fn nvault_get(&self, handle: i32, key: &str) -> Option<String> {
                let b = key.as_bytes();
                Self::read_str_with(4096, |p, c| {
                    raw::nvault_get(handle, b.as_ptr() as i32, b.len() as i32, p, c)
                })
            }

            fn nvault_set(&mut self, handle: i32, key: &str, val: &str) -> i32 {
                let kb = key.as_bytes();
                let vb = val.as_bytes();
                raw::nvault_set(
                    handle,
                    kb.as_ptr() as i32,
                    kb.len() as i32,
                    vb.as_ptr() as i32,
                    vb.len() as i32,
                )
            }

            fn nvault_remove(&mut self, handle: i32, key: &str) -> i32 {
                let kb = key.as_bytes();
                raw::nvault_remove(handle, kb.as_ptr() as i32, kb.len() as i32)
            }

            fn leaderboard_upsert(
                &mut self,
                table: &str,
                key: &str,
                score: f64,
                payload_json: &str,
            ) -> i32 {
                let tb = table.as_bytes();
                let kb = key.as_bytes();
                let pb = payload_json.as_bytes();
                raw::leaderboard_upsert(
                    tb.as_ptr() as i32,
                    tb.len() as i32,
                    kb.as_ptr() as i32,
                    kb.len() as i32,
                    score,
                    pb.as_ptr() as i32,
                    pb.len() as i32,
                )
            }

            fn leaderboard_rank(
                &self,
                table: &str,
                key: &str,
            ) -> Option<($crate::LeaderboardEntry, usize)> {
                let tb = table.as_bytes();
                let kb = key.as_bytes();
                let mut buf = vec![0u8; 4096];
                let rank = raw::leaderboard_rank(
                    tb.as_ptr() as i32,
                    tb.len() as i32,
                    kb.as_ptr() as i32,
                    kb.len() as i32,
                    buf.as_mut_ptr() as i32,
                    buf.len() as i32,
                );
                if rank <= 0 {
                    return None;
                }
                let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                let raw_text = std::str::from_utf8(&buf[..end]).ok()?;
                $crate::parse_leaderboard_rank_row(key, raw_text)
            }

            fn leaderboard_top(
                &self,
                table: &str,
                offset: usize,
                limit: usize,
            ) -> Vec<$crate::LeaderboardEntry> {
                let tb = table.as_bytes();
                let Some(text) = Self::read_str_with(16_384, |p, c| {
                    raw::leaderboard_top(
                        tb.as_ptr() as i32,
                        tb.len() as i32,
                        offset as i32,
                        limit as i32,
                        p,
                        c,
                    )
                }) else {
                    return Vec::new();
                };
                $crate::parse_leaderboard_top_rows(&text)
            }

            fn http_post_json(&mut self, url: &str, body_json: &str, cb_id: i32) -> i32 {
                let ub = url.as_bytes();
                let bb = body_json.as_bytes();
                raw::http_post_json(
                    ub.as_ptr() as i32,
                    ub.len() as i32,
                    bb.as_ptr() as i32,
                    bb.len() as i32,
                    cb_id,
                )
            }
        }
    };
}

/// Materializes `wasm32` ABI v2 `extern "C"` entrypoints (`cs_mod_init`, `cs_mod_hook`,
/// `cs_mod_timer`, `cs_mod_menu`, `cs_mod_rpc`, `cs_mod_event`, `cs_mod_webhook`,
/// `cs_mod_client_hud`, `cs_mod_client_usermsg`, `cs_mod_client_input`) inside a guest mod crate
/// without repeating raw `#[unsafe(no_mangle)]` boilerplate.
///
/// Requires [`cs16_wasm_host_bridge!`] to be invoked in the same module.
/// On non-`wasm32` targets this expands to nothing so native tests and libraries stay 100%
/// `#![forbid(unsafe_code)]`.
#[macro_export]
macro_rules! cs16_wasm_exports {
    () => {};
    (init: $init_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_init() -> i32 {
            let mut host = WasmHostBridge;
            $init_fn(&mut host)
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (hook: $hook_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_hook(hook_id: i32, phase: i32) -> i32 {
            let (Some(hook), Some(ph)) = (
                $crate::HookId::from_i32(hook_id),
                $crate::HookPhase::from_i32(phase),
            ) else {
                return $crate::HookAction::Continue as i32;
            };
            let mut host = WasmHostBridge;
            let action: $crate::HookAction = $hook_fn(&mut host, hook, ph);
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (hooks: [ $( ($hook_id:path, $phase:path) => $handler:path ),* $(,)? ] $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        #[allow(clippy::single_match)]
        pub extern "C" fn cs_mod_hook(hook_id: i32, phase: i32) -> i32 {
            let (Some(hook), Some(ph)) = (
                $crate::HookId::from_i32(hook_id),
                $crate::HookPhase::from_i32(phase),
            ) else {
                return $crate::HookAction::Continue as i32;
            };
            let mut host = WasmHostBridge;
            let action: $crate::HookAction = match (hook, ph) {
                $( ($hook_id, $phase) => $handler(&mut host), )*
                _ => $crate::HookAction::Continue,
            };
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (timer: $timer_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_timer(cb_id: i32, arg: i32) -> i32 {
            let mut host = WasmHostBridge;
            let action: $crate::HookAction = $timer_fn(&mut host, cb_id, arg);
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (menu: $menu_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_menu(
            cb_id: i32,
            slot: i32,
            menu_id: i32,
            item_index: i32,
        ) -> i32 {
            let mut host = WasmHostBridge;
            let action: $crate::HookAction =
                $menu_fn(&mut host, cb_id, slot.clamp(0, 31) as u8, menu_id as u32, item_index);
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (rpc: $rpc_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_rpc(cb_id: i32) -> i32 {
            let mut host = WasmHostBridge;
            $rpc_fn(&mut host, cb_id)
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (event: $event_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_event(cb_id: i32) -> i32 {
            let mut host = WasmHostBridge;
            let action: $crate::HookAction = $event_fn(&mut host, cb_id);
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (webhook: $webhook_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_webhook(cb_id: i32, status: i32) -> i32 {
            let mut host = WasmHostBridge;
            let action: $crate::HookAction = $webhook_fn(&mut host, cb_id, status);
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (client_hud: $hud_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_client_hud(
            dt_ms: i32,
            health: i32,
            armor: i32,
            money: i32,
        ) {
            let mut host = WasmHostBridge;
            $hud_fn(&mut host, dt_ms, health, armor, money);
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (client_usermsg: $usermsg_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_client_usermsg() -> i32 {
            let mut host = WasmHostBridge;
            let action: $crate::HookAction = $usermsg_fn(&mut host);
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
    (client_input: $input_fn:path $(, $($rest:tt)*)?) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn cs_mod_client_input(key: i32, pressed: i32) -> i32 {
            let mut host = WasmHostBridge;
            let action: $crate::HookAction = $input_fn(&mut host, key, pressed != 0);
            action as i32
        }
        $crate::cs16_wasm_exports!($($($rest)*)?);
    };
}
