//! 1-to-1 Counter-Strike 1.6 HUD rendered directly from real Valve `sprites/hud.txt`
//! (`640hud7.spr`, `640hud1.spr`, `640hud2.spr`, `640hud16.spr`, `radar640.spr`):
//! Health, Armor/Helmet, Buyzone/C4/Defuser left icons, Sprite-Digit Round Timer,
//! Sprite-Digit Money & Clip/Reserve Ammo, Translucent Radar, Weapon-Icon Killfeed,
//! Top Bucket Selector, AWP Sniper Scope, and C4 Plant/Defuse Bar.

use std::collections::HashMap;

use cs_assets::bsp::SpawnTeam;
use cs_assets::scheme::{HudSpriteEntry, load_hud_txt};
use cs_assets::spr::GoldSrcSprite;
use cs_sim::rules::{ChatMessage, KillFeedEntry, PlantedC4, PlayerState, RoundPhase};
use cs_sim::weapons::WeaponId;
use cs_vfs::Vfs;
use egui::{Color32, FontId, Pos2, Rect, Stroke, TextureHandle, Vec2};
use glam::Vec3;

const HUD_AMBER: Color32 = Color32::from_rgb(255, 160, 0);
const HUD_RED: Color32 = Color32::from_rgb(255, 24, 16);
const HUD_GREEN: Color32 = Color32::from_rgb(0, 196, 0);
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

pub struct HudState<'a> {
    pub local: &'a PlayerState,
    pub all_players: &'a [PlayerState],
    pub phase: RoundPhase,
    pub phase_timer: f32,
    pub in_buy_zone: bool,
    pub in_bomb_zone: bool,
    pub in_rescue_zone: bool,
    pub planted_c4: Option<&'a PlantedC4>,
    pub killfeed: &'a [KillFeedEntry],
    pub chat: &'a [ChatMessage],
    pub banner: Option<&'a str>,
    pub time_sec: f32,
    pub weapon_switch_timer: f32,
    pub spectate_target: Option<&'a PlayerState>,
    pub spectate_free: bool,
    pub radio_menu: Option<u8>,
}

