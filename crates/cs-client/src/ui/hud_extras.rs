//! Additional authentic CS 1.6 HUD modules driven by Protocol 48 user messages:
//! - Map overviews (`overviews/<map>.txt` + `overviews/<map>.bmp` green chromakey)
//! - Target status bar (`StatusText` / `StatusValue` + `hud_centerid`)
//! - Directional `Damage` chevrons + left-margin `dmg_*` hazard icons
//! - `WeaponList`-driven weapon buckets + `hud_fastswitch`
//! - `titles.txt` / `HudText` / `HudTextArgs` + `TE_TEXTMESSAGE` AMX HUD/DHUD FX (`0` fade, `1` flicker, `2` typewriter)

use std::collections::{BTreeMap, HashSet};

use cs_assets::scheme::{LocalizationTable, load_overview_image_rgba};
use cs_assets::vgui_scripts::{OverviewSpec, TitlesTable};
use cs_clientcore::HudState;
use cs_clientcore::hud::DamageState;
use egui::{Color32, FontId, Pos2, Rect, Stroke, TextureHandle, Vec2};
use glam::Vec3;

/// Damage bitflags (`const.h` / ReGameDLL `DMG_*`).
pub const DMG_CRUSH: u32 = 1 << 0;
pub const DMG_BURN: u32 = 1 << 3;
pub const DMG_FREEZE: u32 = 1 << 4;
pub const DMG_SHOCK: u32 = 1 << 8;
pub const DMG_DROWN: u32 = 1 << 14;
pub const DMG_NERVEGAS: u32 = 1 << 16;
pub const DMG_POISON: u32 = 1 << 17;
pub const DMG_RADIATION: u32 = 1 << 18;
pub const DMG_ACID: u32 = 1 << 20;

#[derive(Clone)]
pub struct OverviewMapTex {
    pub spec: OverviewSpec,
    pub texture: TextureHandle,
}

impl OverviewMapTex {
    pub fn load(ctx: &egui::Context, vfs: &cs_vfs::Vfs, map_name: &str) -> Option<Self> {
        let spec = OverviewSpec::load(vfs, map_name)?;
        if spec.image.is_empty() {
            return None;
        }
        let img = load_overview_image_rgba(vfs, &spec.image).ok()?;
        let col = egui::ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        );
        let texture = ctx.load_texture(
            format!("cs16_overview_{map_name}"),
            col,
            egui::TextureOptions::LINEAR,
        );
        Some(Self { spec, texture })
    }
}

