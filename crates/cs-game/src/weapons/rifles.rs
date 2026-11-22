//! The rifles and the machine gun. Placeholders until Task 10 ports them; see [`super::weapon::new_weapon`].

use super::info::WeaponId;
use super::weapon::stub_weapon;

stub_weapon!(
    /// The Galil.
    Galil, WeaponId::Galil, "10"
);

stub_weapon!(
    /// The Famas.
    Famas, WeaponId::Famas, "10"
);

stub_weapon!(
    /// The AK-47.
    Ak47, WeaponId::Ak47, "10"
);

stub_weapon!(
    /// The M4A1.
    M4a1, WeaponId::M4a1, "10"
);

stub_weapon!(
    /// The AUG.
    Aug, WeaponId::Aug, "10"
);

stub_weapon!(
    /// The SG552.
    Sg552, WeaponId::Sg552, "10"
);

stub_weapon!(
    /// The M249.
    M249, WeaponId::M249, "10"
);
