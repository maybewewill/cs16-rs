//! The sniper rifles. Placeholders until Task 11 ports them; see [`super::weapon::new_weapon`].

use super::info::WeaponId;
use super::weapon::stub_weapon;

stub_weapon!(
    /// The Scout.
    Scout, WeaponId::Scout, "11"
);

stub_weapon!(
    /// The AWP.
    Awp, WeaponId::Awp, "11"
);

stub_weapon!(
    /// The G3SG1.
    G3sg1, WeaponId::G3sg1, "11"
);

stub_weapon!(
    /// The SG550.
    Sg550, WeaponId::Sg550, "11"
);