/// Format GoldSrc `StatusText` (`"1 %c1: %p2  %h: %i3%%"`) using `StatusValue` indices.
pub fn format_status_bar(
    state: &HudState,
    names: &BTreeMap<u8, String>,
    loc: Option<&LocalizationTable>,
) -> Option<(u8, String)> {
    let (_, raw_fmt) = state.status_text.iter().next()?;
    let resolved_fmt = if raw_fmt.contains('#')
        && let Some(l) = loc
    {
        l.resolve_inline(raw_fmt, None)
    } else {
        raw_fmt.clone()
    };
    let trimmed = resolved_fmt.trim();
    if trimmed.is_empty() {
        return None;
    }
    let resolve_lbl = |key: &'static str, fallback: &'static str| -> &str {
        match loc.map(|l| l.resolve(key)) {
            Some(s) if !s.is_empty() && !s.starts_with('#') => s,
            _ => fallback,
        }
    };
    let team_id = state.status_values.get(&1).copied().unwrap_or(0);
    let mut lines_out = Vec::new();
    for raw_line in trimmed.split(['\n', '\r']) {
        let mut line = raw_line.trim_start();
        // GoldSrc StatusText prefixes lines with `"1 "` / `"2 "` alignment/line indices.
        if let Some(first) = line.chars().next()
            && first.is_ascii_digit()
            && line.chars().nth(1).is_some_and(|c| c == ' ' || c == '\t')
        {
            line = line[2..].trim_start();
        }
        if line.is_empty() {
            continue;
        }
        let mut out = String::new();
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] == '%' && i + 1 < chars.len() {
                let kind = chars[i + 1];
                if kind == '%' {
                    out.push('%');
                    i += 2;
                    continue;
                }
                let idx = chars
                    .get(i + 2)
                    .and_then(|c| c.to_digit(10))
                    .map(|d| d as u8);
                let val = idx.and_then(|n| state.status_values.get(&n).copied());
                match kind {
                    'p' => {
                        if let Some(slot1) = val.filter(|&v| v > 0) {
                            let slot0 = (slot1 - 1) as u8;
                            let name = names
                                .get(&slot0)
                                .cloned()
                                .unwrap_or_else(|| format!("Player #{slot1}"));
                            out.push_str(&name);
                        }
                        i += if idx.is_some() { 3 } else { 2 };
                        continue;
                    }
                    'c' => {
                        let lbl = match val.unwrap_or(team_id) {
                            1 => resolve_lbl("#Cstrike_Status_Friend", "Friend"),
                            2 => resolve_lbl("#Cstrike_Status_Enemy", "Enemy"),
                            3 => resolve_lbl("#Cstrike_Status_Hostage", "Hostage"),
                            _ => "",
                        };
                        out.push_str(lbl);
                        i += if idx.is_some() { 3 } else { 2 };
                        continue;
                    }
                    'i' => {
                        out.push_str(&val.unwrap_or(0).to_string());
                        i += if idx.is_some() { 3 } else { 2 };
                        continue;
                    }
                    'h' => {
                        out.push_str("HP");
                        i += if idx.is_some() { 3 } else { 2 };
                        continue;
                    }
                    _ => {}
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        let clean = out.trim();
        if !clean.is_empty() {
            lines_out.push(clean.to_string());
        }
    }
    let clean = lines_out.join("  ");
    (!clean.is_empty()).then_some((team_id.max(0) as u8, clean))
}

/// Active directional damage intensities `[front, right, back, left]` in `0.0..=1.0`.
pub fn compute_damage_quadrants(
    history: &std::collections::VecDeque<(DamageState, f64)>,
    eye: Vec3,
    yaw_deg: f32,
    now: f64,
) -> [f32; 4] {
    let mut quads = [0.0f32; 4];
    let yaw = yaw_deg.to_radians();
    let fwd = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    for (dmg, started) in history {
        let age = (now - *started) as f32;
        if !(0.0..=1.5).contains(&age) {
            continue;
        }
        let fade = 1.0 - (age / 1.5);
        let dir = (dmg.origin - eye).with_z(0.0);
        if dir.length_squared() < 1.0 {
            continue;
        }
        let d = dir.normalize();
        let f_dot = d.dot(fwd);
        let r_dot = d.dot(right);
        if f_dot > 0.35 {
            quads[0] = quads[0].max(fade);
        }
        if r_dot > 0.35 {
            quads[1] = quads[1].max(fade);
        }
        if f_dot < -0.35 {
            quads[2] = quads[2].max(fade);
        }
        if r_dot < -0.35 {
            quads[3] = quads[3].max(fade);
        }
    }
    quads
}

/// Map `Damage` bitmask to GoldSrc `hud.txt` `dmg_*` hazard sprite names.
pub fn damage_hazard_icons(bits: u32) -> Vec<&'static str> {
    let mut out = Vec::new();
    if bits & DMG_BURN != 0 {
        out.push("dmg_heat");
    }
    if bits & DMG_FREEZE != 0 {
        out.push("dmg_cold");
    }
    if bits & DMG_SHOCK != 0 {
        out.push("dmg_shock");
    }
    if bits & (DMG_POISON | DMG_ACID) != 0 {
        out.push("dmg_poison");
    }
    if bits & (DMG_NERVEGAS | DMG_DROWN) != 0 {
        out.push("dmg_gas");
    }
    if bits & DMG_RADIATION != 0 {
        out.push("dmg_rad");
    }
    out
}

/// GoldSrc weapon selection bucket sound cues (`CHudAmmo` `wpn_*.wav`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponBucketSound {
    HudOn,
    MoveSelect,
    Select,
    DenySelect,
}

impl WeaponBucketSound {
    pub fn vfs_path(self) -> &'static str {
        match self {
            Self::HudOn => "common/wpn_hudon.wav",
            Self::MoveSelect => "common/wpn_moveselect.wav",
            Self::Select => "common/wpn_select.wav",
            Self::DenySelect => "common/wpn_denyselect.wav",
        }
    }
}

