//! [`cs_amxx::AmxxCsHooks`] implementation for [`CsGame`].

use cs_amxx::natives::{AmxxCsHooks, CS_ARMOR_KEVLAR, CS_ARMOR_NONE, CS_ARMOR_VESTHELM};
use cs_engine::{EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;

use crate::CsGame;
use crate::player::{ArmorType, Team};

impl AmxxCsHooks for CsGame {
    fn get_user_money(&self, slot: u8) -> i32 {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.money)
    }

    fn set_user_money(&mut self, slot: u8, amount: i32) {
        if let Some(p) = self
            .state
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.money = amount.clamp(0, crate::money::MAX_ACCOUNT);
        }
    }

    fn get_user_team(&self, slot: u8) -> i32 {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| i32::from(p.team.id()))
    }

    fn set_user_team(&mut self, slot: u8, team: i32) {
        if let Some(p) = self
            .state
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.team = match team {
                1 => Team::Terrorist,
                2 => Team::Ct,
                3 => Team::Spectator,
                _ => Team::Unassigned,
            };
        }
    }

    fn get_user_deaths(&self, slot: u8) -> i32 {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map_or(0, |p| p.deaths)
    }

    fn set_user_deaths(&mut self, slot: u8, deaths: i32) {
        if let Some(p) = self
            .state
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.deaths = deaths;
        }
    }

    fn give_item(&mut self, slot: u8, eng: &mut EngineApi<'_>, item: &str) -> bool {
        match item {
            "item_kevlar" => {
                self.set_user_armor(slot, eng, 100, CS_ARMOR_KEVLAR);
                true
            }
            "item_assaultsuit" => {
                self.set_user_armor(slot, eng, 100, CS_ARMOR_VESTHELM);
                true
            }
            "item_thighpack" => {
                crate::player::give_defuser(&mut self.state, eng, slot);
                true
            }
            "weapon_c4" => crate::player::make_bomber(&mut self.state, eng, slot),
            _ => crate::inventory::give_named_item(&mut self.state, eng, slot, item),
        }
    }

    fn strip_user_weapons(&mut self, slot: u8, eng: &mut EngineApi<'_>) {
        crate::inventory::remove_all_items(&mut self.state, eng, slot, false);
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
        let clamped = armor.clamp(0, 255);
        let kevlar = match armor_type {
            CS_ARMOR_KEVLAR => ArmorType::Vest,
            CS_ARMOR_VESTHELM => ArmorType::VestHelm,
            CS_ARMOR_NONE => ArmorType::None,
            _ => ArmorType::None,
        };
        if let Some(p) = self
            .state
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.armor = clamped as f32;
            p.kevlar = kevlar;
        } else {
            return;
        }
        eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::Battery(clamped as i16));
        eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::ArmorType(u8::from(kevlar == ArmorType::VestHelm)),
        );
    }
}
