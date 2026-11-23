//! The objective zones of a map and its `info_map_parameters`, ported from ReGameDLL_CS
//! (`triggers.cpp` `CBombTarget`, `CHostageRescue`, `CEscapeZone`, `CVIP_SafetyZone`;
//! `mapinfo.cpp` `CMapInfo`; `subs.cpp` for the point classes; read 2026-11-23), the vanilla
//! `mp.dll` behaviour (not `REGAMEDLL_FIXES`/`REGAMEDLL_ADD`: no `IsPlayerInBombSite` check, no
//! `legacy_bombtarget_touch`).
//!
//! A zone is a trigger brush. While a player overlaps it the engine touches the zone every
//! frame, and the touch only raises a signal in the player's [`Signals`](crate::buy::Signals);
//! `HandleSignals` ([`crate::buy`]) turns the raised signals into the state and the status icons
//! every half second, so the icon and the `PLAYER_IN_BOMB_ZONE` bit of `iuser3` lag the touch.
//!
//! - `func_bomb_target` raises `SIGNAL_BOMB` for a player that carries the C4 (`m_bHasC4`) and
//!   remembers the site (`m_pentCurBombTarget`). Vanilla has no check that the player stands
//!   inside the zone's X/Y extent: any overlap of the boxes counts. `Use` fires the zone's own
//!   `target` (`SUB_UseTargets(nullptr, USE_TOGGLE, 0)`), the hook the exploding bomb pulls for
//!   maps that destroy something with it.
//! - `func_hostage_rescue` raises `SIGNAL_RESCUE`. (The touch of a `hostage_entity`, which sets
//!   its `m_bRescueMe`, comes with the hostages, task 6.)
//! - `func_escapezone` marks a terrorist as escaped (once) and tells the team; for a
//!   counter-terrorist it raises `SIGNAL_ESCAPE`.
//! - `func_vip_safetyzone` raises `SIGNAL_VIPSAFETY`; the VIP that touches it has escaped: it
//!   disappears ([`crate::player::disappear`]) and is paid 2500.
//! - `info_bomb_target`, `info_hostage_rescue` and `info_vip_start` are plain points that the
//!   legacy checks of `HandleSignals` and the VIP spawn look for.
//! - `info_map_parameters` (`CMapInfo`) carries `buying` and `bombradius`; `CheckMapInfo` copies
//!   them into the rules at the activation of the map and at every round restart.
//!
//! Deviations (gaps):
//! - `InitTrigger`'s `SetMovedir` for a trigger with `angles` (a one-way touch) is not computed:
//!   `angles` are cleared, as for [`BuyZone`](crate::buy::BuyZone).
//! - `UTIL_LogPrintf` (the HLDS log lines `Terrorist_Escaped`, `Escaped_As_VIP`) goes to the
//!   tracing log instead of a server log file.
//! - `CheckMapInfo`'s `ALERT(at_console, ...)` lines go to the tracing log (debug level).

use cs_engine::consts::{MOVETYPE_NONE, SOLID_NOT, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use crate::GameState;
use crate::buy::{SIGNAL_BOMB, SIGNAL_ESCAPE, SIGNAL_RESCUE, SIGNAL_VIPSAFETY};
use crate::chat::client_print;
use crate::entity::GameEntity;
use crate::menus::HUD_PRINTCENTER;
use crate::money;
use crate::player::{EF_NODRAW, Team, player_slot};
use crate::rules;
use crate::entity::UseType;
use crate::targets;

/// `MAX_BOMB_RADIUS` (`mapinfo.h`): the largest `bombradius` a map may ask for.
const MAX_BOMB_RADIUS: f32 = 2048.0;

/// `InfoMapBuyParam` (`gamerules.h`): who may buy, from the `buying` key.
const BUYING_EVERYONE: i32 = 0;
const BUYING_ONLY_CTS: i32 = 1;
const BUYING_ONLY_TERRORISTS: i32 = 2;
const BUYING_NO_ONE: i32 = 3;

/// `CBaseTrigger::InitTrigger` (`triggers.cpp` 458-480, read 2026-11-23): a non-solid trigger
/// of the brush model (or of the origin when it has none), invisible unless `showtriggers` is
/// set. `false` when the edict is gone. See the module documentation for `angles`.
pub(crate) fn init_trigger(eng: &mut EngineApi<'_>, id: EdictId) -> bool {
    let showtriggers = eng.cvar_f32("showtriggers") != 0.0;
    let Some(v) = eng.ent_mut(id) else {
        return false;
    };
    v.angles = Vec3::ZERO;
    v.solid = SOLID_TRIGGER;
    v.movetype = MOVETYPE_NONE;
    let (model, origin) = (v.model.clone(), v.origin);
    if !showtriggers {
        v.effects |= EF_NODRAW;
    }
    if model.is_empty() {
        eng.set_origin(id, origin);
    } else if !eng.set_model(id, &model) {
        tracing::warn!(model = %model, "trigger model could not be set");
    }
    true
}

/// `func_bomb_target` (`CBombTarget`): a bomb site.
pub(crate) struct BombTarget;

impl GameEntity for BombTarget {
    fn classname(&self) -> &'static str {
        "func_bomb_target"
    }

    /// `CBombTarget::Spawn`: `InitTrigger`, the touch and the use callbacks.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        init_trigger(eng, id)
    }

    /// `CBombTarget::BombTargetTouch`: the bomb carrier signals the site and remembers it.
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        let Some(slot) = player_slot(g, eng, other) else {
            return;
        };
        if let Some(p) = g.players[slot].as_mut()
            && p.has_c4
        {
            p.signals.signal(SIGNAL_BOMB);
            p.cur_bomb_target = Some(id);
        }
    }

    /// `CBombTarget::BombTargetUse`: `SUB_UseTargets(nullptr, USE_TOGGLE, 0)`.
    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        targets::use_targets(g, eng, id, None, UseType::Toggle, 0.0);
    }
}

