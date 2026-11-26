//! Clean-room GoldSrc `SPR v2` (`IDSP`) HUD & world sprites, `sprites/hud.txt` & weapon HUD
//! scripts, and synthesized retro 16-bit mono PCM `.wav` cues for `dm_rusthouse`.

use crate::spr::{SPR_MAGIC, SPR_VERSION};

/// Build all clean-room HUD/world `.spr` files, `sprites/hud.txt`, and `sprites/weapon_*.txt`.
pub fn build_demo_sprites() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();

    // 1. `sprites/640hud7.spr` (256x256 Additive): digits 0..9, health cross, armor/helmet icons,
    //    dollar/plus/minus/divider/stopwatch, status icons, and exact ammo icons at `hud.rs:731-745`.
    let hud7 = build_640hud7_spr();
    out.push(("sprites/640hud7.spr".to_string(), hud7));

    // 2. `sprites/640hud1.spr` (256x256 Additive): killfeed weapon icons (`d_*`) + weapon bucket cards.
    let hud1 = build_640hud1_spr();
    out.push(("sprites/640hud1.spr".to_string(), hud1));

    // 3. `sprites/hud.txt` mapping every HUD element cleanly onto `640hud7` & `640hud1`.
    out.push(("sprites/hud.txt".to_string(), build_hud_txt().into_bytes()));

    // 4. Minimal `sprites/weapon_*.txt` entries for weapon buckets & sniper scope.
    for wpn in [
        "ak47",
        "m4a1",
        "awp",
        "scout",
        "deagle",
        "usp",
        "glock18",
        "p228",
        "fiveseven",
        "elite",
        "mp5navy",
        "ump45",
        "p90",
        "mac10",
        "tmp",
        "galil",
        "famas",
        "aug",
        "sg552",
        "sg550",
        "g3sg1",
        "m3",
        "xm1014",
        "m249",
        "knife",
        "hegrenade",
        "flashbang",
        "smokegrenade",
        "c4",
    ] {
        let mut txt =
            String::from("weapon 640 640hud1 0 128 128 36\nweapon_s 640 640hud1 0 164 128 36\n");
        if matches!(wpn, "awp" | "scout" | "g3sg1" | "sg550") {
            txt.push_str("zoom 640 sniper_scope 0 0 256 256\n");
        }
        out.push((format!("sprites/weapon_{wpn}.txt"), txt.into_bytes()));
    }

    // 5. Tactical Radar (`radar640.spr` + `radaropaque640.spr`)
    let radar = build_radar_spr();
    out.push(("sprites/radar640.spr".to_string(), radar.clone()));
    out.push(("sprites/radaropaque640.spr".to_string(), radar));

    // 6. Sniper Scope (`sniper_scope.spr`, 256x256 AlphaTest)
    out.push((
        "sprites/sniper_scope.spr".to_string(),
        build_sniper_scope_spr(),
    ));

    // 7. Directional damage pain arcs (`640_pain.spr`, 4 frames)
    out.push(("sprites/640_pain.spr".to_string(), build_pain_spr()));

    // 8. World FX sprites (muzzleflash, smoke puff, glow)
    let muzzle = build_muzzleflash_spr();
    for n in ["muzzleflash1", "muzzleflash2", "muzzleflash3"] {
        out.push((format!("sprites/{n}.spr"), muzzle.clone()));
    }
    let smoke = build_radial_spr(32, [215, 220, 212], 1, 1.3);
    out.push(("sprites/gas_puff_01.spr".to_string(), smoke.clone()));
    out.push(("sprites/smokepuff.spr".to_string(), smoke));
    let glow = build_radial_spr(32, [255, 224, 148], 1, 2.2);
    out.push(("sprites/glow01.spr".to_string(), glow.clone()));
    out.push((
        "sprites/ledglow.spr".to_string(),
        build_radial_spr(16, [255, 64, 48], 1, 2.0),
    ));
    out.push(("sprites/radio.spr".to_string(), glow));

    out
}

