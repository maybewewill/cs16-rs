//! 1-to-1 Counter-Strike 1.6 HUD rendered directly from real Valve `sprites/hud.txt`
//! (`640hud7.spr`, `640hud1.spr`, `640hud2.spr`, `640hud16.spr`, `radar640.spr`):
//! Health, Armor/Helmet, Buyzone/C4/Defuser left icons, Sprite-Digit Round Timer,
//! Sprite-Digit Money & Clip/Reserve Ammo, Translucent Radar, Weapon-Icon Killfeed,
//! Top Bucket Selector, AWP Sniper Scope, and C4 Plant/Defuse Bar.

use std::collections::{BTreeMap, HashMap, HashSet};

use cs_assets::scheme::{HudSpriteEntry, load_hud_txt, parse_hud_txt};
use cs_assets::spr::GoldSrcSprite;
use cs_assets::vgui_font::SchemeFontSet;
use cs_assets::vgui_res::TextAlignment;
use cs_clientcore::HudState;
use cs_clientcore::hud::{DeathNotice, ScreenFade, StatusIcon};
use cs_vfs::Vfs;
use egui::{Color32, FontId, Pos2, Rect, Stroke, TextureHandle, Vec2};
use glam::Vec3;

use crate::ui::hud_extras::{
    OverviewMapTex, WeaponBucketState, compute_damage_quadrants, damage_hazard_icons,
    evaluate_dhud_text_fx, format_hud_text_entry, format_status_bar,
};
use crate::ui::vgui2::{VguiRuntime, VguiTextDrawOpts};
use crate::view_effects::evaluate_screen_fade;

const HUD_AMBER: Color32 = Color32::from_rgb(255, 160, 0);
const HUD_RED: Color32 = Color32::from_rgb(255, 24, 16);
const TEAM_CT_COLOR: Color32 = Color32::from_rgb(153, 204, 255);
const TEAM_T_COLOR: Color32 = Color32::from_rgb(255, 64, 64);
const CHAT_SPEC_COLOR: Color32 = Color32::from_rgb(204, 204, 204);
const CHAT_GREEN_COLOR: Color32 = Color32::from_rgb(0, 204, 0);

/// A formatted chat line with its sender's team for `CHudSayText` `\x02`/`\x03` team color lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudChatEntry {
    pub text: String,
    pub team: Option<String>,
}

/// Team color used for `CHudSayText` (`\x02` / `\x03`) spans: CT blue, T red, Spectator/other gray.
pub fn chat_team_color(team: Option<&str>) -> Color32 {
    match team {
        Some("CT") => TEAM_CT_COLOR,
        Some("TERRORIST") => TEAM_T_COLOR,
        _ => CHAT_SPEC_COLOR,
    }
}

/// What kind of item appeared in `CHudAmmo::DrawHistory`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickupKind {
    Weapon { weapon_id: u8, empty: bool },
    Ammo { ammo_id: u8, amount: u8 },
    Item { classname: String },
}

/// One pickup history card shown on the right edge (`CHudAmmo::DrawHistory`, 5.0 s life).
#[derive(Debug, Clone, PartialEq)]
pub struct PickupHistoryEntry {
    pub kind: PickupKind,
    pub end_time: f64,
}

/// Map a CS 1.6 `WeaponId` (`1..=30`) to its `weapon_*` HUD script name.
pub fn weapon_name_from_id(weapon_id: u8) -> Option<&'static str> {
    match weapon_id {
        1 => Some("weapon_p228"),
        3 => Some("weapon_scout"),
        4 => Some("weapon_hegrenade"),
        5 => Some("weapon_xm1014"),
        6 => Some("weapon_c4"),
        7 => Some("weapon_mac10"),
        8 => Some("weapon_aug"),
        9 => Some("weapon_smokegrenade"),
        10 => Some("weapon_elite"),
        11 => Some("weapon_fiveseven"),
        12 => Some("weapon_ump45"),
        13 => Some("weapon_sg550"),
        14 => Some("weapon_galil"),
        15 => Some("weapon_famas"),
        16 => Some("weapon_usp"),
        17 => Some("weapon_glock18"),
        18 => Some("weapon_awp"),
        19 => Some("weapon_mp5navy"),
        20 => Some("weapon_m249"),
        21 => Some("weapon_m3"),
        22 => Some("weapon_m4a1"),
        23 => Some("weapon_tmp"),
        24 => Some("weapon_g3sg1"),
        25 => Some("weapon_flashbang"),
        26 => Some("weapon_deagle"),
        27 => Some("weapon_sg552"),
        28 => Some("weapon_ak47"),
        29 => Some("weapon_knife"),
        30 => Some("weapon_p90"),
        _ => None,
    }
}

/// Compute stacked vertical Y coordinate for `CHudAmmo::DrawHistory` slot `slot` (`0..12`).
pub fn pickup_history_slot_y(screen_bottom: f32, max_h: f32, slot: usize, scale: f32) -> f32 {
    screen_bottom + (-36.0 * scale - max_h) * 2.0 - (max_h + 5.0 * scale) * (slot as f32)
}

/// Compute fade alpha (`0..=255`) for `CHudAmmo::DrawHistory`: `min(255, (end_time - now) * 80.0)`.
pub fn pickup_history_alpha(now: f64, end_time: f64) -> Option<u8> {
    let rem = end_time - now;
    if rem <= 0.0 {
        return None;
    }
    Some((rem * 80.0).clamp(0.0, 255.0) as u8)
}

pub const HIDEHUD_WEAPONS: u8 = 1 << 0;
pub const HIDEHUD_FLASHLIGHT: u8 = 1 << 1;
pub const HIDEHUD_ALL: u8 = 1 << 2;
pub const HIDEHUD_HEALTH: u8 = 1 << 3;
pub const HIDEHUD_TIMER: u8 = 1 << 4;
pub const HIDEHUD_MONEY: u8 = 1 << 5;
pub const HIDEHUD_CROSSHAIR: u8 = 1 << 6;

/// GoldSrc `HideWeapon` visibility mask resolved from `state.hidden` (`HIDEHUD_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudVisibility {
    pub health_armor: bool,
    pub timer: bool,
    pub money: bool,
    pub ammo: bool,
    pub weapon_buckets: bool,
    pub radar: bool,
    pub crosshair: bool,
}

pub fn compute_hud_visibility(
    alive: bool,
    _health: i32,
    hidden: u8,
    timer_visible: bool,
) -> HudVisibility {
    if (hidden & HIDEHUD_ALL) != 0 {
        return HudVisibility {
            health_armor: false,
            timer: false,
            money: false,
            ammo: false,
            weapon_buckets: false,
            radar: false,
            crosshair: false,
        };
    }
    HudVisibility {
        health_armor: alive && (hidden & HIDEHUD_HEALTH) == 0,
        timer: alive && timer_visible && (hidden & HIDEHUD_TIMER) == 0,
        money: (hidden & HIDEHUD_MONEY) == 0,
        ammo: alive && (hidden & HIDEHUD_WEAPONS) == 0,
        weapon_buckets: alive && (hidden & HIDEHUD_WEAPONS) == 0,
        radar: alive && (hidden & (HIDEHUD_HEALTH | HIDEHUD_WEAPONS)) == 0,
        crosshair: alive && (hidden & (HIDEHUD_WEAPONS | HIDEHUD_CROSSHAIR)) == 0,
    }
}

pub struct HudSprites {
    sheets: HashMap<String, (TextureHandle, [f32; 2])>,
    entries: HashMap<String, HudSpriteEntry>,
    weapon_cards: HashMap<String, HudSpriteEntry>,
    radar_tex: Option<TextureHandle>,
}

const WEAPON_CARD_FILES: &[&str] = &[
    "weapon_p228",
    "weapon_scout",
    "weapon_hegrenade",
    "weapon_xm1014",
    "weapon_c4",
    "weapon_mac10",
    "weapon_aug",
    "weapon_smokegrenade",
    "weapon_elite",
    "weapon_fiveseven",
    "weapon_ump45",
    "weapon_sg550",
    "weapon_galil",
    "weapon_famas",
    "weapon_usp",
    "weapon_glock18",
    "weapon_awp",
    "weapon_mp5navy",
    "weapon_m249",
    "weapon_m3",
    "weapon_m4a1",
    "weapon_tmp",
    "weapon_g3sg1",
    "weapon_flashbang",
    "weapon_deagle",
    "weapon_sg552",
    "weapon_ak47",
    "weapon_knife",
    "weapon_p90",
    "weapon_shield",
    "weapon_shieldgun",
];

impl HudSprites {
    pub fn load(ctx: &egui::Context, vfs: &Vfs) -> Self {
        let mut entries: HashMap<String, HudSpriteEntry> = HashMap::new();
        if let Ok(list) = load_hud_txt(vfs) {
            for e in list {
                let replace = match entries.get(&e.name) {
                    None => true,
                    Some(prev) => e.resolution >= prev.resolution,
                };
                if replace {
                    entries.insert(e.name.clone(), e);
                }
            }
        }

        let mut weapon_cards: HashMap<String, HudSpriteEntry> = HashMap::new();
        for wname in WEAPON_CARD_FILES {
            let txt_path = format!("sprites/{wname}.txt");
            if let Ok(text) = vfs.read_string_lossy(&txt_path) {
                let mut picked: Option<HudSpriteEntry> = None;
                for e in parse_hud_txt(&text) {
                    if e.name != "weapon" {
                        continue;
                    }
                    let replace = match &picked {
                        None => true,
                        Some(prev) => {
                            (e.resolution == 640 && prev.resolution != 640)
                                || (prev.resolution != 640 && e.resolution > prev.resolution)
                        }
                    };
                    if replace {
                        picked = Some(e);
                    }
                }
                if let Some(entry) = picked {
                    let short = wname.strip_prefix("weapon_").unwrap_or(wname).to_string();
                    weapon_cards.insert(short, entry);
                }
            }
        }

        let mut sheets = HashMap::new();
        for name in [
            "640hud1", "640hud2", "640hud3", "640hud4", "640hud5", "640hud6", "640hud7",
            "640hud8", "640hud9", "640hud10", "640hud11", "640hud12", "640hud13", "640hud14",
            "640hud15", "640hud16", "640hud17", "640hud18", "320hud1", "320hud2",
        ] {
            let path = format!("sprites/{name}.spr");
            if let Ok(spr) = GoldSrcSprite::load(vfs, &path)
                && let Some(frame) = spr.frames.first()
            {
                // GoldSrc HUD sprites are additive luminance maps: map luminance to alpha so
                // tinting with HUD_AMBER / HUD_RED / HUD_GREEN renders crisp transparent icons.
                let mut tinted_mask = Vec::with_capacity(frame.rgba.len());
                for px in frame.rgba.as_chunks::<4>().0 {
                    let lum = px[0].max(px[1]).max(px[2]);
                    tinted_mask.extend_from_slice(&[255, 255, 255, lum]);
                }
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &tinted_mask,
                );
                let handle = ctx.load_texture(
                    format!("cs16_hud_{name}"),
                    img,
                    egui::TextureOptions::NEAREST,
                );
                sheets.insert(
                    name.to_string(),
                    (handle, [frame.width as f32, frame.height as f32]),
                );
            }
        }