/// Weapon bucket selector state (`WeaponList` + `hud_fastswitch` 0/1/2 confirm).
#[derive(Debug, Clone, Default)]
pub struct WeaponBucketState {
    pub active_slot: Option<u8>,
    pub highlighted_weapon_id: Option<u8>,
    pub timer_sec: f32,
}

impl WeaponBucketState {
    pub fn on_slot_press(
        &mut self,
        hud: &HudState,
        owned: &HashSet<u8>,
        slot: u8,
        fastswitch: bool,
    ) -> Option<String> {
        let mode = if fastswitch { 1 } else { 0 };
        self.on_slot_press_mode(hud, owned, slot, mode).0
    }

    /// Authentic CS 1.6 `CHudAmmo::SlotInput`:
    /// - `hud_fastswitch 0`: always opens/cycles the top bucket bar (`active_slot = Some(slot)`)
    ///   and requires `MOUSE1` (`confirm_selection`) to equip.
    /// - `hud_fastswitch 1`: equips immediately WITHOUT opening the top bar when the slot has 1
    ///   weapon; opens/cycles the top bucket bar when the slot holds multiple weapons (e.g. grenades).
    /// - `hud_fastswitch >= 2`: always equips immediately WITHOUT opening the top bar.
    pub fn on_slot_press_mode(
        &mut self,
        hud: &HudState,
        owned: &HashSet<u8>,
        slot: u8,
        fastswitch_mode: i32,
    ) -> (Option<String>, Option<WeaponBucketSound>) {
        let mut slot_weapons: Vec<_> = hud
            .weapons
            .values()
            .filter(|w| w.slot + 1 == slot && owned.contains(&w.weapon_id))
            .collect();
        slot_weapons.sort_by_key(|w| w.position);
        if slot_weapons.is_empty() {
            return (None, Some(WeaponBucketSound::DenySelect));
        }
        let was_open = self.active_slot.is_some();
        let next_w = match self
            .highlighted_weapon_id
            .filter(|_| self.active_slot == Some(slot))
        {
            Some(cur_id) => {
                let pos = slot_weapons
                    .iter()
                    .position(|w| w.weapon_id == cur_id)
                    .map_or(0, |i| (i + 1) % slot_weapons.len());
                slot_weapons[pos]
            }
            None => {
                let cur_active = hud.current_weapon.map(|c| c.weapon_id);
                if let Some(cur_idx) =
                    cur_active.and_then(|id| slot_weapons.iter().position(|w| w.weapon_id == id))
                {
                    if slot_weapons.len() > 1 {
                        slot_weapons[(cur_idx + 1) % slot_weapons.len()]
                    } else {
                        slot_weapons[0]
                    }
                } else {
                    slot_weapons[0]
                }
            }
        };
        let immediate = fastswitch_mode >= 2 || (fastswitch_mode == 1 && slot_weapons.len() == 1);
        if immediate {
            self.active_slot = None;
            self.highlighted_weapon_id = Some(next_w.weapon_id);
            self.timer_sec = 0.0;
            (Some(next_w.name.clone()), Some(WeaponBucketSound::Select))
        } else {
            self.active_slot = Some(slot);
            self.highlighted_weapon_id = Some(next_w.weapon_id);
            self.timer_sec = 2.5;
            let snd = if was_open {
                WeaponBucketSound::MoveSelect
            } else {
                WeaponBucketSound::HudOn
            };
            (None, Some(snd))
        }
    }

    pub fn confirm_selection(&mut self, hud: &HudState) -> Option<String> {
        let id = self.highlighted_weapon_id?;
        self.active_slot = None;
        self.timer_sec = 0.0;
        hud.weapons.get(&id).map(|w| w.name.clone())
    }

    pub fn close(&mut self) {
        self.active_slot = None;
        self.timer_sec = 0.0;
    }

    pub fn step(&mut self, dt: f32) {
        self.timer_sec = (self.timer_sec - dt).max(0.0);
        if self.timer_sec <= 0.0 {
            self.active_slot = None;
        }
    }
}