/// Encode a GoldSrc `SPR v2` (`IDSP`) from one or more paletted frames.
fn encode_spr_v2(
    sprite_type: i32,
    tex_format: i32,
    max_w: u32,
    max_h: u32,
    palette: &[[u8; 3]; 256],
    frames: &[(i32, i32, u32, u32, &[u8])],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(42 + 768 + frames.len() * (max_w * max_h) as usize);
    out.extend_from_slice(SPR_MAGIC);
    out.extend_from_slice(&SPR_VERSION.to_le_bytes());
    out.extend_from_slice(&sprite_type.to_le_bytes());
    out.extend_from_slice(&tex_format.to_le_bytes());
    let radius = (((max_w * max_w + max_h * max_h) as f32).sqrt() * 0.5).to_bits();
    out.extend_from_slice(&radius.to_le_bytes());
    out.extend_from_slice(&(max_w as i32).to_le_bytes());
    out.extend_from_slice(&(max_h as i32).to_le_bytes());
    out.extend_from_slice(&(frames.len() as i32).to_le_bytes());
    out.extend_from_slice(&0.0f32.to_le_bytes()); // beamlength
    out.extend_from_slice(&0i32.to_le_bytes()); // synctype
    out.extend_from_slice(&256u16.to_le_bytes());
    for rgb in palette {
        out.extend_from_slice(rgb);
    }
    for &(ox, oy, w, h, pixels) in frames {
        out.extend_from_slice(&0i32.to_le_bytes()); // single frame
        out.extend_from_slice(&ox.to_le_bytes());
        out.extend_from_slice(&oy.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(h as i32).to_le_bytes());
        out.extend_from_slice(pixels);
    }
    out
}

fn grayscale_palette() -> [[u8; 3]; 256] {
    let mut pal = [[0u8; 3]; 256];
    for (i, slot) in pal.iter_mut().enumerate() {
        let v = i as u8;
        *slot = [v, v, v];
    }
    pal
}

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![0u8; w * h],
        }
    }

    fn set(&mut self, x: usize, y: usize, v: u8) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = v;
        }
    }

    fn fill_rect(&mut self, x0: usize, y0: usize, rw: usize, rh: usize, v: u8) {
        for y in y0..y0 + rh {
            for x in x0..x0 + rw {
                self.set(x, y, v);
            }
        }
    }

    fn stroke_rect(&mut self, x0: usize, y0: usize, rw: usize, rh: usize, thick: usize, v: u8) {
        self.fill_rect(x0, y0, rw, thick, v);
        self.fill_rect(x0, y0 + rh.saturating_sub(thick), rw, thick, v);
        self.fill_rect(x0, y0, thick, rh, v);
        self.fill_rect(x0 + rw.saturating_sub(thick), y0, thick, rh, v);
    }
}

/// Crisp 7-segment tactical HUD digits `0..=9` drawn in a 20x24 cell.
fn draw_hud_digit(c: &mut Canvas, ox: usize, oy: usize, digit: u8) {
    // Segments: a=top, b=tr, c=br, d=bot, e=bl, f=tl, g=mid
    const MASK: [u8; 10] = [
        0b0111111, // 0
        0b0000110, // 1
        0b1011011, // 2
        0b1001111, // 3
        0b1100110, // 4
        0b1101101, // 5
        0b1111101, // 6
        0b0000111, // 7
        0b1111111, // 8
        0b1101111, // 9
    ];
    let m = MASK[(digit as usize) % 10];
    if m & 0b0000001 != 0 {
        c.fill_rect(ox + 3, oy + 1, 14, 3, 245);
    }
    if m & 0b0000010 != 0 {
        c.fill_rect(ox + 15, oy + 3, 3, 9, 245);
    }
    if m & 0b0000100 != 0 {
        c.fill_rect(ox + 15, oy + 12, 3, 9, 245);
    }
    if m & 0b0001000 != 0 {
        c.fill_rect(ox + 3, oy + 20, 14, 3, 245);
    }
    if m & 0b0010000 != 0 {
        c.fill_rect(ox + 2, oy + 12, 3, 9, 245);
    }
    if m & 0b0100000 != 0 {
        c.fill_rect(ox + 2, oy + 3, 3, 9, 245);
    }
    if m & 0b1000000 != 0 {
        c.fill_rect(ox + 4, oy + 10, 12, 3, 245);
    }
}

