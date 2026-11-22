//! The shotguns. Placeholders until Task 11 ports them; see [`super::weapon::new_weapon`].

use super::info::WeaponId;
use super::weapon::stub_weapon;

stub_weapon!(
    /// The M3.
    M3, WeaponId::M3, "11"
);

stub_weapon!(
    /// The XM1014.
    Xm1014, WeaponId::Xm1014, "11"
);