pub fn draw_cs16_hud(
    ctx: &egui::Context,
    sprites: Option<&HudSprites>,
    hud: &HudState<'_>,
) {
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::unique("cs16_authentic_hud"),
    ));

    // 1. NightVision green scanline overlay (`N`) + AWP / Scout sniper scope mask
    if hud.local.alive && hud.local.nvg_active {
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
    if hud.local.alive && hud.local.zoom_level > 0 && hud.local.active_weapon_id().spec().double_zoom {
        draw_sniper_scope(&painter, screen);
    }

    // 2. Damage red vignette & Flashbang whiteout
    if hud.local.damage_flash > 0.02 {
        let a = (hud.local.damage_flash.clamp(0.0, 1.0) * 90.0) as u8;
        painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(180, 15, 10, a));
    }
    if hud.local.flash_alpha > 0.01 {
        let a = (hud.local.flash_alpha.clamp(0.0, 1.0) * 255.0) as u8;
        painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, a));
    }

    let scale = (screen.height() / 768.0).clamp(0.85, 1.45);
    let bottom_y = screen.bottom() - 40.0 * scale;

    // 3. Bottom-left: Health & Armor (exact CS 1.6 `640hud7` cross, suit, and digits)
    let hp_color = if hud.local.health <= 25 {
        HUD_RED
    } else {
        HUD_AMBER
    };
    let hp_x = 18.0 * scale;
    if let Some(sp) = sprites {
        let _ = sp.draw_sprite(&painter, "cross", Pos2::new(hp_x, bottom_y + 1.0), scale, hp_color);
        sp.draw_number(&painter, hud.local.health, Pos2::new(hp_x + 32.0 * scale, bottom_y), scale, hp_color, 1);

        let armor_x = hp_x + 150.0 * scale;
        let shield_name = if hud.local.has_helmet {
            "suithelmet_full"
        } else {
            "suit_full"
        };
        let _ = sp.draw_sprite(&painter, shield_name, Pos2::new(armor_x, bottom_y + 1.0), scale, HUD_AMBER);
        sp.draw_number(&painter, hud.local.armor, Pos2::new(armor_x + 32.0 * scale, bottom_y), scale, HUD_AMBER, 1);
    } else {
        painter.text(
            Pos2::new(hp_x, bottom_y),
            egui::Align2::LEFT_TOP,
            format!("+ {:3}    A {:3}", hud.local.health, hud.local.armor),
            FontId::monospace(24.0 * scale),
            hp_color,
        );
    }

    // 4. Left-margin iconic CS 1.6 status icons (REAL `640hud7` sprites: buyzone cart, C4, defuser pliers)
    if hud.local.alive {
        let mut icon_y = screen.center().y - 16.0 * scale;
        let icon_x = 18.0 * scale;
        if let Some(sp) = sprites {
            if hud.in_buy_zone {
                let _ = sp.draw_sprite(&painter, "buyzone", Pos2::new(icon_x, icon_y), scale, HUD_GREEN);
                icon_y += 38.0 * scale;
            }
            if hud.local.has_weapon(WeaponId::C4) {
                let c4_col = if hud.in_bomb_zone {
                    if (hud.time_sec * 6.0).sin() > 0.0 { HUD_RED } else { HUD_GREEN }
                } else {
                    HUD_GREEN
                };
                let _ = sp.draw_sprite(&painter, "c4", Pos2::new(icon_x, icon_y), scale, c4_col);
                icon_y += 38.0 * scale;
            }
            if hud.local.has_defuser {
                let _ = sp.draw_sprite(&painter, "defuser", Pos2::new(icon_x, icon_y), scale, HUD_GREEN);
                icon_y += 38.0 * scale;
            }
            if hud.in_rescue_zone {
                let _ = sp.draw_sprite(&painter, "rescue", Pos2::new(icon_x, icon_y), scale, HUD_GREEN);
            }
        }
    }

    // 5. Bottom-center: Authentic `640hud7` Round Timer (stopwatch + sprite digits) OR Pulsing Red C4
    let center_x = screen.center().x;
    if hud.phase == RoundPhase::BombPlanted {
        let pulse = (hud.time_sec * 5.0).sin() > 0.0;
        let col = if pulse { HUD_RED } else { Color32::from_rgb(120, 15, 10) };
        if let Some(sp) = sprites {
            let spr = if pulse { "bombticking" } else { "bombticking1" };
            if sp
                .draw_sprite(&painter, spr, Pos2::new(center_x - 16.0 * scale, bottom_y - 4.0 * scale), scale, col)
                .is_none()
            {
                let _ = sp.draw_sprite(&painter, "c4", Pos2::new(center_x - 16.0 * scale, bottom_y - 4.0 * scale), scale, col);
            }
        }
    } else {
        let secs_total = hud.phase_timer.max(0.0).ceil() as i32;
        let mins = secs_total / 60;
        let secs = secs_total % 60;
        let timer_col = if secs_total <= 10 && hud.phase == RoundPhase::Live {
            HUD_RED
        } else {
            HUD_AMBER
        };
        if let Some(sp) = sprites {
            let total_w = 24.0 * scale + 8.0 * scale + 21.0 * scale + 10.0 * scale + 42.0 * scale;
            let mut tx = center_x - total_w * 0.5;
            let _ = sp.draw_sprite(&painter, "stopwatch", Pos2::new(tx, bottom_y + 1.0), scale, timer_col);
            tx += 32.0 * scale;
            tx += sp.draw_number(&painter, mins, Pos2::new(tx, bottom_y), scale, timer_col, 1);
            // Crisp CS 1.6 pixel colon between minutes and seconds
            let dot_sz = (3.0 * scale).max(2.0);
            let colon_x = tx + 3.0 * scale;
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(colon_x, bottom_y + 7.0 * scale), Vec2::splat(dot_sz)),
                0.0,
                timer_col,
            );
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(colon_x, bottom_y + 16.0 * scale), Vec2::splat(dot_sz)),
                0.0,
                timer_col,
            );
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

    // 6. Bottom-right: Real `640hud7` Money (`$` sprite + digits) & Clip | Reserve Ammo
    let right_x = screen.right() - 22.0 * scale;
    let money_y = bottom_y - 36.0 * scale;
    if let Some(sp) = sprites {
        let num_w = sp.draw_number_right(&painter, hud.local.money, right_x, money_y, scale, HUD_AMBER, 1);
        let dollar_x = (right_x - num_w - 22.0 * scale).min(right_x - 110.0 * scale);
        let _ = sp.draw_sprite(&painter, "dollar", Pos2::new(dollar_x, money_y), scale, HUD_AMBER);
    } else {
        painter.text(
            Pos2::new(right_x, money_y),
            egui::Align2::RIGHT_TOP,
            format!("$ {}", hud.local.money),
            FontId::monospace(22.0 * scale),
            HUD_AMBER,
        );
    }

    if let Some(w) = hud.local.active_weapon() {
        let spec = w.id.spec();
        if spec.clip_size > 0 {
            let ammo_col = if w.clip == 0 { HUD_RED } else { HUD_AMBER };
            if let Some(sp) = sprites {
                let res_w = sp.draw_number_right(&painter, w.reserve as i32, right_x, bottom_y, scale, HUD_AMBER, 1);
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
                sp.draw_number_right(
                    &painter,
                    w.clip as i32,
                    div_x - 10.0 * scale,
                    bottom_y,
                    scale,
                    ammo_col,
                    1,
                );
            } else {
                painter.text(
                    Pos2::new(right_x, bottom_y),
                    egui::Align2::RIGHT_TOP,
                    format!("{} | {}", w.clip, w.reserve),
                    FontId::monospace(24.0 * scale),
                    ammo_col,
                );
            }
        }
    }

    // 7. Top-left: Authentic CS 1.6 Translucent Radar (`radar640.spr`) — clean, no extra text underneath
    draw_radar(&painter, sprites, hud, scale);

    // 8. Top-center: Classic CS 1.6 weapon slot bucket strip (`bucket1..5`) briefly after switching weapons
    if hud.weapon_switch_timer > 0.01 && hud.local.alive
        && let Some(sp) = sprites
    {
        let alpha = ((hud.weapon_switch_timer / 0.4).min(1.0) * 235.0) as u8;
        let active_slot = hud.local.active_weapon_id().spec().slot as u8;
        let start_x = screen.left() + 148.0 * scale;
        let top_y = 14.0 * scale;
        for slot in 1..=5u8 {
            let has_slot = hud.local.weapons.iter().any(|w| w.id.spec().slot as u8 == slot);
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
                Pos2::new(start_x + ((slot - 1) as f32) * 28.0 * scale, top_y),
                scale,
                col,
            );
        }
    }

    // 9. Top-right: Authentic CS 1.6 Killfeed with REAL `d_*` weapon silhouette sprites + `d_headshot`!
    let mut kf_y = 16.0 * scale;
    let kf_font = FontId::proportional((12.5 * scale).clamp(12.0, 17.0));
    for entry in hud.killfeed {
        let mut cursor_x = screen.right() - 16.0 * scale;
        let v_col = team_color(entry.victim_team);
        let k_col = team_color(entry.killer_team);

        // Victim name (rightmost)
        let v_galley = painter.layout_no_wrap(entry.victim_name.clone(), kf_font.clone(), v_col);
        cursor_x -= v_galley.size().x;
        painter.text(Pos2::new(cursor_x + 1.0, kf_y + 1.0), egui::Align2::LEFT_TOP, &entry.victim_name, kf_font.clone(), Color32::from_black_alpha(190));
        painter.text(Pos2::new(cursor_x, kf_y), egui::Align2::LEFT_TOP, &entry.victim_name, kf_font.clone(), v_col);
        cursor_x -= 8.0 * scale;

        // Optional real `d_headshot` sprite
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
            if let Some(w_sz) = sp.sprite_size(d_spr, scale) {
                cursor_x -= w_sz.x;
                let _ = sp.draw_sprite(&painter, d_spr, Pos2::new(cursor_x, kf_y), scale, HUD_AMBER);
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

        // Killer name (leftmost)
        let k_galley = painter.layout_no_wrap(entry.killer_name.clone(), kf_font.clone(), k_col);
        cursor_x -= k_galley.size().x;
        painter.text(Pos2::new(cursor_x + 1.0, kf_y + 1.0), egui::Align2::LEFT_TOP, &entry.killer_name, kf_font.clone(), Color32::from_black_alpha(190));
        painter.text(Pos2::new(cursor_x, kf_y), egui::Align2::LEFT_TOP, &entry.killer_name, kf_font.clone(), k_col);

        kf_y += 20.0 * scale;
    }

    // 10. Plant / Defuse progress bar (exact amber outline + fill bar in CS 1.6)
    let (prog_label, progress) = if hud.local.plant_progress > 0.0 {
        (Some("Planting C4..."), hud.local.plant_progress)
    } else if hud.local.defuse_progress > 0.0 {
        (
            Some(if hud.local.has_defuser {
                "Defusing bomb with defuse kit..."
            } else {
                "Defusing bomb..."
            }),
            hud.local.defuse_progress,
        )
    } else {
        (None, 0.0)
    };
    if let Some(label) = prog_label {
        let bar_w = 220.0 * scale;
        let bar_h = 10.0 * scale;
        let bar_rect = Rect::from_center_size(
            Pos2::new(center_x, screen.center().y + 92.0 * scale),
            Vec2::new(bar_w, bar_h),
        );
        painter.rect_filled(bar_rect, 0.0, Color32::from_rgba_unmultiplied(0, 0, 0, 180));
        let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(bar_w * progress.clamp(0.0, 1.0), bar_h));
        painter.rect_filled(fill_rect, 0.0, HUD_AMBER);
        painter.rect_stroke(bar_rect, 0.0, Stroke::new(1.0, HUD_AMBER), egui::StrokeKind::Outside);
        painter.text(
            Pos2::new(center_x, bar_rect.top() - 6.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            FontId::proportional(13.0 * scale),
            HUD_AMBER,
        );
    }

    // 11. Authentic CS 1.6 center status print (ONLY for real events: Bomb planted / Team wins)
    if let Some(banner_text) = hud.banner {
        let b_pos = Pos2::new(center_x, screen.top() + screen.height() * 0.34);
        let b_font = FontId::proportional(16.0 * scale);
        painter.text(b_pos + Vec2::new(1.0, 1.0), egui::Align2::CENTER_CENTER, banner_text, b_font.clone(), Color32::from_black_alpha(200));
        painter.text(b_pos, egui::Align2::CENTER_CENTER, banner_text, b_font, Color32::from_rgb(255, 196, 64));
    }

    // 12. Chat log bottom-left above Health
    let chat_font = FontId::proportional(13.0 * scale);
    let mut cy = bottom_y - 46.0 * scale;
    for msg in hud.chat.iter().rev().take(5) {
        let name_col = msg.team.map(team_color).unwrap_or(HUD_AMBER);
        let prefix = format!("{} : ", msg.sender);
        let p_galley = painter.layout_no_wrap(prefix.clone(), chat_font.clone(), name_col);
        let x = 22.0 * scale;
        painter.text(Pos2::new(x + 1.0, cy + 1.0), egui::Align2::LEFT_BOTTOM, &prefix, chat_font.clone(), Color32::from_black_alpha(180));
        painter.text(Pos2::new(x, cy), egui::Align2::LEFT_BOTTOM, &prefix, chat_font.clone(), name_col);
        painter.text(
            Pos2::new(x + p_galley.size().x, cy),
            egui::Align2::LEFT_BOTTOM,
            &msg.text,
            chat_font.clone(),
            Color32::from_rgb(255, 220, 110),
        );
        cy -= 17.0 * scale;
    }

    // 13. Classic CS 1.6 Radio Command menu (`Z` / `X` / `C`) on left side
    if let Some(rm) = hud.radio_menu
        && hud.local.alive
    {
        let items = radio_menu_entries(rm);
        let title = match rm {
            1 => "Radio Commands",
            2 => "Group Radio Commands",
            _ => "Radio Responses/Reports",
        };
        let rx = 24.0 * scale;
        let mut ry = screen.center().y - 195.0 * scale;
        let rfont = FontId::proportional(13.5 * scale);
        painter.text(Pos2::new(rx, ry), egui::Align2::LEFT_TOP, title, rfont.clone(), HUD_AMBER);
        ry += 22.0 * scale;
        for (idx, (label, _)) in items.iter().enumerate() {
            let line = format!("{}. {label}", idx + 1);
            painter.text(Pos2::new(rx + 1.0, ry + 1.0), egui::Align2::LEFT_TOP, &line, rfont.clone(), Color32::from_black_alpha(190));
            painter.text(Pos2::new(rx, ry), egui::Align2::LEFT_TOP, &line, rfont.clone(), Color32::WHITE);
            ry += 17.0 * scale;
        }
        ry += 8.0 * scale;
        painter.text(Pos2::new(rx, ry), egui::Align2::LEFT_TOP, "0. Exit", rfont, Color32::WHITE);
    }

    // 14. Authentic CS 1.6 Spectator Letterbox Bars (`BottomSpectator.res`) when dead!
    if !hud.local.alive {
        let bar_h = 54.0 * scale;
        let top_bar = Rect::from_min_size(screen.min, Vec2::new(screen.width(), bar_h));
        let bot_bar = Rect::from_min_size(
            Pos2::new(screen.left(), screen.bottom() - bar_h),
            Vec2::new(screen.width(), bar_h),
        );
        painter.rect_filled(top_bar, 0.0, Color32::from_black_alpha(225));
        painter.rect_filled(bot_bar, 0.0, Color32::from_black_alpha(225));
        painter.line_segment(
            [Pos2::new(screen.left(), top_bar.bottom()), Pos2::new(screen.right(), top_bar.bottom())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 150)),
        );
        painter.line_segment(
            [Pos2::new(screen.left(), bot_bar.top()), Pos2::new(screen.right(), bot_bar.top())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 150)),
        );

        let (label, sub) = if hud.spectate_free {
            (
                "Free Look Spectator".to_string(),
                "WASD: Fly  |  Mouse1: Follow Teammate  |  Mouse2: Toggle Mode".to_string(),
            )
        } else if let Some(tgt) = hud.spectate_target {
            (
                format!(
                    "{}   ({} HP  |  ${}  |  {})",
                    tgt.name,
                    tgt.health,
                    tgt.money,
                    tgt.active_weapon_id().spec().display_name
                ),
                "Mouse1: Next Player  |  Mouse2: Free Look".to_string(),
            )
        } else {
            (
                "Spectating — Waiting for next round".to_string(),
                "Mouse2: Free Look".to_string(),
            )
        };
        painter.text(
            Pos2::new(center_x, bot_bar.top() + 18.0 * scale),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(15.0 * scale),
            HUD_AMBER,
        );
        painter.text(
            Pos2::new(center_x, bot_bar.top() + 38.0 * scale),
            egui::Align2::CENTER_CENTER,
            sub,
            FontId::proportional(11.5 * scale),
            Color32::from_rgb(190, 165, 110),
        );
    }
}