fn build_640hud7_spr() -> Vec<u8> {
    let mut c = Canvas::new(256, 256);

    // Row 0 (y=0..24): digits 0..9 at x = d * 20
    for d in 0..10u8 {
        draw_hud_digit(&mut c, (d as usize) * 20, 0, d);
    }
    // `divider` at (204, 0, 4, 32)
    c.fill_rect(205, 2, 2, 28, 220);

    // Row 1 (y=24..48):
    // `cross` (health medkit cross) at (0, 24, 24, 24)
    c.fill_rect(9, 26, 6, 20, 250);
    c.fill_rect(2, 33, 20, 6, 250);
    // `suit_empty` at (24, 24, 24, 24)
    c.stroke_rect(28, 26, 16, 20, 2, 160);
    // `suit_full` at (48, 24, 24, 24)
    c.fill_rect(52, 26, 16, 20, 240);
    // `suithelmet_empty` at (72, 24, 24, 24)
    c.stroke_rect(76, 30, 16, 16, 2, 160);
    c.fill_rect(78, 25, 12, 4, 200);
    // `suithelmet_full` at (96, 24, 24, 24)
    c.fill_rect(100, 30, 16, 16, 240);
    c.fill_rect(102, 25, 12, 4, 250);
    // `dollar` at (120, 24, 18, 24)
    c.fill_rect(128, 25, 2, 22, 245);
    c.fill_rect(123, 28, 12, 3, 245);
    c.fill_rect(123, 28, 3, 8, 245);
    c.fill_rect(123, 34, 12, 3, 245);
    c.fill_rect(132, 34, 3, 8, 245);
    c.fill_rect(123, 41, 12, 3, 245);
    // `plus` at (140, 24, 16, 24)
    c.fill_rect(147, 30, 2, 12, 245);
    c.fill_rect(142, 35, 12, 2, 245);
    // `minus` at (158, 24, 16, 24)
    c.fill_rect(160, 35, 12, 2, 245);
    // `stopwatch` at (176, 24, 24, 24)
    c.stroke_rect(180, 29, 16, 16, 2, 240);
    c.fill_rect(186, 26, 4, 3, 240);
    c.fill_rect(187, 32, 2, 6, 240);
    c.fill_rect(187, 37, 5, 2, 240);
    // `buyzone` (cart/crate icon) at (200, 24, 32, 32)
    c.stroke_rect(204, 28, 24, 22, 2, 245);
    c.fill_rect(210, 34, 12, 3, 245);
    c.fill_rect(210, 40, 12, 3, 245);

    // Row 2 (y=48..72): status/c4/defuser/flashlight icons
    // `c4` at (0, 48, 24, 24)
    c.stroke_rect(3, 52, 18, 16, 2, 240);
    c.fill_rect(8, 56, 8, 8, 240);
    // `defuser` at (24, 48, 24, 24)
    c.fill_rect(30, 52, 12, 4, 240);
    c.fill_rect(34, 56, 4, 12, 240);
    // `flash_empty` at (48, 48, 32, 24)
    c.stroke_rect(50, 54, 22, 12, 2, 180);
    // `flash_full` at (80, 48, 32, 24)
    c.fill_rect(82, 54, 22, 12, 245);
    // `flash_beam` at (112, 48, 24, 24)
    c.fill_rect(114, 54, 18, 12, 200);

    // Rows 3 & 4 (y=72..120): exact 24x24 ammo icons matching `hud.rs::draw_ammo_icon`!
    for &(ax, ay) in &[
        (0, 72),
        (24, 72),
        (48, 72),
        (72, 72),
        (96, 72),
        (120, 72),
        (0, 96),
        (24, 96),
        (48, 96),
        (72, 96),
        (96, 96),
        (120, 96),
        (144, 96),
    ] {
        // Stylized cartridge / shell silhouette (3 brass rounds)
        for dx in [4usize, 10, 16] {
            c.fill_rect(ax + dx, ay + 5, 4, 3, 250);
            c.fill_rect(ax + dx, ay + 8, 4, 11, 210);
        }
    }

    let pal = grayscale_palette();
    encode_spr_v2(2, 1, 256, 256, &pal, &[(0, 0, 256, 256, &c.px)])
}

fn build_640hud1_spr() -> Vec<u8> {
    let mut c = Canvas::new(256, 256);

    // Compact weapon silhouettes for killfeed (`d_*`) along top rows (y=0..120)
    // Rifle icon (0, 0, 48, 16)
    c.fill_rect(4, 6, 36, 4, 245);
    c.fill_rect(36, 7, 10, 2, 245);
    c.fill_rect(22, 10, 5, 5, 220);
    c.fill_rect(2, 8, 8, 6, 220);
    // Sniper icon (48, 0, 56, 16)
    c.fill_rect(52, 7, 46, 3, 245);
    c.fill_rect(66, 3, 18, 3, 245);
    c.fill_rect(50, 8, 10, 5, 220);
    // Pistol icon (104, 0, 32, 16)
    c.fill_rect(108, 4, 20, 4, 245);
    c.fill_rect(108, 8, 6, 6, 220);
    // Knife icon (136, 0, 36, 16)
    c.fill_rect(140, 7, 10, 4, 210);
    c.fill_rect(150, 6, 18, 3, 250);
    // Headshot icon (172, 0, 24, 16) — target reticle
    c.stroke_rect(176, 2, 16, 12, 2, 250);
    c.fill_rect(183, 5, 2, 6, 250);
    c.fill_rect(180, 7, 8, 2, 250);
    // Grenade / skull icon (196, 0, 24, 16)
    c.fill_rect(202, 4, 12, 10, 245);
    c.fill_rect(205, 1, 6, 3, 220);

    // Weapon bucket card (`weapon` at y=128..164, `weapon_s` highlighted at y=164..200)
    c.stroke_rect(2, 130, 124, 32, 1, 130);
    c.fill_rect(18, 144, 86, 6, 190);
    c.fill_rect(54, 150, 10, 8, 160);
    c.stroke_rect(2, 166, 124, 32, 2, 245);
    c.fill_rect(18, 180, 86, 6, 255);
    c.fill_rect(54, 186, 10, 8, 230);

    let pal = grayscale_palette();
    encode_spr_v2(2, 1, 256, 256, &pal, &[(0, 0, 256, 256, &c.px)])
}

