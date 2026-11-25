// SPDX-License-Identifier: GPL-3.0-or-later
//! Economy-aware CS 1.6 bot buy planner (`decide_bot_buy`).
//!
//! Ported from ReGameDLL_CS (`bot/cs_bot_buy.cpp`) and CS 1.6 weapon tables
//! (`crates/cs-game/src/weapons/info.rs`, `crates/cs-game/src/buy.rs`).

use crate::profile::{BotProfile, BotTeam, BotWeapon, WeaponCategory, mix64};

/// Vanilla CS 1.6 equipment prices (`weapontype.h` / `crates/cs-game/src/weapons/info.rs`).
pub const KEVLAR_PRICE: i32 = 650;
pub const HELMET_PRICE: i32 = 350;
pub const ASSAULTSUIT_PRICE: i32 = 1000;
pub const DEFUSEKIT_PRICE: i32 = 200;
pub const FLASHBANG_PRICE: i32 = 200;
pub const HEGRENADE_PRICE: i32 = 300;
pub const SMOKEGRENADE_PRICE: i32 = 300;
pub const SHIELDGUN_PRICE: i32 = 2200;

/// Weapon restriction cvars (`bot_allow_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponRestrictions {
    pub allow_pistols: bool,
    pub allow_shotguns: bool,
    pub allow_sub_machine_guns: bool,
    pub allow_rifles: bool,
    pub allow_snipers: bool,
    pub allow_machine_guns: bool,
    pub allow_grenades: bool,
    pub allow_shield: bool,
    pub allow_rogues: bool,
}

impl Default for WeaponRestrictions {
    fn default() -> Self {
        Self {
            allow_pistols: true,
            allow_shotguns: true,
            allow_sub_machine_guns: true,
            allow_rifles: true,
            allow_snipers: true,
            allow_machine_guns: true,
            allow_grenades: true,
            allow_shield: true,
            allow_rogues: true,
        }
    }
}

impl WeaponRestrictions {
    /// `bot_all_weapons`: enables every weapon category.
    pub fn all_weapons(&mut self) {
        self.allow_pistols = true;
        self.allow_shotguns = true;
        self.allow_sub_machine_guns = true;
        self.allow_rifles = true;
        self.allow_snipers = true;
        self.allow_machine_guns = true;
        self.allow_grenades = true;
        self.allow_shield = true;
    }

    /// `bot_knives_only`: disables all firearm/grenade/shield categories.
    pub fn knives_only(&mut self) {
        self.allow_pistols = false;
        self.allow_shotguns = false;
        self.allow_sub_machine_guns = false;
        self.allow_rifles = false;
        self.allow_snipers = false;
        self.allow_machine_guns = false;
        self.allow_grenades = false;
        self.allow_shield = false;
    }

    /// `bot_pistols_only`: allows only pistols.
    pub fn pistols_only(&mut self) {
        self.knives_only();
        self.allow_pistols = true;
    }

    /// `bot_snipers_only`: allows only snipers.
    pub fn snipers_only(&mut self) {
        self.knives_only();
        self.allow_snipers = true;
    }

    /// Returns true if `weapon` is permitted by the current `bot_allow_*` flags.
    pub const fn allows_weapon(&self, weapon: BotWeapon) -> bool {
        match weapon.category() {
            WeaponCategory::Knife | WeaponCategory::Equipment => true,
            WeaponCategory::Pistol => self.allow_pistols,
            WeaponCategory::Shotgun => self.allow_shotguns,
            WeaponCategory::Smg => self.allow_sub_machine_guns,
            WeaponCategory::Rifle => self.allow_rifles,
            WeaponCategory::Sniper => self.allow_snipers,
            WeaponCategory::MachineGun => self.allow_machine_guns,
            WeaponCategory::Grenade => self.allow_grenades,
            WeaponCategory::Shield => self.allow_shield,
        }
    }

    /// True when no primary, pistol, shield, or grenade category is enabled (knives-only).
    pub const fn is_knives_only(&self) -> bool {
        !self.allow_pistols
            && !self.allow_shotguns
            && !self.allow_sub_machine_guns
            && !self.allow_rifles
            && !self.allow_snipers
            && !self.allow_machine_guns
            && !self.allow_grenades
            && !self.allow_shield
    }
}

