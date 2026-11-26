//! The weapon tables of ReGameDLL_CS, read 2026-11-22: the ids, the prices, the clips and the
//! ammo of `weapontype.cpp`/`weapontype.h`, the `ItemInfo` every weapon returns from
//! `GetItemInfo` (`wpn_shared/wpn_*.cpp`), the ammo registry of `WeaponsPrecache`
//! (`weapons.cpp`) and the aliases.
//!
//! The target is the vanilla build (not `REGAMEDLL_FIXES`/`ADD`/`BUILD_LATEST`), so:
//! - `g_weaponInfo` has no rows for the C4, the knife and the three grenades, and the grenades'
//!   `GetItemInfo` falls back to `MAX_AMMO_*` and `WEAPON_NOCLIP`;
//! - the Galil costs $2000 (the "fix" to $2250 is not vanilla);
//! - the `WEAPON_GLOCK` row exists and names the entity `weapon_glock18`;
//! - the USP and the UMP45 name their ammo `"45ACP"` while the other weapons say `"45acp"`; the
//!   ammo registry compares names without case and keeps the first spelling it saw.

use std::sync::LazyLock;

/// `MAX_WEAPONS`: the size of the item info array (weapon ids are below it, but for the shield).
pub const MAX_WEAPONS: usize = 32;
/// `MAX_AMMO_SLOTS`: the size of the ammo registry (index 0 is unused).
pub const MAX_AMMO_SLOTS: usize = 32;
/// `WEAPON_NOCLIP`: the clip size of a weapon that has no clip.
pub const WEAPON_NOCLIP: i32 = -1;
/// `WEAPON_SUIT` (`cdll_dll.h`): the suit bit in `pev->weapons`.
pub const WEAPON_SUIT: i32 = 31;

/// `ITEM_FLAG_SELECTONEMPTY`.
pub const ITEM_FLAG_SELECTONEMPTY: i32 = 1 << 0;
/// `ITEM_FLAG_NOAUTORELOAD`.
pub const ITEM_FLAG_NOAUTORELOAD: i32 = 1 << 1;
/// `ITEM_FLAG_NOAUTOSWITCHEMPTY`.
pub const ITEM_FLAG_NOAUTOSWITCHEMPTY: i32 = 1 << 2;
/// `ITEM_FLAG_LIMITINWORLD`.
pub const ITEM_FLAG_LIMITINWORLD: i32 = 1 << 3;
/// `ITEM_FLAG_EXHAUSTIBLE`: a player can exhaust their ammo supply and lose the weapon.
pub const ITEM_FLAG_EXHAUSTIBLE: i32 = 1 << 4;
/// `ITEM_FLAG_NOFIREUNDERWATER`.
pub const ITEM_FLAG_NOFIREUNDERWATER: i32 = 1 << 5;
/// `ITEM_FLAG_EXHAUST_SECONDARYAMMO`.
pub const ITEM_FLAG_EXHAUST_SECONDARYAMMO: i32 = 1 << 6;

/// `WeaponIdType` (`weapontype.h`). `Glock` (2) is unused by the game but has rows in the tables.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WeaponId {
    None = 0,
    P228 = 1,
    Glock = 2,
    Scout = 3,
    HeGrenade = 4,
    Xm1014 = 5,
    C4 = 6,
    Mac10 = 7,
    Aug = 8,
    SmokeGrenade = 9,
    Elite = 10,
    FiveSeven = 11,
    Ump45 = 12,
    Sg550 = 13,
    Galil = 14,
    Famas = 15,
    Usp = 16,
    Glock18 = 17,
    Awp = 18,
    Mp5n = 19,
    M249 = 20,
    M3 = 21,
    M4a1 = 22,
    Tmp = 23,
    G3sg1 = 24,
    Flashbang = 25,
    Deagle = 26,
    Sg552 = 27,
    Ak47 = 28,
    Knife = 29,
    P90 = 30,
    ShieldGun = 99,
}

impl WeaponId {
    /// The weapon id with this number, or `None` when the number is not a weapon id.
    pub fn from_i32(n: i32) -> Option<WeaponId> {
        use WeaponId::*;
        Some(match n {
            0 => WeaponId::None,
            1 => P228,
            2 => Glock,
            3 => Scout,
            4 => HeGrenade,
            5 => Xm1014,
            6 => C4,
            7 => Mac10,
            8 => Aug,
            9 => SmokeGrenade,
            10 => Elite,
            11 => FiveSeven,
            12 => Ump45,
            13 => Sg550,
            14 => Galil,
            15 => Famas,
            16 => Usp,
            17 => Glock18,
            18 => Awp,
            19 => Mp5n,
            20 => M249,
            21 => M3,
            22 => M4a1,
            23 => Tmp,
            24 => G3sg1,
            25 => Flashbang,
            26 => Deagle,
            27 => Sg552,
            28 => Ak47,
            29 => Knife,
            30 => P90,
            99 => ShieldGun,
            _ => return Option::None,
        })
    }

    /// The bit of this weapon in `pev->weapons` (`1 << id`), 0 for the shield (id 99).
    pub fn bit(self) -> u32 {
        let id = self as i32;
        if (0..32).contains(&id) { 1u32 << id } else { 0 }
    }
}

/// `AmmoType` (`weapontype.h`). The discriminants are the same as the indices of the ammo
/// registry `WeaponsPrecache` builds (see [`ammo_registry`]), which the tests pin.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmmoType {
    None = 0,
    Magnum338 = 1,
    Nato762 = 2,
    NatoBox556 = 3,
    Nato556 = 4,
    Buckshot = 5,
    Acp45 = 6,
    Mm57 = 7,
    Ae50 = 8,
    Sig357 = 9,
    Mm9 = 10,
    Flashbang = 11,
    HeGrenade = 12,
    SmokeGrenade = 13,
    C4 = 14,
}

/// `AMMO_MAX_TYPES`.
pub const AMMO_MAX_TYPES: i32 = 15;

/// `Bullet` (`weapontype.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BulletType {
    None = 0,
    Player9mm = 1,
    PlayerMp5 = 2,
    Player357 = 3,
    PlayerBuckshot = 4,
    PlayerCrowbar = 5,
    Monster9mm = 6,
    MonsterMp5 = 7,
    Monster12mm = 8,
    Player45acp = 9,
    Player338mag = 10,
    Player762mm = 11,
    Player556mm = 12,
    Player50ae = 13,
    Player57mm = 14,
    Player357sig = 15,
}

/// `WeaponState` (`weapontype.h`): the bits of `m_iWeaponState`.
pub const WPNSTATE_USP_SILENCED: i32 = 1 << 0;
/// `WPNSTATE_GLOCK18_BURST_MODE`.
pub const WPNSTATE_GLOCK18_BURST_MODE: i32 = 1 << 1;
/// `WPNSTATE_M4A1_SILENCED`.
pub const WPNSTATE_M4A1_SILENCED: i32 = 1 << 2;
/// `WPNSTATE_ELITE_LEFT`.
pub const WPNSTATE_ELITE_LEFT: i32 = 1 << 3;
/// `WPNSTATE_FAMAS_BURST_MODE`.
pub const WPNSTATE_FAMAS_BURST_MODE: i32 = 1 << 4;
/// `WPNSTATE_SHIELD_DRAWN`.
pub const WPNSTATE_SHIELD_DRAWN: i32 = 1 << 5;