/// Evaluate AMX `TE_TEXTMESSAGE` / `HudText` effect (`0` fade, `1` flicker, `2` typewriter).
pub fn evaluate_dhud_text_fx(
    text: &str,
    effect: u8,
    fadein: f32,
    hold: f32,
    fadeout: f32,
    fxtime: f32,
    elapsed: f32,
) -> Option<(String, f32, bool)> {
    if elapsed < 0.0 {
        return None;
    }
    if effect == 2 {
        let char_rate = fxtime.max(0.01);
        let total_write = char_rate * (text.chars().count().max(1) as f32);
        let total = total_write + hold + fadeout;
        if elapsed >= total {
            return None;
        }
        let shown = if elapsed < total_write {
            ((elapsed / char_rate).floor() as usize + 1).min(text.chars().count())
        } else {
            text.chars().count()
        };
        let alpha = if elapsed > total_write + hold && fadeout > 0.0 {
            1.0 - ((elapsed - total_write - hold) / fadeout).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let sliced: String = text.chars().take(shown).collect();
        return Some((sliced, alpha, elapsed < total_write));
    }
    let total = fadein + hold + fadeout;
    if elapsed >= total {
        return None;
    }
    let mut alpha = if elapsed < fadein && fadein > 0.0 {
        (elapsed / fadein).clamp(0.0, 1.0)
    } else if elapsed < fadein + hold {
        1.0
    } else if fadeout > 0.0 {
        1.0 - ((elapsed - fadein - hold) / fadeout).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if effect == 1 && ((elapsed * 16.0).sin() > 0.4) {
        alpha *= 0.45;
    }
    Some((text.to_string(), alpha, false))
}

/// Render full-screen or spectator map overview (`overviews/<map>.txt` + `.bmp`) with player/bomb/hostage markers.
#[allow(clippy::too_many_arguments)]
pub fn draw_overview_map(
    painter: &egui::Painter,
    rect: Rect,
    ov: &OverviewMapTex,
    hud: &HudState,
    local_eye: Vec3,
    local_yaw_deg: f32,
    draw_cones: bool,
    draw_names: bool,
    names: &BTreeMap<u8, String>,
) {
    painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(10, 14, 10, 210));
    painter.image(
        ov.texture.id(),
        rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );

    let to_screen = |xy: [f32; 2]| -> Pos2 {
        let uv = ov.spec.world_to_overview_uv(xy);
        Pos2::new(
            rect.left() + uv[0].clamp(0.02, 0.98) * rect.width(),
            rect.top() + uv[1].clamp(0.02, 0.98) * rect.height(),
        )
    };

    for (&slot, &pos) in &hud.radar {
        let p = to_screen([pos.x, pos.y]);
        painter.circle_filled(p, 4.0, Color32::from_rgb(130, 220, 255));
        if draw_names && let Some(name) = names.get(&slot.saturating_sub(1)) {
            painter.text(
                p + Vec2::new(6.0, -4.0),
                egui::Align2::LEFT_CENTER,
                name,
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
    }
    for (flag, pos) in hud.hostages.values() {
        let col = if *flag == 0 {
            Color32::from_rgb(255, 60, 60)
        } else {
            Color32::from_rgb(110, 175, 255)
        };
        painter.circle_filled(to_screen([pos.x, pos.y]), 4.0, col);
    }
    if let Some((pos, _)) = hud.bomb {
        painter.rect_filled(
            Rect::from_center_size(to_screen([pos.x, pos.y]), Vec2::splat(7.0)),
            0.0,
            Color32::from_rgb(255, 48, 32),
        );
    }

    let me = to_screen([local_eye.x, local_eye.y]);
    painter.circle_filled(me, 4.5, Color32::from_rgb(255, 210, 60));
    if draw_cones {
        let rad = local_yaw_deg.to_radians();
        let dir = Vec2::new(rad.cos(), -rad.sin()) * 26.0;
        painter.line_segment(
            [me, me + dir],
            Stroke::new(1.5, Color32::from_rgb(255, 220, 80)),
        );
    }
}

/// Format localized tutor/hint text from `HudText` / `HudTextArgs` using `titles.txt` + `LocalizationTable`.
pub fn format_hud_text_entry(
    raw: &str,
    args: &[String],
    titles: Option<&TitlesTable>,
    loc: Option<&LocalizationTable>,
) -> String {
    let base = titles
        .and_then(|t| t.get(raw))
        .map(|e| e.text.as_str())
        .unwrap_or(raw);
    crate::hudtext::format_message_with_titles(base, args, loc, titles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::step_inventory_cycle_mode;
    use cs_clientcore::hud::{CurrentWeapon, WeaponInfo};

    fn sample_hud() -> (HudState, HashSet<u8>) {
        let mut hud = HudState::default();
        hud.weapons.insert(
            28,
            WeaponInfo {
                weapon_id: 28,
                name: "weapon_ak47".into(),
                slot: 0,
                position: 1,
                ammo1_type: 2,
                ammo1_max: 90,
                ammo2_type: 255,
                ammo2_max: 255,
                flags: 0,
            },
        );
        hud.weapons.insert(
            4,
            WeaponInfo {
                weapon_id: 4,
                name: "weapon_hegrenade".into(),
                slot: 3,
                position: 1,
                ammo1_type: 12,
                ammo1_max: 1,
                ammo2_type: 255,
                ammo2_max: 255,
                flags: 0,
            },
        );
        hud.weapons.insert(
            25,
            WeaponInfo {
                weapon_id: 25,
                name: "weapon_flashbang".into(),
                slot: 3,
                position: 2,
                ammo1_type: 11,
                ammo1_max: 2,
                ammo2_type: 255,
                ammo2_max: 255,
                flags: 0,
            },
        );
        hud.current_weapon = Some(CurrentWeapon {
            weapon_id: 28,
            clip: 30,
            is_active: true,
        });
        let owned = HashSet::from([28u8, 4, 25]);
        (hud, owned)
    }

    #[test]
    fn weapon_buckets_respect_hud_fastswitch_0_1_2_and_play_wpn_sounds() {
        let (hud, owned) = sample_hud();
        let mut b = WeaponBucketState::default();

        // Empty slot 2 -> deny sound, bucket stays closed.
        assert_eq!(
            b.on_slot_press_mode(&hud, &owned, 2, 0),
            (None, Some(WeaponBucketSound::DenySelect))
        );
        assert!(b.active_slot.is_none());

        // hud_fastswitch 0: slot 1 opens bucket (HudOn) and waits for MOUSE1 confirm.
        assert_eq!(
            b.on_slot_press_mode(&hud, &owned, 1, 0),
            (None, Some(WeaponBucketSound::HudOn))
        );
        assert_eq!(b.active_slot, Some(1));
        assert_eq!(b.confirm_selection(&hud).as_deref(), Some("weapon_ak47"));
        assert!(b.active_slot.is_none());

        // hud_fastswitch 1: single-item slot 1 equips immediately WITHOUT opening bucket.
        assert_eq!(
            b.on_slot_press_mode(&hud, &owned, 1, 1),
            (Some("weapon_ak47".into()), Some(WeaponBucketSound::Select))
        );
        assert!(b.active_slot.is_none());

        // hud_fastswitch 1: multi-item slot 4 (HE + Flash) opens bucket and cycles with MoveSelect.
        assert_eq!(
            b.on_slot_press_mode(&hud, &owned, 4, 1),
            (None, Some(WeaponBucketSound::HudOn))
        );
        assert_eq!(b.active_slot, Some(4));
        assert_eq!(
            b.on_slot_press_mode(&hud, &owned, 4, 1),
            (None, Some(WeaponBucketSound::MoveSelect))
        );

        // hud_fastswitch 1: wheel (`invnext`) opens bucket; hud_fastswitch 2 equips immediately.
        b.close();
        assert_eq!(
            step_inventory_cycle_mode(&mut b, &hud, &owned, true, 1),
            (None, Some(WeaponBucketSound::HudOn))
        );
        assert!(b.active_slot.is_some());
        let (pick2, snd2) = step_inventory_cycle_mode(&mut b, &hud, &owned, true, 2);
        assert!(pick2.is_some());
        assert_eq!(snd2, Some(WeaponBucketSound::Select));
        assert!(b.active_slot.is_none());
    }
}