        let radar_tex = GoldSrcSprite::load(vfs, "sprites/radar640.spr")
            .ok()
            .and_then(|spr| {
                let frame = spr.frames.first()?;
                let mut rgba = Vec::with_capacity(frame.rgba.len());
                for px in frame.rgba.as_chunks::<4>().0 {
                    let lum = px[0].max(px[1]).max(px[2]);
                    if lum < 12 {
                        rgba.extend_from_slice(&[0, 0, 0, 0]);
                    } else {
                        rgba.extend_from_slice(&[px[0], px[1], px[2], 175]);
                    }
                }
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width as usize, frame.height as usize],
                    &rgba,
                );
                Some(ctx.load_texture(
                    "cs16_hud_radar640",
                    img,
                    egui::TextureOptions::LINEAR,
                ))
            });

        Self {
            sheets,
            entries,
            weapon_cards,
            radar_tex,
        }
    }

    pub fn weapon_card_size(&self, name: &str, scale: f32) -> Option<Vec2> {
        let short = name.strip_prefix("weapon_").unwrap_or(name);
        let e = self.weapon_cards.get(short)?;
        if !self.sheets.contains_key(&e.sprite_name) {
            return None;
        }
        Some(Vec2::new(e.width as f32 * scale, e.height as f32 * scale))
    }

    pub fn draw_weapon_card(
        &self,
        painter: &egui::Painter,
        name: &str,
        pos: Pos2,
        scale: f32,
        tint: Color32,
    ) -> Option<Vec2> {
        let short = name.strip_prefix("weapon_").unwrap_or(name);
        let e = self.weapon_cards.get(short)?;
        let (tex, [tw, th]) = self.sheets.get(&e.sprite_name)?;
        let u0 = e.x as f32 / *tw;
        let v0 = e.y as f32 / *th;
        let u1 = (e.x + e.width) as f32 / *tw;
        let v1 = (e.y + e.height) as f32 / *th;
        let size = Vec2::new(e.width as f32 * scale, e.height as f32 * scale);
        let rect = Rect::from_min_size(pos, size);
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)),
            tint,
        );
        Some(size)
    }

    pub fn sprite_size(&self, name: &str, scale: f32) -> Option<Vec2> {
        let e = self.entries.get(name)?;
        if !self.sheets.contains_key(&e.sprite_name) {
            return None;
        }
        Some(Vec2::new(e.width as f32 * scale, e.height as f32 * scale))
    }

    fn draw_sprite(
        &self,
        painter: &egui::Painter,
        name: &str,
        pos: Pos2,
        scale: f32,
        tint: Color32,
    ) -> Option<Vec2> {
        let e = self.entries.get(name)?;
        let (tex, [tw, th]) = self.sheets.get(&e.sprite_name)?;
        let u0 = e.x as f32 / *tw;
        let v0 = e.y as f32 / *th;
        let u1 = (e.x + e.width) as f32 / *tw;
        let v1 = (e.y + e.height) as f32 / *th;
        let size = Vec2::new(e.width as f32 * scale, e.height as f32 * scale);
        let rect = Rect::from_min_size(pos, size);
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)),
            tint,
        );
        Some(size)
    }

    fn draw_sprite_clipped_bottom(
        &self,
        painter: &egui::Painter,
        name: &str,
        pos: Pos2,
        scale: f32,
        tint: Color32,
        fill_frac: f32,
    ) -> Option<Vec2> {
        let e = self.entries.get(name)?;
        let (tex, [tw, th]) = self.sheets.get(&e.sprite_name)?;
        let size = Vec2::new(e.width as f32 * scale, e.height as f32 * scale);
        let f = fill_frac.clamp(0.0, 1.0);
        if f <= 0.001 {
            return Some(size);
        }
        let u0 = e.x as f32 / *tw;
        let u1 = (e.x + e.width) as f32 / *tw;
        let v_bot = (e.y + e.height) as f32 / *th;
        let v_top = (e.y as f32 + e.height as f32 * (1.0 - f)) / *th;
        let top_y = pos.y + size.y * (1.0 - f);
        let rect = Rect::from_min_size(Pos2::new(pos.x, top_y), Vec2::new(size.x, size.y * f));
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::new(u0, v_top), Pos2::new(u1, v_bot)),
            tint,
        );
        Some(size)
    }

    fn draw_ammo_icon(
        &self,
        painter: &egui::Painter,
        weapon_id: u8,
        pos: Pos2,
        scale: f32,
        tint: Color32,
    ) -> Option<Vec2> {
        let (sx, sy) = match weapon_id {
            1 => (120.0, 72.0),                          // p228
            3 | 24 | 28 => (72.0, 72.0),                 // scout / g3sg1 / ak47
            4 => (72.0, 96.0),                           // hegrenade
            5 | 21 | 29 => (0.0, 72.0),                  // xm1014 / m3 / knife
            6 => (96.0, 96.0),                           // c4
            7 | 12 | 16 => (96.0, 72.0),                 // mac10 / ump45 / usp
            8 | 13 | 14 | 15 | 20 | 22 | 27 => (0.0, 96.0), // aug / sg550 / galil / famas / m249 / m4a1 / sg552
            9 => (144.0, 96.0),                          // smokegrenade
            10 | 17 | 19 | 23 => (48.0, 72.0),           // elite / glock18 / mp5navy / tmp
            11 | 30 => (120.0, 96.0),                    // fiveseven / p90
            18 => (24.0, 96.0),                          // awp
            25 => (48.0, 96.0),                          // flashbang
            26 => (24.0, 72.0),                          // deagle
            _ => return None,
        };
        let (tex, [tw, th]) = self.sheets.get("640hud7")?;
        let u0 = sx / *tw;
        let v0 = sy / *th;
        let u1 = (sx + 24.0) / *tw;
        let v1 = (sy + 24.0) / *th;
        let size = Vec2::splat(24.0 * scale);
        painter.image(
            tex.id(),
            Rect::from_min_size(pos, size),
            Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)),
            tint,
        );
        Some(size)
    }

    fn measure_number(&self, value: i32, scale: f32, min_digits: usize) -> f32 {
        let s = format!("{:0width$}", value.max(0), width = min_digits);
        let step = 21.0 * scale;
        (s.len() as f32) * step
    }

    fn draw_number(
        &self,
        painter: &egui::Painter,
        value: i32,
        pos: Pos2,
        scale: f32,
        tint: Color32,
        min_digits: usize,
    ) -> f32 {
        let s = format!("{:0width$}", value.max(0), width = min_digits);
        let mut x = pos.x;
        let digit_w = 20.0 * scale;
        for ch in s.chars() {
            let spr_name = format!("number_{ch}");
            if let Some(sz) = self.draw_sprite(painter, &spr_name, Pos2::new(x, pos.y), scale, tint) {
                x += sz.x + 1.0 * scale;
            } else {
                painter.text(
                    Pos2::new(x, pos.y),
                    egui::Align2::LEFT_TOP,
                    ch.to_string(),
                    FontId::monospace(24.0 * scale),
                    tint,
                );
                x += digit_w;
            }
        }
        x - pos.x
    }

    /// Draw a right-aligned sprite number ending at `right_x`.
    #[allow(clippy::too_many_arguments)]
    fn draw_number_right(
        &self,
        painter: &egui::Painter,
        value: i32,
        right_x: f32,
        y: f32,
        scale: f32,
        tint: Color32,
        min_digits: usize,
    ) -> f32 {
        let w = self.measure_number(value, scale, min_digits);
        self.draw_number(painter, value, Pos2::new(right_x - w, y), scale, tint, min_digits);
        w
    }
}

/// What the HUD draws one frame: the state the server's messages built, the shell's own timed
/// logs, and the local player's place for the radar.
pub struct HudView<'a> {
    pub hud: &'a HudState,
    /// The session clock (what `RoundTime` and the bars count against).
    pub now: f64,
    pub time_sec: f32,
    pub names: &'a BTreeMap<u8, String>,
    /// The weapons the player carries (`weapon_id`s).
    pub owned: &'a HashSet<u8>,
    pub feed: &'a [DeathNotice],
    pub chat: &'a [String],
    pub notify: &'a [String],
    pub center: Option<&'a str>,
    pub weapon_switch_timer: f32,
    pub alive: bool,
    /// A sniper scope is up (the fov is narrow).
    pub scoped: bool,
    pub eye: Vec3,
    pub yaw_deg: f32,
    pub underwater_tint: Option<[f32; 4]>,
}

/// The seconds a `ScreenFade` message is shown for, and its colour alpha now (0..=255).
/// Assumption: `duration` and `hold_time` are in 1/4096 s, `FFADE_IN` (flag bit 0 clear) holds
/// the colour for `hold_time` and then fades out over `duration`; `FFADE_OUT` fades in over
/// `duration`; `FFADE_STAYOUT` (bit 2) keeps it.
pub fn screen_fade_alpha(fade: &ScreenFade, now: f64) -> f32 {
    let dur = f64::from(fade.duration) / 4096.0;
    let hold = f64::from(fade.hold_time) / 4096.0;
    let t = (now - fade.started_at).max(0.0);
    let a = f64::from(fade.color[3]);
    let fade_out = fade.flags & 1 != 0;
    let stay = fade.flags & 4 != 0;
    let v = if fade_out {
        let k = if dur > 0.0 { (t / dur).clamp(0.0, 1.0) } else { 1.0 };
        if stay || t < dur + hold { a * k } else { 0.0 }
    } else if t < hold {
        a
    } else if dur > 0.0 {
        a * (1.0 - ((t - hold) / dur).clamp(0.0, 1.0))
    } else {
        0.0
    };
    v as f32
}

fn status_color(icon: &StatusIcon, flash_on: bool) -> Option<Color32> {
    if icon.status == 0 || (icon.status == 2 && !flash_on) {
        return None;
    }
    Some(match icon.color {
        Some([r, g, b]) => Color32::from_rgb(r, g, b),
        None => HUD_AMBER,
    })
}