/// `InventorySlotType` (`weapontype.h`): the slot a weapon is carried in (not the HUD slot of
/// [`ItemInfo::slot`]).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InventorySlot {
    None = 0,
    PrimaryWeapon = 1,
    Pistol = 2,
    Knife = 3,
    Grenade = 4,
    C4 = 5,
}

/// `WeaponClassType` (`weapontype.h`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponClass {
    None = 0,
    Knife = 1,
    Pistol = 2,
    Grenade = 3,
    SubMachineGun = 4,
    Shotgun = 5,
    MachineGun = 6,
    Rifle = 7,
    SniperRifle = 8,
}

/// `ItemCostType` (`weapontype.h`).
pub const ASSAULTSUIT_PRICE: i32 = 1000;
/// `FLASHBANG_PRICE`.
pub const FLASHBANG_PRICE: i32 = 200;
/// `HEGRENADE_PRICE`.
pub const HEGRENADE_PRICE: i32 = 300;
/// `SMOKEGRENADE_PRICE`.
pub const SMOKEGRENADE_PRICE: i32 = 300;
/// `KEVLAR_PRICE`.
pub const KEVLAR_PRICE: i32 = 650;
/// `HELMET_PRICE`.
pub const HELMET_PRICE: i32 = 350;
/// `NVG_PRICE`.
pub const NVG_PRICE: i32 = 1250;
/// `DEFUSEKIT_PRICE`.
pub const DEFUSEKIT_PRICE: i32 = 200;

/// `MAX_AMMO_SMOKEGRENADE` and the other custom `MaxAmmoType` values the grenades and the C4 use
/// when `g_weaponInfo` has no row for them (vanilla).
pub const MAX_AMMO_SMOKEGRENADE: i32 = 1;
/// `MAX_AMMO_HEGRENADE`.
pub const MAX_AMMO_HEGRENADE: i32 = 1;
/// `MAX_AMMO_FLASHBANG`.
pub const MAX_AMMO_FLASHBANG: i32 = 2;
/// `MAX_AMMO_C4`.
pub const MAX_AMMO_C4: i32 = 1;

/// `ItemInfo` (`weapons.h`): what `GetItemInfo` of a weapon fills in. `slot` and `position` are
/// the HUD slot and position (slot 0 is the first column), `id` is the weapon id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemInfo {
    pub slot: i32,
    pub position: i32,
    pub ammo1: Option<&'static str>,
    pub max_ammo1: i32,
    pub ammo2: Option<&'static str>,
    pub max_ammo2: i32,
    /// `pszName`: the classname of the weapon entity.
    pub name: &'static str,
    pub max_clip: i32,
    pub id: WeaponId,
    pub flags: i32,
    pub weight: i32,
}

/// `WeaponInfoStruct` (`weapontype.h`): one row of `g_weaponInfo_default`. `ammo_name1` is `""`
/// where the C row has `nullptr` (the shield).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponInfo {
    pub id: WeaponId,
    pub cost: i32,
    pub clip_cost: i32,
    pub buy_clip_size: i32,
    pub gun_clip_size: i32,
    pub max_rounds: i32,
    pub ammo_type: AmmoType,
    pub entity_name: &'static str,
    pub ammo_name1: &'static str,
    pub ammo_name2: Option<&'static str>,
}

/// `AmmoInfoStruct` (`weapontype.h`): one row of `g_ammoInfo_default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmoInfo {
    pub ammo_type: AmmoType,
    pub clip_cost: i32,
    pub buy_clip_size: i32,
    pub max_rounds: i32,
    pub ammo_name1: Option<&'static str>,
    pub ammo_name2: &'static str,
}

/// `WeaponStruct` (`weapontype.cpp` `g_weaponStruct`): who can buy a weapon. `side` is the team
/// mask (`TERRORIST = 1`, `CT = 2`), `class` an `AUTOBUYCLASS_*` bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponStruct {
    pub id: WeaponId,
    pub price: i32,
    pub side: i32,
    pub class: i32,
    pub ammo_price: i32,
}

/// `WeaponSlotInfo` (`weapontype.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponSlotInfo {
    pub id: WeaponId,
    pub slot: InventorySlot,
    pub weapon_name: &'static str,
}

/// `AutoBuyInfoStruct` (`weapontype.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoBuyInfo {
    pub class: i32,
    pub command: &'static str,
    pub classname: &'static str,
}

/// `AutoBuyClassType` bits.
pub const AUTOBUYCLASS_PRIMARY: i32 = 1 << 0;
/// `AUTOBUYCLASS_SECONDARY`.
pub const AUTOBUYCLASS_SECONDARY: i32 = 1 << 1;
/// `AUTOBUYCLASS_AMMO`.
pub const AUTOBUYCLASS_AMMO: i32 = 1 << 2;
/// `AUTOBUYCLASS_ARMOR`.
pub const AUTOBUYCLASS_ARMOR: i32 = 1 << 3;
/// `AUTOBUYCLASS_DEFUSER`.
pub const AUTOBUYCLASS_DEFUSER: i32 = 1 << 4;
/// `AUTOBUYCLASS_PISTOL`.
pub const AUTOBUYCLASS_PISTOL: i32 = 1 << 5;
/// `AUTOBUYCLASS_SMG`.
pub const AUTOBUYCLASS_SMG: i32 = 1 << 6;
/// `AUTOBUYCLASS_RIFLE`.
pub const AUTOBUYCLASS_RIFLE: i32 = 1 << 7;
/// `AUTOBUYCLASS_SNIPERRIFLE`.
pub const AUTOBUYCLASS_SNIPERRIFLE: i32 = 1 << 8;
/// `AUTOBUYCLASS_SHOTGUN`.
pub const AUTOBUYCLASS_SHOTGUN: i32 = 1 << 9;
/// `AUTOBUYCLASS_MACHINEGUN`.
pub const AUTOBUYCLASS_MACHINEGUN: i32 = 1 << 10;
/// `AUTOBUYCLASS_GRENADE`.
pub const AUTOBUYCLASS_GRENADE: i32 = 1 << 11;
/// `AUTOBUYCLASS_NIGHTVISION`.
pub const AUTOBUYCLASS_NIGHTVISION: i32 = 1 << 12;
/// `AUTOBUYCLASS_SHIELD`.
pub const AUTOBUYCLASS_SHIELD: i32 = 1 << 13;

const T: i32 = 1;
const CT: i32 = 2;

macro_rules! item {
    ($id:ident, $slot:expr, $pos:expr, $ammo:expr, $maxammo:expr, $name:expr, $clip:expr, $flags:expr, $weight:expr) => {
        ItemInfo {
            slot: $slot,
            position: $pos,
            ammo1: $ammo,
            max_ammo1: $maxammo,
            ammo2: None,
            max_ammo2: -1,
            name: $name,
            max_clip: $clip,
            id: WeaponId::$id,
            flags: $flags,
            weight: $weight,
        }
    };
}