/// `func_hostage_rescue` (`CHostageRescue`): a zone the hostages are led to.
pub(crate) struct HostageRescue;

impl GameEntity for HostageRescue {
    fn classname(&self) -> &'static str {
        "func_hostage_rescue"
    }

    /// `CHostageRescue::Spawn`: `InitTrigger` and the touch.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        init_trigger(eng, id)
    }

    /// `CHostageRescue::HostageRescueTouch`: a player signals the zone; a `hostage_entity`
    /// gets `m_bRescueMe = TRUE`.
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, _id: EdictId, other: EdictId) {
        if let Some(slot) = player_slot(g, eng, other)
            && let Some(p) = g.players[slot].as_mut()
        {
            p.signals.signal(SIGNAL_RESCUE);
            return;
        }
        if eng
            .ent(other)
            .is_some_and(|v| v.classname == "hostage_entity")
        {
            crate::entity::with_entity(g, eng, other, |ent, _, _| {
                ent.mark_hostage_rescue();
            });
        }
    }
}

/// `func_escapezone` (`CEscapeZone`): where the terrorists of an `es_` map get away.
pub(crate) struct EscapeZone;

impl GameEntity for EscapeZone {
    fn classname(&self) -> &'static str {
        "func_escapezone"
    }

    /// `CEscapeZone::Spawn`: `InitTrigger` and the touch.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        init_trigger(eng, id)
    }

    /// `CEscapeZone::EscapeTouch`: a terrorist that has not escaped yet does: the rules look at
    /// the teams again and the terrorists are told (`#Terrorist_Escaped`). A counter-terrorist
    /// signals the zone.
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, _id: EdictId, other: EdictId) {
        let Some(slot) = player_slot(g, eng, other) else {
            return;
        };
        let Some(team) = g.players[slot].as_ref().map(|p| p.team) else {
            return;
        };
        match team {
            Team::Terrorist => {
                let Some(p) = g.players[slot].as_mut() else {
                    return;
                };
                if p.escaped {
                    return;
                }
                p.escaped = true;
                rules::check_win_conditions(g, eng);
                // `UTIL_LogPrintf("... triggered \"Terrorist_Escaped\"")`: see the module
                // documentation.
                tracing::info!(slot, "Terrorist_Escaped");
                let terrorists: Vec<u8> = g
                    .players
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.as_ref().is_some_and(|p| p.team == Team::Terrorist))
                    .map(|(i, _)| i as u8)
                    .collect();
                for to in terrorists {
                    client_print(eng, to, HUD_PRINTCENTER, "#Terrorist_Escaped", &[]);
                }
            }
            Team::Ct => {
                if let Some(p) = g.players[slot].as_mut() {
                    p.signals.signal(SIGNAL_ESCAPE);
                }
            }
            Team::Unassigned | Team::Spectator => {}
        }
    }
}

/// `func_vip_safetyzone` (`CVIP_SafetyZone`): where the VIP of an `as_` map is safe.
pub(crate) struct VipSafetyZone;

