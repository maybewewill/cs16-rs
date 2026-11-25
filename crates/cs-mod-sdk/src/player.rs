//! Ergonomic [`Player`] handle wrapper and CS 1.6 chat/menu/rendering constants.

use cs_mod_api::{AdminFlags, ArmorKind, ScreenFadeSpec, ScreenShakeSpec, TeamKind};

use crate::bridge::HostBridge;

/// Classic Counter-Strike 1.6 chat color control codes (`SayText`) and GoldSrc menu escapes.
pub mod color {
    pub const CHAT_DEFAULT: &str = "\x01";
    pub const CHAT_TEAM: &str = "\x03";
    pub const CHAT_GREEN: &str = "\x04";

    pub const MENU_YELLOW: &str = "\\y";
    pub const MENU_WHITE: &str = "\\w";
    pub const MENU_RED: &str = "\\r";
    pub const MENU_GREY: &str = "\\d";
    pub const MENU_RIGHT: &str = "\\R";
}

/// GoldSrc `kRenderFx*` constants (`const.h` / AMXX `amxconst.inc`).
pub mod render_fx {
    pub const NONE: u8 = 0;
    pub const PULSE_SLOW: u8 = 1;
    pub const PULSE_FAST: u8 = 2;
    pub const STROBE_SLOW: u8 = 9;
    pub const GLOW_SHELL: u8 = 19;
}

/// GoldSrc `kRender*` render mode constants (`const.h` / AMXX `amxconst.inc`).
pub mod render_mode {
    pub const NORMAL: u8 = 0;
    pub const TRANS_COLOR: u8 = 1;
    pub const TRANS_TEXTURE: u8 = 2;
    pub const GLOW: u8 = 3;
    pub const TRANS_ALPHA: u8 = 4;
    pub const TRANS_ADD: u8 = 5;
}

/// CS 1.6 numeric weapon IDs (`CSW_*`, matching `cs-game` and AMXX `amxconst.inc`).
pub mod weapon_id {
    pub const P228: u8 = 1;
    pub const SCOUT: u8 = 3;
    pub const HEGRENADE: u8 = 4;
    pub const XM1014: u8 = 5;
    pub const C4: u8 = 6;
    pub const MAC10: u8 = 7;
    pub const AUG: u8 = 8;
    pub const SMOKEGRENADE: u8 = 9;
    pub const ELITE: u8 = 10;
    pub const FIVESEVEN: u8 = 11;
    pub const UMP45: u8 = 12;
    pub const SG550: u8 = 13;
    pub const GALIL: u8 = 14;
    pub const FAMAS: u8 = 15;
    pub const USP: u8 = 16;
    pub const GLOCK18: u8 = 17;
    pub const AWP: u8 = 18;
    pub const MP5NAVY: u8 = 19;
    pub const M249: u8 = 20;
    pub const M3: u8 = 21;
    pub const M4A1: u8 = 22;
    pub const TMP: u8 = 23;
    pub const G3SG1: u8 = 24;
    pub const FLASHBANG: u8 = 25;
    pub const DEAGLE: u8 = 26;
    pub const SG552: u8 = 27;
    pub const AK47: u8 = 28;
    pub const KNIFE: u8 = 29;
    pub const P90: u8 = 30;
}

/// Lightweight handle for a connected player slot (`0..32`), delegating all queries and
/// mutations safely through any [`HostBridge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Player(pub u8);

impl Player {
    #[inline]
    pub const fn new(slot: u8) -> Self {
        Self(slot)
    }

    #[inline]
    pub const fn slot(self) -> u8 {
        self.0
    }

    /// GoldSrc 1-based entity index for this player slot (`slot + 1`).
    #[inline]
    pub const fn edict_index(self) -> u16 {
        (self.0 as u16) + 1
    }

    pub fn is_active(self, host: &impl HostBridge) -> bool {
        host.player_is_active(self.0)
    }

    pub fn is_alive(self, host: &impl HostBridge) -> bool {
        host.player_is_alive(self.0)
    }

    pub fn name(self, host: &impl HostBridge) -> String {
        host.player_name(self.0).unwrap_or_default()
    }

    pub fn ip(self, host: &impl HostBridge) -> String {
        host.player_get_ip(self.0).unwrap_or_default()
    }

    pub fn steamid(self, host: &impl HostBridge) -> String {
        host.player_get_steamid(self.0).unwrap_or_default()
    }

    pub fn ping(self, host: &impl HostBridge) -> i32 {
        host.player_get_ping(self.0)
    }

    pub fn loss(self, host: &impl HostBridge) -> i32 {
        host.player_get_loss(self.0)
    }

    pub fn aim_origin(self, host: &impl HostBridge) -> Option<[f32; 3]> {
        host.player_get_aim_origin(self.0)
    }

    pub fn origin(self, host: &impl HostBridge) -> Option<[f32; 3]> {
        host.ent_get_origin(self.edict_index())
    }