/// Optional CS 1.6 CVar, localization, overview, and weapon-bucket context for `draw_cs16_hud_with_extras`.
/// Persistent HUD animation/fade timers (`CHudHealth`, `CHudBattery`, `CHudAmmo`, `CHudAccountBalance`, `CHudRadar`).
#[derive(Debug, Clone, Default)]
pub struct HudAnimState {
    pub last_health: Option<u8>,
    pub health_fade: f32,
    pub last_armor: Option<i16>,
    pub armor_fade: f32,
    pub last_ammo: Option<(i8, u8)>,
    pub ammo_fade: f32,
    pub last_money: Option<i32>,
    pub money_delta: i32,
    pub money_fade: f32,
    pub money_delta_fade: f32,
    /// Active radio-sender flash expiry timestamp (`now + 3.3s`, 22 toggles @ 0.15s) per 1-based sender slot.
    pub radio_flashes: HashMap<u8, f64>,
}

impl HudAnimState {
    pub fn step(&mut self, dt: f32, hud: &HudState, now: f64) {
        let dt = dt.clamp(0.0, 0.25);
        self.health_fade = (self.health_fade - dt * 20.0).max(0.0);
        self.armor_fade = (self.armor_fade - dt * 20.0).max(0.0);
        self.ammo_fade = (self.ammo_fade - dt * 20.0).max(0.0);
        self.money_fade = (self.money_fade - dt).max(0.0);
        self.money_delta_fade = (self.money_delta_fade - dt).max(0.0);

        if self.last_health != Some(hud.health) {
            if self.last_health.is_some() {
                self.health_fade = 200.0;
            }
            self.last_health = Some(hud.health);
        }
        if self.last_armor != Some(hud.armor) {
            if self.last_armor.is_some() {
                self.armor_fade = 200.0;
            }
            self.last_armor = Some(hud.armor);
        }
        let cur_ammo = hud.current_weapon.map(|c| {
            let res = hud
                .weapons
                .get(&c.weapon_id)
                .and_then(|w| hud.ammo.get(&w.ammo1_type))
                .copied()
                .unwrap_or(0);
            (c.clip, res)
        });
        if self.last_ammo != cur_ammo {
            if self.last_ammo.is_some() {
                self.ammo_fade = 200.0;
            }
            self.last_ammo = cur_ammo;
        }
        if self.last_money != Some(hud.money) {
            if let Some(prev) = self.last_money {
                self.money_delta = hud.money - prev;
                self.money_fade = 5.0;
                self.money_delta_fade = 2.5;
            }
            self.last_money = Some(hud.money);
        }
        self.radio_flashes.retain(|_, exp| *exp > now);
    }

    pub fn trigger_radio(&mut self, sender: u8, now: f64) {
        self.radio_flashes.insert(sender, now + 0.15 * 22.0);
    }
}

/// GoldSrc `MIN_ALPHA = 100` fade curve used by `CHudHealth`, `CHudBattery`, and `CHudAmmo`.
pub fn hud_fade_alpha(fade: f32) -> u8 {
    if fade > 0.0 {
        ((fade / 100.0) * 128.0 + 100.0).round().clamp(100.0, 255.0) as u8
    } else {
        100
    }
}

fn hud_tinted_alpha(rgb: Color32, goldsrc_alpha: u8) -> Color32 {
    let vis_a = ((f32::from(goldsrc_alpha) * 0.60) + 102.0).round().clamp(100.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(rgb.r(), rgb.g(), rgb.b(), vis_a)
}

pub struct HudExtrasContext<'a> {
    pub titles: Option<&'a cs_assets::vgui_scripts::TitlesTable>,
    pub loc: Option<&'a cs_assets::scheme::LocalizationTable>,
    pub overview: Option<&'a OverviewMapTex>,
    pub buckets: Option<&'a WeaponBucketState>,
    pub anim: Option<&'a HudAnimState>,
    pub chat_entries: Option<&'a [HudChatEntry]>,
    pub pickup_history: &'a [PickupHistoryEntry],
    pub hud_draw: bool,
    pub hud_centerid: bool,
    pub hud_saytext_internal: bool,
    pub hud_fastswitch: bool,
    pub cl_radartype: bool,
    pub observer_mode: i32,
    pub con_color: [u8; 3],
    pub scr_centertime: f32,
}

impl Default for HudExtrasContext<'_> {
    fn default() -> Self {
        Self {
            titles: None,
            loc: None,
            overview: None,
            buckets: None,
            anim: None,
            chat_entries: None,
            pickup_history: &[],
            hud_draw: true,
            hud_centerid: true,
            hud_saytext_internal: true,
            hud_fastswitch: false,
            cl_radartype: false,
            observer_mode: 0,
            con_color: [255, 180, 30],
            scr_centertime: 2.0,
        }
    }
}

impl HudExtrasContext<'_> {
    /// Populate CVar-controlled HUD toggles (`hud_draw`, `hud_centerid`, `con_color`, etc.)
    /// from `CVarRegistry`. Shared across native `cs-client` and WASM `cs-web`.
    pub fn apply_cvars(&mut self, cvars: &cs_cvar::CVarRegistry) {
        self.hud_draw = cvars.get_bool("hud_draw");
        self.hud_centerid = cvars.get_bool("hud_centerid");
        self.hud_saytext_internal = cvars.get_bool("hud_saytext_internal");
        self.hud_fastswitch = cvars.get_bool("hud_fastswitch");
        self.cl_radartype = cvars.get_bool("cl_radartype");
        let ct = cvars.get_f32("scr_centertime");
        if ct.is_finite() && ct > 0.0 {
            self.scr_centertime = ct;
        }
        let mut nums = cvars
            .get_str("con_color")
            .split_whitespace()
            .filter_map(|p| p.parse::<u8>().ok());
        if let (Some(r), Some(g), Some(b)) = (nums.next(), nums.next(), nums.next()) {
            self.con_color = [r, g, b];
        }
    }
}

struct HudFontCtx<'a> {
    runtime: &'a mut VguiRuntime,
    vfs: Option<&'a Vfs>,
    tracker_fonts: SchemeFontSet,
    screen_h: u32,
}

pub fn draw_cs16_hud(ctx: &egui::Context, sprites: Option<&HudSprites>, hud: &HudView<'_>) {
    draw_cs16_hud_with_extras(ctx, sprites, hud, &HudExtrasContext::default());
}

pub fn draw_cs16_hud_with_extras(
    ctx: &egui::Context,
    sprites: Option<&HudSprites>,
    hud: &HudView<'_>,
    extras: &HudExtrasContext<'_>,
) {
    draw_cs16_hud_impl(ctx, sprites, hud, extras, None);
}

/// Draw the CS 1.6 HUD using `TrackerScheme.res` `EngineFont` (`VguiFontAtlas`) when `runtime` is provided.
pub fn draw_cs16_hud_with_runtime(
    ctx: &egui::Context,
    sprites: Option<&HudSprites>,
    hud: &HudView<'_>,
    extras: &HudExtrasContext<'_>,
    runtime: &mut VguiRuntime,
    vfs: Option<&Vfs>,
) {
    let screen_h = ctx.content_rect().height().round().max(480.0) as u32;
    let tracker_fonts = runtime.tracker_scheme.fonts.clone();
    draw_cs16_hud_impl(
        ctx,
        sprites,
        hud,
        extras,
        Some(HudFontCtx {
            runtime,
            vfs,
            tracker_fonts,
            screen_h,
        }),
    );
}