fn build_hud_txt() -> String {
    let mut s = String::with_capacity(2048);
    s.push_str("64\n");
    for d in 0..10 {
        s.push_str(&format!("number_{d} 640 640hud7 {} 0 20 24\n", d * 20));
    }
    s.push_str(
        "divider 640 640hud7 204 0 4 32\n\
         cross 640 640hud7 0 24 24 24\n\
         suit_empty 640 640hud7 24 24 24 24\n\
         suit_full 640 640hud7 48 24 24 24\n\
         suithelmet_empty 640 640hud7 72 24 24 24\n\
         suithelmet_full 640 640hud7 96 24 24 24\n\
         dollar 640 640hud7 120 24 18 24\n\
         plus 640 640hud7 140 24 16 24\n\
         minus 640 640hud7 158 24 16 24\n\
         stopwatch 640 640hud7 176 24 24 24\n\
         buyzone 640 640hud7 200 24 32 32\n\
         c4 640 640hud7 0 48 24 24\n\
         defuser 640 640hud7 24 48 24 24\n\
         flash_empty 640 640hud7 48 48 32 24\n\
         flash_full 640 640hud7 80 48 32 24\n\
         flash_beam 640 640hud7 112 48 24 24\n\
         d_ak47 640 640hud1 0 0 48 16\n\
         d_m4a1 640 640hud1 0 0 48 16\n\
         d_galil 640 640hud1 0 0 48 16\n\
         d_famas 640 640hud1 0 0 48 16\n\
         d_aug 640 640hud1 0 0 48 16\n\
         d_sg552 640 640hud1 0 0 48 16\n\
         d_mp5navy 640 640hud1 0 0 48 16\n\
         d_ump45 640 640hud1 0 0 48 16\n\
         d_p90 640 640hud1 0 0 48 16\n\
         d_mac10 640 640hud1 0 0 48 16\n\
         d_tmp 640 640hud1 0 0 48 16\n\
         d_m3 640 640hud1 0 0 48 16\n\
         d_xm1014 640 640hud1 0 0 48 16\n\
         d_m249 640 640hud1 0 0 48 16\n\
         d_awp 640 640hud1 48 0 56 16\n\
         d_scout 640 640hud1 48 0 56 16\n\
         d_g3sg1 640 640hud1 48 0 56 16\n\
         d_sg550 640 640hud1 48 0 56 16\n\
         d_usp 640 640hud1 104 0 32 16\n\
         d_glock18 640 640hud1 104 0 32 16\n\
         d_deagle 640 640hud1 104 0 32 16\n\
         d_p228 640 640hud1 104 0 32 16\n\
         d_fiveseven 640 640hud1 104 0 32 16\n\
         d_elite 640 640hud1 104 0 32 16\n\
         d_knife 640 640hud1 136 0 36 16\n\
         d_headshot 640 640hud1 172 0 24 16\n\
         d_grenade 640 640hud1 196 0 24 16\n\
         d_skull 640 640hud1 196 0 24 16\n",
    );
    s
}

fn build_radar_spr() -> Vec<u8> {
    const N: u32 = 128;
    let mut pal = [[0u8; 3]; 256];
    pal[0] = [0, 0, 0];
    pal[1] = [18, 42, 24];
    pal[2] = [44, 96, 56];
    pal[3] = [92, 176, 108];
    pal[255] = [0, 0, 255]; // transparent key for AlphaTest

    let mut px = vec![0u8; (N * N) as usize];
    let c = (N as f32 - 1.0) * 0.5;
    for y in 0..N {
        for x in 0..N {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let r = (dx * dx + dy * dy).sqrt();
            let idx = if r > 62.0 {
                255
            } else if r >= 60.0 || (r - 32.0).abs() < 0.8 {
                3
            } else if dx.abs() < 0.8 || dy.abs() < 0.8 {
                2
            } else {
                1
            };
            px[(y * N + x) as usize] = idx;
        }
    }
    encode_spr_v2(2, 3, N, N, &pal, &[(0, 0, N, N, &px)])
}