/// The `GetItemInfo` of every weapon class that has one (`wpn_shared/wpn_*.cpp`, read
/// 2026-11-22), in the order of the files. `WEAPON_GLOCK` and the shield have none.
static ITEM_INFO: [ItemInfo; 29] = [
    item!(Ak47, 0, 1, Some("762Nato"), 90, "weapon_ak47", 30, 0, 25),
    item!(Aug, 0, 14, Some("556Nato"), 90, "weapon_aug", 30, 0, 25),
    item!(Awp, 0, 2, Some("338Magnum"), 30, "weapon_awp", 10, 0, 30),
    item!(
        C4,
        4,
        3,
        Some("C4"),
        MAX_AMMO_C4,
        "weapon_c4",
        WEAPON_NOCLIP,
        ITEM_FLAG_LIMITINWORLD | ITEM_FLAG_EXHAUSTIBLE,
        3
    ),
    item!(Deagle, 1, 1, Some("50AE"), 35, "weapon_deagle", 7, 0, 7),
    item!(Elite, 1, 5, Some("9mm"), 120, "weapon_elite", 30, 0, 5),
    item!(
        Famas,
        0,
        18,
        Some("556Nato"),
        90,
        "weapon_famas",
        25,
        ITEM_FLAG_NOFIREUNDERWATER,
        75
    ),
    item!(
        FiveSeven,
        1,
        6,
        Some("57mm"),
        100,
        "weapon_fiveseven",
        20,
        0,
        5
    ),
    item!(
        Flashbang,
        3,
        2,
        Some("Flashbang"),
        MAX_AMMO_FLASHBANG,
        "weapon_flashbang",
        WEAPON_NOCLIP,
        ITEM_FLAG_LIMITINWORLD | ITEM_FLAG_EXHAUSTIBLE,
        1
    ),
    item!(G3sg1, 0, 3, Some("762Nato"), 90, "weapon_g3sg1", 20, 0, 20),
    item!(
        Galil,
        0,
        17,
        Some("556Nato"),
        90,
        "weapon_galil",
        35,
        ITEM_FLAG_NOFIREUNDERWATER,
        25
    ),
    item!(Glock18, 1, 2, Some("9mm"), 120, "weapon_glock18", 20, 0, 5),
    item!(
        HeGrenade,
        3,
        1,
        Some("HEGrenade"),
        MAX_AMMO_HEGRENADE,
        "weapon_hegrenade",
        WEAPON_NOCLIP,
        ITEM_FLAG_LIMITINWORLD | ITEM_FLAG_EXHAUSTIBLE,
        2
    ),
    item!(Knife, 2, 1, None, -1, "weapon_knife", WEAPON_NOCLIP, 0, 0),
    item!(
        M249,
        0,
        4,
        Some("556NatoBox"),
        200,
        "weapon_m249",
        100,
        0,
        25
    ),
    item!(
        M3,
        0,
        5,
        Some("buckshot"),
        32,
        "weapon_m3",
        8,
        ITEM_FLAG_NOFIREUNDERWATER,
        20
    ),
    item!(M4a1, 0, 6, Some("556Nato"), 90, "weapon_m4a1", 30, 0, 25),
    item!(Mac10, 0, 13, Some("45acp"), 100, "weapon_mac10", 30, 0, 25),
    item!(Mp5n, 0, 7, Some("9mm"), 120, "weapon_mp5navy", 30, 0, 25),
    item!(P228, 1, 3, Some("357SIG"), 52, "weapon_p228", 13, 0, 5),
    item!(P90, 0, 8, Some("57mm"), 100, "weapon_p90", 50, 0, 26),
    item!(Scout, 0, 9, Some("762Nato"), 90, "weapon_scout", 10, 0, 30),
    item!(Sg550, 0, 16, Some("556Nato"), 90, "weapon_sg550", 30, 0, 20),
    item!(Sg552, 0, 10, Some("556Nato"), 90, "weapon_sg552", 30, 0, 25),
    item!(
        SmokeGrenade,
        3,
        3,
        Some("SmokeGrenade"),
        MAX_AMMO_SMOKEGRENADE,
        "weapon_smokegrenade",
        WEAPON_NOCLIP,
        ITEM_FLAG_LIMITINWORLD | ITEM_FLAG_EXHAUSTIBLE,
        1
    ),
    item!(Tmp, 0, 11, Some("9mm"), 120, "weapon_tmp", 30, 0, 25),
    // Quirk (vanilla): "45ACP" in capitals, unlike the "45acp" of the MAC-10.
    item!(Ump45, 0, 15, Some("45ACP"), 100, "weapon_ump45", 25, 0, 25),
    item!(Usp, 1, 4, Some("45ACP"), 100, "weapon_usp", 12, 0, 5),
    item!(
        Xm1014,
        0,
        12,
        Some("buckshot"),
        32,
        "weapon_xm1014",
        7,
        ITEM_FLAG_NOFIREUNDERWATER,
        20
    ),
];

macro_rules! winfo {
    ($id:ident, $cost:expr, $clipc:expr, $buy:expr, $gun:expr, $max:expr, $at:ident, $ent:expr, $n1:expr, $n2:expr) => {
        WeaponInfo {
            id: WeaponId::$id,
            cost: $cost,
            clip_cost: $clipc,
            buy_clip_size: $buy,
            gun_clip_size: $gun,
            max_rounds: $max,
            ammo_type: AmmoType::$at,
            entity_name: $ent,
            ammo_name1: $n1,
            ammo_name2: $n2,
        }
    };
}