fn draw_cs16_hud_impl(
    ctx: &egui::Context,
    sprites: Option<&HudSprites>,
    hud: &HudView<'_>,
    extras: &HudExtrasContext<'_>,
    mut font_ctx: Option<HudFontCtx<'_>>,
) {
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::unique("cs16_authentic_hud"),
    ));
    let state = hud.hud;

    // 1. Night vision overlay and the sniper scope mask.
    if hud.alive && state.night_vision {
        painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(20, 215, 45, 78));
        let mut sy = screen.top();
        let line_col = Color32::from_rgba_unmultiplied(0, 30, 0, 55);
        while sy < screen.bottom() {
            painter.line_segment(
                [Pos2::new(screen.left(), sy), Pos2::new(screen.right(), sy)],
                Stroke::new(1.0, line_col),
            );
            sy += 3.0;
        }
    }
    if hud.alive && hud.scoped {
        draw_sniper_scope(&painter, screen);
    }
    if hud.alive
        && let Some([r, g, b, a]) = hud.underwater_tint
    {
        painter.rect_filled(
            screen,
            0.0,
            Color32::from_rgba_unmultiplied(
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
                (a * 255.0) as u8,
            ),
        );
    }

    // 2. The server's screen fade (flashbang whiteout, death fade, FFADE_MODULATE).
    if let Some(ev) = evaluate_screen_fade(state.screen_fade.as_ref(), hud.now) {
        let [r, g, b, a] = ev.rgba;
        let col = if ev.modulate {
            Color32::from_rgba_unmultiplied(
                (r * 160.0) as u8,
                (g * 160.0) as u8,
                (b * 160.0) as u8,
                (a * 220.0) as u8,
            )
        } else {
            Color32::from_rgba_unmultiplied(
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
                (a * 255.0) as u8,
            )
        };
        painter.rect_filled(screen, 0.0, col);
    }

    let scale = if screen.width() < 640.0 { 0.5 } else { 1.0 };
    let num_h = 24.0 * scale;
    let bottom_y = (screen.bottom() - num_h * 1.5).floor();
    let flash_on = ((hud.time_sec / 0.1) as i32) % 2 == 0;

    if hud.alive {
        let quads = compute_damage_quadrants(&state.damage_history, hud.eye, hud.yaw_deg, hud.now);
        let cx = screen.center().x;
        let cy = screen.center().y;
        let r = 135.0 * scale;
        let thick = 6.0 * scale;
        let span = 38.0 * scale;
        for (i, q) in quads.iter().enumerate() {
            if *q <= 0.02 {
                continue;
            }
            let a = (*q * 210.0).clamp(0.0, 210.0) as u8;
            let col = Color32::from_rgba_unmultiplied(235, 32, 24, a);
            let rect = match i {
                0 => Rect::from_center_size(Pos2::new(cx, cy - r), Vec2::new(span * 2.0, thick)),
                1 => Rect::from_center_size(Pos2::new(cx + r, cy), Vec2::new(thick, span * 2.0)),
                2 => Rect::from_center_size(Pos2::new(cx, cy + r), Vec2::new(span * 2.0, thick)),
                _ => Rect::from_center_size(Pos2::new(cx - r, cy), Vec2::new(thick, span * 2.0)),
            };
            painter.rect_filled(rect, 2.0, col);
        }
    }

    if !extras.hud_draw {
        return;
    }

    let vis = compute_hud_visibility(
        hud.alive,
        i32::from(state.health),
        state.hidden,
        state.timer_visible,
    );

    // 3. Bottom-left: health and armor (`640hud7` cross, suit, digits) matching `FUN_1004d9e0` & `FUN_10046f20`.
    if vis.health_armor {
        let hp_alpha = if state.health < 16 {
            255
        } else {
            extras.anim.map_or(100, |a| hud_fade_alpha(a.health_fade))
        };
        let hp_base = if state.health < 26 { HUD_RED } else { HUD_AMBER };
        let hp_color = hud_tinted_alpha(hp_base, hp_alpha);
        let armor_alpha = extras.anim.map_or(100, |a| hud_fade_alpha(a.armor_fade));
        let armor_color = hud_tinted_alpha(HUD_AMBER, armor_alpha);
        let hp_x = 16.0 * scale;
        let health = i32::from(state.health);
        let armor = i32::from(state.armor);
        if let Some(sp) = sprites {
            let _ = sp.draw_sprite(&painter, "cross", Pos2::new(hp_x, bottom_y), scale, hp_color);
            sp.draw_number(&painter, health, Pos2::new(hp_x + 28.0 * scale, bottom_y), scale, hp_color, 1);

            let armor_x = (screen.width() / 5.0).floor();
            let (empty_spr, full_spr) = if state.armor_type == 1 {
                ("suithelmet_empty", "suithelmet_full")
            } else {
                ("suit_empty", "suit_full")
            };
            let icon_y = bottom_y - 4.0 * scale;
            let _ = sp.draw_sprite(&painter, empty_spr, Pos2::new(armor_x, icon_y), scale, armor_color);
            let fill = (f32::from(state.armor) / 100.0).clamp(0.0, 1.0);
            let _ = sp.draw_sprite_clipped_bottom(&painter, full_spr, Pos2::new(armor_x, icon_y), scale, armor_color, fill);
            sp.draw_number_right(&painter, armor, armor_x + (24.0 + 60.0) * scale, bottom_y, scale, armor_color, 1);
        } else {
            painter.text(
                Pos2::new(hp_x, bottom_y),
                egui::Align2::LEFT_TOP,
                format!("+ {health:3}    A {armor:3}"),
                FontId::monospace(24.0 * scale),
                hp_color,
            );
        }
    }

    // 4. Left-margin status icons (`FUN_10065570`): stacked upward from `screen_h / 2`.
    if hud.alive {
        let mut icon_bot = screen.center().y - 10.0 * scale;
        let icon_x = 16.0 * scale;
        if let Some(sp) = sprites {
            for (name, icon) in state.status_icons.iter().take(4) {
                let Some(col) = status_color(icon, flash_on) else {
                    continue;
                };
                if let Some(sz) = sp.sprite_size(name, scale) {
                    let y = icon_bot - sz.y;
                    let _ = sp.draw_sprite(&painter, name, Pos2::new(icon_x, y), scale, col);
                    icon_bot = y - 5.0 * scale;
                }
            }
            for (dmg, started) in &state.damage_history {
                if hud.now - *started <= 2.5 {
                    for haz in damage_hazard_icons(dmg.bits) {
                        let col = if flash_on { HUD_RED } else { HUD_AMBER };
                        if let Some(sz) = sp.sprite_size(haz, scale) {
                            let y = icon_bot - sz.y;
                            let _ = sp.draw_sprite(&painter, haz, Pos2::new(icon_x, y), scale, col);
                            icon_bot = y - 5.0 * scale;
                        }
                    }
                }
            }
        }
    }

    // 5. Bottom-centre: the round timer (`FUN_1004dd80` with progressive red blink <= 30s).
    let center_x = screen.center().x;
    if vis.timer {
        let rem = state.round_seconds_left(hud.now);
        let secs_total = rem.ceil().max(0.0) as i32;
        let mins = secs_total / 60;
        let secs = secs_total % 60;
        let timer_col = if rem <= 0.0 {
            Color32::from_rgb(255, 16, 16)
        } else if rem <= 30.0 {
            let iv = if rem < 3.0 {
                0.05
            } else if rem < 6.0 {
                0.1
            } else if rem < 11.0 {
                0.2
            } else if rem < 21.0 {
                0.4
            } else {
                0.8
            };
            if ((hud.time_sec / iv) as i32) % 2 == 0 {
                Color32::from_rgb(255, 16, 16)
            } else {
                hud_tinted_alpha(HUD_AMBER, 100)
            }
        } else {
            hud_tinted_alpha(HUD_AMBER, 100)
        };
        if let Some(sp) = sprites {
            let mut tx = center_x - 60.0 * scale;
            let _ = sp.draw_sprite(&painter, "stopwatch", Pos2::new(tx, bottom_y), scale, timer_col);
            tx += 28.0 * scale;
            tx += sp.draw_number(&painter, mins, Pos2::new(tx, bottom_y), scale, timer_col, 1);
            let dot_sz = (3.0 * scale).max(2.0);
            let colon_x = tx + 3.0 * scale;
            for dy in [7.0, 16.0] {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(colon_x, bottom_y + dy * scale), Vec2::splat(dot_sz)),
                    0.0,
                    timer_col,
                );
            }
            tx += 10.0 * scale;
            sp.draw_number(&painter, secs, Pos2::new(tx, bottom_y), scale, timer_col, 2);
        } else {
            painter.text(
                Pos2::new(center_x, bottom_y + 12.0 * scale),
                egui::Align2::CENTER_CENTER,
                format!("{mins}:{secs:02}"),
                FontId::monospace(24.0 * scale),
                timer_col,
            );
        }
        if let Some(ref sc) = state.scenario
            && sc.alpha > 0
            && !sc.sprite.is_empty()
        {
            let pulse = match sc.flash {
                Some((rate, _)) if rate > 0 => {
                    (hud.time_sec * f32::from(rate).max(2.0)).sin() > 0.0
                }
                _ => true,
            };
            if pulse {
                let sx = center_x - 92.0 * scale;
                let col = HUD_RED.linear_multiply(f32::from(sc.alpha) / 255.0);
                if let Some(sp) = sprites
                    && sp
                        .draw_sprite(&painter, &sc.sprite, Pos2::new(sx, bottom_y), scale, col)
                        .is_some()
                {
                } else {
                    painter.text(
                        Pos2::new(sx, bottom_y + 12.0 * scale),
                        egui::Align2::CENTER_CENTER,
                        "[C4]",
                        FontId::monospace(14.0 * scale),
                        col,
                    );
                }
            }
        }
    }

    // 6. Bottom-right: money (`FUN_10043690`) and clip / reserve / weapon ammo icon (`FUN_10043a60`).
    let right_margin = screen.right() - if screen.width() >= 640.0 { 16.0 } else { 12.0 };
    let money_y = (screen.bottom() - num_h * 3.0).floor();
    if vis.money && (hud.alive || extras.observer_mode == 0) {
        let (money_col, delta_info) = if let Some(anim) = extras.anim
            && anim.money_fade > 0.0
        {
            let t = 1.0 - (anim.money_fade / 5.0).clamp(0.0, 1.0);
            let base_rgb = if anim.money_delta >= 0 { [0u8, 255, 0] } else { [255u8, 0, 0] };
            let r = (f32::from(base_rgb[0]) + (255.0 - f32::from(base_rgb[0])) * t).round() as u8;
            let g = (f32::from(base_rgb[1]) + (160.0 - f32::from(base_rgb[1])) * t).round() as u8;
            let b = (f32::from(base_rgb[2]) * (1.0 - t)).round() as u8;
            let a = ((anim.money_fade / 5.0) * 128.0 + 100.0).round().clamp(100.0, 255.0) as u8;
            let col = hud_tinted_alpha(Color32::from_rgb(r, g, b), a);
            let d = (anim.money_delta_fade > 0.0 && anim.money_delta != 0)
                .then_some((anim.money_delta, ((anim.money_delta_fade / 2.5) * 255.0).clamp(0.0, 255.0) as u8));
            (col, d)
        } else {
            (hud_tinted_alpha(HUD_AMBER, 100), None)
        };
        let dollar_x = right_margin - 20.0 * scale * 5.0 - 18.0 * scale;
        if let Some(sp) = sprites {
            sp.draw_number_right(&painter, state.money, right_margin, money_y, scale, money_col, 1);
            let _ = sp.draw_sprite(&painter, "dollar", Pos2::new(dollar_x, money_y), scale, money_col);
        } else {
            painter.text(
                Pos2::new(right_margin, money_y),
                egui::Align2::RIGHT_TOP,
                format!("$ {}", state.money),
                FontId::monospace(22.0 * scale),
                money_col,
            );
        }
        if let Some((delta, da)) = delta_info {
            let delta_y = money_y - num_h * 1.5;
            let d_rgb = if delta > 0 { Color32::from_rgba_unmultiplied(0, 255, 0, da) } else { Color32::from_rgba_unmultiplied(255, 40, 40, da) };
            let txt = if delta > 0 { format!("+${}", delta) } else { format!("-${}", -delta) };
            hud_paint_text(ctx, &painter, &mut font_ctx, egui::Align2::RIGHT_TOP, Pos2::new(right_margin, delta_y), &txt, 16.0 * scale, d_rgb);
        }
    }

    if vis.ammo
        && let Some(cur) = state.current_weapon
    {
        let reserve = state
            .weapons
            .get(&cur.weapon_id)
            .and_then(|w| state.ammo.get(&w.ammo1_type))
            .copied()
            .unwrap_or(0);
        let ammo_a = extras.anim.map_or(100, |a| hud_fade_alpha(a.ammo_fade));
        let ammo_amber = hud_tinted_alpha(HUD_AMBER, ammo_a);
        let clip_col = if cur.clip == 0 {
            hud_tinted_alpha(HUD_RED, 255)
        } else {
            ammo_amber
        };
        let icon_x = right_margin - 24.0 * scale;
        let has_icon = sprites
            .and_then(|sp| sp.draw_ammo_icon(&painter, cur.weapon_id, Pos2::new(icon_x, bottom_y), scale, ammo_amber))
            .is_some();
        let num_right = if has_icon { icon_x - 6.0 * scale } else { right_margin };
        if cur.clip >= 0 {
            if let Some(sp) = sprites {
                let res_w = sp.draw_number_right(&painter, i32::from(reserve), num_right, bottom_y, scale, ammo_amber, 1);
                let div_x = (num_right - res_w.max(44.0 * scale) - 10.0 * scale).round();
                if sp
                    .draw_sprite(&painter, "divider", Pos2::new(div_x, bottom_y - 6.0 * scale), scale, HUD_AMBER)
                    .is_none()
                {
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(div_x, bottom_y), Vec2::new((2.0 * scale).max(1.0), num_h)),
                        0.0,
                        HUD_AMBER,
                    );
                }
                sp.draw_number_right(&painter, i32::from(cur.clip), div_x - 8.0 * scale, bottom_y, scale, clip_col, 1);
            } else {
                painter.text(
                    Pos2::new(num_right, bottom_y),
                    egui::Align2::RIGHT_TOP,
                    format!("{} | {}", cur.clip, reserve),
                    FontId::monospace(24.0 * scale),
                    clip_col,
                );
            }
        } else if reserve > 0
            && let Some(sp) = sprites
        {
            sp.draw_number_right(&painter, i32::from(reserve), num_right, bottom_y, scale, ammo_amber, 1);
        }
    }

    // 6b. Pickup history cards (`CHudAmmo::DrawHistory`, FUN_10046bc0).
    if vis.ammo && hud.alive && !extras.pickup_history.is_empty() {
        let max_h = 45.0 * scale;
        for (slot, entry) in extras.pickup_history.iter().take(12).enumerate() {
            let Some(alpha) = pickup_history_alpha(hud.now, entry.end_time) else {
                continue;
            };
            let y = pickup_history_slot_y(screen.bottom(), max_h, slot, scale);
            match &entry.kind {
                PickupKind::Weapon { weapon_id, empty } => {
                    let base_col = if *empty { Color32::from_rgb(255, 16, 16) } else { HUD_AMBER };
                    let tint = hud_tinted_alpha(base_col, alpha);
                    let wname = state
                        .weapons
                        .get(weapon_id)
                        .map(|w| w.name.as_str())
                        .or_else(|| weapon_name_from_id(*weapon_id));
                    if let (Some(sp), Some(name)) = (sprites, wname)
                        && let Some(sz) = sp.weapon_card_size(name, scale)
                    {
                        let x = screen.right() - sz.x - 10.0 * scale;
                        let _ = sp.draw_weapon_card(&painter, name, Pos2::new(x, y), scale, tint);
                    } else if let Some(name) = wname {
                        let short = name.strip_prefix("weapon_").unwrap_or(name);
                        hud_paint_text(
                            ctx,
                            &painter,
                            &mut font_ctx,
                            egui::Align2::RIGHT_TOP,
                            Pos2::new(screen.right() - 10.0 * scale, y),
                            short,
                            14.0 * scale,
                            tint,
                        );
                    }
                }
                PickupKind::Ammo { ammo_id, amount } => {
                    let tint = hud_tinted_alpha(HUD_AMBER, alpha);
                    let wid = state
                        .weapons
                        .values()
                        .find(|w| w.ammo1_type == *ammo_id)
                        .map(|w| w.weapon_id);
                    let icon_x = screen.right() - 24.0 * scale;
                    let has_icon = sprites
                        .zip(wid)
                        .and_then(|(sp, w)| sp.draw_ammo_icon(&painter, w, Pos2::new(icon_x, y), scale, tint))
                        .is_some();
                    let num_right = if has_icon { icon_x - 6.0 * scale } else { screen.right() - 24.0 * scale };
                    if let Some(sp) = sprites {
                        sp.draw_number_right(&painter, i32::from(*amount), num_right, y, scale, tint, 1);
                    } else {
                        hud_paint_text(
                            ctx,
                            &painter,
                            &mut font_ctx,
                            egui::Align2::RIGHT_TOP,
                            Pos2::new(num_right, y),
                            &amount.to_string(),
                            14.0 * scale,
                            tint,
                        );
                    }
                }
                PickupKind::Item { classname } => {
                    let tint = hud_tinted_alpha(HUD_AMBER, alpha);
                    let sprite_name = match classname.as_str() {
                        "item_kevlar" => "suit_full",
                        "item_assaultsuit" => "suithelmet_full",
                        "item_thighpack" => "defuser",
                        other => other,
                    };
                    if let Some(sp) = sprites
                        && let Some(sz) = sp.sprite_size(sprite_name, scale)
                    {
                        let x = screen.right() - sz.x - 10.0 * scale;
                        let _ = sp.draw_sprite(&painter, sprite_name, Pos2::new(x, y), scale, tint);
                    } else {
                        hud_paint_text(
                            ctx,
                            &painter,
                            &mut font_ctx,
                            egui::Align2::RIGHT_TOP,
                            Pos2::new(screen.right() - 10.0 * scale, y),
                            classname,
                            14.0 * scale,
                            tint,
                        );
                    }
                }
            }
        }
    }

    // 7. Top-left: the radar of the teammates the server reports (`Radar`) and the bomb.
    if vis.radar {
        draw_radar_with_extras(&painter, sprites, hud, extras, scale);
    }

    // 8. Top-centre: the weapon slot strip (`bucket1..5`) briefly after a switch.
    let bucket_open = extras.buckets.is_some_and(|b| b.active_slot.is_some());
    if (hud.weapon_switch_timer > 0.01 || bucket_open) && vis.weapon_buckets {
        let alpha = if bucket_open {
            240u8
        } else {
            ((hud.weapon_switch_timer / 0.4).min(1.0) * 235.0) as u8
        };
        let active_slot = extras
            .buckets
            .and_then(|b| b.active_slot)
            .or_else(|| {
                state
                    .current_weapon
                    .and_then(|c| state.weapons.get(&c.weapon_id))
                    .map(|w| w.slot + 1)
            })
            .unwrap_or(0);
        let start_x = screen.left() + 148.0 * scale;
        let top_y = 14.0 * scale;
        if let Some(sp) = sprites {
            for slot in 1..=5u8 {
                let has_slot = state
                    .weapons
                    .values()
                    .any(|w| w.slot + 1 == slot && hud.owned.contains(&w.weapon_id));
                let col = if slot == active_slot {
                    Color32::from_rgba_unmultiplied(255, 200, 30, alpha)
                } else if has_slot {
                    Color32::from_rgba_unmultiplied(220, 140, 0, (alpha as f32 * 0.65) as u8)
                } else {
                    Color32::from_rgba_unmultiplied(120, 75, 0, (alpha as f32 * 0.3) as u8)
                };
                let b_name = format!("bucket{slot}");
                let _ = sp.draw_sprite(
                    &painter,
                    &b_name,
                    Pos2::new(start_x + f32::from(slot - 1) * 28.0 * scale, top_y),
                    scale,
                    col,
                );
            }
        }
        if let Some(b) = extras.buckets
            && let Some(sel_slot) = b.active_slot
            && !extras.hud_fastswitch
        {
            let mut slot_weapons: Vec<_> = state
                .weapons
                .values()
                .filter(|w| w.slot + 1 == sel_slot && hud.owned.contains(&w.weapon_id))
                .collect();
            slot_weapons.sort_by_key(|w| w.position);
            let mut wy = top_y + 26.0 * scale;
            let wx = start_x + f32::from(sel_slot.saturating_sub(1)) * 28.0 * scale;
            for w in slot_weapons {
                let is_hi = b.highlighted_weapon_id == Some(w.weapon_id);
                let box_r = Rect::from_min_size(Pos2::new(wx, wy), Vec2::new(118.0 * scale, 22.0 * scale));
                painter.rect_filled(
                    box_r,
                    2.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, if is_hi { 205 } else { 145 }),
                );
                painter.rect_stroke(
                    box_r,
                    2.0,
                    Stroke::new(1.0, if is_hi { HUD_AMBER } else { Color32::from_rgb(130, 80, 15) }),
                    egui::StrokeKind::Inside,
                );
                let short = w.name.strip_prefix("weapon_").unwrap_or(&w.name);
                painter.text(
                    box_r.center(),
                    egui::Align2::CENTER_CENTER,
                    short,
                    FontId::proportional(11.5 * scale),
                    if is_hi { Color32::from_rgb(255, 225, 110) } else { HUD_AMBER },
                );
                wy += 25.0 * scale;
            }
        }
    }

    // 9. Top-right: flashlight icon + battery bar (`Flashlight` / `FlashBat`), then killfeed.
    let show_flashlight = hud.alive
        && (state.hidden & (HIDEHUD_FLASHLIGHT | HIDEHUD_ALL)) == 0
        && (state.flashlight_on || state.flash_battery < 100);
    if show_flashlight {
        let fx = screen.right() - 48.0 * scale;
        let fy = 14.0 * scale;
        let col = if state.flashlight_on {
            Color32::from_rgb(255, 220, 90)
        } else {
            Color32::from_rgba_unmultiplied(255, 160, 0, 140)
        };
        if let Some(sp) = sprites {
            let spr = if state.flashlight_on { "flash_full" } else { "flash_empty" };
            let _ = sp.draw_sprite(&painter, spr, Pos2::new(fx, fy), scale, col);
            if state.flashlight_on {
                let _ = sp.draw_sprite(
                    &painter,
                    "flash_beam",
                    Pos2::new(fx + 20.0 * scale, fy),
                    scale,
                    col,
                );
            }
        }
        let bar_w = 34.0 * scale;
        let bar_h = 4.0 * scale;
        let frac = (f32::from(state.flash_battery) / 100.0).clamp(0.0, 1.0);
        let bg = Rect::from_min_size(Pos2::new(fx, fy + 24.0 * scale), Vec2::new(bar_w, bar_h));
        painter.rect_filled(bg, 0.0, Color32::from_black_alpha(170));
        painter.rect_filled(
            Rect::from_min_size(bg.min, Vec2::new(bar_w * frac, bar_h)),
            0.0,
            col,
        );
    }

    let mut kf_y = if show_flashlight { 48.0 * scale } else { 16.0 * scale };
    let name_of = |slot: u8| -> String {
        if slot == 0 {
            return "world".to_string();
        }
        hud.names
            .get(&(slot - 1))
            .cloned()
            .unwrap_or_else(|| format!("player {slot}"))
    };
    let team_of = |slot: u8| -> Color32 { killfeed_team_color(state, slot) };
    let kf_fallback_sz = (12.5 * scale).clamp(12.0, 17.0);
    let shown_feed: Vec<_> = hud.feed.iter().rev().take(4).collect();
    for entry in shown_feed.into_iter().rev() {
        let mut cursor_x = screen.right() - 16.0 * scale;
        let victim = name_of(entry.victim);
        let killer = name_of(entry.killer);
        let v_col = team_of(entry.victim);
        let k_col = team_of(entry.killer);
        let is_tk = entry.killer != 0
            && entry.killer != entry.victim
            && matches!(
                (state.players.get(&entry.killer), state.players.get(&entry.victim)),
                (Some(kp), Some(vp)) if !kp.team.is_empty() && kp.team == vp.team
            );
        let spr_tint = if is_tk {
            Color32::from_rgb(10, 240, 10)
        } else {
            Color32::from_rgb(255, 80, 0)
        };

        let v_sz = hud_measure_text(ctx, &painter, &mut font_ctx, &victim, kf_fallback_sz);
        cursor_x -= v_sz.x;
        hud_paint_text(
            ctx,
            &painter,
            &mut font_ctx,
            egui::Align2::LEFT_TOP,
            Pos2::new(cursor_x, kf_y),
            &victim,
            kf_fallback_sz,
            v_col,
        );
        cursor_x -= 8.0 * scale;

        let mut drew_icon = false;
        if let Some(sp) = sprites {
            if entry.headshot
                && let Some(hs_sz) = sp.sprite_size("d_headshot", scale)
            {
                cursor_x -= hs_sz.x;
                let _ = sp.draw_sprite(&painter, "d_headshot", Pos2::new(cursor_x, kf_y), scale, spr_tint);
                cursor_x -= 4.0 * scale;
            }
            let d_spr = killfeed_sprite_name(&entry.weapon);
            if let Some(w_sz) = sp.sprite_size(&d_spr, scale) {
                cursor_x -= w_sz.x;
                let _ = sp.draw_sprite(&painter, &d_spr, Pos2::new(cursor_x, kf_y), scale, spr_tint);
                drew_icon = true;
            }
        }
        if !drew_icon {
            let fallback = if entry.headshot {
                format!("[{}] [HS]", entry.weapon)
            } else {
                format!("[{}]", entry.weapon)
            };
            let m_sz = hud_measure_text(ctx, &painter, &mut font_ctx, &fallback, kf_fallback_sz);
            cursor_x -= m_sz.x;
            hud_paint_text(
                ctx,
                &painter,
                &mut font_ctx,
                egui::Align2::LEFT_TOP,
                Pos2::new(cursor_x, kf_y),
                &fallback,
                kf_fallback_sz,
                spr_tint,
            );
        }
        cursor_x -= 8.0 * scale;

        if entry.killer != 0 && entry.killer != entry.victim {
            let k_sz = hud_measure_text(ctx, &painter, &mut font_ctx, &killer, kf_fallback_sz);
            cursor_x -= k_sz.x;
            hud_paint_text(
                ctx,
                &painter,
                &mut font_ctx,
                egui::Align2::LEFT_TOP,
                Pos2::new(cursor_x, kf_y),
                &killer,
                kf_fallback_sz,
                k_col,
            );
        }
        kf_y += 20.0 * scale;
    }

    // 10. The plant / defuse bar (`BarTime`).
    if let Some(bar) = &state.bar
        && bar.duration > 0
    {
        let elapsed = (hud.now - bar.started_at).max(0.0) as f32;
        let progress = (f32::from(bar.start_percent) / 100.0
            + elapsed / f32::from(bar.duration))
        .clamp(0.0, 1.0);
        if progress < 1.0 {
            let bar_w = 220.0 * scale;
            let bar_h = 10.0 * scale;
            let bar_rect = Rect::from_center_size(
                Pos2::new(center_x, screen.center().y + 92.0 * scale),
                Vec2::new(bar_w, bar_h),
            );
            painter.rect_filled(bar_rect, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 180));
            let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(bar_w * progress, bar_h));
            painter.rect_filled(fill_rect, 0.0, HUD_AMBER);
            painter.rect_stroke(bar_rect, 0.0, Stroke::new(1.0, HUD_AMBER), egui::StrokeKind::Outside);
        }
    }

    // 10b. Crosshair target status bar (`StatusText` / `StatusValue` + `hud_centerid`).
    if hud.alive
        && let Some((team_rel, line)) = format_status_bar(state, hud.names, extras.loc)
    {
        let col = match team_rel {
            1 => TEAM_CT_COLOR,
            2 => TEAM_T_COLOR,
            3 => Color32::from_rgb(130, 210, 255),
            _ => HUD_AMBER,
        };
        if extras.hud_centerid {
            let pos = Pos2::new(center_x, screen.center().y + 34.0 * scale);
            hud_paint_text(ctx, &painter, &mut font_ctx, egui::Align2::CENTER_TOP, pos, &line, 14.5 * scale, col);
        } else {
            let pos = Pos2::new(16.0 * scale, bottom_y - 34.0 * scale);
            hud_paint_text(ctx, &painter, &mut font_ctx, egui::Align2::LEFT_BOTTOM, pos, &line, 14.5 * scale, col);
        }
    }

    // 11. Centre text (`HUD_PRINTCENTER`, the centre print).
    if let Some(text) = hud.center {
        let b_pos = Pos2::new(center_x, screen.top() + screen.height() * 0.34);
        let center_col = Color32::from_rgb(extras.con_color[0], extras.con_color[1], extras.con_color[2]);
        hud_paint_text(
            ctx,
            &painter,
            &mut font_ctx,
            egui::Align2::CENTER_CENTER,
            b_pos,
            text,
            16.0 * scale,
            center_col,
        );
    }

    // 11b. Tutor/hint banners (`HudText` / `HudTextArgs` resolved via `titles.txt` + localization).
    let mut hint_y = screen.top() + screen.height() * 0.70;
    for entry in state.hud_texts.iter().rev() {
        let age = hud.now - entry.received_at;
        if !(0.0..=4.2).contains(&age) {
            continue;
        }
        let msg = format_hud_text_entry(&entry.text, &entry.args, extras.titles, extras.loc);
        if !msg.is_empty() {
            let clean_key = entry.text.trim().trim_start_matches('#');
            let is_hint = clean_key.starts_with("Hint_");
            let title_entry = extras.titles.and_then(|t| t.get(&entry.text));
            let (pos, col) = if is_hint {
                let p = Pos2::new(center_x, hint_y);
                hint_y += 18.0 * scale;
                (p, Color32::from_rgb(40, 200, 40))
            } else if let Some(te) = title_entry {
                let px = if te.position[0] < 0.0 {
                    center_x
                } else {
                    screen.left() + screen.width() * te.position[0].clamp(0.05, 0.95)
                };
                let py = if te.position[1] < 0.0 {
                    screen.center().y
                } else {
                    screen.top() + screen.height() * te.position[1].clamp(0.05, 0.95)
                };
                let c = if te.color[0] == 0 && te.color[1] == 0 && te.color[2] == 0 {
                    Color32::from_rgb(255, 196, 64)
                } else {
                    Color32::from_rgb(te.color[0], te.color[1], te.color[2])
                };
                (Pos2::new(px, py), c)
            } else {
                let p = Pos2::new(center_x, hint_y);
                hint_y += 18.0 * scale;
                (p, Color32::from_rgb(255, 212, 96))
            };
            hud_paint_text(
                ctx,
                &painter,
                &mut font_ctx,
                egui::Align2::CENTER_CENTER,
                pos,
                &msg,
                14.0 * scale,
                col,
            );
        }
    }

    // 12. Notifications under the radar, chat bottom-left above the health.
    let mut ny = 150.0 * scale;
    for line in hud.notify {
        hud_paint_text(
            ctx,
            &painter,
            &mut font_ctx,
            egui::Align2::LEFT_TOP,
            Pos2::new(22.0 * scale, ny),
            line,
            13.0 * scale,
            Color32::from_rgb(255, 220, 110),
        );
        ny += 16.0 * scale;
    }
    if extras.hud_saytext_internal {
        let normal_col = Color32::from_rgb(extras.con_color[0], extras.con_color[1], extras.con_color[2]);
        let green_col = CHAT_GREEN_COLOR;
        let mut cy = if !hud.alive || extras.observer_mode != 0 {
            screen.bottom() - 60.0 * scale
        } else {
            screen.bottom() - (num_h * 2.75)
        };
        let fallback_lines: Vec<HudChatEntry>;
        let chat_iter: &[HudChatEntry] = if let Some(entries) = extras.chat_entries {
            entries
        } else {
            fallback_lines = hud
                .chat
                .iter()
                .map(|line| HudChatEntry {
                    text: line.clone(),
                    team: Some("CT".to_string()),
                })
                .collect();
            &fallback_lines
        };
        for entry in chat_iter.iter().rev().take(5) {
            let team_col = chat_team_color(entry.team.as_deref());
            let mut sx = 10.0 * scale;
            let mut cur_col = normal_col;
            for seg in parse_saytext_spans(&entry.text, normal_col, team_col, green_col) {
                if seg.text.is_empty() {
                    cur_col = seg.color;
                    continue;
                }
                cur_col = seg.color;
                hud_paint_text(
                    ctx,
                    &painter,
                    &mut font_ctx,
                    egui::Align2::LEFT_BOTTOM,
                    Pos2::new(sx, cy),
                    &seg.text,
                    13.0 * scale,
                    cur_col,
                );
                let sz = hud_measure_text(ctx, &painter, &mut font_ctx, &seg.text, 13.0 * scale);
                sx += sz.x;
            }
            let _ = cur_col;
            cy -= 17.0 * scale;
        }
    }

    // 12b. TE_TEXTMESSAGE on-screen channel messages (`fx.hud.te_text_messages`) with FX 0/1/2.
    for msg in &state.te_text_messages {
        let fade_in = (f64::from(msg.fadein) / 256.0) as f32;
        let hold = (f64::from(msg.holdtime) / 256.0) as f32;
        let fade_out = (f64::from(msg.fadeout) / 256.0) as f32;
        let fx_time = (f64::from(msg.fxtime.unwrap_or(0)) / 256.0) as f32;
        let elapsed = (hud.now - msg.received_at).max(0.0) as f32;
        let Some((shown_text, alpha_f, typing)) = evaluate_dhud_text_fx(
            &msg.text,
            msg.effect,
            fade_in,
            hold,
            fade_out,
            fx_time,
            elapsed,
        ) else {
            continue;
        };
        if alpha_f <= 0.01 {
            continue;
        }
        let nx = if msg.x < 0 { 0.5 } else { (f32::from(msg.x) / 8192.0).clamp(0.05, 0.95) };
        let ny = if msg.y < 0 { 0.5 } else { (f32::from(msg.y) / 8192.0).clamp(0.05, 0.95) };
        let pos = Pos2::new(screen.left() + screen.width() * nx, screen.top() + screen.height() * ny);
        let base_c = if typing { msg.color2 } else { msg.color1 };
        let a = (alpha_f.clamp(0.0, 1.0) * f32::from(base_c[3].max(180))) as u8;
        let col = Color32::from_rgba_unmultiplied(base_c[0], base_c[1], base_c[2], a);
        hud_paint_text(ctx, &painter, &mut font_ctx, egui::Align2::CENTER_CENTER, pos, &shown_text, 16.0 * scale, col);
    }

    // Spectator bars are drawn exclusively by `SpectatorVguiUi` (`Spectator.res`).
}