/// Vanilla CS 1.6 purchase price for `weapon` (`weapontype.cpp`).
pub const fn weapon_cost(weapon: BotWeapon) -> i32 {
    match weapon {
        BotWeapon::Glock18 => 400,
        BotWeapon::Usp => 500,
        BotWeapon::P228 => 600,
        BotWeapon::Deagle => 650,
        BotWeapon::FiveSeven => 750,
        BotWeapon::Elite => 800,
        BotWeapon::M3 => 1700,
        BotWeapon::Xm1014 => 3000,
        BotWeapon::Mac10 => 1400,
        BotWeapon::Tmp => 1250,
        BotWeapon::Mp5n => 1500,
        BotWeapon::Ump45 => 1700,
        BotWeapon::P90 => 2350,
        BotWeapon::Galil => 2000,
        BotWeapon::Famas => 2250,
        BotWeapon::Ak47 => 2500,
        BotWeapon::M4a1 => 3100,
        BotWeapon::Aug => 3500,
        BotWeapon::Sg552 => 3500,
        BotWeapon::Scout => 2750,
        BotWeapon::Awp => 4750,
        BotWeapon::Sg550 => 4200,
        BotWeapon::G3sg1 => 5000,
        BotWeapon::M249 => 5750,
        BotWeapon::ShieldGun => SHIELDGUN_PRICE,
        BotWeapon::HeGrenade => HEGRENADE_PRICE,
        BotWeapon::Flashbang => FLASHBANG_PRICE,
        BotWeapon::SmokeGrenade => SMOKEGRENADE_PRICE,
        BotWeapon::Knife | BotWeapon::C4 => 0,
    }
}

/// Vanilla CS 1.6 team + assassination (`as_*`) weapon buy legality (`CanBuyWeaponByMaptype`).
pub const fn can_team_buy_weapon(team: BotTeam, weapon: BotWeapon, is_vip_map: bool) -> bool {
    if is_vip_map {
        return match team {
            BotTeam::CounterTerrorist => matches!(
                weapon,
                BotWeapon::P228
                    | BotWeapon::Xm1014
                    | BotWeapon::Aug
                    | BotWeapon::FiveSeven
                    | BotWeapon::Ump45
                    | BotWeapon::Sg550
                    | BotWeapon::Famas
                    | BotWeapon::Usp
                    | BotWeapon::Glock18
                    | BotWeapon::Mp5n
                    | BotWeapon::M249
                    | BotWeapon::M3
                    | BotWeapon::M4a1
                    | BotWeapon::Tmp
                    | BotWeapon::Deagle
                    | BotWeapon::P90
                    | BotWeapon::ShieldGun
                    | BotWeapon::HeGrenade
                    | BotWeapon::Flashbang
                    | BotWeapon::SmokeGrenade
            ),
            BotTeam::Terrorist => matches!(
                weapon,
                BotWeapon::P228
                    | BotWeapon::Mac10
                    | BotWeapon::Elite
                    | BotWeapon::Ump45
                    | BotWeapon::Galil
                    | BotWeapon::Usp
                    | BotWeapon::Glock18
                    | BotWeapon::Awp
                    | BotWeapon::Deagle
                    | BotWeapon::Ak47
                    | BotWeapon::HeGrenade
                    | BotWeapon::Flashbang
                    | BotWeapon::SmokeGrenade
            ),
        };
    }

    match team {
        BotTeam::CounterTerrorist => matches!(
            weapon,
            BotWeapon::P228
                | BotWeapon::Scout
                | BotWeapon::Xm1014
                | BotWeapon::Aug
                | BotWeapon::FiveSeven
                | BotWeapon::Ump45
                | BotWeapon::Sg550
                | BotWeapon::Famas
                | BotWeapon::Usp
                | BotWeapon::Glock18
                | BotWeapon::Awp
                | BotWeapon::Mp5n
                | BotWeapon::M249
                | BotWeapon::M3
                | BotWeapon::M4a1
                | BotWeapon::Tmp
                | BotWeapon::Deagle
                | BotWeapon::P90
                | BotWeapon::ShieldGun
                | BotWeapon::HeGrenade
                | BotWeapon::Flashbang
                | BotWeapon::SmokeGrenade
        ),
        BotTeam::Terrorist => matches!(
            weapon,
            BotWeapon::P228
                | BotWeapon::Scout
                | BotWeapon::Xm1014
                | BotWeapon::Mac10
                | BotWeapon::Elite
                | BotWeapon::Ump45
                | BotWeapon::Galil
                | BotWeapon::Usp
                | BotWeapon::Glock18
                | BotWeapon::Awp
                | BotWeapon::Mp5n
                | BotWeapon::M249
                | BotWeapon::M3
                | BotWeapon::G3sg1
                | BotWeapon::Deagle
                | BotWeapon::Sg552
                | BotWeapon::Ak47
                | BotWeapon::P90
                | BotWeapon::HeGrenade
                | BotWeapon::Flashbang
                | BotWeapon::SmokeGrenade
        ),
    }
}