/// `g_weaponInfo_default` (`weapontype.cpp`, read 2026-11-22), vanilla branch, without the
/// all-zero terminator row. Quirk: the `WEAPON_GLOCK` row names `weapon_glock18`.
static WEAPON_INFO: [WeaponInfo; 26] = [
    winfo!(
        P228,
        600,
        50,
        13,
        13,
        52,
        Sig357,
        "weapon_p228",
        "ammo_357sig",
        Some("357SIG")
    ),
    winfo!(
        Glock,
        400,
        20,
        30,
        20,
        120,
        Mm9,
        "weapon_glock18",
        "ammo_9mm",
        Some("9mm")
    ),
    winfo!(
        Glock18,
        400,
        20,
        30,
        20,
        120,
        Mm9,
        "weapon_glock18",
        "ammo_9mm",
        Some("9mm")
    ),
    winfo!(
        Scout,
        2750,
        80,
        30,
        10,
        90,
        Nato762,
        "weapon_scout",
        "ammo_762nato",
        Some("762Nato")
    ),
    winfo!(
        Xm1014,
        3000,
        65,
        8,
        7,
        32,
        Buckshot,
        "weapon_xm1014",
        "ammo_buckshot",
        Some("buckshot")
    ),
    winfo!(
        Mac10,
        1400,
        25,
        12,
        30,
        100,
        Acp45,
        "weapon_mac10",
        "ammo_45acp",
        Some("45acp")
    ),
    winfo!(
        Aug,
        3500,
        60,
        30,
        30,
        90,
        Nato556,
        "weapon_aug",
        "ammo_556nato",
        Some("556Nato")
    ),
    winfo!(
        Elite,
        800,
        20,
        30,
        30,
        120,
        Mm9,
        "weapon_elite",
        "ammo_9mm",
        Some("9mm")
    ),
    winfo!(
        FiveSeven,
        750,
        50,
        50,
        20,
        100,
        Mm57,
        "weapon_fiveseven",
        "ammo_57mm",
        Some("57mm")
    ),
    winfo!(
        Ump45,
        1700,
        25,
        12,
        25,
        100,
        Acp45,
        "weapon_ump45",
        "ammo_45acp",
        Some("45acp")
    ),
    winfo!(
        Sg550,
        4200,
        60,
        30,
        30,
        90,
        Nato556,
        "weapon_sg550",
        "ammo_556nato",
        Some("556Nato")
    ),
    winfo!(
        Galil,
        2000,
        60,
        30,
        35,
        90,
        Nato556,
        "weapon_galil",
        "ammo_556nato",
        Some("556Nato")
    ),
    winfo!(
        Famas,
        2250,
        60,
        30,
        25,
        90,
        Nato556,
        "weapon_famas",
        "ammo_556nato",
        Some("556Nato")
    ),
    winfo!(
        Usp,
        500,
        25,
        12,
        12,
        100,
        Acp45,
        "weapon_usp",
        "ammo_45acp",
        Some("45acp")
    ),
    winfo!(
        Awp,
        4750,
        125,
        10,
        10,
        30,
        Magnum338,
        "weapon_awp",
        "ammo_338magnum",
        Some("338Magnum")
    ),
    winfo!(
        Mp5n,
        1500,
        20,
        30,
        30,
        120,
        Mm9,
        "weapon_mp5navy",
        "ammo_9mm",
        Some("9mm")
    ),
    winfo!(
        M249,
        5750,
        60,
        30,
        100,
        200,
        NatoBox556,
        "weapon_m249",
        "ammo_556natobox",
        Some("556NatoBox")
    ),
    winfo!(
        M3,
        1700,
        65,
        8,
        8,
        32,
        Buckshot,
        "weapon_m3",
        "ammo_buckshot",
        Some("buckshot")
    ),
    winfo!(
        M4a1,
        3100,
        60,
        30,
        30,
        90,
        Nato556,
        "weapon_m4a1",
        "ammo_556nato",
        Some("556Nato")
    ),
    winfo!(
        Tmp,
        1250,
        20,
        30,
        30,
        120,
        Mm9,
        "weapon_tmp",
        "ammo_9mm",
        Some("9mm")
    ),
    winfo!(
        G3sg1,
        5000,
        80,
        30,
        20,
        90,
        Nato762,
        "weapon_g3sg1",
        "ammo_762nato",
        Some("762Nato")
    ),
    winfo!(
        Deagle,
        650,
        40,
        7,
        7,
        35,
        Ae50,
        "weapon_deagle",
        "ammo_50ae",
        Some("50AE")
    ),
    winfo!(
        Sg552,
        3500,
        60,
        30,
        30,
        90,
        Nato556,
        "weapon_sg552",
        "ammo_556nato",
        Some("556Nato")
    ),
    winfo!(
        Ak47,
        2500,
        80,
        30,
        30,
        90,
        Nato762,
        "weapon_ak47",
        "ammo_762nato",
        Some("762Nato")
    ),
    winfo!(
        P90,
        2350,
        50,
        50,
        50,
        100,
        Mm57,
        "weapon_p90",
        "ammo_57mm",
        Some("57mm")
    ),
    winfo!(ShieldGun, 2200, 0, 0, 0, 0, None, "weapon_shield", "", None),
];

/// `g_ammoInfo_default` (`weapontype.cpp`, read 2026-11-22).
static AMMO_INFO: [AmmoInfo; 14] = [
    AmmoInfo {
        ammo_type: AmmoType::Magnum338,
        clip_cost: 125,
        buy_clip_size: 10,
        max_rounds: 30,
        ammo_name1: Some("ammo_338magnum"),
        ammo_name2: "338Magnum",
    },
    AmmoInfo {
        ammo_type: AmmoType::Nato762,
        clip_cost: 80,
        buy_clip_size: 30,
        max_rounds: 90,
        ammo_name1: Some("ammo_762nato"),
        ammo_name2: "762Nato",
    },
    AmmoInfo {
        ammo_type: AmmoType::NatoBox556,
        clip_cost: 60,
        buy_clip_size: 30,
        max_rounds: 200,
        ammo_name1: Some("ammo_556natobox"),
        ammo_name2: "556NatoBox",
    },
    AmmoInfo {
        ammo_type: AmmoType::Nato556,
        clip_cost: 60,
        buy_clip_size: 30,
        max_rounds: 90,
        ammo_name1: Some("ammo_556nato"),
        ammo_name2: "556Nato",
    },
    AmmoInfo {
        ammo_type: AmmoType::Buckshot,
        clip_cost: 65,
        buy_clip_size: 8,
        max_rounds: 32,
        ammo_name1: Some("ammo_buckshot"),
        ammo_name2: "buckshot",
    },
    AmmoInfo {
        ammo_type: AmmoType::Acp45,
        clip_cost: 25,
        buy_clip_size: 12,
        max_rounds: 100,
        ammo_name1: Some("ammo_45acp"),
        ammo_name2: "45acp",
    },
    AmmoInfo {
        ammo_type: AmmoType::Mm57,
        clip_cost: 50,
        buy_clip_size: 50,
        max_rounds: 100,
        ammo_name1: Some("ammo_57mm"),
        ammo_name2: "57mm",
    },
    AmmoInfo {
        ammo_type: AmmoType::Ae50,
        clip_cost: 40,
        buy_clip_size: 7,
        max_rounds: 35,
        ammo_name1: Some("ammo_50ae"),
        ammo_name2: "50AE",
    },
    AmmoInfo {
        ammo_type: AmmoType::Sig357,
        clip_cost: 50,
        buy_clip_size: 13,
        max_rounds: 52,
        ammo_name1: Some("ammo_357sig"),
        ammo_name2: "357SIG",
    },
    AmmoInfo {
        ammo_type: AmmoType::Mm9,
        clip_cost: 20,
        buy_clip_size: 30,
        max_rounds: 120,
        ammo_name1: Some("ammo_9mm"),
        ammo_name2: "9mm",
    },
    AmmoInfo {
        ammo_type: AmmoType::Flashbang,
        clip_cost: FLASHBANG_PRICE,
        buy_clip_size: 1,
        max_rounds: MAX_AMMO_FLASHBANG,
        ammo_name1: None,
        ammo_name2: "Flashbang",
    },
    AmmoInfo {
        ammo_type: AmmoType::HeGrenade,
        clip_cost: HEGRENADE_PRICE,
        buy_clip_size: 1,
        max_rounds: MAX_AMMO_HEGRENADE,
        ammo_name1: None,
        ammo_name2: "HEGrenade",
    },
    AmmoInfo {
        ammo_type: AmmoType::SmokeGrenade,
        clip_cost: SMOKEGRENADE_PRICE,
        buy_clip_size: 1,
        max_rounds: MAX_AMMO_SMOKEGRENADE,
        ammo_name1: None,
        ammo_name2: "SmokeGrenade",
    },
    AmmoInfo {
        ammo_type: AmmoType::C4,
        clip_cost: 0,
        buy_clip_size: 0,
        max_rounds: MAX_AMMO_C4,
        ammo_name1: None,
        ammo_name2: "C4",
    },
];