struct SayTextSpan {
    color: Color32,
    text: String,
}

/// Split CS 1.6 `SayText` control codes (`\x01` normal, `\x02` author-name team prefix, `\x03` team, `\x04` green).
fn parse_saytext_spans(
    line: &str,
    normal_col: Color32,
    team_col: Color32,
    green_col: Color32,
) -> Vec<SayTextSpan> {
    if let Some(rest) = line.strip_prefix('\x02') {
        if let Some(idx) = rest
            .find(" (RADIO):")
            .or_else(|| rest.find(" :  "))
            .or_else(|| rest.find(" : "))
            .or_else(|| rest.find(':'))
            && idx > 0
        {
            return vec![
                SayTextSpan {
                    color: team_col,
                    text: rest[..idx].to_string(),
                },
                SayTextSpan {
                    color: normal_col,
                    text: rest[idx..].to_string(),
                },
            ];
        }
        return if rest.is_empty() {
            Vec::new()
        } else {
            vec![SayTextSpan {
                color: team_col,
                text: rest.to_string(),
            }]
        };
    }

    let mut spans = Vec::new();
    let mut cur_col = normal_col;
    let mut buf = String::new();
    for ch in line.chars() {
        let next = match ch {
            '\x01' => Some(normal_col),
            '\x03' => Some(team_col),
            '\x04' => Some(green_col),
            '\x02' => continue,
            _ => None,
        };
        if let Some(col) = next {
            if !buf.is_empty() {
                spans.push(SayTextSpan {
                    color: cur_col,
                    text: std::mem::take(&mut buf),
                });
            }
            cur_col = col;
        } else {
            buf.push(ch);
        }
    }
    if !buf.is_empty() {
        spans.push(SayTextSpan { color: cur_col, text: buf });
    }
    spans
}

