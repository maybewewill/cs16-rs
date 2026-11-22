//! The submachine guns. Placeholders until Task 10 ports them; see [`super::weapon::new_weapon`].

use super::info::WeaponId;
use super::weapon::stub_weapon;

stub_weapon!(
    /// The MAC-10.
    Mac10, WeaponId::Mac10, "10"
);

stub_weapon!(
    /// The TMP.
    Tmp, WeaponId::Tmp, "10"
);

stub_weapon!(
    /// The MP5 Navy.
    Mp5n, WeaponId::Mp5n, "10"
);

stub_weapon!(
    /// The UMP45.
    Ump45, WeaponId::Ump45, "10"
);

stub_weapon!(
    /// The P90.
    P90, WeaponId::P90, "10"
);
