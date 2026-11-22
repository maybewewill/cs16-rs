//! The grenades and the C4. Placeholders until Task 13 ports the grenades; the C4 waits for
//! the bomb of phase 2E (it refuses to deploy until then). The explosion events of the HE and
//! the smoke grenade are precached here, where their `Precache` precaches them, so the event
//! indexes keep the order of `WeaponsPrecache`.

use cs_engine::EngineApi;

use super::core::WeaponCtx;
use super::info::WeaponId;
use super::weapon::{WeaponBehavior, stub_weapon};

/// The HE grenade. Stub: Task 13.
#[derive(Debug, Clone, Default)]
pub struct HeGrenade;

impl WeaponBehavior for HeGrenade {
    fn id(&self) -> WeaponId {
        WeaponId::HeGrenade
    }

    /// `CHEGrenade::Precache` (rg_wpn_hegrenade.cpp:36, read 2026-11-22): the event of the
    /// explosion (the models and sounds come with Task 13).
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        eng.precache_event("events/createexplo.sc");
    }

    fn deploy(&mut self, _w: &mut WeaponCtx<'_, '_>) -> bool {
        false
    }
}

stub_weapon!(
    /// The flashbang.
    Flashbang, WeaponId::Flashbang, "13"
);

/// The smoke grenade. Stub: Task 13.
#[derive(Debug, Clone, Default)]
pub struct SmokeGrenade;

impl WeaponBehavior for SmokeGrenade {
    fn id(&self) -> WeaponId {
        WeaponId::SmokeGrenade
    }

    /// `CSmokeGrenade::Precache` (rg_wpn_smokegrenade.cpp:34, read 2026-11-22): the event of the
    /// smoke (the models and sounds come with Task 13).
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        eng.precache_event("events/createsmoke.sc");
    }

    fn deploy(&mut self, _w: &mut WeaponCtx<'_, '_>) -> bool {
        false
    }
}

stub_weapon!(
    /// The C4; phase 2E ports it.
    C4, WeaponId::C4, "2E"
);