/// Economic stance chosen for the current round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuyTier {
    /// Save almost all money so the team can full-buy next round.
    Save,
    /// Light pistol/defuser eco buy without a primary weapon.
    Eco,
    /// Partial/force buy (SMG/shotgun/budget rifle or Deagle + Kevlar).
    ForceBuy,
    /// Full rifle/sniper + armor + utility buy.
    FullBuy,
}

/// Individual purchase instruction emitted by [`decide_bot_buy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuyItem {
    Weapon(BotWeapon),
    Vest,
    Helmet,
    VestHelm,
    Defuser,
    PrimaryAmmo,
    SecondaryAmmo,
}

impl BuyItem {
    /// Returns the CS 1.6 console buy alias (`crates/cs-game/src/buy.rs`).
    pub const fn to_buy_command(self) -> &'static str {
        match self {
            Self::Weapon(w) => w.buy_alias(),
            Self::Vest => "vest",
            Self::Helmet => "vesthelm",
            Self::VestHelm => "vesthelm",
            Self::Defuser => "defuser",
            Self::PrimaryAmmo => "primammo",
            Self::SecondaryAmmo => "secammo",
        }
    }

    /// Fixed purchase cost (`0` for ammo aliases whose cost depends on carried weapon).
    pub const fn fixed_cost(self) -> i32 {
        match self {
            Self::Weapon(w) => weapon_cost(w),
            Self::Vest => KEVLAR_PRICE,
            Self::Helmet => HELMET_PRICE,
            Self::VestHelm => ASSAULTSUIT_PRICE,
            Self::Defuser => DEFUSEKIT_PRICE,
            Self::PrimaryAmmo | Self::SecondaryAmmo => 0,
        }
    }
}

/// Current player & team economic state passed to [`decide_bot_buy`].
#[derive(Debug, Clone)]
pub struct BuyContext<'a> {
    pub team: BotTeam,
    pub money: i32,
    pub round_number: u32,
    pub consecutive_losses: u32,
    pub team_avg_money: i32,
    pub is_pistol_round: bool,
    pub is_bomb_map: bool,
    pub is_vip_map: bool,
    pub is_vip: bool,
    pub has_primary: Option<BotWeapon>,
    pub has_secondary: Option<BotWeapon>,
    pub has_shield: bool,
    pub armor: f32,
    pub has_helmet: bool,
    pub has_defuser: bool,
    pub he_count: u8,
    pub flash_count: u8,
    pub smoke_count: u8,
    pub needs_primary_ammo: bool,
    pub needs_secondary_ammo: bool,
    pub profile: &'a BotProfile,
    pub restrictions: &'a WeaponRestrictions,
    pub seed: u64,
}

/// Planned round purchases produced by [`decide_bot_buy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuyPlan {
    pub tier: BuyTier,
    pub items: Vec<BuyItem>,
    pub total_cost: i32,
    pub remaining_money: i32,
}

