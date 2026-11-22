//! The pistols (`wpn_glock18.cpp`, `wpn_usp.cpp`, `wpn_p228.cpp`, `wpn_deagle.cpp`, `wpn_fiveseven.cpp`, `wpn_elite.cpp`). Placeholders until Task 8 ports them; see [`super::weapon::new_weapon`].

use super::info::WeaponId;
use super::weapon::stub_weapon;

stub_weapon!(
    /// The P228.
    P228, WeaponId::P228, "8"
);

stub_weapon!(
    /// The Glock 18.
    Glock18, WeaponId::Glock18, "8"
);

stub_weapon!(
    /// The USP.
    Usp, WeaponId::Usp, "8"
);

stub_weapon!(
    /// The Desert Eagle.
    Deagle, WeaponId::Deagle, "8"
);

stub_weapon!(
    /// The Five-seven.
    FiveSeven, WeaponId::FiveSeven, "8"
);

stub_weapon!(
    /// The dual Berettas.
    Elite, WeaponId::Elite, "8"
);