    pub fn velocity(self, host: &impl HostBridge) -> Option<[f32; 3]> {
        host.ent_get_velocity(self.edict_index())
    }

    pub fn health(self, host: &impl HostBridge) -> f32 {
        host.player_get_health(self.0)
    }

    pub fn set_health(self, host: &mut impl HostBridge, health: f32) -> i32 {
        host.player_set_health(self.0, health)
    }

    pub fn set_max_health(self, host: &mut impl HostBridge, max_health: f32) -> i32 {
        host.player_set_max_health(self.0, max_health)
    }

    pub fn armor(self, host: &impl HostBridge) -> i32 {
        host.player_get_armor(self.0)
    }

    pub fn armor_type(self, host: &impl HostBridge) -> ArmorKind {
        host.player_get_armor_type(self.0)
    }

    pub fn set_armor(self, host: &mut impl HostBridge, armor: i32, kind: ArmorKind) -> i32 {
        host.player_set_armor(self.0, armor, kind)
    }

    pub fn money(self, host: &impl HostBridge) -> i32 {
        host.player_get_money(self.0)
    }

    pub fn set_money(self, host: &mut impl HostBridge, amount: i32, flash: bool) -> i32 {
        host.player_set_money(self.0, amount, flash)
    }

    pub fn add_money(self, host: &mut impl HostBridge, delta: i32, flash: bool) -> i32 {
        host.player_add_money(self.0, delta, flash)
    }

    pub fn frags(self, host: &impl HostBridge) -> i32 {
        host.player_get_frags(self.0)
    }

    pub fn deaths(self, host: &impl HostBridge) -> i32 {
        host.player_get_deaths(self.0)
    }

    pub fn set_score(self, host: &mut impl HostBridge, frags: i32, deaths: i32) -> i32 {
        host.player_set_score(self.0, frags, deaths)
    }

    pub fn reset_score(self, host: &mut impl HostBridge) -> i32 {
        host.player_set_score(self.0, 0, 0)
    }

    pub fn team(self, host: &impl HostBridge) -> TeamKind {
        host.player_get_team(self.0)
    }

    pub fn set_team(
        self,
        host: &mut impl HostBridge,
        team: TeamKind,
        model_class: Option<u8>,
    ) -> i32 {
        host.player_set_team(self.0, team, model_class)
    }

    pub fn respawn(self, host: &mut impl HostBridge) -> i32 {
        host.player_respawn(self.0)
    }

    pub fn slap(self, host: &mut impl HostBridge, power: i32, random_dir: bool) -> i32 {
        host.player_slap(self.0, power, random_dir)
    }

    pub fn slay(self, host: &mut impl HostBridge, preserve_frags: bool) -> i32 {
        host.player_slay(self.0, preserve_frags)
    }

    pub fn damage(
        self,
        host: &mut impl HostBridge,
        attacker: Option<Player>,
        damage: f32,
        bits: u32,
    ) -> i32 {
        host.player_damage(self.0, attacker.map(Player::slot), damage, bits)
    }

    pub fn kick(self, host: &mut impl HostBridge, reason: &str) -> i32 {
        host.player_kick(self.0, reason)
    }

    pub fn set_godmode(self, host: &mut impl HostBridge, enabled: bool) -> i32 {
        host.player_set_godmode(self.0, enabled)
    }

    pub fn set_noclip(self, host: &mut impl HostBridge, enabled: bool) -> i32 {
        host.player_set_noclip(self.0, enabled)
    }

    pub fn set_maxspeed(self, host: &mut impl HostBridge, maxspeed: f32) -> i32 {
        host.player_set_maxspeed(self.0, maxspeed)
    }

    pub fn set_gravity(self, host: &mut impl HostBridge, gravity: f32) -> i32 {
        host.player_set_gravity(self.0, gravity)
    }

    pub fn set_footsteps(self, host: &mut impl HostBridge, silent: bool) -> i32 {
        host.player_set_footsteps(self.0, silent)
    }

    pub fn set_rendering(
        self,
        host: &mut impl HostBridge,
        fx: u8,
        rgb: [u8; 3],
        mode: u8,
        amount: u8,
    ) -> i32 {
        host.player_set_rendering(self.0, fx, rgb, mode, amount)
    }

    /// Convenience helper applying classic GoldSrc `GlowShell` highlight around this player.
    pub fn set_glow(self, host: &mut impl HostBridge, rgb: [u8; 3], thickness: u8) -> i32 {
        host.player_set_rendering(
            self.0,
            render_fx::GLOW_SHELL,
            rgb,
            render_mode::NORMAL,
            thickness,
        )
    }

    pub fn clear_rendering(self, host: &mut impl HostBridge) -> i32 {
        host.player_set_rendering(self.0, render_fx::NONE, [255, 255, 255], render_mode::NORMAL, 255)
    }