fn hud_measure_text(
    ctx: &egui::Context,
    painter: &egui::Painter,
    font_ctx: &mut Option<HudFontCtx<'_>>,
    text: &str,
    fallback_size: f32,
) -> Vec2 {
    if let Some(fc) = font_ctx.as_mut()
        && fc.vfs.is_some()
    {
        let m = fc.runtime.font_atlas.measure_text(
            ctx,
            fc.vfs,
            &fc.tracker_fonts,
            "tracker",
            "EngineFont",
            fc.screen_h,
            text,
        );
        if m.x > 0.5 && m.y > 0.5 {
            return m;
        }
    }
    painter
        .layout_no_wrap(text.to_string(), FontId::proportional(fallback_size), Color32::WHITE)
        .size()
}

fn rect_for_anchored_pos(anchor: egui::Align2, pos: Pos2, size: Vec2) -> (TextAlignment, Rect) {
    let w = size.x.max(4.0);
    let h = size.y.max(4.0);
    let x = match anchor.x() {
        egui::Align::LEFT => pos.x,
        egui::Align::Center => pos.x - w * 0.5,
        egui::Align::RIGHT => pos.x - w,
    };
    let y = match anchor.y() {
        egui::Align::TOP => pos.y,
        egui::Align::Center => pos.y - h * 0.5,
        egui::Align::BOTTOM => pos.y - h,
    };
    let align = match (anchor.x(), anchor.y()) {
        (egui::Align::LEFT, egui::Align::TOP) => TextAlignment::NorthWest,
        (egui::Align::Center, egui::Align::TOP) => TextAlignment::North,
        (egui::Align::RIGHT, egui::Align::TOP) => TextAlignment::NorthEast,
        (egui::Align::LEFT, egui::Align::Center) => TextAlignment::West,
        (egui::Align::Center, egui::Align::Center) => TextAlignment::Center,
        (egui::Align::RIGHT, egui::Align::Center) => TextAlignment::East,
        (egui::Align::LEFT, egui::Align::BOTTOM) => TextAlignment::SouthWest,
        (egui::Align::Center, egui::Align::BOTTOM) => TextAlignment::South,
        (egui::Align::RIGHT, egui::Align::BOTTOM) => TextAlignment::SouthEast,
    };
    (align, Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)))
}