macro_rules! wstruct {
    ($id:ident, $price:expr, $side:expr, $class:expr, $ammo:expr) => {
        WeaponStruct {
            id: WeaponId::$id,
            price: $price,
            side: $side,
            class: $class,
            ammo_price: $ammo,
        }
    };
}

/// `g_weaponStruct` (`weapontype.cpp`, read 2026-11-22) without its all-zero rows (the C array
/// has `MAX_WEAPONS` entries; row 0 and the last six are zero). Quirk: the Galil is $2000 here
/// and in `g_weaponInfo`; the "$2250" of the source comment is the fix, not vanilla.
static WEAPON_STRUCT: [WeaponStruct; 25] = [
    wstruct!(P228, 600, T | CT, AUTOBUYCLASS_SECONDARY, 50),
    wstruct!(Scout, 2750, T | CT, AUTOBUYCLASS_PRIMARY, 80),
    wstruct!(Xm1014, 3000, T | CT, AUTOBUYCLASS_PRIMARY, 65),
    wstruct!(Mac10, 1400, CT, AUTOBUYCLASS_PRIMARY, 25),
    wstruct!(Aug, 3500, T, AUTOBUYCLASS_PRIMARY, 60),
    wstruct!(Elite, 800, CT, AUTOBUYCLASS_SECONDARY, 20),
    wstruct!(FiveSeven, 750, T | CT, AUTOBUYCLASS_SECONDARY, 50),
    wstruct!(Ump45, 1700, T, AUTOBUYCLASS_PRIMARY, 25),
    wstruct!(Sg550, 4200, T, AUTOBUYCLASS_PRIMARY, 60),
    wstruct!(Usp, 500, T | CT, AUTOBUYCLASS_SECONDARY, 25),
    wstruct!(Glock18, 400, T | CT, AUTOBUYCLASS_SECONDARY, 20),
    wstruct!(Mp5n, 1500, T | CT, AUTOBUYCLASS_PRIMARY, 20),
    wstruct!(Awp, 4750, T | CT, AUTOBUYCLASS_PRIMARY, 125),
    wstruct!(M249, 5750, T | CT, AUTOBUYCLASS_PRIMARY, 60),
    wstruct!(M3, 1700, T | CT, AUTOBUYCLASS_PRIMARY, 65),
    wstruct!(M4a1, 3100, T, AUTOBUYCLASS_PRIMARY, 60),
    wstruct!(Tmp, 1250, T, AUTOBUYCLASS_PRIMARY, 20),
    wstruct!(G3sg1, 5000, CT, AUTOBUYCLASS_PRIMARY, 80),
    wstruct!(Deagle, 650, T | CT, AUTOBUYCLASS_SECONDARY, 40),
    wstruct!(Sg552, 3500, CT, AUTOBUYCLASS_PRIMARY, 60),
    wstruct!(Ak47, 2500, CT, AUTOBUYCLASS_PRIMARY, 80),
    wstruct!(P90, 2350, T | CT, AUTOBUYCLASS_PRIMARY, 50),
    wstruct!(Famas, 2250, T, AUTOBUYCLASS_PRIMARY, 60),
    wstruct!(Galil, 2000, CT, AUTOBUYCLASS_PRIMARY, 60),
    wstruct!(ShieldGun, 2200, T, AUTOBUYCLASS_PRIMARY, 0),
];

macro_rules! slot {
    ($id:ident, $slot:ident, $name:expr) => {
        WeaponSlotInfo {
            id: WeaponId::$id,
            slot: InventorySlot::$slot,
            weapon_name: $name,
        }
    };
}

/// `g_weaponSlotInfo_default` (`weapontype.cpp`, read 2026-11-22). Quirk: the `WEAPON_GLOCK` row
/// names `weapon_glock`, not `weapon_glock18`.
static WEAPON_SLOT_INFO: [WeaponSlotInfo; 31] = [
    slot!(C4, C4, "weapon_c4"),
    slot!(Knife, Knife, "weapon_knife"),
    slot!(P228, Pistol, "weapon_p228"),
    slot!(Glock, Pistol, "weapon_glock"),
    slot!(Elite, Pistol, "weapon_elite"),
    slot!(FiveSeven, Pistol, "weapon_fiveseven"),
    slot!(Usp, Pistol, "weapon_usp"),
    slot!(Glock18, Pistol, "weapon_glock18"),
    slot!(Deagle, Pistol, "weapon_deagle"),
    slot!(HeGrenade, Grenade, "weapon_hegrenade"),
    slot!(SmokeGrenade, Grenade, "weapon_smokegrenade"),
    slot!(Flashbang, Grenade, "weapon_flashbang"),
    slot!(Scout, PrimaryWeapon, "weapon_scout"),
    slot!(Xm1014, PrimaryWeapon, "weapon_xm1014"),
    slot!(Mac10, PrimaryWeapon, "weapon_mac10"),
    slot!(Aug, PrimaryWeapon, "weapon_aug"),
    slot!(Ump45, PrimaryWeapon, "weapon_ump45"),
    slot!(Sg550, PrimaryWeapon, "weapon_sg550"),
    slot!(Galil, PrimaryWeapon, "weapon_galil"),
    slot!(Famas, PrimaryWeapon, "weapon_famas"),
    slot!(Awp, PrimaryWeapon, "weapon_awp"),
    slot!(Mp5n, PrimaryWeapon, "weapon_mp5navy"),
    slot!(M249, PrimaryWeapon, "weapon_m249"),
    slot!(M3, PrimaryWeapon, "weapon_m3"),
    slot!(M4a1, PrimaryWeapon, "weapon_m4a1"),
    slot!(Tmp, PrimaryWeapon, "weapon_tmp"),
    slot!(G3sg1, PrimaryWeapon, "weapon_g3sg1"),
    slot!(Sg552, PrimaryWeapon, "weapon_sg552"),
    slot!(Ak47, PrimaryWeapon, "weapon_ak47"),
    slot!(P90, PrimaryWeapon, "weapon_p90"),
    slot!(ShieldGun, None, "weapon_shield"),
];

macro_rules! ab {
    ($class:expr, $cmd:expr, $cn:expr) => {
        AutoBuyInfo {
            class: $class,
            command: $cmd,
            classname: $cn,
        }
    };
}

