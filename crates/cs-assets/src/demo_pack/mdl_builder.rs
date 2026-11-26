//! Clean-room GoldSrc StudioModel v10 (`IDST`) builder for articulated CT/T humanoid players,
//! first-person viewmodels (`v_*.mdl`), third-person weapon attachments (`p_*.mdl`), and
//! dropped worldmodels (`w_*.mdl`).

use glam::Vec3;

use crate::mdl::{MDL_MAGIC, MDL_VERSION};

// GoldSrc `Activity_s` constants matching `cs-game/src/player_anim.rs`.
const ACT_IDLE: i32 = 1;
const ACT_WALK: i32 = 3;
const ACT_RUN: i32 = 4;
const ACT_HOP: i32 = 7;
const ACT_CROUCH: i32 = 17;
const ACT_CROUCHIDLE: i32 = 18;
const ACT_RANGE_ATTACK1: i32 = 28;
const ACT_RELOAD: i32 = 32;
const ACT_DIESIMPLE: i32 = 36;
const ACT_DIEBACKWARD: i32 = 37;
const ACT_DIEFORWARD: i32 = 38;
const ACT_DIE_HEADSHOT: i32 = 66;
const ACT_FLINCH: i32 = 77;
const STUDIO_LOOPING: i32 = 0x0001;

struct BoneSpec {
    name: &'static str,
    parent: i32,
    pos: Vec3,
    rot: Vec3,
    pos_scale: Vec3,
    rot_scale: Vec3,
}

struct HitboxSpec {
    bone: i32,
    group: i32,
    bbmin: Vec3,
    bbmax: Vec3,
}

struct AttachmentSpec {
    name: &'static str,
    bone: i32,
    org: Vec3,
}

/// Per-frame `[dx, dy, dz, rx, ry, rz]` integer RLE keyframes for each bone.
struct SeqSpec {
    label: &'static str,
    fps: f32,
    flags: i32,
    activity: i32,
    num_frames: usize,
    /// `[bone_idx][frame_idx] -> [i16; 6]` (scaled by bone's `pos_scale`/`rot_scale`).
    bone_frames: Vec<Vec<[i16; 6]>>,
}

struct SkinTexSpec {
    name: &'static str,
    width: u32,
    height: u32,
    indices: Vec<u8>,
    palette: [[u8; 3]; 256],
}

type QuadRec = (usize, [u16; 4], u16, [(i16, i16); 4]);

#[derive(Default)]
struct MeshBuilder {
    verts: Vec<Vec3>,
    vert_bones: Vec<u8>,
    norms: Vec<Vec3>,
    norm_bones: Vec<u8>,
    /// Quads grouped by texture/skinref: `(skinref, [v0,v1,v2,v3], n_idx, [(s,t); 4])`.
    quads: Vec<QuadRec>,
}

impl MeshBuilder {
    /// Append an axis-aligned box in `bone`'s local space with UV-mapped 6 faces.
    fn push_box(
        &mut self,
        bone: u8,
        skinref: usize,
        mn: Vec3,
        mx: Vec3,
        uv_rect: (i16, i16, i16, i16),
    ) {
        let (x0, y0, z0) = (mn.x, mn.y, mn.z);
        let (x1, y1, z1) = (mx.x, mx.y, mx.z);
        let (u0, v0, u1, v1) = uv_rect;
        let uvs = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)];
        let faces: [([Vec3; 4], Vec3); 6] = [
            // +X front
            (
                [
                    Vec3::new(x1, y0, z1),
                    Vec3::new(x1, y1, z1),
                    Vec3::new(x1, y1, z0),
                    Vec3::new(x1, y0, z0),
                ],
                Vec3::X,
            ),
            // -X back
            (
                [
                    Vec3::new(x0, y1, z1),
                    Vec3::new(x0, y0, z1),
                    Vec3::new(x0, y0, z0),
                    Vec3::new(x0, y1, z0),
                ],
                Vec3::NEG_X,
            ),
            // +Y left
            (
                [
                    Vec3::new(x1, y1, z1),
                    Vec3::new(x0, y1, z1),
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x1, y1, z0),
                ],
                Vec3::Y,
            ),
            // -Y right
            (
                [
                    Vec3::new(x0, y0, z1),
                    Vec3::new(x1, y0, z1),
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x0, y0, z0),
                ],
                Vec3::NEG_Y,
            ),
            // +Z top
            (
                [
                    Vec3::new(x0, y0, z1),
                    Vec3::new(x0, y1, z1),
                    Vec3::new(x1, y1, z1),
                    Vec3::new(x1, y0, z1),
                ],
                Vec3::Z,
            ),
            // -Z bottom
            (
                [
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x0, y0, z0),
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x1, y1, z0),
                ],
                Vec3::NEG_Z,
            ),
        ];

        for (corners, normal) in faces {
            let base = self.verts.len() as u16;
            for p in corners {
                self.verts.push(p);
                self.vert_bones.push(bone);
            }
            let n_idx = self.norms.len() as u16;
            self.norms.push(normal);
            self.norm_bones.push(bone);
            self.quads
                .push((skinref, [base, base + 1, base + 2, base + 3], n_idx, uvs));
        }
    }
}

/// Color theme for a clean-room humanoid character (`CT` vs `T` variants).
#[derive(Clone, Copy)]
pub struct PlayerPalette {
    pub primary: [u8; 3],
    pub vest: [u8; 3],
    pub helmet: [u8; 3],
    pub visor_or_face: [u8; 3],
    pub pants: [u8; 3],
    pub boots: [u8; 3],
}

pub const CT_PALETTE: PlayerPalette = PlayerPalette {
    primary: [52, 78, 108],        // Navy tactical uniform
    vest: [34, 44, 56],            // Dark kevlar plate carrier
    helmet: [44, 58, 72],          // Tactical Kevlar helmet
    visor_or_face: [96, 178, 214], // Bright cyan tactical visor
    pants: [46, 66, 90],
    boots: [28, 30, 32],
};