#[allow(clippy::too_many_arguments)]
fn hud_paint_text(
    ctx: &egui::Context,
    painter: &egui::Painter,
    font_ctx: &mut Option<HudFontCtx<'_>>,
    anchor: egui::Align2,
    pos: Pos2,
    text: &str,
    fallback_size: f32,
    color: Color32,
) {
    if text.is_empty() {
        return;
    }
    if let Some(fc) = font_ctx.as_mut()
        && fc.vfs.is_some()
    {
        let m = fc.runtime.font_atlas.measure_text(
            ctx,
            fc.vfs,
            &fc.tracker_fonts,
            "tracker",
            "EngineFont",
            fc.screen_h,
            text,
        );
        if m.y > 0.5 {
            let (align, rect) = rect_for_anchored_pos(anchor, pos, m);
            fc.runtime.font_atlas.paint_text(
                painter,
                ctx,
                fc.vfs,
                &fc.tracker_fonts,
                "tracker",
                "EngineFont",
                fc.screen_h,
                rect,
                text,
                color,
                VguiTextDrawOpts {
                    align,
                    process_hotkey: false,
                    proportional_scale: false,
                },
            );
            return;
        }
    }
    let font = FontId::proportional(fallback_size);
    let shadow_a = (u16::from(color.a()) * 190 / 255) as u8;
    painter.text(
        pos + Vec2::new(1.0, 1.0),
        anchor,
        text,
        font.clone(),
        Color32::from_black_alpha(shadow_a),
    );
    painter.text(pos, anchor, text, font, color);
}

/// Team color for a 1-based killfeed player entity index in `HudState::players`.
fn killfeed_team_color(state: &HudState, slot: u8) -> Color32 {
    state
        .players
        .get(&slot)
        .map_or(HUD_AMBER, |p| team_color(&p.team))
}

/// The weapon sprite of a `DeathMsg` weapon name (`d_<name>` in `hud.txt`).
fn killfeed_sprite_name(weapon: &str) -> String {
    let w = weapon.to_ascii_lowercase();
    match w.as_str() {
        "mp5" => "d_mp5navy".to_string(),
        "glock" => "d_glock18".to_string(),
        "grenade" | "hegrenade" => "d_grenade".to_string(),
        "" => "d_skull".to_string(),
        other => format!("d_{other}"),
    }
}

fn draw_radar_with_extras(
    painter: &egui::Painter,
    sprites: Option<&HudSprites>,
    hud: &HudView<'_>,
    extras: &HudExtrasContext<'_>,
    scale: f32,
) {
    let box_size = if scale < 0.75 { 64.0 } else { 128.0 };
    let radius = box_size * 0.5;
    let center = Pos2::new(radius, radius);
    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::splat(box_size));

    if extras.cl_radartype {
        painter.circle_filled(center, radius, Color32::from_rgba_unmultiplied(12, 16, 12, 232));
        if let Some(ov) = extras.overview {
            let uv = ov.spec.world_to_overview_uv([hud.eye.x, hud.eye.y]);
            let span = 0.14f32;
            let uv_rect = Rect::from_min_max(
                Pos2::new((uv[0] - span).clamp(0.0, 1.0 - span * 2.0), (uv[1] - span).clamp(0.0, 1.0 - span * 2.0)),
                Pos2::new((uv[0] + span).clamp(span * 2.0, 1.0), (uv[1] + span).clamp(span * 2.0, 1.0)),
            );
            painter.image(ov.texture.id(), rect, uv_rect, Color32::from_rgba_unmultiplied(190, 225, 190, 150));
        }
        painter.circle_stroke(center, radius - 0.5, Stroke::new(1.5, Color32::from_rgb(200, 200, 200)));
    } else if let Some(tex) = sprites.and_then(|s| s.radar_tex.as_ref()) {
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_rgba_unmultiplied(150, 255, 150, 175),
        );
    } else {
        painter.circle_filled(center, radius, Color32::from_rgba_unmultiplied(25, 75, 25, 155));
        painter.circle_stroke(center, radius - 0.5, Stroke::new(1.2, Color32::from_rgba_unmultiplied(90, 215, 90, 195)));
    }

    let cone_col = Color32::from_rgba_unmultiplied(165, 255, 165, 120);
    let diag = radius * 0.707;
    painter.line_segment([center, Pos2::new(center.x - diag, center.y - diag)], Stroke::new(1.0, cone_col));
    painter.line_segment([center, Pos2::new(center.x + diag, center.y - diag)], Stroke::new(1.0, cone_col));

    let yaw = hud.yaw_deg.to_radians();
    let fwd = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let radar_range = if box_size <= 64.0 { 1024.0 } else { 2048.0 };
    let blip_at = |pos: Vec3| -> Pos2 {
        let delta = pos - hud.eye;
        let mut v = Vec2::new(delta.dot(right) / radar_range, -delta.dot(fwd) / radar_range);
        if v.length() > 0.88 {
            v = v.normalized() * 0.88;
        }
        center + v * radius
    };

    let radio_blink = ((hud.time_sec / 0.15) as i32) % 2 == 0;
    for (&slot, &pos) in &hud.hud.radar {
        let p = blip_at(pos);
        let dz = pos.z - hud.eye.z;
        let is_radio = extras
            .anim
            .and_then(|a| a.radio_flashes.get(&slot))
            .is_some_and(|exp| *exp > hud.now);
        if is_radio && radio_blink {
            let d = 3.0 * scale;
            let rc = Color32::from_rgb(255, 220, 60);
            painter.line_segment([p - Vec2::splat(d), p + Vec2::splat(d)], Stroke::new(1.2, rc));
            painter.line_segment([Pos2::new(p.x - d, p.y + d), Pos2::new(p.x + d, p.y - d)], Stroke::new(1.2, rc));
        } else if dz.abs() > 128.0 {
            let c = Color32::from_rgb(175, 255, 175);
            let s = (2.0 * scale).max(1.5);
            if dz > 128.0 {
                // Upward T / chevron
                painter.rect_filled(Rect::from_center_size(p, Vec2::new(s * 2.0, s * 0.7)), 0.0, c);
                painter.rect_filled(Rect::from_min_size(Pos2::new(p.x - s * 0.4, p.y), Vec2::new(s * 0.8, s * 1.8)), 0.0, c);
            } else {
                // Downward chevron
                painter.rect_filled(Rect::from_center_size(Pos2::new(p.x, p.y + s), Vec2::new(s * 2.0, s * 0.7)), 0.0, c);
                painter.rect_filled(Rect::from_min_size(Pos2::new(p.x - s * 0.4, p.y - s), Vec2::new(s * 0.8, s * 1.8)), 0.0, c);
            }
        } else {
            painter.rect_filled(
                Rect::from_center_size(p, Vec2::splat(3.5 * scale)),
                0.0,
                Color32::from_rgb(175, 255, 175),
            );
        }
    }
    for (flag, pos) in hud.hud.hostages.values() {
        let p = blip_at(*pos);
        if *flag == 0 {
            if ((hud.time_sec / 0.15) as i32) % 2 == 0 {
                let d = 3.0 * scale;
                painter.line_segment([p - Vec2::splat(d), p + Vec2::splat(d)], Stroke::new(1.5, HUD_RED));
                painter.line_segment(
                    [Pos2::new(p.x - d, p.y + d), Pos2::new(p.x + d, p.y - d)],
                    Stroke::new(1.5, HUD_RED),
                );
            }
        } else {
            painter.circle_filled(p, 2.5 * scale, Color32::from_rgb(110, 185, 255));
        }
    }
    if let Some((pos, _planted)) = hud.hud.bomb
        && ((hud.time_sec / 0.35) as i32) % 2 == 0
    {
        painter.rect_filled(
            Rect::from_center_size(blip_at(pos), Vec2::splat(5.0 * scale)),
            0.0,
            HUD_RED,
        );
    }
    painter.rect_filled(Rect::from_center_size(center, Vec2::splat(3.0 * scale)), 0.0, Color32::WHITE);
}