/// `g_autoBuyInfo` (`weapontype.cpp`, read 2026-11-22), without the `nullptr` terminator.
static AUTO_BUY_INFO: [AutoBuyInfo; 34] = [
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_RIFLE,
        "galil",
        "weapon_galil"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_RIFLE,
        "ak47",
        "weapon_ak47"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SNIPERRIFLE,
        "scout",
        "weapon_scout"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_RIFLE,
        "sg552",
        "weapon_sg552"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SNIPERRIFLE,
        "awp",
        "weapon_awp"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SNIPERRIFLE,
        "g3sg1",
        "weapon_g3sg1"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_RIFLE,
        "famas",
        "weapon_famas"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_RIFLE,
        "m4a1",
        "weapon_m4a1"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_RIFLE,
        "aug",
        "weapon_aug"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SNIPERRIFLE,
        "sg550",
        "weapon_sg550"
    ),
    ab!(
        AUTOBUYCLASS_SECONDARY | AUTOBUYCLASS_PISTOL,
        "glock",
        "weapon_glock18"
    ),
    ab!(
        AUTOBUYCLASS_SECONDARY | AUTOBUYCLASS_PISTOL,
        "usp",
        "weapon_usp"
    ),
    ab!(
        AUTOBUYCLASS_SECONDARY | AUTOBUYCLASS_PISTOL,
        "p228",
        "weapon_p228"
    ),
    ab!(
        AUTOBUYCLASS_SECONDARY | AUTOBUYCLASS_PISTOL,
        "deagle",
        "weapon_deagle"
    ),
    ab!(
        AUTOBUYCLASS_SECONDARY | AUTOBUYCLASS_PISTOL,
        "elites",
        "weapon_elite"
    ),
    ab!(
        AUTOBUYCLASS_SECONDARY | AUTOBUYCLASS_PISTOL,
        "fn57",
        "weapon_fiveseven"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SHOTGUN,
        "m3",
        "weapon_m3"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SHOTGUN,
        "xm1014",
        "weapon_xm1014"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SMG,
        "mac10",
        "weapon_mac10"
    ),
    ab!(AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SMG, "tmp", "weapon_tmp"),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SMG,
        "mp5",
        "weapon_mp5navy"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SMG,
        "ump45",
        "weapon_ump45"
    ),
    ab!(AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SMG, "p90", "weapon_p90"),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_MACHINEGUN,
        "m249",
        "weapon_m249"
    ),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_AMMO,
        "primammo",
        "primammo"
    ),
    ab!(
        AUTOBUYCLASS_SECONDARY | AUTOBUYCLASS_AMMO,
        "secammo",
        "secammo"
    ),
    ab!(AUTOBUYCLASS_ARMOR, "vest", "item_kevlar"),
    ab!(AUTOBUYCLASS_ARMOR, "vesthelm", "item_assaultsuit"),
    ab!(AUTOBUYCLASS_GRENADE, "flash", "weapon_flashbang"),
    ab!(AUTOBUYCLASS_GRENADE, "hegren", "weapon_hegrenade"),
    ab!(AUTOBUYCLASS_GRENADE, "sgren", "weapon_smokegrenade"),
    ab!(AUTOBUYCLASS_NIGHTVISION, "nvgs", "nvgs"),
    ab!(AUTOBUYCLASS_DEFUSER, "defuser", "defuser"),
    ab!(
        AUTOBUYCLASS_PRIMARY | AUTOBUYCLASS_SHIELD,
        "shield",
        "shield"
    ),
];

/// `g_weaponAliasInfo` (`weapontype.cpp`, read 2026-11-22), without the terminator. The first
/// match wins (`WeaponIDToAlias`), so `HeGrenade` maps back to `"hegren"` and `Glock` to `"???"`.
static WEAPON_ALIAS: [(&str, WeaponId); 38] = [
    ("p228", WeaponId::P228),
    ("???", WeaponId::Glock),
    ("scout", WeaponId::Scout),
    ("hegren", WeaponId::HeGrenade),
    ("xm1014", WeaponId::Xm1014),
    ("c4", WeaponId::C4),
    ("mac10", WeaponId::Mac10),
    ("aug", WeaponId::Aug),
    ("sgren", WeaponId::SmokeGrenade),
    ("elites", WeaponId::Elite),
    ("fn57", WeaponId::FiveSeven),
    ("ump45", WeaponId::Ump45),
    ("sg550", WeaponId::Sg550),
    ("galil", WeaponId::Galil),
    ("famas", WeaponId::Famas),
    ("usp", WeaponId::Usp),
    ("glock", WeaponId::Glock18),
    ("awp", WeaponId::Awp),
    ("mp5", WeaponId::Mp5n),
    ("m249", WeaponId::M249),
    ("m3", WeaponId::M3),
    ("m4a1", WeaponId::M4a1),
    ("tmp", WeaponId::Tmp),
    ("g3sg1", WeaponId::G3sg1),
    ("flash", WeaponId::Flashbang),
    ("deagle", WeaponId::Deagle),
    ("sg552", WeaponId::Sg552),
    ("ak47", WeaponId::Ak47),
    ("knife", WeaponId::Knife),
    ("p90", WeaponId::P90),
    ("shield", WeaponId::ShieldGun),
    ("none", WeaponId::None),
    ("grenade", WeaponId::HeGrenade),
    ("hegrenade", WeaponId::HeGrenade),
    ("glock18", WeaponId::Glock18),
    ("elite", WeaponId::Elite),
    ("fiveseven", WeaponId::FiveSeven),
    ("mp5navy", WeaponId::Mp5n),
];

/// `g_weaponBuyAliasInfo` (`weapontype.cpp`, read 2026-11-22) plus Build 10210 `autobuy.txt`
/// full alias `nighthawk`: alias, weapon, the `failName` (the localised name shown when the player
/// cannot buy it).
static WEAPON_BUY_ALIAS: [(&str, WeaponId, &str); 43] = [
    ("galil", WeaponId::Galil, "#Galil"),
    ("defender", WeaponId::Galil, "#Galil"),
    ("ak47", WeaponId::Ak47, "#AK47"),
    ("cv47", WeaponId::Ak47, "#AK47"),
    ("scout", WeaponId::Scout, "#Scout"),
    ("sg552", WeaponId::Sg552, "#SG552"),
    ("krieg552", WeaponId::Sg552, "#SG552"),
    ("awp", WeaponId::Awp, "#ArcticWarfareMagnum"),
    ("magnum", WeaponId::Awp, "#ArcticWarfareMagnum"),
    ("g3sg1", WeaponId::G3sg1, "#G3SG1"),
    ("d3au1", WeaponId::G3sg1, "#G3SG1"),
    ("famas", WeaponId::Famas, "#Famas"),
    ("clarion", WeaponId::Famas, "#Famas"),
    ("m4a1", WeaponId::M4a1, "#M4A1"),
    ("aug", WeaponId::Aug, "#Aug"),
    ("bullpup", WeaponId::Aug, "#Aug"),
    ("sg550", WeaponId::Sg550, "#SG550"),
    ("krieg550", WeaponId::Sg550, "#SG550"),
    ("glock", WeaponId::Glock18, "#Glock18"),
    ("9x19mm", WeaponId::Glock18, "#Glock18"),
    ("usp", WeaponId::Usp, "#USP45"),
    ("km45", WeaponId::Usp, "#USP45"),
    ("p228", WeaponId::P228, "#P228"),
    ("228compact", WeaponId::P228, "#P228"),
    ("deagle", WeaponId::Deagle, "#DesertEagle"),
    ("nighthaw", WeaponId::Deagle, "#DesertEagle"),
    ("nighthawk", WeaponId::Deagle, "#DesertEagle"),
    ("elites", WeaponId::Elite, "#Beretta96G"),
    ("fn57", WeaponId::FiveSeven, "#FiveSeven"),
    ("fiveseven", WeaponId::FiveSeven, "#FiveSeven"),
    ("m3", WeaponId::M3, "#Super90"),
    ("12gauge", WeaponId::M3, "#Super90"),
    ("xm1014", WeaponId::Xm1014, "#XM1014"),
    ("autoshotgun", WeaponId::Xm1014, "#XM1014"),
    ("mac10", WeaponId::Mac10, "#Mac10"),
    ("tmp", WeaponId::Tmp, "#tmp"),
    ("mp", WeaponId::Tmp, "#tmp"),
    ("mp5", WeaponId::Mp5n, "#mp5navy"),
    ("smg", WeaponId::Mp5n, "#mp5navy"),
    ("ump45", WeaponId::Ump45, "#UMP45"),
    ("p90", WeaponId::P90, "#FNP90"),
    ("c90", WeaponId::P90, "#FNP90"),
    ("m249", WeaponId::M249, "#M249"),
];

