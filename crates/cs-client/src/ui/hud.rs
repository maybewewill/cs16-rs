//! 1-to-1 Counter-Strike 1.6 HUD rendered directly from real Valve `sprites/hud.txt`
//! (`640hud7.spr`, `640hud1.spr`, `640hud2.spr`, `640hud16.spr`, `radar640.spr`):
//! Health, Armor/Helmet, Buyzone/C4/Defuser left icons, Sprite-Digit Round Timer,
//! Sprite-Digit Money & Clip/Reserve Ammo, Translucent Radar, Weapon-Icon Killfeed,
//! Top Bucket Selector, AWP Sniper Scope, and C4 Plant/Defuse Bar.

use std::collections::{BTreeMap, HashMap, HashSet};

use cs_assets::scheme::{HudSpriteEntry, load_hud_txt};
use cs_assets::spr::GoldSrcSprite;
use cs_clientcore::HudState;
use cs_clientcore::hud::{DeathNotice, ScreenFade, StatusIcon};
use cs_vfs::Vfs;
use egui::{Color32, FontId, Pos2, Rect, Stroke, TextureHandle, Vec2};
use glam::Vec3;

use crate::view_effects::evaluate_screen_fade;

const HUD_AMBER: Color32 = Color32::from_rgb(255, 160, 0);
const HUD_RED: Color32 = Color32::from_rgb(255, 24, 16);
const TEAM_CT_COLOR: Color32 = Color32::from_rgb(153, 204, 255);
const TEAM_T_COLOR: Color32 = Color32::from_rgb(255, 64, 64);

pub struct HudSprites {
    sheets: HashMap<String, (TextureHandle, [f32; 2])>,
    entries: HashMap<String, HudSpriteEntry>,
    radar_tex: Option<TextureHandle>,
}

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

        let mut sheets = HashMap::new();
        for name in ["640hud7", "640hud1", "640hud2", "640hud16", "640hud3", "320hud2"] {
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
            radar_tex,
        }
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