/// Computes an economy-aware CS 1.6 purchase sequence for a bot.
pub fn decide_bot_buy(ctx: &BuyContext<'_>) -> BuyPlan {
    let mut budget = ctx.money.max(0);
    let mut items = Vec::new();
    let mut total_cost = 0i32;

    if ctx.is_vip || ctx.restrictions.is_knives_only() {
        return BuyPlan {
            tier: BuyTier::Save,
            items,
            total_cost: 0,
            remaining_money: budget,
        };
    }

    let tier = classify_buy_tier(ctx);

    let spend = |item: BuyItem,
                 cost: i32,
                 budget: &mut i32,
                 total_cost: &mut i32,
                 items: &mut Vec<BuyItem>| {
        if *budget >= cost {
            *budget -= cost;
            *total_cost += cost;
            items.push(item);
            true
        } else {
            false
        }
    };

    match tier {
        BuyTier::Save => {
            // Pure save: buy only a defuser on bomb maps if surplus exceeds $2000.
            if ctx.is_bomb_map
                && ctx.team == BotTeam::CounterTerrorist
                && !ctx.has_defuser
                && budget >= 2000
            {
                spend(
                    BuyItem::Defuser,
                    DEFUSEKIT_PRICE,
                    &mut budget,
                    &mut total_cost,
                    &mut items,
                );
            }
        }
        BuyTier::Eco => {
            if ctx.is_pistol_round {
                plan_pistol_round(ctx, &mut budget, &mut total_cost, &mut items);
            } else {
                // Upgrade pistol only if we can keep >= $1600 for next round, or if forced on eco.
                if ctx.has_secondary.is_none_or(|p| matches!(p, BotWeapon::Glock18 | BotWeapon::Usp))
                    && ctx.restrictions.allow_pistols
                    && budget >= 2100
                    && let Some(pistol) = pick_preferred_pistol(ctx, budget - 1400)
                {
                    let c = weapon_cost(pistol);
                    spend(
                        BuyItem::Weapon(pistol),
                        c,
                        &mut budget,
                        &mut total_cost,
                        &mut items,
                    );
                }
                if ctx.is_bomb_map
                    && ctx.team == BotTeam::CounterTerrorist
                    && !ctx.has_defuser
                    && budget >= 1600
                {
                    spend(
                        BuyItem::Defuser,
                        DEFUSEKIT_PRICE,
                        &mut budget,
                        &mut total_cost,
                        &mut items,
                    );
                }
                if ctx.restrictions.allow_grenades && budget >= 2200 && ctx.flash_count == 0 {
                    spend(
                        BuyItem::Weapon(BotWeapon::Flashbang),
                        FLASHBANG_PRICE,
                        &mut budget,
                        &mut total_cost,
                        &mut items,
                    );
                }
            }
        }
        BuyTier::ForceBuy => {
            let reserve_for_armor = if ctx.armor < 50.0 { KEVLAR_PRICE } else { 0 };
            let mut bought_primary = ctx.has_primary.is_some() || ctx.has_shield;

            if !bought_primary
                && let Some(weapon) = pick_primary_weapon(
                    ctx,
                    (budget - reserve_for_armor).max(0),
                    false,
                )
            {
                let c = weapon_cost(weapon);
                if spend(
                    BuyItem::Weapon(weapon),
                    c,
                    &mut budget,
                    &mut total_cost,
                    &mut items,
                ) {
                    bought_primary = true;
                }
            }

            buy_armor(&mut budget, &mut total_cost, &mut items, ctx, false);

            if !bought_primary
                && ctx.restrictions.allow_pistols
                && let Some(pistol) = pick_preferred_pistol(ctx, budget)
            {
                let c = weapon_cost(pistol);
                spend(
                    BuyItem::Weapon(pistol),
                    c,
                    &mut budget,
                    &mut total_cost,
                    &mut items,
                );
            }

            if ctx.is_bomb_map && ctx.team == BotTeam::CounterTerrorist && !ctx.has_defuser {
                spend(
                    BuyItem::Defuser,
                    DEFUSEKIT_PRICE,
                    &mut budget,
                    &mut total_cost,
                    &mut items,
                );
            }

            if bought_primary && !ctx.has_shield {
                items.push(BuyItem::PrimaryAmmo);
            }
            if ctx.needs_secondary_ammo {
                items.push(BuyItem::SecondaryAmmo);
            }
            buy_utility(&mut budget, &mut total_cost, &mut items, ctx, 1);
        }
        BuyTier::FullBuy => {
            // Reserve at least Kevlar ($650) + defuser if CT bomb map ($200) so full buy stays protected.
            let armor_reserve = if ctx.armor < 95.0 {
                if ctx.has_helmet {
                    KEVLAR_PRICE
                } else {
                    ASSAULTSUIT_PRICE
                }
            } else if !ctx.has_helmet {
                HELMET_PRICE
            } else {
                0
            };
            let defuser_reserve =
                if ctx.is_bomb_map && ctx.team == BotTeam::CounterTerrorist && !ctx.has_defuser {
                    DEFUSEKIT_PRICE
                } else {
                    0
                };
            let weapon_budget = (budget - armor_reserve - defuser_reserve).max(budget / 2);

            let mut bought_primary = ctx.has_primary.is_some() || ctx.has_shield;
            if !bought_primary
                && let Some(weapon) = pick_primary_weapon(ctx, weapon_budget, true)
            {
                let c = weapon_cost(weapon);
                if spend(
                    BuyItem::Weapon(weapon),
                    c,
                    &mut budget,
                    &mut total_cost,
                    &mut items,
                ) {
                    bought_primary = true;
                }
            }

            buy_armor(&mut budget, &mut total_cost, &mut items, ctx, true);

            if ctx.is_bomb_map && ctx.team == BotTeam::CounterTerrorist && !ctx.has_defuser {
                spend(
                    BuyItem::Defuser,
                    DEFUSEKIT_PRICE,
                    &mut budget,
                    &mut total_cost,
                    &mut items,
                );
            }

            // Upgrade sidearm if plenty of money remains (especially for snipers).
            let owns_upgraded_pistol = ctx
                .has_secondary
                .is_some_and(|p| !matches!(p, BotWeapon::Glock18 | BotWeapon::Usp));
            let wants_pistol = !owns_upgraded_pistol
                && (ctx.has_primary.is_some_and(|w| w.is_sniper())
                    || !bought_primary
                    || budget >= 2400);
            if wants_pistol
                && ctx.restrictions.allow_pistols
                && let Some(pistol) = pick_preferred_pistol(ctx, budget)
            {
                let c = weapon_cost(pistol);
                spend(
                    BuyItem::Weapon(pistol),
                    c,
                    &mut budget,
                    &mut total_cost,
                    &mut items,
                );
            }

            if bought_primary && !ctx.has_shield {
                items.push(BuyItem::PrimaryAmmo);
            }
            if ctx.needs_secondary_ammo || items.iter().any(|i| matches!(i, BuyItem::Weapon(w) if w.is_pistol())) {
                items.push(BuyItem::SecondaryAmmo);
            }

            buy_utility(&mut budget, &mut total_cost, &mut items, ctx, 3);
        }
    }

    BuyPlan {
        tier,
        items,
        total_cost,
        remaining_money: budget,
    }
}