pub const T_PALETTE: PlayerPalette = PlayerPalette {
    primary: [152, 86, 52], // Terracotta / desert guerrilla jacket
    vest: [94, 78, 56],     // Olive-khaki tactical webbing
    helmet: [184, 72, 46],  // Distinctive red-ochre bandana/headwrap
    visor_or_face: [204, 166, 134],
    pants: [128, 114, 86], // Khaki cargo trousers
    boots: [44, 36, 28],
};

/// Build an articulated 8-bone humanoid GoldSrc StudioModel v10 with 7 hitboxes and animations.
pub fn build_player_mdl(name: &str, pal: PlayerPalette) -> Vec<u8> {
    let ps = Vec3::splat(0.1);
    let rs = Vec3::splat(0.01); // 100 units = 1 radian
    let bones = vec![
        BoneSpec {
            name: "Bip01",
            parent: -1,
            pos: Vec3::new(0.0, 0.0, 0.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "Bip01 Spine",
            parent: 0,
            pos: Vec3::new(0.0, 0.0, 4.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "Bip01 Head",
            parent: 1,
            pos: Vec3::new(0.0, 0.0, 21.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "Bip01 L UpperArm",
            parent: 1,
            pos: Vec3::new(0.0, 10.5, 16.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "Bip01 R UpperArm",
            parent: 1,
            pos: Vec3::new(0.0, -10.5, 16.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "Bip01 R Hand",
            parent: 4,
            pos: Vec3::new(14.0, 2.0, -4.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "Bip01 L Thigh",
            parent: 0,
            pos: Vec3::new(0.0, 5.0, -2.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "Bip01 R Thigh",
            parent: 0,
            pos: Vec3::new(0.0, -5.0, -2.0),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
    ];

    // 64x64 atlas partitioned into 6 color regions so body parts look crisp and distinct!
    let tex = build_player_skin_tex("player_skin.bmp", pal);

    let mut mesh = MeshBuilder::default();
    // Pelvis (bone 0): z in [-4..4]
    mesh.push_box(
        0,
        0,
        Vec3::new(-5.5, -8.0, -4.0),
        Vec3::new(5.5, 8.0, 4.0),
        (0, 32, 31, 63),
    );
    // Torso / Tactical Vest (bone 1, local origin z=+4 => world z in [4..23])
    mesh.push_box(
        1,
        0,
        Vec3::new(-6.0, -9.0, 0.0),
        Vec3::new(6.0, 9.0, 18.0),
        (0, 0, 31, 31),
    );
    // Tactical chest armor plate
    mesh.push_box(
        1,
        0,
        Vec3::new(-7.0, -7.5, 3.0),
        Vec3::new(7.0, 7.5, 16.0),
        (32, 0, 63, 21),
    );
    // Head + Helmet/Headwrap (bone 2, local origin world z=+25 => world z in [23..35])
    mesh.push_box(
        2,
        0,
        Vec3::new(-4.5, -4.5, -2.0),
        Vec3::new(4.5, 4.5, 5.0),
        (32, 42, 63, 63),
    );
    mesh.push_box(
        2,
        0,
        Vec3::new(-5.2, -5.2, 4.0),
        Vec3::new(5.2, 5.2, 10.0),
        (32, 22, 63, 41),
    );
    // Left arm extended slightly forward in rifle-ready stance (bone 3)
    mesh.push_box(
        3,
        0,
        Vec3::new(-3.0, -2.5, -10.0),
        Vec3::new(12.0, 3.0, 2.5),
        (0, 0, 31, 31),
    );
    // Right arm extended forward holding weapon (bone 4)
    mesh.push_box(
        4,
        0,
        Vec3::new(-3.0, -3.0, -10.0),
        Vec3::new(14.0, 2.5, 2.5),
        (0, 0, 31, 31),
    );
    // Compact rifle silhouette integrated on right hand (bone 5) so bots visibly carry guns even without p_*.mdl
    mesh.push_box(
        5,
        0,
        Vec3::new(-4.0, -1.5, -2.0),
        Vec3::new(18.0, 1.5, 2.5),
        (48, 0, 63, 15),
    );
    // Left leg + boot (bone 6, local origin world z=-2 => feet at z=-36!)
    mesh.push_box(
        6,
        0,
        Vec3::new(-4.0, -3.5, -26.0),
        Vec3::new(4.0, 3.5, 0.0),
        (0, 32, 31, 63),
    );
    mesh.push_box(
        6,
        0,
        Vec3::new(-4.5, -3.8, -34.0),
        Vec3::new(6.0, 3.8, -26.0),
        (48, 48, 63, 63),
    );
    // Right leg + boot (bone 7, feet at z=-36!)
    mesh.push_box(
        7,
        0,
        Vec3::new(-4.0, -3.5, -26.0),
        Vec3::new(4.0, 3.5, 0.0),
        (0, 32, 31, 63),
    );
    mesh.push_box(
        7,
        0,
        Vec3::new(-4.5, -3.8, -34.0),
        Vec3::new(6.0, 3.8, -26.0),
        (48, 48, 63, 63),
    );

    // Authentic GoldSrc hitboxes (`group 1 = head`, `2 = chest`, `3 = stomach`, `4/5 = arms`, `6/7 = legs`)
    let hitboxes = vec![
        HitboxSpec {
            bone: 2,
            group: 1,
            bbmin: Vec3::new(-5.5, -5.5, -2.0),
            bbmax: Vec3::new(5.5, 5.5, 11.0),
        },
        HitboxSpec {
            bone: 1,
            group: 2,
            bbmin: Vec3::new(-7.5, -9.5, 7.0),
            bbmax: Vec3::new(7.5, 9.5, 19.0),
        },
        HitboxSpec {
            bone: 1,
            group: 3,
            bbmin: Vec3::new(-7.0, -8.5, -4.0),
            bbmax: Vec3::new(7.0, 8.5, 7.0),
        },
        HitboxSpec {
            bone: 3,
            group: 4,
            bbmin: Vec3::new(-3.5, -3.5, -11.0),
            bbmax: Vec3::new(13.0, 3.5, 3.5),
        },
        HitboxSpec {
            bone: 4,
            group: 5,
            bbmin: Vec3::new(-3.5, -3.5, -11.0),
            bbmax: Vec3::new(15.0, 3.5, 3.5),
        },
        HitboxSpec {
            bone: 6,
            group: 6,
            bbmin: Vec3::new(-5.0, -4.5, -34.0),
            bbmax: Vec3::new(6.0, 4.5, 0.0),
        },
        HitboxSpec {
            bone: 7,
            group: 7,
            bbmin: Vec3::new(-5.0, -4.5, -34.0),
            bbmax: Vec3::new(6.0, 4.5, 0.0),
        },
    ];

    let attachments = vec![AttachmentSpec {
        name: "muzzle",
        bone: 5,
        org: Vec3::new(18.0, 0.0, 1.0),
    }];

    // Build real animated sequences for idle, run/walk stride, crouch, aim/shoot, flinch, and death!
    let nb = bones.len();
    let static_seq =
        |label: &'static str, activity: i32, flags: i32, root_dz: i16, spine_ry: i16| -> SeqSpec {
            let mut bf = vec![vec![[0i16; 6]; 2]; nb];
            for f in 0..2 {
                bf[0][f][2] = root_dz;
                bf[1][f][4] = spine_ry;
            }
            SeqSpec {
                label,
                fps: 15.0,
                flags,
                activity,
                num_frames: 2,
                bone_frames: bf,
            }
        };

    let make_stride_seq = |label: &'static str, activity: i32, fps: f32, swing: f32| -> SeqSpec {
        let nf = 12;
        let mut bf = vec![vec![[0i16; 6]; nf]; nb];
        for f in 0..nf {
            let phase = (f as f32 / (nf - 1) as f32) * std::f32::consts::TAU;
            let leg_pitch = (phase.sin() * swing * 100.0) as i16;
            let bob = (phase.cos().abs() * 16.0) as i16; // +1.6u bounce
            bf[0][f][2] = bob;
            bf[6][f][4] = leg_pitch; // L leg pitch
            bf[7][f][4] = -leg_pitch; // R leg pitch
            bf[3][f][4] = -leg_pitch / 3;
            bf[4][f][4] = leg_pitch / 3;
        }
        SeqSpec {
            label,
            fps,
            flags: STUDIO_LOOPING,
            activity,
            num_frames: nf,
            bone_frames: bf,
        }
    };

    let make_shoot_seq = |label: &'static str| -> SeqSpec {
        let nf = 5;
        let mut bf = vec![vec![[0i16; 6]; nf]; nb];
        let kicks = [0i16, -14, -8, -3, 0];
        for (f, &k) in kicks.iter().enumerate() {
            bf[1][f][4] = k;
            bf[4][f][4] = k * 2;
        }
        SeqSpec {
            label,
            fps: 30.0,
            flags: 0,
            activity: ACT_RANGE_ATTACK1,
            num_frames: nf,
            bone_frames: bf,
        }
    };

    let make_die_seq = |label: &'static str, activity: i32| -> SeqSpec {
        let nf = 8;
        let mut bf = vec![vec![[0i16; 6]; nf]; nb];
        for (f, frame) in bf[0].iter_mut().enumerate().take(nf) {
            let t = f as f32 / (nf - 1) as f32;
            frame[2] = (-300.0 * t) as i16; // drop 30u toward floor
            frame[4] = (-150.0 * t) as i16; // fall backward ~86 deg
        }
        SeqSpec {
            label,
            fps: 20.0,
            flags: 0,
            activity,
            num_frames: nf,
            bone_frames: bf,
        }
    };

    let sequences = vec![
        static_seq("idle1", ACT_IDLE, STUDIO_LOOPING, 0, 0),
        make_stride_seq("walk", ACT_WALK, 22.0, 0.35),
        make_stride_seq("run", ACT_RUN, 30.0, 0.58),
        static_seq("crouch_idle", ACT_CROUCHIDLE, STUDIO_LOOPING, -160, 12),
        make_stride_seq("crouch_run", ACT_CROUCH, 20.0, 0.25),
        static_seq("jump", ACT_HOP, 0, 20, 0),
        static_seq("ref_aim_ak47", ACT_IDLE, STUDIO_LOOPING, 0, 0),
        make_shoot_seq("ref_shoot_ak47"),
        static_seq("ref_aim_carbine", ACT_IDLE, STUDIO_LOOPING, 0, 0),
        make_shoot_seq("ref_shoot_carbine"),
        static_seq("ref_aim_rifle", ACT_IDLE, STUDIO_LOOPING, 0, 0),
        make_shoot_seq("ref_shoot_rifle"),
        static_seq("ref_aim_onehanded", ACT_IDLE, STUDIO_LOOPING, 0, 0),
        make_shoot_seq("ref_shoot_onehanded"),
        static_seq("ref_aim_knife", ACT_IDLE, STUDIO_LOOPING, 0, 0),
        make_shoot_seq("ref_shoot_knife"),
        static_seq("ref_reload_ak47", ACT_RELOAD, 0, 0, 18),
        static_seq("head_flinch", ACT_FLINCH, 0, 0, -16),
        static_seq("gut_flinch", ACT_FLINCH, 0, 0, 18),
        make_die_seq("die_simple", ACT_DIESIMPLE),
        make_die_seq("death1", ACT_DIEBACKWARD),
        make_die_seq("death2", ACT_DIEFORWARD),
        make_die_seq("head", ACT_DIE_HEADSHOT),
        make_die_seq("crouch_die", ACT_DIESIMPLE),
        make_die_seq("left", ACT_DIESIMPLE),
        make_die_seq("right", ACT_DIESIMPLE),
    ];

    serialize_mdl_v10(
        name,
        Vec3::new(0.0, 0.0, 28.0),
        Vec3::new(-16.0, -16.0, -36.0),
        Vec3::new(16.0, 16.0, 36.0),
        &bones,
        &hitboxes,
        &attachments,
        &sequences,
        &[tex],
        &mesh,
    )
}

/// Distinct visual archetype for procedural clean-room weapons.
#[derive(Clone, Copy)]
pub enum WeaponStyle {
    Ak47,
    M4a1,
    Sniper,
    Smg,
    Shotgun,
    Pistol,
    Knife,
    Grenade,
    C4,
}

/// Build a clean-room first-person viewmodel (`models/v_*.mdl`) positioned neatly in the
/// bottom-right quadrant of the player's view (`+X` forward, `+Y` right when right-handed, `-Z` down).
pub fn build_viewmodel_mdl(name: &str, style: WeaponStyle) -> Vec<u8> {
    let ps = Vec3::splat(0.1);
    let rs = Vec3::splat(0.01);
    let bones = vec![
        BoneSpec {
            name: "root",
            parent: -1,
            pos: Vec3::ZERO,
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
        BoneSpec {
            name: "weapon",
            parent: 0,
            pos: Vec3::new(14.0, 6.2, -6.5),
            rot: Vec3::ZERO,
            pos_scale: ps,
            rot_scale: rs,
        },
    ];

    let (metal_col, accent_col, glove_col) = match style {
        WeaponStyle::Ak47 => ([96, 102, 108], [156, 98, 48], [68, 76, 60]),
        WeaponStyle::M4a1 => ([78, 84, 92], [104, 110, 118], [58, 70, 84]),
        WeaponStyle::Sniper => ([82, 88, 94], [84, 108, 66], [58, 68, 58]),
        WeaponStyle::Smg => ([86, 92, 98], [112, 116, 122], [62, 68, 76]),
        WeaponStyle::Shotgun => ([90, 94, 100], [136, 92, 54], [64, 72, 60]),
        WeaponStyle::Pistol => ([112, 118, 128], [54, 58, 64], [68, 78, 62]),
        WeaponStyle::Knife => ([196, 206, 216], [56, 60, 52], [64, 74, 62]),
        WeaponStyle::Grenade => ([84, 104, 64], [182, 164, 68], [64, 72, 60]),
        WeaponStyle::C4 => ([136, 128, 104], [206, 56, 44], [64, 72, 60]),
    };

    let tex = build_weapon_skin_tex("wpn_skin.bmp", metal_col, accent_col, glove_col);
    let mut mesh = MeshBuilder::default();

    // Gloved supporting hand/forearm anchored behind weapon
    mesh.push_box(
        1,
        0,
        Vec3::new(-9.0, -1.5, -4.4),
        Vec3::new(1.2, 1.6, -0.7),
        (32, 32, 63, 63),
    );

    let muzzle_x = match style {
        WeaponStyle::Ak47 | WeaponStyle::M4a1 => {
            // Receiver
            mesh.push_box(
                1,
                0,
                Vec3::new(-3.0, -1.2, -1.2),
                Vec3::new(9.0, 1.2, 1.6),
                (0, 0, 31, 31),
            );
            // Handguard / wood or tactical rail
            mesh.push_box(
                1,
                0,
                Vec3::new(5.5, -1.3, -1.0),
                Vec3::new(14.0, 1.3, 1.4),
                (32, 0, 63, 31),
            );
            // Barrel + front sight post
            mesh.push_box(
                1,
                0,
                Vec3::new(13.5, -0.45, 0.1),
                Vec3::new(21.5, 0.45, 1.0),
                (0, 32, 31, 63),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(18.5, -0.25, 1.0),
                Vec3::new(19.5, 0.25, 2.2),
                (0, 32, 31, 63),
            );
            // Magazine
            mesh.push_box(
                1,
                0,
                Vec3::new(2.0, -0.85, -5.8),
                Vec3::new(5.2, 0.85, -1.0),
                (32, 0, 63, 31),
            );
            // Carry handle / rear sight
            mesh.push_box(
                1,
                0,
                Vec3::new(-1.0, -0.6, 1.6),
                Vec3::new(5.5, 0.6, 2.6),
                (0, 0, 31, 31),
            );
            22.0
        }
        WeaponStyle::Sniper => {
            // Olive stock & receiver
            mesh.push_box(
                1,
                0,
                Vec3::new(-5.0, -1.3, -1.5),
                Vec3::new(12.0, 1.3, 1.4),
                (32, 0, 63, 31),
            );
            // Long fluted barrel
            mesh.push_box(
                1,
                0,
                Vec3::new(11.5, -0.45, 0.2),
                Vec3::new(26.0, 0.45, 1.1),
                (0, 32, 31, 63),
            );
            // Scope tube + rings
            mesh.push_box(
                1,
                0,
                Vec3::new(0.0, -0.75, 1.8),
                Vec3::new(10.5, 0.75, 3.3),
                (0, 0, 31, 31),
            );
            26.5
        }
        WeaponStyle::Smg | WeaponStyle::Shotgun => {
            mesh.push_box(
                1,
                0,
                Vec3::new(-2.0, -1.2, -1.0),
                Vec3::new(9.5, 1.2, 1.6),
                (0, 0, 31, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(4.0, -1.25, -1.6),
                Vec3::new(12.5, 1.25, 0.4),
                (32, 0, 63, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(9.5, -0.5, 0.3),
                Vec3::new(17.5, 0.5, 1.2),
                (0, 32, 31, 63),
            );
            18.0
        }
        WeaponStyle::Pistol => {
            // Crisp two-tone brushed steel slide + polymer lower rail + grip + sights
            mesh.push_box(
                1,
                0,
                Vec3::new(-1.0, -0.82, 0.35),
                Vec3::new(9.2, 0.82, 2.05),
                (0, 0, 31, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(-0.8, -0.76, -0.45),
                Vec3::new(8.8, 0.76, 0.35),
                (32, 0, 63, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(-1.4, -0.74, -3.6),
                Vec3::new(2.0, 0.74, -0.35),
                (32, 0, 63, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(8.2, -0.18, 2.05),
                Vec3::new(8.9, 0.18, 2.48),
                (0, 32, 31, 63),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(-0.6, -0.65, 2.05),
                Vec3::new(0.1, -0.22, 2.45),
                (0, 32, 31, 63),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(-0.6, 0.22, 2.05),
                Vec3::new(0.1, 0.65, 2.45),
                (0, 32, 31, 63),
            );
            9.5
        }
        WeaponStyle::Knife => {
            // Angled tactical combat knife blade + dark hilt
            mesh.push_box(
                1,
                0,
                Vec3::new(-1.0, -0.9, -2.5),
                Vec3::new(2.2, 0.9, 0.6),
                (32, 0, 63, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(2.0, -0.25, -0.5),
                Vec3::new(11.5, 0.25, 1.5),
                (0, 0, 31, 31),
            );
            11.0
        }
        WeaponStyle::Grenade => {
            mesh.push_box(
                1,
                0,
                Vec3::new(0.0, -2.0, -2.5),
                Vec3::new(4.5, 2.0, 2.8),
                (0, 0, 31, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(1.0, -0.8, 2.8),
                Vec3::new(3.5, 0.8, 4.2),
                (32, 0, 63, 31),
            );
            5.0
        }
        WeaponStyle::C4 => {
            mesh.push_box(
                1,
                0,
                Vec3::new(-1.0, -3.5, -2.0),
                Vec3::new(5.5, 3.5, 2.5),
                (0, 0, 31, 31),
            );
            mesh.push_box(
                1,
                0,
                Vec3::new(1.0, -1.5, 2.5),
                Vec3::new(3.8, 1.5, 3.2),
                (32, 0, 63, 31),
            );
            5.0
        }
    };

    let attachments = vec![AttachmentSpec {
        name: "muzzle",
        bone: 1,
        org: Vec3::new(muzzle_x, 0.0, 0.7),
    }];

    // Standard GoldSrc viewmodel sequence ordering:
    // 0: idle, 1: reload, 2: draw, 3..5: shoot1..shoot3 (covers AK/M4/Pistols/Knife/AWP)
    let make_vm_seq = |label: &'static str, fps: f32, flags: i32, frames: &[[i16; 6]]| -> SeqSpec {
        let nf = frames.len();
        let mut bf = vec![vec![[0i16; 6]; nf]; 2];
        bf[1] = frames.to_vec();
        SeqSpec {
            label,
            fps,
            flags,
            activity: 0,
            num_frames: nf,
            bone_frames: bf,
        }
    };

    let idle_f = [[0, 0, 0, 0, 0, 0], [0, 0, 2, 0, 0, 0], [0, 0, 0, 0, 0, 0]];
    let shoot_f = [
        [0, 0, 0, 0, 0, 0],
        [-22, 0, 8, 0, -10, 0],
        [-12, 0, 4, 0, -5, 0],
        [-4, 0, 1, 0, -2, 0],
        [0, 0, 0, 0, 0, 0],
    ];
    let reload_f = [
        [0, 0, 0, 0, 0, 0],
        [-10, 0, -32, 20, 18, 0],
        [-12, 0, -38, 28, 22, 0],
        [-8, 0, -24, 15, 12, 0],
        [0, 0, 0, 0, 0, 0],
    ];
    let draw_f = [
        [-25, 0, -45, 0, 30, 0],
        [-12, 0, -20, 0, 14, 0],
        [-4, 0, -6, 0, 4, 0],
        [0, 0, 0, 0, 0, 0],
    ];

    let sequences = vec![
        make_vm_seq("idle1", 12.0, STUDIO_LOOPING, &idle_f),
        make_vm_seq("reload", 16.0, 0, &reload_f),
        make_vm_seq("draw", 24.0, 0, &draw_f),
        make_vm_seq("shoot1", 32.0, 0, &shoot_f),
        make_vm_seq("shoot2", 32.0, 0, &shoot_f),
        make_vm_seq("shoot3", 32.0, 0, &shoot_f),
    ];

    serialize_mdl_v10(
        name,
        Vec3::ZERO,
        Vec3::new(-16.0, -16.0, -16.0),
        Vec3::new(32.0, 16.0, 16.0),
        &bones,
        &[],
        &attachments,
        &sequences,
        &[tex],
        &mesh,
    )
}

/// Build a compact worldmodel (`models/w_*.mdl`) or bone-merged player weapon (`models/p_*.mdl`).
pub fn build_world_or_player_weapon_mdl(
    name: &str,
    style: WeaponStyle,
    is_p_model: bool,
) -> Vec<u8> {
    let bone_name = if is_p_model { "Bip01 R Hand" } else { "root" };
    let bones = vec![BoneSpec {
        name: bone_name,
        parent: -1,
        pos: Vec3::ZERO,
        rot: Vec3::ZERO,
        pos_scale: Vec3::splat(0.1),
        rot_scale: Vec3::splat(0.01),
    }];
    let (c1, c2) = match style {
        WeaponStyle::Ak47 => ([64, 68, 72], [148, 92, 46]),
        WeaponStyle::Sniper => ([52, 56, 60], [74, 96, 58]),
        WeaponStyle::Knife => ([188, 198, 208], [48, 52, 46]),
        WeaponStyle::Grenade => ([76, 94, 58], [168, 152, 62]),
        WeaponStyle::C4 => ([122, 114, 94], [192, 52, 40]),
        _ => ([54, 58, 64], [82, 86, 92]),
    };
    let tex = build_weapon_skin_tex("w_skin.bmp", c1, c2, [44, 48, 52]);
    let mut mesh = MeshBuilder::default();
    let len = match style {
        WeaponStyle::Sniper => 24.0,
        WeaponStyle::Ak47 | WeaponStyle::M4a1 | WeaponStyle::Shotgun => 18.0,
        WeaponStyle::Smg => 13.0,
        WeaponStyle::Pistol | WeaponStyle::Knife => 8.0,
        WeaponStyle::Grenade | WeaponStyle::C4 => 5.0,
    };
    mesh.push_box(
        0,
        0,
        Vec3::new(-3.0, -1.5, -1.5),
        Vec3::new(len, 1.5, 2.0),
        (0, 0, 31, 31),
    );
    mesh.push_box(
        0,
        0,
        Vec3::new(2.0, -1.2, -3.5),
        Vec3::new(6.0, 1.2, -1.0),
        (32, 0, 63, 31),
    );

    let sequences = vec![SeqSpec {
        label: "idle",
        fps: 10.0,
        flags: STUDIO_LOOPING,
        activity: 0,
        num_frames: 1,
        bone_frames: vec![vec![[0i16; 6]]],
    }];

    serialize_mdl_v10(
        name,
        Vec3::ZERO,
        Vec3::new(-6.0, -4.0, -4.0),
        Vec3::new(len + 2.0, 4.0, 4.0),
        &bones,
        &[],
        &[],
        &sequences,
        &[tex],
        &mesh,
    )
}

fn build_player_skin_tex(name: &'static str, p: PlayerPalette) -> SkinTexSpec {
    const N: u32 = 64;
    let mut palette = [[0u8; 3]; 256];
    let colors: [[u8; 3]; 8] = [
        p.primary,
        shade(p.primary, 0.78),
        p.vest,
        shade(p.vest, 1.25),
        p.helmet,
        p.visor_or_face,
        p.pants,
        p.boots,
    ];
    for (i, &c) in colors.iter().enumerate() {
        palette[i] = c;
    }

    let mut indices = vec![0u8; (N * N) as usize];
    for y in 0..N {
        for x in 0..N {
            let ci = if x < 32 && y < 32 {
                // Uniform cloth with subtle tactical seam grid
                if x % 8 == 0 || y % 8 == 0 { 1 } else { 0 }
            } else if x >= 32 && y < 22 {
                // Tactical kevlar vest / webbing pouches
                if x >= 48 && y < 16 {
                    7 // dark rifle metal patch
                } else if y % 6 == 0 || x % 8 == 0 {
                    3
                } else {
                    2
                }
            } else if x >= 32 && y < 42 {
                // Helmet / bandana
                4
            } else if x < 32 && y >= 32 {
                // Pants
                if x % 10 == 0 { 1 } else { 6 }
            } else if x >= 48 && y >= 48 {
                // Boots
                7
            } else {
                // Visor / face
                5
            };
            indices[(y * N + x) as usize] = ci;
        }
    }
    SkinTexSpec {
        name,
        width: N,
        height: N,
        indices,
        palette,
    }
}

fn build_weapon_skin_tex(
    name: &'static str,
    metal: [u8; 3],
    accent: [u8; 3],
    glove: [u8; 3],
) -> SkinTexSpec {
    const N: u32 = 64;
    let mut palette = [[0u8; 3]; 256];
    palette[0] = metal;
    palette[1] = shade(metal, 1.28);
    palette[2] = shade(metal, 0.72);
    palette[3] = accent;
    palette[4] = shade(accent, 0.78);
    palette[5] = glove;
    palette[6] = shade(glove, 0.75);

    let mut indices = vec![0u8; (N * N) as usize];
    for y in 0..N {
        for x in 0..N {
            let ci = if x < 32 && y < 32 {
                // Brushed receiver steel
                if y % 4 == 0 {
                    1
                } else if x % 8 == 0 {
                    2
                } else {
                    0
                }
            } else if x >= 32 && y < 32 {
                // Wood/polymer furniture
                if y % 5 == 0 { 4 } else { 3 }
            } else if x < 32 && y >= 32 {
                // Barrel dark gunmetal
                if y % 6 == 0 { 0 } else { 2 }
            } else {
                // Tactical glove
                if (x + y) % 6 == 0 { 6 } else { 5 }
            };
            indices[(y * N + x) as usize] = ci;
        }
    }
    SkinTexSpec {
        name,
        width: N,
        height: N,
        indices,
        palette,
    }
}

fn shade(c: [u8; 3], factor: f32) -> [u8; 3] {
    [
        (c[0] as f32 * factor).clamp(0.0, 255.0) as u8,
        (c[1] as f32 * factor).clamp(0.0, 255.0) as u8,
        (c[2] as f32 * factor).clamp(0.0, 255.0) as u8,
    ]
}

#[allow(clippy::too_many_arguments)]
fn serialize_mdl_v10(
    name: &str,
    eye_pos: Vec3,
    bbmin: Vec3,
    bbmax: Vec3,
    bones: &[BoneSpec],
    hitboxes: &[HitboxSpec],
    attachments: &[AttachmentSpec],
    sequences: &[SeqSpec],
    textures: &[SkinTexSpec],
    mesh: &MeshBuilder,
) -> Vec<u8> {
    let mut out = vec![0u8; 244];
    out[0..4].copy_from_slice(MDL_MAGIC);
    out[4..8].copy_from_slice(&MDL_VERSION.to_le_bytes());
    write_fixed_str(&mut out[8..72], name);
    write_vec3_at(&mut out, 76, eye_pos);
    write_vec3_at(&mut out, 88, bbmin);
    write_vec3_at(&mut out, 100, bbmax);
    write_vec3_at(&mut out, 112, bbmin);
    write_vec3_at(&mut out, 124, bbmax);

    let align4 = |buf: &mut Vec<u8>| {
        while !buf.len().is_multiple_of(4) {
            buf.push(0);
        }
    };

    // 1. Bones (112 bytes each)
    align4(&mut out);
    let bone_index = out.len() as i32;
    for b in bones {
        let start = out.len();
        out.resize(start + 112, 0);
        write_fixed_str(&mut out[start..start + 32], b.name);
        out[start + 32..start + 36].copy_from_slice(&b.parent.to_le_bytes());
        for c in 0..6 {
            out[start + 40 + c * 4..start + 44 + c * 4].copy_from_slice(&(-1i32).to_le_bytes());
        }
        write_vec3_at(&mut out, start + 64, b.pos);
        write_vec3_at(&mut out, start + 76, b.rot);
        write_vec3_at(&mut out, start + 88, b.pos_scale);
        write_vec3_at(&mut out, start + 100, b.rot_scale);
    }

    // 2. Attachments (88 bytes each)
    align4(&mut out);
    let attachment_index = out.len() as i32;
    for a in attachments {
        let start = out.len();
        out.resize(start + 88, 0);
        write_fixed_str(&mut out[start..start + 32], a.name);
        out[start + 36..start + 40].copy_from_slice(&a.bone.to_le_bytes());
        write_vec3_at(&mut out, start + 40, a.org);
    }

    // 3. Hitboxes (32 bytes each)
    align4(&mut out);
    let hitbox_index = out.len() as i32;
    for h in hitboxes {
        let start = out.len();
        out.resize(start + 32, 0);
        out[start..start + 4].copy_from_slice(&h.bone.to_le_bytes());
        out[start + 4..start + 8].copy_from_slice(&h.group.to_le_bytes());
        write_vec3_at(&mut out, start + 8, h.bbmin);
        write_vec3_at(&mut out, start + 20, h.bbmax);
    }

    // 4. Sequence groups (1 entry = 104 bytes)
    align4(&mut out);
    let seqgroup_index = out.len() as i32;
    {
        let start = out.len();
        out.resize(start + 104, 0);
        write_fixed_str(&mut out[start..start + 32], "default");
    }

    // 5. Sequence RLE animation blocks (written first so `animindex` offsets are known)
    let mut seq_anim_offsets = Vec::with_capacity(sequences.len());
    for seq in sequences {
        align4(&mut out);
        let anim_base = out.len();
        seq_anim_offsets.push(anim_base as i32);
        // Reserve `num_bones * 12` bytes for `mstudioanim_t` offset table.
        out.resize(anim_base + bones.len() * 12, 0);
        for (bi, frames) in seq.bone_frames.iter().enumerate() {
            let entry_off = anim_base + bi * 12;
            let nf = seq.num_frames.min(255);
            for ch in 0..6 {
                let any_nonzero = frames.iter().take(nf).any(|f| f[ch] != 0);
                if !any_nonzero {
                    continue;
                }
                let rel = (out.len() - entry_off) as u16;
                out[entry_off + ch * 2..entry_off + ch * 2 + 2].copy_from_slice(&rel.to_le_bytes());
                // RLE chunk: `valid = nf`, `total = nf`, followed by `nf` i16 values.
                out.push(nf as u8);
                out.push(nf as u8);
                for f in frames.iter().take(nf) {
                    out.extend_from_slice(&f[ch].to_le_bytes());
                }
            }
        }
    }

    // 6. Sequences (`mstudioseqdesc_t`, 176 bytes each)
    align4(&mut out);
    let seq_index = out.len() as i32;
    for (si, seq) in sequences.iter().enumerate() {
        let start = out.len();
        out.resize(start + 176, 0);
        write_fixed_str(&mut out[start..start + 32], seq.label);
        out[start + 32..start + 36].copy_from_slice(&seq.fps.to_le_bytes());
        out[start + 36..start + 40].copy_from_slice(&seq.flags.to_le_bytes());
        out[start + 40..start + 44].copy_from_slice(&seq.activity.to_le_bytes());
        out[start + 44..start + 48].copy_from_slice(&1i32.to_le_bytes()); // actweight = 1
        out[start + 56..start + 60].copy_from_slice(&(seq.num_frames as i32).to_le_bytes());
        // Non-zero pivotindex (unused by parser/pose) ensures `studio_bounds` uses exact mesh verts.
        out[start + 64..start + 68].copy_from_slice(&1i32.to_le_bytes());
        out[start + 120..start + 124].copy_from_slice(&1i32.to_le_bytes()); // numblends = 1
        out[start + 124..start + 128].copy_from_slice(&seq_anim_offsets[si].to_le_bytes());
        // seqgroup = 0 at +156
    }

    // 7. Bodyparts -> Submodel -> Vertices, Normals, Meshes, Tristrips
    align4(&mut out);
    let vert_info_idx = out.len() as i32;
    out.extend_from_slice(&mesh.vert_bones);

    align4(&mut out);
    let vert_idx = out.len() as i32;
    for v in &mesh.verts {
        out.extend_from_slice(&v.x.to_le_bytes());
        out.extend_from_slice(&v.y.to_le_bytes());
        out.extend_from_slice(&v.z.to_le_bytes());
    }

    align4(&mut out);
    let norm_info_idx = out.len() as i32;
    out.extend_from_slice(&mesh.norm_bones);

    align4(&mut out);
    let norm_idx = out.len() as i32;
    for n in &mesh.norms {
        out.extend_from_slice(&n.x.to_le_bytes());
        out.extend_from_slice(&n.y.to_le_bytes());
        out.extend_from_slice(&n.z.to_le_bytes());
    }

    // Group quads by skinref into one mesh per texture
    let num_tex = textures.len().max(1);
    let mut mesh_records: Vec<(i32, i32, i32)> = Vec::new();
    for sr in 0..num_tex {
        let mut tri_count = 0i32;
        align4(&mut out);
        let tri_index = out.len() as i32;
        for &(q_sr, qv, qn, quv) in &mesh.quads {
            if q_sr != sr {
                continue;
            }
            // Strip of 4 vertices: v0, v1, v3, v2 => produces 2 CCW triangles
            out.extend_from_slice(&4i16.to_le_bytes());
            for &(vi, (su, sv)) in &[
                (qv[0], quv[0]),
                (qv[1], quv[1]),
                (qv[3], quv[3]),
                (qv[2], quv[2]),
            ] {
                out.extend_from_slice(&(vi as i16).to_le_bytes());
                out.extend_from_slice(&(qn as i16).to_le_bytes());
                out.extend_from_slice(&su.to_le_bytes());
                out.extend_from_slice(&sv.to_le_bytes());
            }
            tri_count += 2;
        }
        if tri_count > 0 {
            out.extend_from_slice(&0i16.to_le_bytes()); // end of tristrip command list
            mesh_records.push((tri_count, tri_index, sr as i32));
        }
    }

    align4(&mut out);
    let mesh_index = out.len() as i32;
    for &(tri_count, tri_index, sr) in &mesh_records {
        out.extend_from_slice(&tri_count.to_le_bytes());
        out.extend_from_slice(&tri_index.to_le_bytes());
        out.extend_from_slice(&sr.to_le_bytes());
        out.extend_from_slice(&(mesh.norms.len() as i32).to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
    }

    align4(&mut out);
    let model_index = out.len() as i32;
    {
        let start = out.len();
        out.resize(start + 112, 0);
        write_fixed_str(&mut out[start..start + 64], "mesh");
        out[start + 72..start + 76].copy_from_slice(&(mesh_records.len() as i32).to_le_bytes());
        out[start + 76..start + 80].copy_from_slice(&mesh_index.to_le_bytes());
        out[start + 80..start + 84].copy_from_slice(&(mesh.verts.len() as i32).to_le_bytes());
        out[start + 84..start + 88].copy_from_slice(&vert_info_idx.to_le_bytes());
        out[start + 88..start + 92].copy_from_slice(&vert_idx.to_le_bytes());
        out[start + 92..start + 96].copy_from_slice(&(mesh.norms.len() as i32).to_le_bytes());
        out[start + 96..start + 100].copy_from_slice(&norm_info_idx.to_le_bytes());
        out[start + 100..start + 104].copy_from_slice(&norm_idx.to_le_bytes());
    }

    align4(&mut out);
    let bodypart_index = out.len() as i32;
    {
        let start = out.len();
        out.resize(start + 76, 0);
        write_fixed_str(&mut out[start..start + 64], "studio");
        out[start + 64..start + 68].copy_from_slice(&1i32.to_le_bytes());
        out[start + 68..start + 72].copy_from_slice(&1i32.to_le_bytes());
        out[start + 72..start + 76].copy_from_slice(&model_index.to_le_bytes());
    }

    // 8. Textures & Skin table
    align4(&mut out);
    let texture_index = out.len() as i32;
    let tex_hdr_start = out.len();
    out.resize(tex_hdr_start + textures.len() * 80, 0);

    align4(&mut out);
    let skin_index = out.len() as i32;
    for i in 0..textures.len() {
        out.extend_from_slice(&(i as u16).to_le_bytes());
    }

    align4(&mut out);
    let texturedataindex = out.len() as i32;
    for (ti, t) in textures.iter().enumerate() {
        align4(&mut out);
        let pix_off = out.len() as i32;
        out.extend_from_slice(&t.indices);
        for rgb in &t.palette {
            out.extend_from_slice(rgb);
        }
        let hb = tex_hdr_start + ti * 80;
        write_fixed_str(&mut out[hb..hb + 64], t.name);
        out[hb + 64..hb + 68].copy_from_slice(&0i32.to_le_bytes());
        out[hb + 68..hb + 72].copy_from_slice(&(t.width as i32).to_le_bytes());
        out[hb + 72..hb + 76].copy_from_slice(&(t.height as i32).to_le_bytes());
        out[hb + 76..hb + 80].copy_from_slice(&pix_off.to_le_bytes());
    }

    // Patch header offsets & file length
    let file_len = out.len() as i32;
    out[72..76].copy_from_slice(&file_len.to_le_bytes());
    out[140..144].copy_from_slice(&(bones.len() as i32).to_le_bytes());
    out[144..148].copy_from_slice(&bone_index.to_le_bytes());
    out[156..160].copy_from_slice(&(hitboxes.len() as i32).to_le_bytes());
    out[160..164].copy_from_slice(&hitbox_index.to_le_bytes());
    out[164..168].copy_from_slice(&(sequences.len() as i32).to_le_bytes());
    out[168..172].copy_from_slice(&seq_index.to_le_bytes());
    out[172..176].copy_from_slice(&1i32.to_le_bytes());
    out[176..180].copy_from_slice(&seqgroup_index.to_le_bytes());
    out[180..184].copy_from_slice(&(textures.len() as i32).to_le_bytes());
    out[184..188].copy_from_slice(&texture_index.to_le_bytes());
    out[188..192].copy_from_slice(&texturedataindex.to_le_bytes());
    out[192..196].copy_from_slice(&(textures.len() as i32).to_le_bytes());
    out[196..200].copy_from_slice(&1i32.to_le_bytes());
    out[200..204].copy_from_slice(&skin_index.to_le_bytes());
    out[204..208].copy_from_slice(&1i32.to_le_bytes());
    out[208..212].copy_from_slice(&bodypart_index.to_le_bytes());
    out[212..216].copy_from_slice(&(attachments.len() as i32).to_le_bytes());
    out[216..220].copy_from_slice(&attachment_index.to_le_bytes());
    out
}

fn write_fixed_str(dst: &mut [u8], s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&b[..n]);
}

fn write_vec3_at(buf: &mut [u8], off: usize, v: Vec3) {
    buf[off..off + 4].copy_from_slice(&v.x.to_le_bytes());
    buf[off + 4..off + 8].copy_from_slice(&v.y.to_le_bytes());
    buf[off + 8..off + 12].copy_from_slice(&v.z.to_le_bytes());
}