fn build_sniper_scope_spr() -> Vec<u8> {
    const N: u32 = 256;
    let mut pal = [[0u8; 3]; 256];
    pal[0] = [8, 10, 8];
    pal[1] = [180, 22, 22];
    pal[255] = [0, 0, 255]; // AlphaTest transparent center

    let mut px = vec![255u8; (N * N) as usize];
    let c = (N as f32 - 1.0) * 0.5;
    for y in 0..N {
        for x in 0..N {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let r = (dx * dx + dy * dy).sqrt();
            if r > 118.0 {
                px[(y * N + x) as usize] = 0;
            } else if dx.abs() < 1.0 || dy.abs() < 1.0 {
                px[(y * N + x) as usize] = if r < 3.0 { 1 } else { 0 };
            } else if (dx.abs() < 4.0 && (dy as i32) % 24 == 0)
                || (dy.abs() < 4.0 && (dx as i32) % 24 == 0)
            {
                px[(y * N + x) as usize] = 0;
            }
        }
    }
    encode_spr_v2(2, 3, N, N, &pal, &[(0, 0, N, N, &px)])
}

fn build_pain_spr() -> Vec<u8> {
    const N: u32 = 32;
    let pal = grayscale_palette();
    let mut frames_data = Vec::with_capacity(4);
    for dir in 0..4 {
        let mut px = vec![0u8; (N * N) as usize];
        for y in 0..N {
            for x in 0..N {
                let near = match dir {
                    0 => y < 8 && (6..26).contains(&x),
                    1 => x >= 24 && (6..26).contains(&y),
                    2 => y >= 24 && (6..26).contains(&x),
                    _ => x < 8 && (6..26).contains(&y),
                };
                if near {
                    px[(y * N + x) as usize] = 220;
                }
            }
        }
        frames_data.push(px);
    }
    let refs: Vec<(i32, i32, u32, u32, &[u8])> = frames_data
        .iter()
        .map(|f| (0, 0, N, N, f.as_slice()))
        .collect();
    encode_spr_v2(2, 1, N, N, &pal, &refs)
}

fn build_muzzleflash_spr() -> Vec<u8> {
    const N: u32 = 32;
    let mut pal = [[0u8; 3]; 256];
    for (i, slot) in pal.iter_mut().enumerate() {
        let t = i as f32 / 255.0;
        *slot = [
            (255.0 * t.powf(0.65)) as u8,
            (220.0 * t.powf(0.95)) as u8,
            (110.0 * t.powf(1.6)) as u8,
        ];
    }
    let mut px = vec![0u8; (N * N) as usize];
    let c = (N as f32 - 1.0) * 0.5;
    for y in 0..N {
        for x in 0..N {
            let dx = x as f32 - c;
            let dy = y as f32 - c;
            let r = (dx * dx + dy * dy).sqrt() / c;
            let ang = dy.atan2(dx);
            let spikes = 0.72 + 0.28 * (ang * 5.0).cos().abs();
            let intensity = (1.0 - r / spikes).clamp(0.0, 1.0).powf(1.5);
            px[(y * N + x) as usize] = (intensity * 255.0) as u8;
        }
    }
    encode_spr_v2(2, 1, N, N, &pal, &[(-16, 16, N, N, &px)])
}

fn build_radial_spr(size: u32, tint: [u8; 3], tex_format: i32, power: f32) -> Vec<u8> {
    let mut pal = [[0u8; 3]; 256];
    for (i, slot) in pal.iter_mut().enumerate() {
        let t = i as f32 / 255.0;
        *slot = [
            (tint[0] as f32 * t) as u8,
            (tint[1] as f32 * t) as u8,
            (tint[2] as f32 * t) as u8,
        ];
    }
    let mut px = vec![0u8; (size * size) as usize];
    let c = (size as f32 - 1.0) * 0.5;
    for y in 0..size {
        for x in 0..size {
            let dx = (x as f32 - c) / c;
            let dy = (y as f32 - c) / c;
            let r = (dx * dx + dy * dy).sqrt();
            let v = (1.0 - r).clamp(0.0, 1.0).powf(power);
            px[(y * size + x) as usize] = (v * 255.0) as u8;
        }
    }
    let half = (size / 2) as i32;
    encode_spr_v2(
        2,
        tex_format,
        size,
        size,
        &pal,
        &[(-half, half, size, size, &px)],
    )
}