fn classify_buy_tier(ctx: &BuyContext<'_>) -> BuyTier {
    if ctx.is_pistol_round || ctx.round_number <= 1 && ctx.money <= 900 {
        return if ctx.money >= 650 {
            BuyTier::Eco
        } else {
            BuyTier::Save
        };
    }

    let full_buy_threshold = match ctx.team {
        BotTeam::Terrorist => 3700,
        BotTeam::CounterTerrorist => 4100,
    };

    if ctx.has_primary.is_some() && ctx.money >= 1000 {
        return BuyTier::FullBuy;
    }

    if ctx.money >= full_buy_threshold {
        return BuyTier::FullBuy;
    }

    // Check if team economy calls for saving rather than force-buying.
    let team_can_buy = ctx.team_avg_money >= 3400 || ctx.money >= 3200;
    let high_aggression_force = ctx.profile.aggression >= 0.72 && ctx.money >= 2400;
    let streak_force = ctx.consecutive_losses >= 2 && ctx.money >= 2600;

    if (team_can_buy && ctx.money >= 2100) || high_aggression_force || streak_force {
        BuyTier::ForceBuy
    } else if ctx.money >= 1500 {
        BuyTier::Eco
    } else {
        BuyTier::Save
    }
}

fn plan_pistol_round(
    ctx: &BuyContext<'_>,
    budget: &mut i32,
    total_cost: &mut i32,
    items: &mut Vec<BuyItem>,
) {
    let prefer_pistol_upgrade = ctx.profile.weapon_preferences.iter().any(|w| {
        w.is_pistol()
            && !matches!(w, BotWeapon::Glock18 | BotWeapon::Usp)
            && ctx.restrictions.allows_weapon(*w)
            && can_team_buy_weapon(ctx.team, *w, ctx.is_vip_map)
    });

    if prefer_pistol_upgrade
        && ctx.restrictions.allow_pistols
        && let Some(pistol) = pick_preferred_pistol(ctx, *budget)
    {
        let c = weapon_cost(pistol);
        if *budget >= c {
            *budget -= c;
            *total_cost += c;
            items.push(BuyItem::Weapon(pistol));
            items.push(BuyItem::SecondaryAmmo);
            return;
        }
    }

    // Standard pistol round: Kevlar ($650) or Defuser + Grenades.
    if ctx.armor < 50.0 && *budget >= KEVLAR_PRICE && (ctx.team == BotTeam::Terrorist || (mix64(ctx.seed) & 1) == 0) {
        *budget -= KEVLAR_PRICE;
        *total_cost += KEVLAR_PRICE;
        items.push(BuyItem::Vest);
        return;
    }

    if ctx.is_bomb_map && ctx.team == BotTeam::CounterTerrorist && !ctx.has_defuser && *budget >= DEFUSEKIT_PRICE {
        *budget -= DEFUSEKIT_PRICE;
        *total_cost += DEFUSEKIT_PRICE;
        items.push(BuyItem::Defuser);
    }
    buy_utility(budget, total_cost, items, ctx, 2);
}

