//! The grenades and the C4. Placeholders until Task 13 ports the grenades; the C4 waits for
//! the bomb of phase 2E (it refuses to deploy until then).

use super::info::WeaponId;
use super::weapon::stub_weapon;

stub_weapon!(
    /// The HE grenade.
    HeGrenade, WeaponId::HeGrenade, "13"
);

stub_weapon!(
    /// The flashbang.
    Flashbang, WeaponId::Flashbang, "13"
);

stub_weapon!(
    /// The smoke grenade.
    SmokeGrenade, WeaponId::SmokeGrenade, "13"
);

stub_weapon!(
    /// The C4; phase 2E ports it.
    C4, WeaponId::C4, "2E"
);