/// Synthesize all clean-room retro 22,050 Hz 16-bit mono PCM `.wav` files for `dm_rusthouse`.
pub fn build_demo_audio() -> Vec<(String, Vec<u8>)> {
    const SR: u32 = 22_050;

    let rifle = synth_gunshot(SR, 0.20, 155.0, 52.0, 0.85, 0.28, 1);
    let rifle2 = synth_gunshot(SR, 0.19, 175.0, 62.0, 0.82, 0.34, 2);
    let silenced = synth_gunshot(SR, 0.11, 290.0, 110.0, 0.45, 0.60, 3);
    let sniper = synth_gunshot(SR, 0.32, 115.0, 36.0, 0.96, 0.22, 4);
    let pistol = synth_gunshot(SR, 0.14, 220.0, 78.0, 0.72, 0.44, 5);
    let deagle = synth_gunshot(SR, 0.22, 140.0, 48.0, 0.90, 0.32, 6);
    let smg = synth_gunshot(SR, 0.12, 240.0, 90.0, 0.68, 0.48, 7);
    let shotgun = synth_gunshot(SR, 0.26, 125.0, 40.0, 0.92, 0.25, 8);
    let knife_slash = synth_swipe(SR, 0.12, 1200.0, 320.0, 11);
    let knife_hit = synth_thump(SR, 0.10, 260.0, 75.0, 0.8, 12);
    let step_conc = synth_footstep(SR, 0.08, 130.0, 0.42, 21);
    let step_metal = synth_footstep(SR, 0.09, 340.0, 0.50, 22);
    let step_wood = synth_footstep(SR, 0.08, 195.0, 0.45, 23);
    let reload = synth_click(SR, 0.09, 920.0, 480.0, 31);
    let ui_click = synth_click(SR, 0.04, 1350.0, 880.0, 32);
    let buy = synth_click(SR, 0.10, 660.0, 990.0, 33);
    let flesh_hit = synth_thump(SR, 0.09, 190.0, 68.0, 0.75, 41);
    let headshot = synth_dink(SR, 0.16, 1850.0, 2420.0);
    let die = synth_thump(SR, 0.22, 145.0, 42.0, 0.65, 42);
    let ricochet = synth_dink(SR, 0.14, 2400.0, 960.0);
    let explode = synth_gunshot(SR, 0.42, 85.0, 24.0, 0.98, 0.18, 51);
    let beep = synth_beep(SR, 0.08, 1760.0);

    let mut out = Vec::new();
    let mut map_many = |paths: &[&str], wav: &Vec<u8>| {
        for p in paths {
            out.push((format!("sound/{p}"), wav.clone()));
        }
    };

    map_many(
        &[
            "weapons/ak47-1.wav",
            "weapons/ak47-2.wav",
            "weapons/galil-1.wav",
            "weapons/galil-2.wav",
            "weapons/sg552-1.wav",
            "weapons/sg552-2.wav",
            "weapons/m249-1.wav",
            "weapons/m249-2.wav",
        ],
        &rifle,
    );
    map_many(
        &[
            "weapons/m4a1_unsil-1.wav",
            "weapons/m4a1_unsil-2.wav",
            "weapons/famas-1.wav",
            "weapons/famas-2.wav",
            "weapons/famas-burst.wav",
            "weapons/aug-1.wav",
        ],
        &rifle2,
    );
    map_many(
        &["weapons/m4a1-1.wav", "weapons/usp1.wav", "weapons/usp2.wav"],
        &silenced,
    );
    map_many(
        &[
            "weapons/awp1.wav",
            "weapons/scout_fire-1.wav",
            "weapons/g3sg1-1.wav",
            "weapons/sg550-1.wav",
        ],
        &sniper,
    );
    map_many(
        &[
            "weapons/glock18-1.wav",
            "weapons/glock18-2.wav",
            "weapons/usp_unsil-1.wav",
            "weapons/p228-1.wav",
            "weapons/fiveseven-1.wav",
            "weapons/elite_fire.wav",
        ],
        &pistol,
    );
    map_many(&["weapons/deagle-1.wav", "weapons/deagle-2.wav"], &deagle);
    map_many(
        &[
            "weapons/mp5-1.wav",
            "weapons/mp5-2.wav",
            "weapons/ump45-1.wav",
            "weapons/p90-1.wav",
            "weapons/mac10-1.wav",
            "weapons/tmp-1.wav",
            "weapons/tmp-2.wav",
        ],
        &smg,
    );
    map_many(&["weapons/m3-1.wav", "weapons/xm1014-1.wav"], &shotgun);
    map_many(
        &[
            "weapons/knife_slash1.wav",
            "weapons/knife_slash2.wav",
            "weapons/knife_deploy1.wav",
        ],
        &knife_slash,
    );
    map_many(
        &[
            "weapons/knife_hit1.wav",
            "weapons/knife_hit2.wav",
            "weapons/knife_hit3.wav",
            "weapons/knife_hit4.wav",
            "weapons/knife_stab.wav",
            "weapons/knife_hitwall1.wav",
        ],
        &knife_hit,
    );
    map_many(
        &[
            "player/pl_step1.wav",
            "player/pl_step2.wav",
            "player/pl_step3.wav",
            "player/pl_step4.wav",
            "player/pl_tile1.wav",
            "player/pl_tile2.wav",
            "player/pl_tile3.wav",
            "player/pl_tile4.wav",
        ],
        &step_conc,
    );
    map_many(
        &[
            "player/pl_metal1.wav",
            "player/pl_metal2.wav",
            "player/pl_metal3.wav",
            "player/pl_metal4.wav",
        ],
        &step_metal,
    );
    map_many(
        &[
            "player/pl_dirt1.wav",
            "player/pl_dirt2.wav",
            "player/pl_dirt3.wav",
            "player/pl_dirt4.wav",
        ],
        &step_wood,
    );
    map_many(
        &[
            "weapons/generic_reload.wav",
            "weapons/generic_shot_reload.wav",
            "weapons/clipout1.wav",
            "weapons/clipin1.wav",
            "weapons/sliderelease1.wav",
            "weapons/slideback1.wav",
            "weapons/ak47_clipout.wav",
            "weapons/ak47_clipin.wav",
            "weapons/ak47_boltpull.wav",
            "weapons/m4a1_clipin.wav",
            "weapons/m4a1_clipout.wav",
            "weapons/m4a1_boltpull.wav",
            "weapons/m4a1_deploy.wav",
            "weapons/de_clipout.wav",
            "weapons/de_clipin.wav",
            "weapons/de_deploy.wav",
            "weapons/dryfire_rifle.wav",
            "weapons/dryfire_pistol.wav",
            "weapons/zoom.wav",
            "weapons/pinpull.wav",
            "weapons/reload1.wav",
            "weapons/reload3.wav",
        ],
        &reload,
    );
    map_many(
        &[
            "ui/buttonclick.wav",
            "ui/buttonclickrelease.wav",
            "ui/buttonrollover.wav",
            "common/wpn_select.wav",
            "common/wpn_denyselect.wav",
        ],
        &ui_click,
    );
    map_many(
        &[
            "items/kevlar.wav",
            "items/ammopickup2.wav",
            "items/9mmclip1.wav",
            "items/equip_nvg.wav",
        ],
        &buy,
    );
    map_many(
        &[
            "player/bhit_flesh-1.wav",
            "player/bhit_flesh-2.wav",
            "player/bhit_flesh-3.wav",
            "player/bhit_kevlar-1.wav",
            "common/bodysplat.wav",
        ],
        &flesh_hit,
    );
    map_many(
        &[
            "player/bhit_helmet-1.wav",
            "player/headshot1.wav",
            "player/headshot2.wav",
            "player/headshot3.wav",
        ],
        &headshot,
    );
    map_many(
        &[
            "player/die1.wav",
            "player/die2.wav",
            "player/die3.wav",
            "player/death6.wav",
        ],
        &die,
    );
    map_many(
        &[
            "weapons/ric1.wav",
            "weapons/ric2.wav",
            "weapons/ric3.wav",
            "weapons/ric4.wav",
            "weapons/ric5.wav",
            "weapons/ric_conc-1.wav",
            "weapons/ric_conc-2.wav",
            "weapons/ric_metal-1.wav",
            "weapons/ric_metal-2.wav",
            "weapons/he_bounce-1.wav",
        ],
        &ricochet,
    );
    map_many(
        &[
            "weapons/hegrenade-1.wav",
            "weapons/hegrenade-2.wav",
            "weapons/flashbang-1.wav",
            "weapons/flashbang-2.wav",
            "weapons/sg_explode.wav",
            "weapons/c4_explode1.wav",
            "weapons/debris1.wav",
            "weapons/debris2.wav",
            "weapons/debris3.wav",
        ],
        &explode,
    );
    map_many(
        &[
            "weapons/c4_click.wav",
            "weapons/c4_plant.wav",
            "weapons/c4_disarm.wav",
            "weapons/c4_beep1.wav",
            "weapons/c4_beep2.wav",
            "weapons/c4_beep3.wav",
            "weapons/c4_beep4.wav",
            "weapons/c4_beep5.wav",
        ],
        &beep,
    );

    out
}