pub fn draw_cs16_hud(ctx: &egui::Context, sprites: Option<&HudSprites>, hud: &HudView<'_>) {
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

    let scale = (screen.height() / 768.0).clamp(0.85, 1.45);
    let bottom_y = screen.bottom() - 40.0 * scale;
    let flash_on = (hud.time_sec * 6.0).sin() > 0.0;

    // 3. Bottom-left: health and armor (`640hud7` cross, suit, digits).
    if hud.alive || state.health > 0 {
        let hp_color = if state.health <= 25 { HUD_RED } else { HUD_AMBER };
        let hp_x = 18.0 * scale;
        let health = i32::from(state.health);
        let armor = i32::from(state.armor);
        if let Some(sp) = sprites {
            let _ = sp.draw_sprite(&painter, "cross", Pos2::new(hp_x, bottom_y + 1.0), scale, hp_color);
            sp.draw_number(&painter, health, Pos2::new(hp_x + 32.0 * scale, bottom_y), scale, hp_color, 1);
            let armor_x = hp_x + 150.0 * scale;
            let shield_name = if state.armor_type == 1 { "suithelmet_full" } else { "suit_full" };
            let _ = sp.draw_sprite(&painter, shield_name, Pos2::new(armor_x, bottom_y + 1.0), scale, HUD_AMBER);
            sp.draw_number(&painter, armor, Pos2::new(armor_x + 32.0 * scale, bottom_y), scale, HUD_AMBER, 1);
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

    // 4. Left-margin status icons: buy zone, bomb, defuser, rescue zone... as the server shows
    // them (`StatusIcon`), with the colour and the flashing it asks for.
    if hud.alive {
        let mut icon_y = screen.center().y - 16.0 * scale;
        let icon_x = 18.0 * scale;
        if let Some(sp) = sprites {
            for (name, icon) in &state.status_icons {
                let Some(col) = status_color(icon, flash_on) else {
                    continue;
                };
                if sp
                    .draw_sprite(&painter, name, Pos2::new(icon_x, icon_y), scale, col)
                    .is_some()
                {
                    icon_y += 38.0 * scale;
                }
            }
        }
    }

    // 5. Bottom-centre: the round timer (stopwatch and sprite digits).
    let center_x = screen.center().x;
    if state.timer_visible {
        let secs_total = state.round_seconds_left(hud.now).ceil() as i32;
        let mins = secs_total / 60;
        let secs = secs_total % 60;
        let timer_col = if secs_total <= 10 { HUD_RED } else { HUD_AMBER };
        if let Some(sp) = sprites {
            let total_w = 24.0 * scale + 8.0 * scale + 21.0 * scale + 10.0 * scale + 42.0 * scale;
            let mut tx = center_x - total_w * 0.5;
            let _ = sp.draw_sprite(&painter, "stopwatch", Pos2::new(tx, bottom_y + 1.0), scale, timer_col);
            tx += 32.0 * scale;
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
    }

    // 6. Bottom-right: money and the clip over the reserve of the weapon in hand.
    let right_x = screen.right() - 22.0 * scale;
    let money_y = bottom_y - 36.0 * scale;
    if hud.alive {
        if let Some(sp) = sprites {
            let num_w = sp.draw_number_right(&painter, state.money, right_x, money_y, scale, HUD_AMBER, 1);
            let dollar_x = (right_x - num_w - 22.0 * scale).min(right_x - 110.0 * scale);
            let _ = sp.draw_sprite(&painter, "dollar", Pos2::new(dollar_x, money_y), scale, HUD_AMBER);
        } else {
            painter.text(
                Pos2::new(right_x, money_y),
                egui::Align2::RIGHT_TOP,
                format!("$ {}", state.money),
                FontId::monospace(22.0 * scale),
                HUD_AMBER,
            );
        }
    }

    if hud.alive
        && let Some(cur) = state.current_weapon
        && cur.clip >= 0
    {
        let reserve = state
            .weapons
            .get(&cur.weapon_id)
            .and_then(|w| state.ammo.get(&w.ammo1_type))
            .copied()
            .unwrap_or(0);
        let ammo_col = if cur.clip == 0 { HUD_RED } else { HUD_AMBER };
        if let Some(sp) = sprites {
            let res_w = sp.draw_number_right(&painter, i32::from(reserve), right_x, bottom_y, scale, HUD_AMBER, 1);
            let div_x = (right_x - res_w.max(64.0 * scale) - 12.0 * scale).round();
            if sp
                .draw_sprite(&painter, "divider", Pos2::new(div_x, bottom_y - 6.0 * scale), scale, HUD_AMBER)
                .is_none()
            {
                painter.line_segment(
                    [Pos2::new(div_x, bottom_y), Pos2::new(div_x, bottom_y + 26.0 * scale)],
                    Stroke::new(2.0, HUD_AMBER),
                );
            }
            sp.draw_number_right(&painter, i32::from(cur.clip), div_x - 10.0 * scale, bottom_y, scale, ammo_col, 1);
        } else {
            painter.text(
                Pos2::new(right_x, bottom_y),
                egui::Align2::RIGHT_TOP,
                format!("{} | {}", cur.clip, reserve),
                FontId::monospace(24.0 * scale),
                ammo_col,
            );
        }
    }

    // 7. Top-left: the radar of the teammates the server reports (`Radar`) and the bomb.
    draw_radar(&painter, sprites, hud, scale);

    // 8. Top-centre: the weapon slot strip (`bucket1..5`) briefly after a switch.
    if hud.weapon_switch_timer > 0.01
        && hud.alive
        && let Some(sp) = sprites
    {
        let alpha = ((hud.weapon_switch_timer / 0.4).min(1.0) * 235.0) as u8;
        let active_slot = state
            .current_weapon
            .and_then(|c| state.weapons.get(&c.weapon_id))
            .map_or(0, |w| w.slot + 1);
        let start_x = screen.left() + 148.0 * scale;
        let top_y = 14.0 * scale;
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

    // 9. Top-right: flashlight icon + battery bar (`Flashlight` / `FlashBat`), then killfeed.
    let show_flashlight = hud.alive && (state.flashlight_on || state.flash_battery < 100);
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
    let kf_font = FontId::proportional((12.5 * scale).clamp(12.0, 17.0));
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
    for entry in hud.feed {
        let mut cursor_x = screen.right() - 16.0 * scale;
        let victim = name_of(entry.victim);
        let killer = name_of(entry.killer);
        let v_col = team_of(entry.victim);
        let k_col = team_of(entry.killer);

        let v_galley = painter.layout_no_wrap(victim.clone(), kf_font.clone(), v_col);
        cursor_x -= v_galley.size().x;
        painter.text(Pos2::new(cursor_x + 1.0, kf_y + 1.0), egui::Align2::LEFT_TOP, &victim, kf_font.clone(), Color32::from_black_alpha(190));
        painter.text(Pos2::new(cursor_x, kf_y), egui::Align2::LEFT_TOP, &victim, kf_font.clone(), v_col);
        cursor_x -= 8.0 * scale;

        let mut drew_icon = false;
        if let Some(sp) = sprites {
            if entry.headshot
                && let Some(hs_sz) = sp.sprite_size("d_headshot", scale)
            {
                cursor_x -= hs_sz.x;
                let _ = sp.draw_sprite(&painter, "d_headshot", Pos2::new(cursor_x, kf_y), scale, HUD_AMBER);
                cursor_x -= 4.0 * scale;
            }
            let d_spr = killfeed_sprite_name(&entry.weapon);
            if let Some(w_sz) = sp.sprite_size(&d_spr, scale) {
                cursor_x -= w_sz.x;
                let _ = sp.draw_sprite(&painter, &d_spr, Pos2::new(cursor_x, kf_y), scale, HUD_AMBER);
                drew_icon = true;
            }
        }
        if !drew_icon {
            let fallback = if entry.headshot {
                format!("[{}] [HS]", entry.weapon)
            } else {
                format!("[{}]", entry.weapon)
            };
            let m_galley = painter.layout_no_wrap(fallback.clone(), kf_font.clone(), HUD_AMBER);
            cursor_x -= m_galley.size().x;
            painter.text(Pos2::new(cursor_x, kf_y), egui::Align2::LEFT_TOP, &fallback, kf_font.clone(), HUD_AMBER);
        }
        cursor_x -= 8.0 * scale;

        if entry.killer != 0 && entry.killer != entry.victim {
            let k_galley = painter.layout_no_wrap(killer.clone(), kf_font.clone(), k_col);
            cursor_x -= k_galley.size().x;
            painter.text(Pos2::new(cursor_x + 1.0, kf_y + 1.0), egui::Align2::LEFT_TOP, &killer, kf_font.clone(), Color32::from_black_alpha(190));
            painter.text(Pos2::new(cursor_x, kf_y), egui::Align2::LEFT_TOP, &killer, kf_font.clone(), k_col);
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

    // 11. Centre text (`HUD_PRINTCENTER`, the centre print).
    if let Some(text) = hud.center {
        let b_pos = Pos2::new(center_x, screen.top() + screen.height() * 0.34);
        let b_font = FontId::proportional(16.0 * scale);
        painter.text(b_pos + Vec2::new(1.0, 1.0), egui::Align2::CENTER_CENTER, text, b_font.clone(), Color32::from_black_alpha(200));
        painter.text(b_pos, egui::Align2::CENTER_CENTER, text, b_font, Color32::from_rgb(255, 196, 64));
    }

    // 12. Notifications under the radar, chat bottom-left above the health.
    let note_font = FontId::proportional(13.0 * scale);
    let mut ny = 150.0 * scale;
    for line in hud.notify {
        painter.text(Pos2::new(22.0 * scale, ny), egui::Align2::LEFT_TOP, line, note_font.clone(), Color32::from_rgb(255, 220, 110));
        ny += 16.0 * scale;
    }
    let mut cy = bottom_y - 46.0 * scale;
    for line in hud.chat.iter().rev().take(5) {
        let x = 22.0 * scale;
        painter.text(Pos2::new(x + 1.0, cy + 1.0), egui::Align2::LEFT_BOTTOM, line, note_font.clone(), Color32::from_black_alpha(180));
        painter.text(Pos2::new(x, cy), egui::Align2::LEFT_BOTTOM, line, note_font.clone(), Color32::from_rgb(255, 220, 110));
        cy -= 17.0 * scale;
    }

    // 12b. TE_TEXTMESSAGE on-screen channel messages (`fx.hud.te_text_messages`).
    for msg in &state.te_text_messages {
        let fade_in = f64::from(msg.fadein) / 256.0;
        let hold = f64::from(msg.holdtime) / 256.0;
        let fade_out = f64::from(msg.fadeout) / 256.0;
        let elapsed = (hud.now - msg.received_at).max(0.0);
        let alpha_f = if elapsed < fade_in && fade_in > 0.0 {
            elapsed / fade_in
        } else if elapsed < fade_in + hold {
            1.0
        } else if fade_out > 0.0 && elapsed < fade_in + hold + fade_out {
            1.0 - (elapsed - fade_in - hold) / fade_out
        } else {
            0.0
        };
        if alpha_f <= 0.01 {
            continue;
        }
        let nx = if msg.x < 0 { 0.5 } else { (f32::from(msg.x) / 8192.0).clamp(0.05, 0.95) };
        let ny = if msg.y < 0 { 0.5 } else { (f32::from(msg.y) / 8192.0).clamp(0.05, 0.95) };
        let pos = Pos2::new(screen.left() + screen.width() * nx, screen.top() + screen.height() * ny);
        let a = (alpha_f.clamp(0.0, 1.0) * f64::from(msg.color1[3].max(180))) as u8;
        let col = Color32::from_rgba_unmultiplied(msg.color1[0], msg.color1[1], msg.color1[2], a);
        painter.text(pos, egui::Align2::CENTER_CENTER, &msg.text, FontId::proportional(16.0 * scale), col);
    }

    // 13. The spectator letterbox bars while dead.
    if !hud.alive {
        let bar_h = 54.0 * scale;
        let top_bar = Rect::from_min_size(screen.min, Vec2::new(screen.width(), bar_h));
        let bot_bar = Rect::from_min_size(
            Pos2::new(screen.left(), screen.bottom() - bar_h),
            Vec2::new(screen.width(), bar_h),
        );
        painter.rect_filled(top_bar, 0.0, Color32::from_black_alpha(225));
        painter.rect_filled(bot_bar, 0.0, Color32::from_black_alpha(225));
        painter.text(
            Pos2::new(center_x, bot_bar.top() + 27.0 * scale),
            egui::Align2::CENTER_CENTER,
            "Spectating",
            FontId::proportional(15.0 * scale),
            HUD_AMBER,
        );
    }
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

fn draw_radar(painter: &egui::Painter, sprites: Option<&HudSprites>, hud: &HudView<'_>, scale: f32) {
    let radius = 58.0 * scale;
    let center = Pos2::new(14.0 * scale + radius, 14.0 * scale + radius);
    let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));

    if let Some(tex) = sprites.and_then(|s| s.radar_tex.as_ref()) {
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_rgba_unmultiplied(150, 255, 150, 175),
        );
    } else {
        painter.circle_filled(center, radius, Color32::from_rgba_unmultiplied(12, 48, 16, 155));
        painter.circle_stroke(center, radius, Stroke::new(1.2, Color32::from_rgba_unmultiplied(80, 210, 80, 180)));
    }

    let cone_col = Color32::from_rgba_unmultiplied(165, 255, 165, 120);
    let diag = radius * 0.707;
    painter.line_segment([center, Pos2::new(center.x - diag, center.y - diag)], Stroke::new(1.0, cone_col));
    painter.line_segment([center, Pos2::new(center.x + diag, center.y - diag)], Stroke::new(1.0, cone_col));

    let yaw = hud.yaw_deg.to_radians();
    let fwd = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let radar_range = 1900.0f32;
    let blip_at = |pos: Vec3| -> Pos2 {
        let delta = pos - hud.eye;
        let mut v = Vec2::new(delta.dot(right) / radar_range, -delta.dot(fwd) / radar_range);
        if v.length() > 0.88 {
            v = v.normalized() * 0.88;
        }
        center + v * radius
    };

    for pos in hud.hud.radar.values() {
        painter.rect_filled(
            Rect::from_center_size(blip_at(*pos), Vec2::splat(4.0 * scale)),
            0.0,
            Color32::from_rgb(175, 255, 175),
        );
    }
    if let Some((pos, _planted)) = hud.hud.bomb
        && (hud.time_sec * 4.5).sin() > 0.0
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
}