fn buy_armor(
    budget: &mut i32,
    total_cost: &mut i32,
    items: &mut Vec<BuyItem>,
    ctx: &BuyContext<'_>,
    prefer_helmet: bool,
) {
    let full_armor = ctx.armor >= 95.0;
    if full_armor && ctx.has_helmet {
        return;
    }

    if prefer_helmet {
        if full_armor && !ctx.has_helmet && *budget >= HELMET_PRICE {
            *budget -= HELMET_PRICE;
            *total_cost += HELMET_PRICE;
            items.push(BuyItem::Helmet);
            return;
        }
        if !full_armor && !ctx.has_helmet && *budget >= ASSAULTSUIT_PRICE {
            *budget -= ASSAULTSUIT_PRICE;
            *total_cost += ASSAULTSUIT_PRICE;
            items.push(BuyItem::VestHelm);
            return;
        }
    }

    if !full_armor && *budget >= KEVLAR_PRICE {
        *budget -= KEVLAR_PRICE;
        *total_cost += KEVLAR_PRICE;
        items.push(BuyItem::Vest);
    }
}

fn buy_utility(
    budget: &mut i32,
    total_cost: &mut i32,
    items: &mut Vec<BuyItem>,
    ctx: &BuyContext<'_>,
    max_nades: u8,
) {
    if !ctx.restrictions.allow_grenades || max_nades == 0 {
        return;
    }
    let mut bought = 0u8;
    if ctx.he_count == 0 && *budget >= HEGRENADE_PRICE && bought < max_nades {
        *budget -= HEGRENADE_PRICE;
        *total_cost += HEGRENADE_PRICE;
        items.push(BuyItem::Weapon(BotWeapon::HeGrenade));
        bought += 1;
    }
    let mut flashes = ctx.flash_count;
    while flashes < 2 && *budget >= FLASHBANG_PRICE && bought < max_nades {
        *budget -= FLASHBANG_PRICE;
        *total_cost += FLASHBANG_PRICE;
        items.push(BuyItem::Weapon(BotWeapon::Flashbang));
        flashes += 1;
        bought += 1;
    }
    if ctx.smoke_count == 0 && *budget >= SMOKEGRENADE_PRICE && bought < max_nades {
        *budget -= SMOKEGRENADE_PRICE;
        *total_cost += SMOKEGRENADE_PRICE;
        items.push(BuyItem::Weapon(BotWeapon::SmokeGrenade));
    }
}

fn is_legal_candidate(ctx: &BuyContext<'_>, w: BotWeapon, budget: i32) -> bool {
    if ctx.has_primary == Some(w) || ctx.has_secondary == Some(w) {
        return false;
    }
    if w == BotWeapon::ShieldGun && (ctx.has_shield || ctx.has_secondary == Some(BotWeapon::Elite)) {
        return false;
    }
    if w == BotWeapon::Elite && ctx.has_shield {
        return false;
    }
    ctx.restrictions.allows_weapon(w)
        && can_team_buy_weapon(ctx.team, w, ctx.is_vip_map)
        && weapon_cost(w) <= budget
}

fn pick_primary_weapon(ctx: &BuyContext<'_>, budget: i32, full_buy: bool) -> Option<BotWeapon> {
    // 1. Honor WeaponPreference order from BotProfile.db.
    for &pref in &ctx.profile.weapon_preferences {
        if (pref.is_primary() || pref == BotWeapon::ShieldGun) && is_legal_candidate(ctx, pref, budget)
        {
            return Some(pref);
        }
    }

    // 2. Fallback meta ladder by team and budget.
    let candidates: &[BotWeapon] = match (ctx.team, full_buy) {
        (BotTeam::CounterTerrorist, true) => &[
            BotWeapon::M4a1,
            BotWeapon::Aug,
            BotWeapon::Famas,
            BotWeapon::Awp,
            BotWeapon::Scout,
            BotWeapon::P90,
            BotWeapon::Mp5n,
            BotWeapon::Xm1014,
            BotWeapon::M3,
            BotWeapon::Ump45,
            BotWeapon::Tmp,
            BotWeapon::M249,
            BotWeapon::ShieldGun,
        ],
        (BotTeam::Terrorist, true) => &[
            BotWeapon::Ak47,
            BotWeapon::Sg552,
            BotWeapon::Galil,
            BotWeapon::Awp,
            BotWeapon::Scout,
            BotWeapon::P90,
            BotWeapon::Mp5n,
            BotWeapon::Xm1014,
            BotWeapon::M3,
            BotWeapon::Ump45,
            BotWeapon::Mac10,
            BotWeapon::M249,
        ],
        (BotTeam::CounterTerrorist, false) => &[
            BotWeapon::Famas,
            BotWeapon::Mp5n,
            BotWeapon::Ump45,
            BotWeapon::M3,
            BotWeapon::Tmp,
            BotWeapon::Scout,
            BotWeapon::ShieldGun,
            BotWeapon::P90,
        ],
        (BotTeam::Terrorist, false) => &[
            BotWeapon::Galil,
            BotWeapon::Mp5n,
            BotWeapon::Ump45,
            BotWeapon::M3,
            BotWeapon::Mac10,
            BotWeapon::Scout,
            BotWeapon::P90,
        ],
    };

    let legal: Vec<BotWeapon> = candidates
        .iter()
        .copied()
        .filter(|&w| is_legal_candidate(ctx, w, budget))
        .collect();
    if legal.is_empty() {
        return None;
    }

    // Pick among the top 2 affordable legal choices using seed for realistic variety.
    let top_n = legal.len().min(2);
    let idx = (mix64(ctx.seed ^ 0xB07B_0101) as usize) % top_n;
    Some(legal[idx])
}

