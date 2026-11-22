//! The weapon tables and the shared random generator against ReGameDLL_CS (`weapontype.cpp`,
//! `wpn_shared/wpn_*.cpp`, `weapons.cpp`, `util.cpp`, read 2026-11-22). The expected rows below
//! were transcribed from those files by a script that is not part of the library, so the tests
//! do not depend on the library's own tables.

use cs_game::weapons::{
    default_give, weapon_struct,
    AmmoType, ItemInfo, WeaponId, alias_to_weapon_id, ammo_index, ammo_registry, buy_alias_info,
    buy_alias_to_weapon_id, item_info, max_ammo_carry, shared_random_float, shared_random_long,
    weapon_id_to_alias, weapon_info, weapon_info_by_name, weapon_slot,
};

fn id(n: i32) -> WeaponId {
    WeaponId::from_i32(n).expect("a weapon id")
}

type WeaponRow = (i32, i32, i32, i32, i32, i32, i32, &'static str, &'static str, Option<&'static str>);

/// `g_weaponInfo_default` (vanilla): id, cost, clipCost, buyClipSize, gunClipSize, maxRounds,
/// ammoType, entityName, ammoName1 (`""` for nullptr), ammoName2.
const WEAPON_ROWS: [WeaponRow; 26] = [
    (1, 600, 50, 13, 13, 52, 9, "weapon_p228", "ammo_357sig", Some("357SIG")),
    (2, 400, 20, 30, 20, 120, 10, "weapon_glock18", "ammo_9mm", Some("9mm")),
    (17, 400, 20, 30, 20, 120, 10, "weapon_glock18", "ammo_9mm", Some("9mm")),
    (3, 2750, 80, 30, 10, 90, 2, "weapon_scout", "ammo_762nato", Some("762Nato")),
    (5, 3000, 65, 8, 7, 32, 5, "weapon_xm1014", "ammo_buckshot", Some("buckshot")),
    (7, 1400, 25, 12, 30, 100, 6, "weapon_mac10", "ammo_45acp", Some("45acp")),
    (8, 3500, 60, 30, 30, 90, 4, "weapon_aug", "ammo_556nato", Some("556Nato")),
    (10, 800, 20, 30, 30, 120, 10, "weapon_elite", "ammo_9mm", Some("9mm")),
    (11, 750, 50, 50, 20, 100, 7, "weapon_fiveseven", "ammo_57mm", Some("57mm")),
    (12, 1700, 25, 12, 25, 100, 6, "weapon_ump45", "ammo_45acp", Some("45acp")),
    (13, 4200, 60, 30, 30, 90, 4, "weapon_sg550", "ammo_556nato", Some("556Nato")),
    (14, 2000, 60, 30, 35, 90, 4, "weapon_galil", "ammo_556nato", Some("556Nato")),
    (15, 2250, 60, 30, 25, 90, 4, "weapon_famas", "ammo_556nato", Some("556Nato")),
    (16, 500, 25, 12, 12, 100, 6, "weapon_usp", "ammo_45acp", Some("45acp")),
    (18, 4750, 125, 10, 10, 30, 1, "weapon_awp", "ammo_338magnum", Some("338Magnum")),
    (19, 1500, 20, 30, 30, 120, 10, "weapon_mp5navy", "ammo_9mm", Some("9mm")),
    (20, 5750, 60, 30, 100, 200, 3, "weapon_m249", "ammo_556natobox", Some("556NatoBox")),
    (21, 1700, 65, 8, 8, 32, 5, "weapon_m3", "ammo_buckshot", Some("buckshot")),
    (22, 3100, 60, 30, 30, 90, 4, "weapon_m4a1", "ammo_556nato", Some("556Nato")),
    (23, 1250, 20, 30, 30, 120, 10, "weapon_tmp", "ammo_9mm", Some("9mm")),
    (24, 5000, 80, 30, 20, 90, 2, "weapon_g3sg1", "ammo_762nato", Some("762Nato")),
    (26, 650, 40, 7, 7, 35, 8, "weapon_deagle", "ammo_50ae", Some("50AE")),
    (27, 3500, 60, 30, 30, 90, 4, "weapon_sg552", "ammo_556nato", Some("556Nato")),
    (28, 2500, 80, 30, 30, 90, 2, "weapon_ak47", "ammo_762nato", Some("762Nato")),
    (30, 2350, 50, 50, 50, 100, 7, "weapon_p90", "ammo_57mm", Some("57mm")),
    (99, 2200, 0, 0, 0, 0, 0, "weapon_shield", "", None),
];

#[test]
fn prices_clips_and_ammo_match_weapontype_cpp() {
    for &(wid, cost, clip_cost, buy, gun, max, ammo, ent, n1, n2) in &WEAPON_ROWS {
        let w = id(wid);
        // `GetWeaponInfo(int)` returns the first row of the id.
        let info = weapon_info(w).unwrap_or_else(|| panic!("no row for {wid}"));
        assert_eq!(info.id, w);
        assert_eq!(info.cost, cost, "cost of {wid}");
        assert_eq!(info.clip_cost, clip_cost, "clip cost of {wid}");
        assert_eq!(info.buy_clip_size, buy, "buy clip of {wid}");
        assert_eq!(info.gun_clip_size, gun, "gun clip of {wid}");
        assert_eq!(info.max_rounds, max, "max rounds of {wid}");
        assert_eq!(info.ammo_type as i32, ammo, "ammo type of {wid}");
        assert_eq!(info.entity_name, ent, "entity of {wid}");
        assert_eq!(info.ammo_name1, n1, "ammo name 1 of {wid}");
        assert_eq!(info.ammo_name2, n2, "ammo name 2 of {wid}");
    }
    assert_eq!(cs_game::weapons::weapon_infos().len(), WEAPON_ROWS.len());

    // Spot checks typed by hand from `WeaponCostType`.
    assert_eq!(weapon_info(WeaponId::Ak47).unwrap().cost, 2500);
    assert_eq!(weapon_info(WeaponId::Awp).unwrap().cost, 4750);
    assert_eq!(weapon_info(WeaponId::M249).unwrap().cost, 5750);
    assert_eq!(weapon_info(WeaponId::ShieldGun).unwrap().cost, 2200);
    // Vanilla quirk: the Galil is $2000 (the $2250 is the `REGAMEDLL_FIXES` branch).
    assert_eq!(weapon_info(WeaponId::Galil).unwrap().cost, 2000);
    // Vanilla: no rows for the knife, the C4 and the grenades.
    for w in [WeaponId::Knife, WeaponId::C4, WeaponId::HeGrenade, WeaponId::Flashbang, WeaponId::SmokeGrenade] {
        assert!(weapon_info(w).is_none(), "{w:?}");
    }
    // Quirk: the `WEAPON_GLOCK` row is first and names `weapon_glock18`.
    assert_eq!(weapon_info_by_name("weapon_glock18").unwrap().id, WeaponId::Glock);
    assert_eq!(weapon_info_by_name("WEAPON_AK47").unwrap().id, WeaponId::Ak47);
    assert!(weapon_info_by_name("weapon_nothing").is_none());
    // `g_weaponSlotInfo`: the quirk of the `WEAPON_GLOCK` name, and the grenade and C4 slots.
    assert_eq!(weapon_slot(WeaponId::Glock).unwrap().weapon_name, "weapon_glock");
    assert_eq!(weapon_slot(WeaponId::Flashbang).unwrap().slot as i32, 4);
    assert_eq!(weapon_slot(WeaponId::C4).unwrap().slot as i32, 5);
}

/// `GetItemInfo` of each weapon: id, slot, position, ammo1, maxAmmo1, maxClip, flags, weight.
type ItemRow = (i32, i32, i32, Option<&'static str>, i32, i32, i32, i32);

const ITEM_ROWS: [ItemRow; 29] = [
    (28, 0, 1, Some("762Nato"), 90, 30, 0, 25),  // AK47
    (8, 0, 14, Some("556Nato"), 90, 30, 0, 25),  // AUG
    (18, 0, 2, Some("338Magnum"), 30, 10, 0, 30),  // AWP
    (6, 4, 3, Some("C4"), 1, -1, 24, 3),  // C4
    (26, 1, 1, Some("50AE"), 35, 7, 0, 7),  // DEAGLE
    (10, 1, 5, Some("9mm"), 120, 30, 0, 5),  // ELITE
    (15, 0, 18, Some("556Nato"), 90, 25, 32, 75),  // FAMAS
    (11, 1, 6, Some("57mm"), 100, 20, 0, 5),  // FIVESEVEN
    (25, 3, 2, Some("Flashbang"), 2, -1, 24, 1),  // FLASHBANG
    (24, 0, 3, Some("762Nato"), 90, 20, 0, 20),  // G3SG1
    (14, 0, 17, Some("556Nato"), 90, 35, 32, 25),  // GALIL
    (17, 1, 2, Some("9mm"), 120, 20, 0, 5),  // GLOCK18
    (4, 3, 1, Some("HEGrenade"), 1, -1, 24, 2),  // HEGRENADE
    (29, 2, 1, None, -1, -1, 0, 0),  // KNIFE
    (20, 0, 4, Some("556NatoBox"), 200, 100, 0, 25),  // M249
    (21, 0, 5, Some("buckshot"), 32, 8, 32, 20),  // M3
    (22, 0, 6, Some("556Nato"), 90, 30, 0, 25),  // M4A1
    (7, 0, 13, Some("45acp"), 100, 30, 0, 25),  // MAC10
    (19, 0, 7, Some("9mm"), 120, 30, 0, 25),  // MP5N
    (1, 1, 3, Some("357SIG"), 52, 13, 0, 5),  // P228
    (30, 0, 8, Some("57mm"), 100, 50, 0, 26),  // P90
    (3, 0, 9, Some("762Nato"), 90, 10, 0, 30),  // SCOUT
    (13, 0, 16, Some("556Nato"), 90, 30, 0, 20),  // SG550
    (27, 0, 10, Some("556Nato"), 90, 30, 0, 25),  // SG552
    (9, 3, 3, Some("SmokeGrenade"), 1, -1, 24, 1),  // SMOKEGRENADE
    (23, 0, 11, Some("9mm"), 120, 30, 0, 25),  // TMP
    (12, 0, 15, Some("45ACP"), 100, 25, 0, 25),  // UMP45
    (16, 1, 4, Some("45ACP"), 100, 12, 0, 5),  // USP
    (5, 0, 12, Some("buckshot"), 32, 7, 32, 20),  // XM1014
];

#[test]
fn item_info_matches_get_item_info() {
    for &(wid, slot, pos, ammo1, max1, clip, flags, weight) in &ITEM_ROWS {
        let w = id(wid);
        let info: &ItemInfo = item_info(w).unwrap_or_else(|| panic!("no item info for {wid}"));
        assert_eq!(info.id, w);
        assert_eq!(info.slot, slot, "slot of {wid}");
        assert_eq!(info.position, pos, "position of {wid}");
        assert_eq!(info.ammo1, ammo1, "ammo1 of {wid}");
        assert_eq!(info.max_ammo1, max1, "max ammo of {wid}");
        assert_eq!(info.ammo2, None);
        assert_eq!(info.max_ammo2, -1);
        assert_eq!(info.max_clip, clip, "clip of {wid}");
        assert_eq!(info.flags, flags, "flags of {wid}");
        assert_eq!(info.weight, weight, "weight of {wid}");
    }
    // No `GetItemInfo` for the unused Glock, the shield and `WEAPON_NONE`.
    assert!(item_info(WeaponId::Glock).is_none());
    assert!(item_info(WeaponId::ShieldGun).is_none());
    assert!(item_info(WeaponId::None).is_none());
    // `pszName` is the classname of the entity.
    assert_eq!(item_info(WeaponId::Mp5n).unwrap().name, "weapon_mp5navy");
    assert_eq!(item_info(WeaponId::Glock18).unwrap().name, "weapon_glock18");
    assert_eq!(item_info(WeaponId::Ak47).unwrap().name, "weapon_ak47");

    // `MaxAmmoCarry(name)`: without case, -1 when unknown.
    assert_eq!(max_ammo_carry("762Nato"), 90);
    assert_eq!(max_ammo_carry("762nato"), 90);
    assert_eq!(max_ammo_carry("556NatoBox"), 200);
    assert_eq!(max_ammo_carry("45acp"), 100);
    assert_eq!(max_ammo_carry("Flashbang"), 2);
    assert_eq!(max_ammo_carry("nonsense"), -1);
}

#[test]
fn ammo_indices_follow_precache_order() {
    // `WeaponsPrecache` order: awp, g3sg1, ak47, scout, m249, m4a1 (.. sg552, aug, sg550), m3,
    // usp, fiveseven, deagle, p228, knife, glock18, flashbang, hegrenade, smokegrenade, c4. The
    // USP is the first to name the .45 ammo and spells it "45ACP".
    let expected = [
        None,
        Some("338Magnum"),
        Some("762Nato"),
        Some("556NatoBox"),
        Some("556Nato"),
        Some("buckshot"),
        Some("45ACP"),
        Some("57mm"),
        Some("50AE"),
        Some("357SIG"),
        Some("9mm"),
        Some("Flashbang"),
        Some("HEGrenade"),
        Some("SmokeGrenade"),
        Some("C4"),
    ];
    let reg = ammo_registry();
    assert_eq!(reg.len(), 32);
    for (i, name) in reg.iter().enumerate() {
        assert_eq!(*name, expected.get(i).copied().flatten(), "index {i}");
    }
    assert_eq!(ammo_index("45acp"), 6);
    assert_eq!(ammo_index("45ACP"), 6);
    assert_eq!(ammo_index("9MM"), 10);
    assert_eq!(ammo_index("C4"), 14);
    assert_eq!(ammo_index("nonsense"), -1);
    assert_eq!(ammo_index(""), -1);
    // `AmmoType` numbers the ammo the same way as the registry.
    for info in cs_game::weapons::weapon_infos() {
        if info.ammo_type != AmmoType::None {
            let n2 = info.ammo_name2.unwrap();
            assert_eq!(ammo_index(n2), info.ammo_type as i32, "{n2}");
        }
    }
}

#[test]
fn aliases_round_trip() {
    for (alias, w) in [
        ("p228", WeaponId::P228),
        ("???", WeaponId::Glock),
        ("hegren", WeaponId::HeGrenade),
        ("grenade", WeaponId::HeGrenade),
        ("hegrenade", WeaponId::HeGrenade),
        ("sgren", WeaponId::SmokeGrenade),
        ("elites", WeaponId::Elite),
        ("elite", WeaponId::Elite),
        ("glock", WeaponId::Glock18),
        ("glock18", WeaponId::Glock18),
        ("mp5", WeaponId::Mp5n),
        ("mp5navy", WeaponId::Mp5n),
        ("fn57", WeaponId::FiveSeven),
        ("shield", WeaponId::ShieldGun),
        ("KNIFE", WeaponId::Knife),
        ("none", WeaponId::None),
    ] {
        assert_eq!(alias_to_weapon_id(alias), Some(w), "{alias}");
    }
    assert_eq!(alias_to_weapon_id("bogus"), None);
    // The first alias of an id is its short one, and it maps back.
    for n in 1..=30 {
        let w = id(n);
        let a = weapon_id_to_alias(w).unwrap_or_else(|| panic!("alias of {n}"));
        assert_eq!(alias_to_weapon_id(a), Some(w), "{a}");
    }
    assert_eq!(weapon_id_to_alias(WeaponId::HeGrenade), Some("hegren"));
    assert_eq!(weapon_id_to_alias(WeaponId::Glock18), Some("glock"));

    for (alias, w, fail) in [
        ("galil", WeaponId::Galil, "#Galil"),
        ("defender", WeaponId::Galil, "#Galil"),
        ("cv47", WeaponId::Ak47, "#AK47"),
        ("magnum", WeaponId::Awp, "#ArcticWarfareMagnum"),
        ("9x19mm", WeaponId::Glock18, "#Glock18"),
        ("nighthaw", WeaponId::Deagle, "#DesertEagle"),
        ("elites", WeaponId::Elite, "#Beretta96G"),
        ("12GAUGE", WeaponId::M3, "#Super90"),
        ("mp", WeaponId::Tmp, "#tmp"),
        ("smg", WeaponId::Mp5n, "#mp5navy"),
        ("c90", WeaponId::P90, "#FNP90"),
        ("m249", WeaponId::M249, "#M249"),
    ] {
        assert_eq!(buy_alias_info(alias), Some((w, fail)), "{alias}");
        assert_eq!(buy_alias_to_weapon_id(alias), Some(w));
    }
    // Not in the buy table: the knife, the grenades, the shield.
    assert_eq!(buy_alias_to_weapon_id("knife"), None);
    assert_eq!(buy_alias_to_weapon_id("hegren"), None);
    assert_eq!(buy_alias_to_weapon_id(""), None);
}

// ---- the shared random generator ----------------------------------------------------------

/// `seed_table` of `util.cpp`, typed again here from the source.
const SEEDS: [u32; 256] = [
    28985, 27138, 26457, 9451, 17764, 10909, 28790, 8716, 6361, 4853, 17798, 21977, 19643, 20662, 10834, 20103,
    27067, 28634, 18623, 25849, 8576, 26234, 23887, 18228, 32587, 4836, 3306, 1811, 3035, 24559, 18399, 315,
    26766, 907, 24102, 12370, 9674, 2972, 10472, 16492, 22683, 11529, 27968, 30406, 13213, 2319, 23620, 16823,
    10013, 23772, 21567, 1251, 19579, 20313, 18241, 30130, 8402, 20807, 27354, 7169, 21211, 17293, 5410, 19223,
    10255, 22480, 27388, 9946, 15628, 24389, 17308, 2370, 9530, 31683, 25927, 23567, 11694, 26397, 32602, 15031,
    18255, 17582, 1422, 28835, 23607, 12597, 20602, 10138, 5212, 1252, 10074, 23166, 19823, 31667, 5902, 24630,
    18948, 14330, 14950, 8939, 23540, 21311, 22428, 22391, 3583, 29004, 30498, 18714, 4278, 2437, 22430, 3439,
    28313, 23161, 25396, 13471, 19324, 15287, 2563, 18901, 13103, 16867, 9714, 14322, 15197, 26889, 19372, 26241,
    31925, 14640, 11497, 8941, 10056, 6451, 28656, 10737, 13874, 17356, 8281, 25937, 1661, 4850, 7448, 12744,
    21826, 5477, 10167, 16705, 26897, 8839, 30947, 27978, 27283, 24685, 32298, 3525, 12398, 28726, 9475, 10208,
    617, 13467, 22287, 2376, 6097, 26312, 2974, 9114, 21787, 28010, 4725, 15387, 3274, 10762, 31695, 17320,
    18324, 12441, 16801, 27376, 22464, 7500, 5666, 18144, 15314, 31914, 31627, 6495, 5226, 31203, 2331, 4668,
    12650, 18275, 351, 7268, 31319, 30119, 7600, 2905, 13826, 11343, 13053, 15583, 30055, 31093, 5067, 761,
    9685, 11070, 21369, 27155, 3663, 26542, 20169, 12161, 15411, 30401, 7580, 31784, 8985, 29367, 20989, 14203,
    29694, 21167, 10337, 1706, 28578, 887, 3373, 19477, 14382, 675, 7033, 15111, 26138, 12252, 30996, 21409,
    25678, 18555, 13256, 23316, 22407, 16727, 991, 9236, 5373, 29402, 6117, 15241, 27715, 19291, 19888, 19847,
];

/// The C globals of `util.cpp`, transliterated line by line:
///
/// ```c
/// unsigned int glSeed;
/// unsigned int U_Random() { glSeed *= 69069; glSeed += seed_table[glSeed & 0xFF] + 1; return glSeed & 0xFFFFFFF; }
/// void U_Srand(unsigned int seed) { glSeed = seed_table[seed & 0xFF]; }
/// int UTIL_SharedRandomLong(unsigned int seed, int low, int high) {
///     unsigned int range = high - low + 1;
///     U_Srand((unsigned int)(high + low + seed));
///     if (range != 1) { int rnum = U_Random(); int offset = rnum % range; return low + offset; }
///     return low;
/// }
/// float UTIL_SharedRandomFloat(unsigned int seed, float low, float high) {
///     unsigned int range = high - low;
///     U_Srand((unsigned int)seed + *(unsigned int *)&low + *(unsigned int *)&high);
///     U_Random(); U_Random();
///     if (range) { int tensixrand = U_Random() & 0xFFFFu; float offset = float(tensixrand) / 0x10000u; return low + offset * range; }
///     return low;
/// }
/// ```
struct C {
    gl_seed: u32,
}

impl C {
    fn u_random(&mut self) -> u32 {
        self.gl_seed = self.gl_seed.wrapping_mul(69069);
        self.gl_seed = self.gl_seed.wrapping_add(SEEDS[(self.gl_seed & 0xFF) as usize].wrapping_add(1));
        self.gl_seed & 0xFFFFFFF
    }

    fn u_srand(&mut self, seed: u32) {
        self.gl_seed = SEEDS[(seed & 0xFF) as usize];
    }

    fn long(&mut self, seed: u32, low: i32, high: i32) -> i32 {
        let range: u32 = (high.wrapping_sub(low).wrapping_add(1)) as u32;
        self.u_srand((high.wrapping_add(low) as u32).wrapping_add(seed));
        if range != 1 {
            let rnum = self.u_random() as i32;
            let offset = (rnum as u32 % range) as i32;
            return low.wrapping_add(offset);
        }
        low
    }

    fn float(&mut self, seed: u32, low: f32, high: f32) -> f32 {
        // float -> unsigned int truncates toward zero (a negative span wraps through 64 bits).
        let range: u32 = ((high - low) as i64) as u32;
        self.u_srand(seed.wrapping_add(low.to_bits()).wrapping_add(high.to_bits()));
        self.u_random();
        self.u_random();
        if range != 0 {
            let tensixrand = self.u_random() & 0xFFFF;
            let offset = tensixrand as f32 / 65536_f32;
            return low + offset * range as f32;
        }
        low
    }
}

#[test]
fn shared_random_matches_util_cpp() {
    let mut c = C { gl_seed: 0 };

    // Values worked out from the algorithm by a third, independent port (a script, run once), so
    // a mistake shared by the library and the transliteration below would still show.
    assert_eq!(shared_random_long(0, 0, 1), 1);
    assert_eq!(shared_random_long(1, 0, 1), 1);
    assert_eq!(shared_random_long(12345, 0, 9), 6);
    assert_eq!(shared_random_long(7, -5, 5), 2);
    assert_eq!(shared_random_long(255, 1, 3), 2);
    assert_eq!(shared_random_long(99999, 10, 10), 10); // range 1: `low`, no draw
    assert_eq!(shared_random_float(0, 0.0, 1.0).to_bits(), 1054931968);
    assert_eq!(shared_random_float(1, -0.5, 0.5).to_bits(), 3201795584);
    assert_eq!(shared_random_float(123, -1.0, 1.0).to_bits(), 3205302272);
    assert_eq!(shared_random_float(77, 2.0, 10.0).to_bits(), 1088641024);
    assert_eq!(shared_random_float(31337, -100.0, 100.0).to_bits(), 3248053888);
    // Quirk: a span below 1 truncates to a range of 0, so the result is `low`.
    assert_eq!(shared_random_float(5, 0.0, 0.5), 0.0);
    assert_eq!(shared_random_float(5, 3.0, 3.99), 3.0);
    // The seed is a sum: different seeds give different draws.
    assert_ne!(shared_random_long(1, 0, 1000), shared_random_long(2, 0, 1000));

    // Against the transliteration: many seeds (also with sums beyond 32 bits), ranges and floats.
    let ranges = [
        (0, 1),
        (0, 9),
        (-5, 5),
        (1, 3),
        (-100, 100),
        (0, 255),
        (3, 3),
        (-7, -2),
        (0, 1_000_000),
        (i32::MIN / 2, i32::MAX / 2),
    ];
    let floats = [
        (0.0f32, 1.0f32),
        (-1.0, 1.0),
        (0.0, 0.5),
        (-0.5, 0.5),
        (2.0, 10.0),
        (-100.0, 100.0),
        (0.0, 0.0),
        (1.5, 1.5),
        (-3.0, 4.25),
        (0.0, 1.0e6),
        (5.0, 2.0), // a negative span: the range wraps
    ];
    for seed in (0u32..600).chain([0xFFFF_FFFF, 0x8000_0000, 0xDEAD_BEEF, 1 << 24]) {
        for &(lo, hi) in &ranges {
            assert_eq!(shared_random_long(seed, lo, hi), c.long(seed, lo, hi), "long {seed} {lo} {hi}");
        }
        for &(lo, hi) in &floats {
            let got = shared_random_float(seed, lo, hi);
            let want = c.float(seed, lo, hi);
            assert_eq!(got.to_bits(), want.to_bits(), "float {seed} {lo} {hi}");
        }
    }
    // No call depends on the earlier ones (the C code seeds first).
    let a = shared_random_long(42, 0, 99);
    let _ = shared_random_float(7, 0.0, 1.0);
    assert_eq!(shared_random_long(42, 0, 99), a);
    // `high == low - 1` is a division by zero in C; here it gives `low`.
    assert_eq!(shared_random_long(3, 5, 4), 5);
}

/// `g_weaponStruct` (`weapontype.cpp`): id, price, side (`TERRORIST` = 1, `CT` = 2), autobuy
/// class (`PRIMARY` = 1, `SECONDARY` = 2), ammo price. Typed from the source; the Galil is
/// $2000 in vanilla.
const STRUCT_ROWS: [(i32, i32, i32, i32, i32); 25] = [
    (1, 600, 3, 2, 50),
    (3, 2750, 3, 1, 80),
    (5, 3000, 3, 1, 65),
    (7, 1400, 2, 1, 25),
    (8, 3500, 1, 1, 60),
    (10, 800, 2, 2, 20),
    (11, 750, 3, 2, 50),
    (12, 1700, 1, 1, 25),
    (13, 4200, 1, 1, 60),
    (16, 500, 3, 2, 25),
    (17, 400, 3, 2, 20),
    (19, 1500, 3, 1, 20),
    (18, 4750, 3, 1, 125),
    (20, 5750, 3, 1, 60),
    (21, 1700, 3, 1, 65),
    (22, 3100, 1, 1, 60),
    (23, 1250, 1, 1, 20),
    (24, 5000, 2, 1, 80),
    (26, 650, 3, 2, 40),
    (27, 3500, 2, 1, 60),
    (28, 2500, 2, 1, 80),
    (30, 2350, 3, 1, 50),
    (15, 2250, 1, 1, 60),
    (14, 2000, 2, 1, 60),
    (99, 2200, 1, 1, 0),
];

#[test]
fn weapon_struct_matches_weapontype_cpp() {
    for &(wid, price, side, class, ammo) in &STRUCT_ROWS {
        let s = weapon_struct(id(wid)).unwrap_or_else(|| panic!("no struct row for {wid}"));
        assert_eq!(s.price, price, "price of {wid}");
        assert_eq!(s.side, side, "side of {wid}");
        assert_eq!(s.class, class, "class of {wid}");
        assert_eq!(s.ammo_price, ammo, "ammo price of {wid}");
    }
    // The C4, the knife, the grenades and the unused Glock are not in the table.
    for w in [WeaponId::C4, WeaponId::Knife, WeaponId::HeGrenade, WeaponId::Glock, WeaponId::None] {
        assert!(weapon_struct(w).is_none(), "{w:?}");
    }
}

/// `ClipGiveDefault` (`weapontype.h`): id, default give. The knife and the shield have none.
const GIVE_ROWS: [(i32, i32); 28] = [
    (1, 13),
    (17, 20),
    (3, 10),
    (4, 1),
    (5, 7),
    (6, 1),
    (7, 30),
    (8, 30),
    (9, 1),
    (10, 30),
    (11, 20),
    (12, 25),
    (13, 30),
    (14, 35),
    (15, 25),
    (16, 12),
    (18, 10),
    (19, 30),
    (20, 100),
    (21, 8),
    (22, 30),
    (23, 30),
    (24, 20),
    (25, 1),
    (26, 7),
    (27, 30),
    (28, 30),
    (30, 50),
];

#[test]
fn default_give_matches_clip_give_default() {
    for &(wid, give) in &GIVE_ROWS {
        assert_eq!(default_give(id(wid)), Some(give), "give of {wid}");
    }
    for w in [WeaponId::Knife, WeaponId::ShieldGun, WeaponId::Glock, WeaponId::None] {
        assert_eq!(default_give(w), None, "{w:?}");
    }
}
