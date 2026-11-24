//! Map ammo boxes (`ammo_*`) and world equipment items (`item_kevlar`,
//! `item_assaultsuit`, `item_generic`) ported from ReGameDLL_CS (`weapons.cpp`, `items.cpp`).

use cs_engine::consts::{MOVETYPE_NONE, MOVETYPE_TOSS, SOLID_NOT, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use crate::GameState;
use crate::buy::{ArmorItem, give_armor_item};
use crate::entity::GameEntity;
use crate::player::{ArmorType, ATTN_NORM, CHAN_ITEM, EF_NODRAW, PITCH_NORM, player_slot};
use crate::weapons::core::give_ammo;
use crate::weapons::max_ammo_carry;

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Ground ammo pickup (`ammo_9mm`, `ammo_556nato`, etc.).
pub(crate) struct AmmoPickupEntity {
    class: &'static str,
    ammo_name: &'static str,
    amount: i32,
    picked_up: bool,
}

impl AmmoPickupEntity {
    pub(crate) fn for_class(classname: &str) -> Option<Self> {
        let (class, ammo_name, amount) = match classname {
            "ammo_338magnum" => ("ammo_338magnum", "338Magnum", 10),
            "ammo_357sig" => ("ammo_357sig", "357SIG", 13),
            "ammo_45acp" => ("ammo_45acp", "45acp", 12),
            "ammo_50ae" => ("ammo_50ae", "50AE", 7),
            "ammo_556nato" => ("ammo_556nato", "556Nato", 30),
            "ammo_556natobox" => ("ammo_556natobox", "556NatoBox", 30),
            "ammo_57mm" => ("ammo_57mm", "57mm", 50),
            "ammo_762nato" => ("ammo_762nato", "762Nato", 30),
            "ammo_9mm" => ("ammo_9mm", "9mm", 30),
            "ammo_buckshot" => ("ammo_buckshot", "buckshot", 8),
            _ => return None,
        };
        Some(Self {
            class,
            ammo_name,
            amount,
            picked_up: false,
        })
    }
}

impl GameEntity for AmmoPickupEntity {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let _ = eng.precache_model("models/w_9mmclip.mdl");
        let _ = eng.precache_sound("items/9mmclip1.wav");
        eng.set_model(id, "models/w_9mmclip.mdl");
        eng.set_size(id, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 16.0));
        self.picked_up = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_TRIGGER;
            v.movetype = MOVETYPE_TOSS;
            v.effects &= !EF_NODRAW;
        }
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.picked_up {
            return;
        }
        let Some(slot) = player_slot(g, eng, other) else {
            return;
        };
        if !g.players[slot].as_ref().is_some_and(|p| p.alive) {
            return;
        }
        let max_carry = max_ammo_carry(self.ammo_name);
        if give_ammo(g, eng, slot as u8, self.amount, Some(self.ammo_name), max_carry) == -1 {
            return;
        }
        self.picked_up = true;
        eng.emit_sound(other, CHAN_ITEM, "items/9mmclip1.wav", 1.0, ATTN_NORM, PITCH_NORM);
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.effects |= EF_NODRAW;
        }
        eng.link_entity(id, false);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.picked_up = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_TRIGGER;
            v.effects &= !EF_NODRAW;
        }
        eng.link_entity(id, false);
    }
}

/// Armor vest (`item_kevlar`, `CItemKevlar`) or vest+helmet (`item_assaultsuit`, `CItemAssaultSuit`).
pub(crate) struct ItemArmor {
    assault: bool,
    picked_up: bool,
}

impl ItemArmor {
    pub(crate) fn new(assault: bool) -> Self {
        Self {
            assault,
            picked_up: false,
        }
    }
}

impl GameEntity for ItemArmor {
    fn classname(&self) -> &'static str {
        if self.assault {
            "item_assaultsuit"
        } else {
            "item_kevlar"
        }
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let model = if self.assault {
            "models/w_assault.mdl"
        } else {
            "models/w_kevlar.mdl"
        };
        let _ = eng.precache_model(model);
        let _ = eng.precache_sound("items/ammopickup2.wav");
        eng.set_model(id, model);
        eng.set_size(id, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 16.0));
        self.picked_up = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_TRIGGER;
            v.movetype = MOVETYPE_TOSS;
            v.effects &= !EF_NODRAW;
        }
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.picked_up {
            return;
        }
        let Some(slot) = player_slot(g, eng, other) else {
            return;
        };
        let Some(p) = g.players[slot].as_ref() else {
            return;
        };
        if !p.alive {
            return;
        }
        if self.assault {
            if p.kevlar == ArmorType::VestHelm && p.armor >= 100.0 {
                return;
            }
            give_armor_item(g, eng, slot as u8, ArmorItem::AssaultSuit);
        } else {
            if p.armor >= 100.0 {
                return;
            }
            give_armor_item(g, eng, slot as u8, ArmorItem::Kevlar);
        }
        self.picked_up = true;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.effects |= EF_NODRAW;
        }
        eng.link_entity(id, false);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.picked_up = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_TRIGGER;
            v.effects &= !EF_NODRAW;
        }
        eng.link_entity(id, false);
    }
}

/// Decorative generic prop (`item_generic`, `CItemGeneric` on `de_cbble`/`de_chateau`).
#[derive(Default)]
pub(crate) struct ItemGeneric;

impl GameEntity for ItemGeneric {
    fn classname(&self) -> &'static str {
        "item_generic"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let model = key_str(keys, "model").unwrap_or("");
        if !model.is_empty() {
            let _ = eng.precache_model(model);
            eng.set_model(id, model);
        }
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
            v.framerate = 1.0;
        }
        eng.link_entity(id, false);
        true
    }
}