fn pick_preferred_pistol(ctx: &BuyContext<'_>, budget: i32) -> Option<BotWeapon> {
    for &pref in &ctx.profile.weapon_preferences {
        if pref.is_pistol() && is_legal_candidate(ctx, pref, budget) {
            return Some(pref);
        }
    }

    let ladder: &[BotWeapon] = match ctx.team {
        BotTeam::CounterTerrorist => &[
            BotWeapon::Deagle,
            BotWeapon::FiveSeven,
            BotWeapon::P228,
            BotWeapon::Usp,
        ],
        BotTeam::Terrorist => &[
            BotWeapon::Deagle,
            BotWeapon::Elite,
            BotWeapon::P228,
            BotWeapon::Usp,
        ],
    };

    ladder
        .iter()
        .copied()
        .find(|&w| is_legal_candidate(ctx, w, budget))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::BotDifficulty;

    fn base_ctx<'a>(profile: &'a BotProfile, restrictions: &'a WeaponRestrictions) -> BuyContext<'a> {
        BuyContext {
            team: BotTeam::CounterTerrorist,
            money: 800,
            round_number: 1,
            consecutive_losses: 0,
            team_avg_money: 800,
            is_pistol_round: true,
            is_bomb_map: true,
            is_vip_map: false,
            is_vip: false,
            has_primary: None,
            has_secondary: Some(BotWeapon::Usp),
            has_shield: false,
            armor: 0.0,
            has_helmet: false,
            has_defuser: false,
            he_count: 0,
            flash_count: 0,
            smoke_count: 0,
            needs_primary_ammo: false,
            needs_secondary_ammo: false,
            profile,
            restrictions,
            seed: 7,
        }
    }

    #[test]
    fn full_buy_honors_weapon_preference_armor_defuser_and_utility() {
        let mut profile = BotProfile::fallback("Cliffe", BotDifficulty::Expert);
        profile.weapon_preferences = vec![BotWeapon::M4a1, BotWeapon::Deagle];
        let restrictions = WeaponRestrictions::default();

        let mut ctx = base_ctx(&profile, &restrictions);
        ctx.round_number = 4;
        ctx.is_pistol_round = false;
        ctx.money = 6000;
        ctx.team_avg_money = 5500;

        let plan = decide_bot_buy(&ctx);
        assert_eq!(plan.tier, BuyTier::FullBuy);
        assert!(plan.items.contains(&BuyItem::Weapon(BotWeapon::M4a1)));
        assert!(plan.items.contains(&BuyItem::VestHelm));
        assert!(plan.items.contains(&BuyItem::Defuser));
        assert!(plan.items.contains(&BuyItem::PrimaryAmmo));
        assert!(plan.items.contains(&BuyItem::Weapon(BotWeapon::HeGrenade)));
        assert!(plan.total_cost <= ctx.money);
        assert_eq!(plan.total_cost + plan.remaining_money, ctx.money);
    }

    #[test]
    fn eco_and_save_thresholds_preserve_money() {
        let profile = BotProfile::fallback("Vinnie", BotDifficulty::Normal);
        let restrictions = WeaponRestrictions::default();
        let mut ctx = base_ctx(&profile, &restrictions);
        ctx.round_number = 2;
        ctx.is_pistol_round = false;
        ctx.money = 1800;
        ctx.team_avg_money = 1700;

        let eco = decide_bot_buy(&ctx);
        assert_eq!(eco.tier, BuyTier::Eco);
        assert!(eco.remaining_money >= 1400);

        ctx.money = 1200;
        let save = decide_bot_buy(&ctx);
        assert_eq!(save.tier, BuyTier::Save);
        assert!(save.items.is_empty());
    }

    #[test]
    fn force_buy_buys_smg_or_budget_rifle_with_kevlar() {
        let profile = BotProfile::fallback("Crusher", BotDifficulty::Hard);
        let restrictions = WeaponRestrictions::default();
        let mut ctx = base_ctx(&profile, &restrictions);
        ctx.team = BotTeam::Terrorist;
        ctx.round_number = 3;
        ctx.is_pistol_round = false;
        ctx.consecutive_losses = 2;
        ctx.money = 2900;
        ctx.team_avg_money = 2800;

        let plan = decide_bot_buy(&ctx);
        assert_eq!(plan.tier, BuyTier::ForceBuy);
        assert!(
            plan.items.iter().any(|i| matches!(i, BuyItem::Weapon(w) if w.is_primary())),
            "force buy should include a primary weapon: {plan:?}"
        );
        assert!(plan.total_cost <= 2900);
    }

    #[test]
    fn respects_all_weapon_restriction_flags_and_presets() {
        let mut profile = BotProfile::fallback("Phoenix", BotDifficulty::Expert);
        profile.weapon_preferences = vec![BotWeapon::Awp, BotWeapon::M4a1, BotWeapon::Deagle];

        let mut restrictions = WeaponRestrictions::default();
        restrictions.snipers_only();
        {
            let mut ctx = base_ctx(&profile, &restrictions);
            ctx.round_number = 5;
            ctx.is_pistol_round = false;
            ctx.money = 6500;
            ctx.team_avg_money = 6500;

            let sniper_plan = decide_bot_buy(&ctx);
            assert!(sniper_plan.items.contains(&BuyItem::Weapon(BotWeapon::Awp)));
            assert!(!sniper_plan.items.contains(&BuyItem::Weapon(BotWeapon::Deagle)));
            assert!(!sniper_plan.items.contains(&BuyItem::Weapon(BotWeapon::HeGrenade)));
        }

        restrictions.pistols_only();
        {
            let mut ctx = base_ctx(&profile, &restrictions);
            ctx.round_number = 5;
            ctx.is_pistol_round = false;
            ctx.money = 6500;
            ctx.team_avg_money = 6500;
            let pistol_plan = decide_bot_buy(&ctx);
            assert!(pistol_plan.items.contains(&BuyItem::Weapon(BotWeapon::Deagle)));
            assert!(!pistol_plan.items.iter().any(|i| matches!(i, BuyItem::Weapon(w) if w.is_primary())));
        }

        restrictions.knives_only();
        {
            let mut ctx = base_ctx(&profile, &restrictions);
            ctx.round_number = 5;
            ctx.is_pistol_round = false;
            ctx.money = 6500;
            ctx.team_avg_money = 6500;
            let knife_plan = decide_bot_buy(&ctx);
            assert!(knife_plan.items.is_empty());
        }

        restrictions.all_weapons();
        restrictions.allow_rifles = false;
        restrictions.allow_snipers = false;
        restrictions.allow_sub_machine_guns = false;
        restrictions.allow_shotguns = false;
        restrictions.allow_machine_guns = false;
        restrictions.allow_shield = false;
        let mut ctx = base_ctx(&profile, &restrictions);
        ctx.round_number = 5;
        ctx.is_pistol_round = false;
        ctx.money = 6500;
        ctx.team_avg_money = 6500;
        let only_pistol_and_nades = decide_bot_buy(&ctx);
        assert!(only_pistol_and_nades.items.contains(&BuyItem::Weapon(BotWeapon::Deagle)));
        assert!(only_pistol_and_nades.items.contains(&BuyItem::Weapon(BotWeapon::HeGrenade)));
    }

    #[test]
    fn vip_and_as_map_restrictions_are_enforced() {
        let mut profile = BotProfile::fallback("VIP", BotDifficulty::Normal);
        profile.weapon_preferences = vec![BotWeapon::Awp];
        let restrictions = WeaponRestrictions::default();
        let mut ctx = base_ctx(&profile, &restrictions);
        ctx.is_vip_map = true;
        ctx.is_pistol_round = false;
        ctx.money = 8000;

        // CTs cannot buy AWP on as_* maps; falls back to legal CT weapon.
        let ct_plan = decide_bot_buy(&ctx);
        assert!(!ct_plan.items.contains(&BuyItem::Weapon(BotWeapon::Awp)));
        assert!(ct_plan.items.iter().any(|i| matches!(i, BuyItem::Weapon(w) if w.is_primary())));

        // The VIP themselves cannot buy anything.
        ctx.is_vip = true;
        let vip_plan = decide_bot_buy(&ctx);
        assert!(vip_plan.items.is_empty());
    }
}