pub fn radio_menu_entries(group: u8) -> &'static [(&'static str, &'static str)] {
    match group {
        1 => &[
            ("Cover Me", "radio/ct_coverme.wav"),
            ("You Take the Point", "radio/ct_point.wav"),
            ("Hold This Position", "radio/position.wav"),
            ("Regroup Team", "radio/regroup.wav"),
            ("Follow Me", "radio/followme.wav"),
            ("Taking Fire, Need Assistance!", "radio/fireassis.wav"),
        ],
        2 => &[
            ("Go go go!", "radio/com_go.wav"),
            ("Team, Fall Back!", "radio/fallback.wav"),
            ("Stick Together, Team", "radio/sticktog.wav"),
            ("Get in Position", "radio/com_getinpos.wav"),
            ("Storm the Front!", "radio/stormfront.wav"),
            ("Report in, Team", "radio/com_reportin.wav"),
        ],
        _ => &[
            ("Affirmative / Roger That", "radio/ct_affirm.wav"),
            ("Enemy Spotted", "radio/ct_enemys.wav"),
            ("Need Backup", "radio/ct_backup.wav"),
            ("Sector Clear", "radio/clear.wav"),
            ("I'm in Position", "radio/ct_inpos.wav"),
            ("Reporting In", "radio/ct_reportingin.wav"),
            ("She's gonna Blow!", "radio/blow.wav"),
            ("Negative", "radio/negative.wav"),
            ("Enemy Down", "radio/enemydown.wav"),
        ],
    }
}