fn draw_sniper_scope(painter: &egui::Painter, screen: Rect) {
    let center = screen.center();
    let radius = (screen.height() * 0.44).max(160.0);
    let black = Color32::BLACK;

    // Solid black surround outside the circular scope ring
    let top_h = (center.y - radius).max(0.0);
    let side_w = (center.x - radius).max(0.0);
    painter.rect_filled(Rect::from_min_max(screen.min, Pos2::new(screen.right(), screen.top() + top_h)), 0.0, black);
    painter.rect_filled(Rect::from_min_max(Pos2::new(screen.left(), center.y + radius), screen.max), 0.0, black);
    painter.rect_filled(Rect::from_min_max(screen.min, Pos2::new(screen.left() + side_w, screen.bottom())), 0.0, black);
    painter.rect_filled(Rect::from_min_max(Pos2::new(center.x + radius, screen.top()), screen.max), 0.0, black);

    // Mask corners of the bounding square with a thick black ring
    let ring_thick = radius * 0.48;
    painter.circle_stroke(center, radius + ring_thick * 0.5, Stroke::new(ring_thick, black));

    // Iconic CS 1.6 thin black sniper crosshairs + mil-dots + red center dot
    let hair = Stroke::new(1.2, black);
    painter.line_segment([Pos2::new(center.x - radius, center.y), Pos2::new(center.x + radius, center.y)], hair);
    painter.line_segment([Pos2::new(center.x, center.y - radius), Pos2::new(center.x, center.y + radius)], hair);

    for step in 1..=3 {
        let d = (step as f32) * (radius * 0.22);
        let tick = 5.0;
        painter.line_segment([Pos2::new(center.x + d, center.y - tick), Pos2::new(center.x + d, center.y + tick)], hair);
        painter.line_segment([Pos2::new(center.x - d, center.y - tick), Pos2::new(center.x - d, center.y + tick)], hair);
        painter.line_segment([Pos2::new(center.x - tick, center.y + d), Pos2::new(center.x + tick, center.y + d)], hair);
        painter.line_segment([Pos2::new(center.x - tick, center.y - d), Pos2::new(center.x + tick, center.y - d)], hair);
    }

    painter.circle_filled(center, 2.2, Color32::from_rgb(230, 30, 20));
}

/// The colour of a `TeamInfo` team name.
pub fn team_color(team: &str) -> Color32 {
    match team {
        "CT" => TEAM_CT_COLOR,
        "TERRORIST" => TEAM_T_COLOR,
        _ => HUD_AMBER,
    }
}

/// Compact GoldSrc-style bottom-right voice speaker badges for active voice chat participants.
pub fn draw_voice_speakers(
    ctx: &egui::Context,
    names: &BTreeMap<u8, String>,
    speakers: &[u8],
    local_talking: bool,
) {
    if !local_talking && speakers.is_empty() {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_voice_speakers"),
    ));
    let mut labels: Vec<(String, Color32)> = Vec::new();
    if local_talking {
        labels.push(("MIC ACTIVE".to_string(), Color32::from_rgb(120, 240, 120)));
    }
    for &slot in speakers {
        let label = names
            .get(&slot)
            .cloned()
            .unwrap_or_else(|| format!("Player #{slot}"));
        labels.push((format!("[VOICE] {label}"), HUD_AMBER));
    }
    let font = FontId::proportional(13.0);
    let mut y = screen.bottom() - 120.0;
    for (text, color) in labels.into_iter().rev().take(6) {
        let rect = Rect::from_min_size(Pos2::new(screen.right() - 190.0, y - 22.0), Vec2::new(176.0, 20.0));
        painter.rect_filled(rect, 3.0, Color32::from_rgba_unmultiplied(12, 16, 12, 185));
        painter.text(
            Pos2::new(rect.left() + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font.clone(),
            color,
        );
        y -= 24.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fade(flags: u16, duration: u16, hold: u16) -> ScreenFade {
        ScreenFade {
            duration,
            hold_time: hold,
            flags,
            color: [255, 255, 255, 200],
            started_at: 10.0,
        }
    }

    #[test]
    fn a_flash_holds_then_fades() {
        // FFADE_IN: the colour holds for one second, then fades out over two.
        let f = fade(0, 8192, 4096);
        assert_eq!(screen_fade_alpha(&f, 10.0), 200.0);
        assert_eq!(screen_fade_alpha(&f, 10.9), 200.0);
        assert!((screen_fade_alpha(&f, 12.0) - 100.0).abs() < 1.0);
        assert_eq!(screen_fade_alpha(&f, 14.0), 0.0);
    }

    #[test]
    fn a_fade_out_rises_and_can_stay() {
        let f = fade(1 | 4, 4096, 0);
        assert_eq!(screen_fade_alpha(&f, 10.0), 0.0);
        assert!((screen_fade_alpha(&f, 10.5) - 100.0).abs() < 1.0);
        assert_eq!(screen_fade_alpha(&f, 99.0), 200.0);
    }

    #[test]
    fn killfeed_names_map_to_hud_sprites() {
        assert_eq!(killfeed_sprite_name("ak47"), "d_ak47");
        assert_eq!(killfeed_sprite_name("mp5"), "d_mp5navy");
        assert_eq!(killfeed_sprite_name("grenade"), "d_grenade");
        assert_eq!(killfeed_sprite_name("hegrenade"), "d_grenade");
        assert_eq!(killfeed_sprite_name(""), "d_skull");
    }

    #[test]
    fn killfeed_team_color_reads_one_based_player_slot() {
        let mut state = HudState::default();
        state.players.insert(
            1,
            cs_clientcore::hud::PlayerScore {
                team: "CT".to_string(),
                ..Default::default()
            },
        );
        assert_eq!(killfeed_team_color(&state, 1), team_color("CT"));
        assert_eq!(killfeed_team_color(&state, 0), HUD_AMBER);
    }

    #[test]
    fn intro_hidehud_shows_only_money() {
        let vis = compute_hud_visibility(
            false,
            0,
            HIDEHUD_WEAPONS | HIDEHUD_FLASHLIGHT | HIDEHUD_HEALTH | HIDEHUD_TIMER,
            true,
        );
        assert!(vis.money);
        assert!(!vis.health_armor);
        assert!(!vis.timer);
        assert!(!vis.ammo);
        assert!(!vis.weapon_buckets);
        assert!(!vis.radar);
        assert!(!vis.crosshair);
    }

    #[test]
    fn intro_without_hidehud_health_shows_only_money() {
        let vis = compute_hud_visibility(false, 100, HIDEHUD_FLASHLIGHT, false);
        assert!(vis.money);
        assert!(!vis.health_armor);
        assert!(!vis.timer);
        assert!(!vis.ammo);
        assert!(!vis.weapon_buckets);
        assert!(!vis.radar);
        assert!(!vis.crosshair);
    }

    #[test]
    fn saytext_spans_apply_valve_stx_team_and_inline_controls() {
        let normal = Color32::from_rgb(255, 180, 30);
        let ct = chat_team_color(Some("CT"));
        let t = chat_team_color(Some("TERRORIST"));
        let spec = chat_team_color(None);
        assert_eq!(ct, TEAM_CT_COLOR);
        assert_eq!(t, TEAM_T_COLOR);
        assert_eq!(spec, CHAT_SPEC_COLOR);

        // Leading \x02 colors only author up to " :  " or " (RADIO):" and resets to normal.
        let all = parse_saytext_spans("\x02Alice :  hi \x04ignore", normal, ct, CHAT_GREEN_COLOR);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].color, ct);
        assert_eq!(all[0].text, "Alice");
        assert_eq!(all[1].color, normal);
        assert_eq!(all[1].text, " :  hi \x04ignore");

        let radio = parse_saytext_spans("\x02Bob (RADIO): Go go go!", normal, t, CHAT_GREEN_COLOR);
        assert_eq!(radio.len(), 2);
        assert_eq!(radio[0].color, t);
        assert_eq!(radio[0].text, "Bob");
        assert_eq!(radio[1].color, normal);
        assert_eq!(radio[1].text, " (RADIO): Go go go!");

        // Inline \x01 / \x03 / \x04 switches cleanly, while stray \x02 does not.
        let inline = parse_saytext_spans(
            "\x01(CT) \x03Bob\x01 :  \x04green\x02still_green",
            normal,
            ct,
            CHAT_GREEN_COLOR,
        );
        assert_eq!(inline.len(), 4);
        assert_eq!((inline[0].color, inline[0].text.as_str()), (normal, "(CT) "));
        assert_eq!((inline[1].color, inline[1].text.as_str()), (ct, "Bob"));
        assert_eq!((inline[2].color, inline[2].text.as_str()), (normal, " :  "));
        assert_eq!((inline[3].color, inline[3].text.as_str()), (CHAT_GREEN_COLOR, "greenstill_green"));
    }

    #[test]
    fn pickup_history_slot_y_and_alpha_match_client_dll() {
        assert_eq!(pickup_history_slot_y(800.0, 45.0, 0, 1.0), 638.0);
        assert_eq!(pickup_history_slot_y(800.0, 45.0, 1, 1.0), 588.0);
        assert_eq!(pickup_history_slot_y(800.0, 45.0, 2, 1.0), 538.0);
        assert_eq!(pickup_history_alpha(10.0, 10.0), None);
        assert_eq!(pickup_history_alpha(11.0, 10.0), None);
        assert_eq!(pickup_history_alpha(10.0, 15.0), Some(255));
        assert_eq!(pickup_history_alpha(14.0, 15.0), Some(80));
        assert_eq!(weapon_name_from_id(16), Some("weapon_usp"));
        assert_eq!(weapon_name_from_id(28), Some("weapon_ak47"));
    }
}