    pub fn set_model(self, host: &mut impl HostBridge, model: &str) -> i32 {
        host.player_set_model(self.0, model)
    }

    pub fn reset_model(self, host: &mut impl HostBridge) -> i32 {
        host.player_reset_model(self.0)
    }

    pub fn give_item(self, host: &mut impl HostBridge, item: &str) -> i32 {
        host.player_give_item(self.0, item)
    }

    /// Alias for [`Self::give_item`], auto-prefixing `"weapon_"` if not already prefixed.
    pub fn give_weapon(self, host: &mut impl HostBridge, name: &str) -> i32 {
        if name.starts_with("weapon_") || name.starts_with("item_") || name.starts_with("ammo_") {
            host.player_give_item(self.0, name)
        } else {
            let full = format!("weapon_{name}");
            host.player_give_item(self.0, &full)
        }
    }

    pub fn give_custom_weapon(self, host: &mut impl HostBridge, id_name: &str) -> i32 {
        host.give_custom_weapon(self.0, id_name)
    }

    pub fn strip_weapons(self, host: &mut impl HostBridge) -> i32 {
        host.player_strip_weapons(self.0)
    }

    pub fn weapon_clip(self, host: &impl HostBridge, weapon_id: u8) -> i32 {
        host.player_get_weapon_clip(self.0, weapon_id)
    }

    pub fn set_weapon_clip(self, host: &mut impl HostBridge, weapon_id: u8, clip: i32) -> i32 {
        host.player_set_weapon_clip(self.0, weapon_id, clip)
    }

    pub fn bpammo(self, host: &impl HostBridge, weapon_id: u8) -> i32 {
        host.player_get_bpammo(self.0, weapon_id)
    }

    pub fn set_bpammo(self, host: &mut impl HostBridge, weapon_id: u8, amount: i32) -> i32 {
        host.player_set_bpammo(self.0, weapon_id, amount)
    }

    pub fn set_defuser(self, host: &mut impl HostBridge, has: bool) -> i32 {
        host.player_set_defuser(self.0, has)
    }

    pub fn set_nvg(self, host: &mut impl HostBridge, has: bool) -> i32 {
        host.player_set_nvg(self.0, has)
    }

    pub fn drop_slot(self, host: &mut impl HostBridge, inv_slot: u8) -> i32 {
        host.player_drop_slot(self.0, inv_slot)
    }

    // --- Admin RBAC ---
    pub fn admin_flags(self, host: &impl HostBridge) -> AdminFlags {
        host.admin_player_flags(self.0)
    }

    pub fn has_flag(self, host: &impl HostBridge, flag: char) -> bool {
        host.admin_has_flag(self.0, flag)
    }

    // --- Per-Player i18n & Messaging ---
    pub fn lang(self, host: &impl HostBridge) -> String {
        host.lang_player_code(Some(self.0))
    }

    pub fn set_lang(self, host: &mut impl HostBridge, code: &str) -> i32 {
        host.lang_set_player_code(self.0, code)
    }

    /// Translates `key` in this player's active language using `LangCatalog`.
    pub fn tr(self, host: &impl HostBridge, key: &str, args: &[(&str, &str)]) -> String {
        host.lang_tr(Some(self.0), key, args)
    }

    pub fn say(self, host: &mut impl HostBridge, text: &str) {
        host.say_text(Some(self.0), text);
    }

    pub fn say_tr(self, host: &mut impl HostBridge, key: &str, args: &[(&str, &str)]) {
        let text = host.lang_tr(Some(self.0), key, args);
        host.say_text(Some(self.0), &text);
    }

    pub fn print(self, host: &mut impl HostBridge, text: &str) -> i32 {
        host.print_client(self.0, text)
    }

    pub fn center_print(self, host: &mut impl HostBridge, text: &str) {
        host.center_print(Some(self.0), text);
    }

    pub fn show_motd(self, host: &mut impl HostBridge, title: &str, body: &str) -> i32 {
        host.show_motd(self.0, title, body)
    }

    pub fn screen_fade(
        self,
        host: &mut impl HostBridge,
        duration_sec: f32,
        hold_sec: f32,
        flags: u16,
        rgba: [u8; 4],
    ) -> i32 {
        host.screen_fade(&ScreenFadeSpec {
            slot: Some(self.0),
            duration_sec,
            hold_sec,
            flags,
            rgba,
        })
    }

    pub fn screen_shake(
        self,
        host: &mut impl HostBridge,
        amplitude: f32,
        duration_sec: f32,
        frequency: f32,
    ) -> i32 {
        host.screen_shake(&ScreenShakeSpec {
            slot: Some(self.0),
            amplitude,
            duration_sec,
            frequency,
        })
    }

    pub fn bar_time(self, host: &mut impl HostBridge, seconds: u16) -> i32 {
        host.bar_time(self.0, seconds)
    }
}