/// `g_weaponClassAliasInfo` (`weapontype.cpp`, read 2026-11-22), without the terminator.
static WEAPON_CLASS_ALIAS: [(&str, WeaponClass); 45] = [
    ("p228", WeaponClass::Pistol),
    ("???", WeaponClass::Pistol),
    ("scout", WeaponClass::SniperRifle),
    ("hegren", WeaponClass::Grenade),
    ("xm1014", WeaponClass::Shotgun),
    ("c4", WeaponClass::Grenade),
    ("mac10", WeaponClass::SubMachineGun),
    ("aug", WeaponClass::Rifle),
    ("sgren", WeaponClass::Grenade),
    ("elites", WeaponClass::Pistol),
    ("fn57", WeaponClass::Pistol),
    ("ump45", WeaponClass::SubMachineGun),
    ("sg550", WeaponClass::SniperRifle),
    ("galil", WeaponClass::Rifle),
    ("famas", WeaponClass::Rifle),
    ("usp", WeaponClass::Pistol),
    ("glock", WeaponClass::Pistol),
    ("awp", WeaponClass::SniperRifle),
    ("mp5", WeaponClass::SubMachineGun),
    ("m249", WeaponClass::MachineGun),
    ("m3", WeaponClass::Shotgun),
    ("m4a1", WeaponClass::Rifle),
    ("tmp", WeaponClass::SubMachineGun),
    ("g3sg1", WeaponClass::SniperRifle),
    ("flash", WeaponClass::Grenade),
    ("deagle", WeaponClass::Pistol),
    ("sg552", WeaponClass::Rifle),
    ("ak47", WeaponClass::Rifle),
    ("knife", WeaponClass::Knife),
    ("p90", WeaponClass::SubMachineGun),
    ("shield", WeaponClass::Pistol),
    ("grenade", WeaponClass::Grenade),
    ("hegrenade", WeaponClass::Grenade),
    ("glock18", WeaponClass::Pistol),
    ("elite", WeaponClass::Pistol),
    ("fiveseven", WeaponClass::Pistol),
    ("mp5navy", WeaponClass::SubMachineGun),
    ("grenade", WeaponClass::Grenade),
    ("pistol", WeaponClass::Pistol),
    ("SMG", WeaponClass::SubMachineGun),
    ("machinegun", WeaponClass::MachineGun),
    ("shotgun", WeaponClass::Shotgun),
    ("rifle", WeaponClass::Rifle),
    ("sniper", WeaponClass::SniperRifle),
    ("none", WeaponClass::None),
];

/// The order `WeaponsPrecache` precaches the weapons in (`weapons.cpp`, read 2026-11-22). The
/// ammo registry is built in this order, not in the order of [`AmmoType`].
pub const WEAPONS_PRECACHE_ORDER: [WeaponId; 29] = [
    WeaponId::Awp,
    WeaponId::G3sg1,
    WeaponId::Ak47,
    WeaponId::Scout,
    WeaponId::M249,
    WeaponId::M4a1,
    WeaponId::Sg552,
    WeaponId::Aug,
    WeaponId::Sg550,
    WeaponId::M3,
    WeaponId::Xm1014,
    WeaponId::Usp,
    WeaponId::Mac10,
    WeaponId::Ump45,
    WeaponId::FiveSeven,
    WeaponId::P90,
    WeaponId::Deagle,
    WeaponId::P228,
    WeaponId::Knife,
    WeaponId::Glock18,
    WeaponId::Mp5n,
    WeaponId::Tmp,
    WeaponId::Elite,
    WeaponId::Flashbang,
    WeaponId::HeGrenade,
    WeaponId::SmokeGrenade,
    WeaponId::C4,
    WeaponId::Galil,
    WeaponId::Famas,
];

/// `m_AmmoInfoArray` after `WeaponsPrecache`: the name at each index (index 0 is unused).
static AMMO_REGISTRY: LazyLock<[Option<&'static str>; MAX_AMMO_SLOTS]> = LazyLock::new(|| {
    let mut reg: [Option<&'static str>; MAX_AMMO_SLOTS] = [None; MAX_AMMO_SLOTS];
    let mut giammo_index = 0usize;
    for id in WEAPONS_PRECACHE_ORDER {
        let Some(info) = item_info(id) else { continue };
        // `AddAmmoNameToAmmoRegistry(info.pszAmmo1)` then `pszAmmo2`.
        for name in [info.ammo1, info.ammo2].into_iter().flatten() {
            if name.is_empty() {
                continue;
            }
            if reg
                .iter()
                .skip(1)
                .flatten()
                .any(|n| n.eq_ignore_ascii_case(name))
            {
                continue;
            }
            giammo_index += 1;
            if giammo_index >= MAX_AMMO_SLOTS {
                giammo_index = 1;
            }
            reg[giammo_index] = Some(name);
        }
    }
    reg
});

/// `CBasePlayerItem::m_ItemInfoArray[id]` after `WeaponsPrecache`: the `ItemInfo` of a weapon, or
/// `None` for an id that has no `GetItemInfo` (`WEAPON_NONE`, `WEAPON_GLOCK`, the shield).
pub fn item_info(id: WeaponId) -> Option<&'static ItemInfo> {
    ITEM_INFO.iter().find(|i| i.id == id)
}

/// `GetWeaponInfo(int weaponID)`: the first row of `g_weaponInfo` with this id. Vanilla has no
/// rows for the C4, the knife and the grenades.
pub fn weapon_info(id: WeaponId) -> Option<&'static WeaponInfo> {
    WEAPON_INFO.iter().find(|i| i.id == id)
}

/// `GetWeaponInfo(const char *weaponName)`: the first row whose entity name matches, without
/// case. Quirk: `"weapon_glock18"` finds the `WEAPON_GLOCK` row (id 2), which comes first.
pub fn weapon_info_by_name(name: &str) -> Option<&'static WeaponInfo> {
    WEAPON_INFO
        .iter()
        .find(|i| i.entity_name.eq_ignore_ascii_case(name))
}

/// Every row of `g_weaponInfo_default`.
pub fn weapon_infos() -> &'static [WeaponInfo] {
    &WEAPON_INFO
}

