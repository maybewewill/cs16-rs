//! [`cs_amxx::AmxxCsHooks`] implementation for [`CsGame`] and [`GameStateAmxxHooks`].
//!
//! Clean-room GPL-3.0-or-later implementation referencing AMX Mod X (`cstrike`, `fun`,
//! `hamsandwich`, `reapi`, read 2026-11-26) and ReGameDLL_CS (`player.cpp`, `weapons.cpp`).

use cs_amxx::natives::{
    AmxxCsHooks, CS_ARMOR_KEVLAR, CS_ARMOR_NONE, CS_ARMOR_VESTHELM, HamFunction, reapi_member,
};
use cs_engine::EngineApi;
use cs_mod_api::{ArmorKind, TeamKind};

use crate::mod_hooks;
use crate::player::ArmorType;
use crate::{CsGame, GameState};

/// Lightweight [`AmxxCsHooks`] adapter over `&mut GameState` so unified `ModdedGame<CsGame>`
/// can invoke `.amxx` hooks synchronously inside [`CsHookSink`](crate::mod_hooks::CsHookSink).
pub struct GameStateAmxxHooks<'a>(pub &'a mut GameState);

impl AmxxCsHooks for GameStateAmxxHooks<'_> {
    fn get_user_money(&self, slot: u8) -> i32 {
        self.0
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.money)
    }

    fn set_user_money(&mut self, slot: u8, amount: i32) {
        if let Some(p) = self
            .0
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.money = amount.clamp(0, crate::money::MAX_ACCOUNT);
        }
    }

    fn get_user_team(&self, slot: u8) -> i32 {
        self.0
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| i32::from(p.team.id()))
    }

    fn set_user_team(&mut self, slot: u8, team: i32) {
        if let Some(p) = self
            .0
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.team = match team {
                1 => crate::player::Team::Terrorist,
                2 => crate::player::Team::Ct,
                3 => crate::player::Team::Spectator,
                _ => crate::player::Team::Unassigned,
            };
        }
    }

    fn get_user_deaths(&self, slot: u8) -> i32 {
        self.0
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.deaths)
    }

    fn set_user_deaths(&mut self, slot: u8, deaths: i32) {
        if let Some(p) = self
            .0
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.deaths = deaths;
        }
    }

    fn give_item(&mut self, slot: u8, eng: &mut EngineApi<'_>, item: &str) -> bool {
        mod_hooks::give_item(self.0, eng, slot, item)
    }

    fn strip_user_weapons(&mut self, slot: u8, eng: &mut EngineApi<'_>) {
        mod_hooks::strip_user_weapons(self.0, eng, slot);
    }

    fn get_user_armor(&self, slot: u8, _eng: &EngineApi<'_>) -> i32 {
        self.0
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.armor.round() as i32)
    }

    fn set_user_armor(
        &mut self,
        slot: u8,
        eng: &mut EngineApi<'_>,
        armor: i32,
        armor_type: i32,
    ) {
        let kind = match armor_type {
            CS_ARMOR_KEVLAR => ArmorKind::Kevlar,
            CS_ARMOR_VESTHELM => ArmorKind::VestHelm,
            CS_ARMOR_NONE => ArmorKind::None,
            _ => ArmorKind::None,
        };
        let _ = mod_hooks::set_player_armor(self.0, eng, slot, armor, kind);
    }

    fn round_respawn(&mut self, slot: u8, eng: &mut EngineApi<'_>) -> bool {
        mod_hooks::respawn_player(self.0, eng, slot)
    }

    fn add_account(&mut self, slot: u8, eng: &mut EngineApi<'_>, delta: i32, flash: bool) {
        let _ = mod_hooks::add_player_money(self.0, eng, slot, delta, flash);
    }

    fn remove_all_items(&mut self, slot: u8, eng: &mut EngineApi<'_>, remove_suit: bool) {
        self.0.custom_weapons.clear_player(slot);
        crate::inventory::remove_all_items(self.0, eng, slot, remove_suit);
    }

    fn get_active_weapon(&self, slot: u8) -> (i32, i32, i32) {
        let Some(p) = self.0.players.get(usize::from(slot)).and_then(Option::as_ref) else {
            return (0, 0, 0);
        };
        let wid = p.active_item().map_or(0, |w| w as i32);
        let clip = mod_hooks::get_weapon_clip(self.0, slot, 0).max(0);
        let bpammo = mod_hooks::get_user_bpammo(self.0, slot, 0).max(0);
        (wid, clip, bpammo)
    }

    fn get_weapon_clip(&self, slot: u8, weapon_id: u8) -> i32 {
        mod_hooks::get_weapon_clip(self.0, slot, weapon_id)
    }

    fn set_weapon_clip(&mut self, slot: u8, weapon_id: u8, clip: i32) {
        let _ = mod_hooks::set_weapon_clip(self.0, slot, weapon_id, clip);
    }

    fn get_user_bpammo(&self, slot: u8, weapon_id: u8) -> i32 {
        mod_hooks::get_user_bpammo(self.0, slot, weapon_id)
    }

    fn set_user_bpammo(
        &mut self,
        slot: u8,
        eng: &mut EngineApi<'_>,
        weapon_id: u8,
        amount: i32,
    ) {
        let _ = mod_hooks::set_user_bpammo(self.0, eng, slot, weapon_id, amount);
    }

    fn get_member(&self, ent_index: u16, eng: &EngineApi<'_>, member: i32) -> i32 {
        if ent_index == 0 || ent_index > u16::from(eng.max_clients()) {
            return 0;
        }
        let slot = (ent_index - 1) as u8;
        let Some(p) = self.0.players.get(usize::from(slot)).and_then(Option::as_ref) else {
            return 0;
        };
        match member {
            reapi_member::M_I_ACCOUNT => p.money,
            reapi_member::M_I_KEVLAR => match p.kevlar {
                ArmorType::None => CS_ARMOR_NONE,
                ArmorType::Vest => CS_ARMOR_KEVLAR,
                ArmorType::VestHelm => CS_ARMOR_VESTHELM,
            },
            reapi_member::M_B_HAS_DEFUSER => i32::from(p.has_defuser),
            reapi_member::M_B_HAS_NIGHTVISION => i32::from(p.has_night_vision()),
            reapi_member::M_I_TEAM => i32::from(p.team.id()),
            reapi_member::M_I_DEATHS => p.deaths,
            reapi_member::M_I_CLIP => mod_hooks::get_weapon_clip(self.0, slot, 0).max(0),
            reapi_member::M_I_FRAGS => p.frags,
            reapi_member::M_FL_MAXSPEED => eng
                .ent(eng.client_edict(slot))
                .map_or(250.0f32, |v| v.max_speed)
                .to_bits() as i32,
            reapi_member::M_B_HAS_C4 => i32::from(p.has_c4),
            _ => 0,
        }
    }

    fn set_member(&mut self, ent_index: u16, eng: &mut EngineApi<'_>, member: i32, val: i32) -> bool {
        if ent_index == 0 || ent_index > u16::from(eng.max_clients()) {
            return false;
        }
        let slot = (ent_index - 1) as u8;
        match member {
            reapi_member::M_I_ACCOUNT => mod_hooks::set_player_money(self.0, eng, slot, val, true),
            reapi_member::M_I_KEVLAR => {
                let armor = self.get_user_armor(slot, eng);
                self.set_user_armor(slot, eng, armor, val);
                true
            }
            reapi_member::M_B_HAS_DEFUSER => mod_hooks::set_user_defuser(self.0, eng, slot, val != 0),
            reapi_member::M_B_HAS_NIGHTVISION => mod_hooks::set_user_nvg(self.0, eng, slot, val != 0),
            reapi_member::M_I_TEAM => {
                mod_hooks::set_player_team(self.0, eng, slot, TeamKind::from_i32(val), None)
            }
            reapi_member::M_I_DEATHS => {
                let frags = self
                    .0
                    .players
                    .get(usize::from(slot))
                    .and_then(Option::as_ref)
                    .map_or(0, |p| p.frags);
                mod_hooks::set_player_score(self.0, eng, slot, frags, val)
            }
            reapi_member::M_I_CLIP => mod_hooks::set_weapon_clip(self.0, slot, 0, val),
            reapi_member::M_I_FRAGS => {
                let deaths = self
                    .0
                    .players
                    .get(usize::from(slot))
                    .and_then(Option::as_ref)
                    .map_or(0, |p| p.deaths);
                mod_hooks::set_player_score(self.0, eng, slot, val, deaths)
            }
            reapi_member::M_FL_MAXSPEED => {
                let speed = f32::from_bits(val as u32);
                mod_hooks::set_player_maxspeed(self.0, eng, slot, speed)
            }
            _ => false,
        }
    }

    fn execute_ham(
        &mut self,
        func: HamFunction,
        ent_index: u16,
        eng: &mut EngineApi<'_>,
        args: &[i32],
    ) -> i32 {
        if ent_index == 0 || ent_index > u16::from(eng.max_clients()) {
            return 0;
        }
        let slot = (ent_index - 1) as u8;
        match func {
            HamFunction::Spawn => i32::from(mod_hooks::respawn_player(self.0, eng, slot)),
            HamFunction::TakeDamage => {
                let attacker_idx = args.get(1).copied().unwrap_or(0);
                let attacker = if attacker_idx >= 1 && attacker_idx <= i32::from(eng.max_clients()) {
                    Some((attacker_idx - 1) as u8)
                } else {
                    None
                };
                let dmg = args
                    .get(2)
                    .copied()
                    .map(|b| f32::from_bits(b as u32))
                    .unwrap_or(0.0);
                let bits = args.get(3).copied().unwrap_or(0) as u32;
                i32::from(mod_hooks::apply_damage(self.0, eng, slot, attacker, dmg, bits))
            }
            HamFunction::Killed => i32::from(mod_hooks::slay_player(self.0, eng, slot, false)),
            _ => 0,
        }
    }
}