impl GameEntity for VipSafetyZone {
    fn classname(&self) -> &'static str {
        "func_vip_safetyzone"
    }

    /// `CVIP_SafetyZone::Spawn`: `InitTrigger` and the touch.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        init_trigger(eng, id)
    }

    /// `CVIP_SafetyZone::VIP_SafetyTouch`: any player signals the zone (the terrorists get the
    /// icon as well); the VIP has escaped: it disappears and is paid.
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, _id: EdictId, other: EdictId) {
        let Some(slot) = player_slot(g, eng, other) else {
            return;
        };
        let Some(p) = g.players[slot].as_mut() else {
            return;
        };
        p.signals.signal(SIGNAL_VIPSAFETY);
        if p.is_vip {
            // `UTIL_LogPrintf("... triggered \"Escaped_As_VIP\"")`: see the module documentation.
            tracing::info!(slot, "Escaped_As_VIP");
            p.escaped = true;
            crate::player::disappear(g, eng, slot as u8);
            money::add_account(g, eng, slot as u8, money::REWARD_VIP_HAVE_SELF_RESCUED, true);
        }
    }
}

/// `info_bomb_target`, `info_hostage_rescue`, `info_vip_start` (`CPointEntity`,
/// `CBaseDMStart`): a point the map names and the game looks for by class; it does nothing.
pub(crate) struct PointMarker(pub(crate) &'static str);

impl GameEntity for PointMarker {
    fn classname(&self) -> &'static str {
        self.0
    }
}

/// `info_map_parameters` (`CMapInfo`): the keys are read by [`register_map_info`]; the entity
/// itself is invisible and inert (`CMapInfo::Spawn`).
pub(crate) struct InfoMapParameters;

impl GameEntity for InfoMapParameters {
    fn classname(&self) -> &'static str {
        "info_map_parameters"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.movetype = MOVETYPE_NONE;
        v.solid = SOLID_NOT;
        v.effects |= EF_NODRAW;
        true
    }
}

/// What `info_map_parameters` (`g_pMapInfo`) holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapInfo {
    /// `m_iBuyingStatus` (`InfoMapBuyParam`): who may buy.
    pub buying: i32,
    /// `m_flBombRadius`: 500 unless the map gives `bombradius` (at most 2048).
    pub bomb_radius: f32,
}

impl MapInfo {
    /// `CMapInfo::OnCreate` and `CMapInfo::KeyValue`: the defaults, then the two keys the class
    /// reads (`buying` and `bombradius`, both through `Q_atoi`; the radius is clamped above).
    fn from_keys(keys: &[(String, String)]) -> Self {
        let mut info = Self {
            buying: BUYING_EVERYONE,
            bomb_radius: 500.0,
        };
        for (key, value) in keys {
            match key.as_str() {
                "buying" => info.buying = crate::menus::atoi(value),
                "bombradius" => {
                    info.bomb_radius = (crate::menus::atoi(value) as f32).min(MAX_BOMB_RADIUS);
                }
                _ => {}
            }
        }
        info
    }
}

/// An `info_map_parameters` has been created with `keys` (the constructor of `CMapInfo`): it is
/// the map's parameters from now on (`g_pMapInfo = this`; with several, the last created wins).
pub(crate) fn register_map_info(g: &mut GameState, keys: &[(String, String)]) {
    g.map_info = Some(MapInfo::from_keys(keys));
}

/// `CMapInfo::CheckMapInfo` (`mapinfo.cpp` 30-68, read 2026-11-23), run by `ServerActivate` and
/// `RestartRound` when the map has an `info_map_parameters`: the buying status says which teams
/// may not buy (`ONLY_CTS` keeps the terrorists out, `ONLY_TERRORISTS` the counter-terrorists,
/// `NO_ONE` both; any other value lets everyone), and the bomb radius goes to the rules.
pub(crate) fn check_map_info(g: &mut GameState) {
    let Some(info) = g.map_info else {
        return;
    };
    let (ct_cant_buy, t_cant_buy) = match info.buying {
        BUYING_EVERYONE => {
            tracing::debug!("EVERYONE CAN BUY!");
            (false, false)
        }
        BUYING_ONLY_CTS => {
            tracing::debug!("Only CT's can buy!!");
            (false, true)
        }
        BUYING_ONLY_TERRORISTS => {
            tracing::debug!("Only T's can buy!!");
            (true, false)
        }
        BUYING_NO_ONE => {
            tracing::debug!("No one can buy!!");
            (true, true)
        }
        _ => (false, false),
    };
    g.rules.set_map_parameters(info.bomb_radius, ct_cant_buy, t_cant_buy);
}

/// The class `test_use_counter`: counts the `Use` calls it receives in its `health`. For tests
/// only (feature `test-hooks`).
#[cfg(feature = "test-hooks")]
pub(crate) struct UseCounter;

#[cfg(feature = "test-hooks")]
impl GameEntity for UseCounter {
    fn classname(&self) -> &'static str {
        "test_use_counter"
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        if let Some(v) = eng.ent_mut(id) {
            v.health += 1.0;
        }
    }
}