/// Every `GetItemInfo` result.
pub fn item_infos() -> &'static [ItemInfo] {
    &ITEM_INFO
}

/// `GetAmmoInfo(AmmoType)`.
pub fn ammo_info(ammo: AmmoType) -> Option<&'static AmmoInfo> {
    AMMO_INFO.iter().find(|a| a.ammo_type == ammo)
}

/// `GetAmmoInfo(const char *ammoName)`: matches `ammoName1` (`"ammo_9mm"`) without case.
pub fn ammo_info_by_name(name: &str) -> Option<&'static AmmoInfo> {
    AMMO_INFO
        .iter()
        .find(|a| a.ammo_name1.is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// `g_weaponStruct`: who can buy the weapon and its ammo price.
pub fn weapon_struct(id: WeaponId) -> Option<&'static WeaponStruct> {
    WEAPON_STRUCT.iter().find(|w| w.id == id)
}

/// `GetWeaponSlot(WeaponIdType)`.
pub fn weapon_slot(id: WeaponId) -> Option<&'static WeaponSlotInfo> {
    WEAPON_SLOT_INFO.iter().find(|s| s.id == id)
}

/// `g_autoBuyInfo` without the terminator.
pub fn auto_buy_info() -> &'static [AutoBuyInfo] {
    &AUTO_BUY_INFO
}

/// `AliasToWeaponID`: the first alias that matches without case. `None` where the C function
/// returns `WEAPON_NONE` for "not found"; the alias `"none"` gives `Some(WeaponId::None)`.
pub fn alias_to_weapon_id(alias: &str) -> Option<WeaponId> {
    WEAPON_ALIAS
        .iter()
        .find(|(a, _)| a.eq_ignore_ascii_case(alias))
        .map(|&(_, id)| id)
}

/// `WeaponIDToAlias`: the first alias of the weapon.
pub fn weapon_id_to_alias(id: WeaponId) -> Option<&'static str> {
    WEAPON_ALIAS
        .iter()
        .find(|&&(_, i)| i == id)
        .map(|&(a, _)| a)
}

/// `BuyAliasToWeaponID`: the weapon and the `failName` of the buy alias, without case.
pub fn buy_alias_info(alias: &str) -> Option<(WeaponId, &'static str)> {
    WEAPON_BUY_ALIAS
        .iter()
        .find(|(a, _, _)| a.eq_ignore_ascii_case(alias))
        .map(|&(_, id, fail)| (id, fail))
}

/// `BuyAliasToWeaponID`, the weapon only.
pub fn buy_alias_to_weapon_id(alias: &str) -> Option<WeaponId> {
    buy_alias_info(alias).map(|(id, _)| id)
}

/// `AliasToWeaponClass`: without case; `WeaponClass::None` when not found.
pub fn alias_to_weapon_class(alias: &str) -> WeaponClass {
    WEAPON_CLASS_ALIAS
        .iter()
        .find(|(a, _)| a.eq_ignore_ascii_case(alias))
        .map_or(WeaponClass::None, |&(_, c)| c)
}

/// `WeaponIDToWeaponClass(int)`: through the alias of the weapon. Quirk: the C4 is a grenade
/// class, the shield a pistol, the Glock (2) a pistol through `"???"`.
pub fn weapon_id_to_class(id: WeaponId) -> WeaponClass {
    weapon_id_to_alias(id).map_or(WeaponClass::None, alias_to_weapon_class)
}

/// `IsPrimaryWeapon`.
pub fn is_primary_weapon(id: WeaponId) -> bool {
    use WeaponId::*;
    matches!(
        id,
        Scout
            | Xm1014
            | Mac10
            | Aug
            | Ump45
            | Sg550
            | Galil
            | Famas
            | Awp
            | Mp5n
            | M249
            | M3
            | M4a1
            | Tmp
            | G3sg1
            | Sg552
            | Ak47
            | P90
            | ShieldGun
    )
}

/// `IsSecondaryWeapon`.
pub fn is_secondary_weapon(id: WeaponId) -> bool {
    use WeaponId::*;
    matches!(id, P228 | Elite | FiveSeven | Usp | Glock18 | Deagle)
}

/// `IsGrenadeWeapon`.
pub fn is_grenade_weapon(id: WeaponId) -> bool {
    use WeaponId::*;
    matches!(id, HeGrenade | Flashbang | SmokeGrenade)
}

/// `MaxAmmoCarry(const char *szName)` (`weapons.cpp`, read 2026-11-22): the maximum of the ammo
/// named `name` (without case), searched in the item infos of the precached weapons; -1 when no
/// weapon uses it (the C code also logs to the console).
pub fn max_ammo_carry(name: &str) -> i32 {
    for info in &ITEM_INFO {
        if info.ammo1.is_some_and(|a| a.eq_ignore_ascii_case(name)) {
            return info.max_ammo1;
        }
        if info.ammo2.is_some_and(|a| a.eq_ignore_ascii_case(name)) {
            return info.max_ammo2;
        }
    }
    -1
}

/// `MaxAmmoCarry(WeaponIdType)`: `m_ItemInfoArray[id].iMaxAmmo1`; 0 for an id without item info
/// (the array is zeroed).
pub fn max_ammo_carry_for_weapon(id: WeaponId) -> i32 {
    item_info(id).map_or(0, |i| i.max_ammo1)
}

/// The ammo registry `AddAmmoNameToAmmoRegistry` builds while `WeaponsPrecache` runs: the name
/// at each index 0..32 (index 0 is never used). It is what the `AmmoX` and `WeaponList` messages
/// number the ammo by.
pub fn ammo_registry() -> &'static [Option<&'static str>; MAX_AMMO_SLOTS] {
    &AMMO_REGISTRY
}

/// `AddAmmoNameToAmmoRegistry` for a name already precached: its registry index (the name is
/// compared without case), or -1 for an empty or unknown name.
pub fn ammo_index(name: &str) -> i32 {
    if name.is_empty() {
        return -1;
    }
    AMMO_REGISTRY
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, n)| n.is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .map_or(-1, |(i, _)| i as i32)
}

/// The `*_DEFAULT_GIVE` of a weapon (`ClipGiveDefault`, `weapontype.h`): the ammo in the clip it
/// spawns with (`m_iDefaultAmmo`). The knife has none (`KNIFE_DEFAULT_GIVE` is commented out).
pub fn default_give(id: WeaponId) -> Option<i32> {
    use WeaponId::*;
    Some(match id {
        P228 => 13,
        Glock18 => 20,
        Scout => 10,
        HeGrenade => 1,
        Xm1014 => 7,
        C4 => 1,
        Mac10 => 30,
        Aug => 30,
        SmokeGrenade => 1,
        Elite => 30,
        FiveSeven => 20,
        Ump45 => 25,
        Sg550 => 30,
        Galil => 35,
        Famas => 25,
        Usp => 12,
        Awp => 10,
        Mp5n => 30,
        M249 => 100,
        M3 => 8,
        M4a1 => 30,
        Tmp => 30,
        G3sg1 => 20,
        Flashbang => 1,
        Deagle => 7,
        Sg552 => 30,
        Ak47 => 30,
        P90 => 50,
        _ => return Option::None,
    })
}