impl AmxxCsHooks for CsGame {
    fn get_user_money(&self, slot: u8) -> i32 {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.money)
    }

    fn set_user_money(&mut self, slot: u8, amount: i32) {
        GameStateAmxxHooks(&mut self.state).set_user_money(slot, amount);
    }

    fn get_user_team(&self, slot: u8) -> i32 {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| i32::from(p.team.id()))
    }

    fn set_user_team(&mut self, slot: u8, team: i32) {
        GameStateAmxxHooks(&mut self.state).set_user_team(slot, team);
    }

    fn get_user_deaths(&self, slot: u8) -> i32 {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.deaths)
    }

    fn set_user_deaths(&mut self, slot: u8, deaths: i32) {
        GameStateAmxxHooks(&mut self.state).set_user_deaths(slot, deaths);
    }

    fn give_item(&mut self, slot: u8, eng: &mut EngineApi<'_>, item: &str) -> bool {
        GameStateAmxxHooks(&mut self.state).give_item(slot, eng, item)
    }

    fn strip_user_weapons(&mut self, slot: u8, eng: &mut EngineApi<'_>) {
        GameStateAmxxHooks(&mut self.state).strip_user_weapons(slot, eng);
    }

    fn get_user_armor(&self, slot: u8, _eng: &EngineApi<'_>) -> i32 {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.armor.round() as i32)
    }

    fn set_user_armor(
        &mut self,
        slot: u8,
        eng: &mut EngineApi<'_>,
        armor: i32,
        armor_type: i32,
    ) {
        GameStateAmxxHooks(&mut self.state).set_user_armor(slot, eng, armor, armor_type);
    }

    fn round_respawn(&mut self, slot: u8, eng: &mut EngineApi<'_>) -> bool {
        GameStateAmxxHooks(&mut self.state).round_respawn(slot, eng)
    }

    fn add_account(&mut self, slot: u8, eng: &mut EngineApi<'_>, delta: i32, flash: bool) {
        GameStateAmxxHooks(&mut self.state).add_account(slot, eng, delta, flash);
    }

    fn remove_all_items(&mut self, slot: u8, eng: &mut EngineApi<'_>, remove_suit: bool) {
        GameStateAmxxHooks(&mut self.state).remove_all_items(slot, eng, remove_suit);
    }

    fn get_active_weapon(&self, slot: u8) -> (i32, i32, i32) {
        let Some(p) = self.state.players.get(usize::from(slot)).and_then(Option::as_ref) else {
            return (0, 0, 0);
        };
        let wid = p.active_item().map_or(0, |w| w as i32);
        let clip = mod_hooks::get_weapon_clip(&self.state, slot, 0).max(0);
        let bpammo = mod_hooks::get_user_bpammo(&self.state, slot, 0).max(0);
        (wid, clip, bpammo)
    }

    fn get_weapon_clip(&self, slot: u8, weapon_id: u8) -> i32 {
        mod_hooks::get_weapon_clip(&self.state, slot, weapon_id)
    }

    fn set_weapon_clip(&mut self, slot: u8, weapon_id: u8, clip: i32) {
        GameStateAmxxHooks(&mut self.state).set_weapon_clip(slot, weapon_id, clip);
    }

    fn get_user_bpammo(&self, slot: u8, weapon_id: u8) -> i32 {
        mod_hooks::get_user_bpammo(&self.state, slot, weapon_id)
    }

    fn set_user_bpammo(
        &mut self,
        slot: u8,
        eng: &mut EngineApi<'_>,
        weapon_id: u8,
        amount: i32,
    ) {
        GameStateAmxxHooks(&mut self.state).set_user_bpammo(slot, eng, weapon_id, amount);
    }

    fn get_member(&self, ent_index: u16, eng: &EngineApi<'_>, member: i32) -> i32 {
        if ent_index == 0 || ent_index > u16::from(eng.max_clients()) {
            return 0;
        }
        let slot = (ent_index - 1) as u8;
        let Some(p) = self.state.players.get(usize::from(slot)).and_then(Option::as_ref) else {
            return 0;
        };
        match member {
            reapi_member::M_I_ACCOUNT => p.money,
            reapi_member::M_I_KEVLAR => match p.kevlar {
                ArmorType::None => CS_ARMOR_NONE,
                ArmorType::Vest => CS_ARMOR_KEVLAR,
                ArmorType::VestHelm => CS_ARMOR_VESTHELM,
            },
            reapi_member::M_B_HAS_DEFUSER => i32::from(p.has_defuser),
            reapi_member::M_B_HAS_NIGHTVISION => i32::from(p.has_night_vision()),
            reapi_member::M_I_TEAM => i32::from(p.team.id()),
            reapi_member::M_I_DEATHS => p.deaths,
            reapi_member::M_I_CLIP => mod_hooks::get_weapon_clip(&self.state, slot, 0).max(0),
            reapi_member::M_I_FRAGS => p.frags,
            reapi_member::M_FL_MAXSPEED => eng
                .ent(eng.client_edict(slot))
                .map_or(250.0f32, |v| v.max_speed)
                .to_bits() as i32,
            reapi_member::M_B_HAS_C4 => i32::from(p.has_c4),
            _ => 0,
        }
    }

    fn set_member(&mut self, ent_index: u16, eng: &mut EngineApi<'_>, member: i32, val: i32) -> bool {
        GameStateAmxxHooks(&mut self.state).set_member(ent_index, eng, member, val)
    }

    fn execute_ham(
        &mut self,
        func: HamFunction,
        ent_index: u16,
        eng: &mut EngineApi<'_>,
        args: &[i32],
    ) -> i32 {
        GameStateAmxxHooks(&mut self.state).execute_ham(func, ent_index, eng, args)
    }
}