fn lcg_noise(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    ((*state >> 8) as f32 / 8_388_608.0) - 1.0
}

fn synth_gunshot(
    sr: u32,
    dur: f32,
    f_start: f32,
    f_end: f32,
    gain: f32,
    lp_alpha: f32,
    seed: u32,
) -> Vec<u8> {
    let n = (sr as f32 * dur) as usize;
    let mut rng = seed.wrapping_mul(0x9E37_79B9) ^ 0xA5A5_1337;
    let mut samples = Vec::with_capacity(n);
    let mut lp = 0.0f32;
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let u = t / dur;
        let noise = lcg_noise(&mut rng);
        lp += lp_alpha * (noise - lp);
        let freq = f_start + (f_end - f_start) * u.powf(0.4);
        phase += std::f32::consts::TAU * freq / sr as f32;
        let punch = phase.sin() * (-u * 14.0).exp();
        let crack = lp * (-u * 8.5).exp();
        let env = if i < 24 { i as f32 / 24.0 } else { 1.0 };
        let s = (punch * 0.55 + crack * 0.75) * gain * env;
        samples.push((s.clamp(-1.0, 1.0) * 30_000.0) as i16);
    }
    encode_wav_pcm16_mono(sr, &samples)
}

fn synth_swipe(sr: u32, dur: f32, f_hi: f32, f_lo: f32, seed: u32) -> Vec<u8> {
    let n = (sr as f32 * dur) as usize;
    let mut rng = seed * 777 + 13;
    let mut samples = Vec::with_capacity(n);
    let mut prev = 0.0f32;
    let mut phase = 0.0f32;
    for i in 0..n {
        let u = i as f32 / n as f32;
        let env = (u * std::f32::consts::PI).sin();
        let noise = lcg_noise(&mut rng);
        let hp = noise - prev * 0.85;
        prev = noise;
        let f = f_hi + (f_lo - f_hi) * u;
        phase += std::f32::consts::TAU * f / sr as f32;
        let s = (hp * 0.45 + phase.sin() * 0.25) * env * 0.6;
        samples.push((s.clamp(-1.0, 1.0) * 28_000.0) as i16);
    }
    encode_wav_pcm16_mono(sr, &samples)
}