fn killfeed_sprite_name(weapon: &str) -> &'static str {
    match weapon.to_ascii_lowercase().as_str() {
        "knife" => "d_knife",
        "ak47" | "cv-47" => "d_ak47",
        "m4a1" => "d_m4a1",
        "awp" => "d_awp",
        "deagle" => "d_deagle",
        "usp" => "d_usp",
        "glock" | "glock18" => "d_glock18",
        "p228" => "d_p228",
        "fiveseven" => "d_fiveseven",
        "elite" | "elites" => "d_elite",
        "m3" => "d_m3",
        "xm1014" => "d_xm1014",
        "mp5" | "mp5navy" => "d_mp5navy",
        "mac10" => "d_mac10",
        "tmp" => "d_tmp",
        "ump45" => "d_ump45",
        "p90" => "d_p90",
        "galil" => "d_galil",
        "famas" => "d_famas",
        "aug" => "d_aug",
        "sg552" => "d_sg552",
        "scout" => "d_scout",
        "g3sg1" => "d_g3sg1",
        "sg550" => "d_sg550",
        "m249" => "d_m249",
        "hegrenade" | "hegren" | "grenade" => "d_grenade",
        _ => "d_skull",
    }
}

fn draw_radar(
    painter: &egui::Painter,
    sprites: Option<&HudSprites>,
    hud: &HudState<'_>,
    scale: f32,
) {
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
        painter.line_segment([Pos2::new(center.x - radius, center.y), Pos2::new(center.x + radius, center.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(70, 180, 70, 100)));
        painter.line_segment([Pos2::new(center.x, center.y - radius), Pos2::new(center.x, center.y + radius)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(70, 180, 70, 100)));
    }

    // FOV wedge lines (exact CS 1.6 90-deg radar cone)
    let cone_col = Color32::from_rgba_unmultiplied(165, 255, 165, 120);
    let diag = radius * 0.707;
    painter.line_segment([center, Pos2::new(center.x - diag, center.y - diag)], Stroke::new(1.0, cone_col));
    painter.line_segment([center, Pos2::new(center.x + diag, center.y - diag)], Stroke::new(1.0, cone_col));

    let yaw = hud.local.yaw_deg.to_radians();
    let fwd = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let radar_range = 1900.0f32;

    // Teammate blips
    for p in hud.all_players {
        if !p.alive || p.id == hud.local.id || p.team != hud.local.team {
            continue;
        }
        let delta = p.mv.origin - hud.local.mv.origin;
        let rx = delta.dot(right) / radar_range;
        let ry = -delta.dot(fwd) / radar_range;
        let mut v = Vec2::new(rx, ry);
        if v.length() > 0.88 {
            v = v.normalized() * 0.88;
        }
        let blip = center + v * radius;
        let col = if p.has_weapon(WeaponId::C4) {
            Color32::from_rgb(255, 80, 30)
        } else {
            Color32::from_rgb(175, 255, 175)
        };
        painter.rect_filled(Rect::from_center_size(blip, Vec2::splat(4.0 * scale)), 0.0, col);
    }

    // Blinking red planted C4 dot
    if let Some(c4) = hud.planted_c4
        && (hud.time_sec * 4.5).sin() > 0.0
    {
        let delta = c4.position - hud.local.mv.origin;
        let mut v = Vec2::new(delta.dot(right) / radar_range, -delta.dot(fwd) / radar_range);
        if v.length() > 0.88 {
            v = v.normalized() * 0.88;
        }
        painter.rect_filled(Rect::from_center_size(center + v * radius, Vec2::splat(5.0 * scale)), 0.0, HUD_RED);
    }

    // Local center dot
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

pub fn team_color(team: SpawnTeam) -> Color32 {
    match team {
        SpawnTeam::CounterTerrorist => TEAM_CT_COLOR,
        SpawnTeam::Terrorist => TEAM_T_COLOR,
    }
}