fn synth_footstep(sr: u32, dur: f32, freq: f32, gain: f32, seed: u32) -> Vec<u8> {
    let n = (sr as f32 * dur) as usize;
    let mut rng = seed * 1337 + 9;
    let mut lp = 0.0f32;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let u = i as f32 / n as f32;
        let env = (-u * 12.0).exp() * (i.min(20) as f32 / 20.0);
        let noise = lcg_noise(&mut rng);
        lp += 0.22 * (noise - lp);
        let tone = (std::f32::consts::TAU * freq * (i as f32 / sr as f32)).sin();
        let s = (lp * 0.65 + tone * 0.35) * env * gain;
        samples.push((s.clamp(-1.0, 1.0) * 28_000.0) as i16);
    }
    encode_wav_pcm16_mono(sr, &samples)
}

fn synth_thump(sr: u32, dur: f32, f0: f32, f1: f32, gain: f32, seed: u32) -> Vec<u8> {
    let n = (sr as f32 * dur) as usize;
    let mut rng = seed * 515 + 3;
    let mut phase = 0.0f32;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let u = i as f32 / n as f32;
        let f = f0 + (f1 - f0) * u;
        phase += std::f32::consts::TAU * f / sr as f32;
        let env = (-u * 9.0).exp();
        let s = (phase.sin() * 0.75 + lcg_noise(&mut rng) * 0.25) * env * gain;
        samples.push((s.clamp(-1.0, 1.0) * 28_000.0) as i16);
    }
    encode_wav_pcm16_mono(sr, &samples)
}

fn synth_click(sr: u32, dur: f32, f0: f32, f1: f32, seed: u32) -> Vec<u8> {
    let n = (sr as f32 * dur) as usize;
    let mut rng = seed * 313 + 7;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let u = i as f32 / n as f32;
        let f = if u < 0.4 { f0 } else { f1 };
        let env = (-((u % 0.5) * 18.0)).exp();
        let t = i as f32 / sr as f32;
        let s =
            ((std::f32::consts::TAU * f * t).sin() * 0.6 + lcg_noise(&mut rng) * 0.25) * env * 0.55;
        samples.push((s.clamp(-1.0, 1.0) * 28_000.0) as i16);
    }
    encode_wav_pcm16_mono(sr, &samples)
}

fn synth_dink(sr: u32, dur: f32, f1: f32, f2: f32) -> Vec<u8> {
    let n = (sr as f32 * dur) as usize;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let u = t / dur;
        let env = (-u * 11.0).exp();
        let s = ((std::f32::consts::TAU * f1 * t).sin() * 0.55
            + (std::f32::consts::TAU * f2 * t).sin() * 0.45)
            * env
            * 0.72;
        samples.push((s.clamp(-1.0, 1.0) * 28_000.0) as i16);
    }
    encode_wav_pcm16_mono(sr, &samples)
}

fn synth_beep(sr: u32, dur: f32, freq: f32) -> Vec<u8> {
    let n = (sr as f32 * dur) as usize;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let env = (i.min(32) as f32 / 32.0) * (n.saturating_sub(i).min(64) as f32 / 64.0);
        let s = (std::f32::consts::TAU * freq * t).sin() * env * 0.55;
        samples.push((s.clamp(-1.0, 1.0) * 28_000.0) as i16);
    }
    encode_wav_pcm16_mono(sr, &samples)
}

fn encode_wav_pcm16_mono(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36 + data_bytes;
    let byte_rate = sample_rate * 2;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM format = 1
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}
