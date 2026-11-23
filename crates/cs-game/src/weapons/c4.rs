//! The C4 explosive weapon (`weapon_c4`, ReGameDLL_CS `wpn_shared/wpn_c4.cpp`), vanilla
//! behaviour (`CLIENT_WEAPONS`, non-`REGAMEDLL_FIXES` / non-`REGAMEDLL_ADD`):
//!
//! - `CC4::Precache`: `models/v_c4.mdl`, `models/w_backpack.mdl`, `weapons/c4_click.wav`.
//! - `CC4::Deploy`: `models/v_c4.mdl`, `models/p_c4.mdl`, `C4_DRAW`, animation set `"c4"`.
//! - `CC4::Holster`: `m_flNextAttack = 0.5`; stops arming; vanilla quirk leaves any progress bar
//!   and frozen speed alone on holster (`REGAMEDLL_FIXES` cancels them); destroys the item if
//!   out of C4 ammo.
//! - `CC4::PrimaryAttack`: requires `SIGNAL_BOMB` and `FL_ONGROUND`, freezes the carrier
//!   (`SET_CLIENT_MAXSPEED(..., 1.0)`), shows a 3-second `BarTime`, plays `C4_DROP` / `HoldBomb`
//!   at 0.75 s remaining, and plants at `t >= armed_time` with zero angles (vanilla quirk:
//!   `REGAMEDLL_FIXES` rotates by `angles.y - 90`), `"%!MRAD_BOMBPL"`, `BombDrop(planted = true)`,
//!   `#Bomb_Planted`, `weapons/c4_plant.wav`, `body = 0`, and retires the weapon.
//! - `CC4::WeaponIdle`: releasing attack while arming cancels arming and unfreezes; vanilla quirk
//!   when idle expires does not bump `time_weapon_idle = 20.0` and plays `C4_DRAW` then `C4_IDLE1`.

use cs_engine::EngineApi;
use cs_engine::consts::FL_ONGROUND;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use super::core::{CHAN_WEAPON, PlayerAnim, WeaponCtx, precache_model, precache_sound};
use super::grenades::retire_weapon;
use super::info::WeaponId;
use super::weapon::{Weapon, WeaponBehavior};
use crate::buy::SIGNAL_BOMB;
use crate::chat::{HUD_PRINTCENTER, client_print};
use crate::player::{ATTN_NORM, PITCH_NORM};

const C4_IDLE1: i32 = 0;
const C4_DRAW: i32 = 1;
const C4_DROP: i32 = 2;
const C4_ARM: i32 = 3;

const C4_MAX_SPEED: f32 = 250.0;
const C4_ARMING_ON_TIME: f32 = 3.0;

/// The C4 explosive carried in slot 5 (`CC4`, `weapon_c4`).
#[derive(Debug, Clone, Default)]
pub struct C4 {
    started_arming: bool,
    bomb_placed_animation: bool,
    armed_time: f32,
}

impl WeaponBehavior for C4 {
    fn id(&self) -> WeaponId {
        WeaponId::C4
    }

    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_c4.mdl");
        precache_model(eng, "models/w_backpack.mdl");
        precache_sound(eng, "weapons/c4_click.wav");
    }

    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        self.started_arming = false;
        self.armed_time = 0.0;
        w.weapon.max_speed = C4_MAX_SPEED;
        w.default_deploy(self, "models/v_c4.mdl", "models/p_c4.mdl", C4_DRAW, "c4", true)
    }

    fn holster(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.player_mut().next_attack = 0.5;
        // Vanilla quirk (`!REGAMEDLL_FIXES`): holstering does not cancel `SetProgressBarTime` or
        // unfreeze; only `started_arming` resets.
        self.started_arming = false;
        if w.ammo(w.weapon.primary_ammo_type) <= 0 {
            w.remove_self = true;
        }
    }

    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let ammo_type = w.weapon.primary_ammo_type;
        if w.ammo(ammo_type) <= 0 {
            return;
        }
        let in_bomb_zone = (w.player().signals.state() & SIGNAL_BOMB) == SIGNAL_BOMB;
        let id = w.edict();
        let on_ground = w.eng.ent(id).is_some_and(|v| v.flags & FL_ONGROUND != 0);
        let place_bomb = on_ground && in_bomb_zone;

        if !self.started_arming {
            if !in_bomb_zone {
                client_print(w.eng, w.slot, HUD_PRINTCENTER, "#C4_Plant_At_Bomb_Spot", &[]);
                w.weapon.next_primary_attack = w.get_next_attack_delay(1.0);
                return;
            }
            if !on_ground {
                client_print(w.eng, w.slot, HUD_PRINTCENTER, "#C4_Plant_Must_Be_On_Ground", &[]);
                w.weapon.next_primary_attack = w.get_next_attack_delay(1.0);
                return;
            }
            self.started_arming = true;
            self.bomb_placed_animation = false;
            self.armed_time = w.now() + C4_ARMING_ON_TIME;
            w.send_weapon_anim(C4_ARM, true);
            crate::player::freeze_max_speed(w.eng, w.slot);
            w.set_animation(PlayerAnim::Attack1);
            crate::player::set_progress_bar_time(w.g, w.eng, w.slot, C4_ARMING_ON_TIME as i32);
        } else if place_bomb {
            if self.armed_time <= w.now() {
                self.started_arming = false;
                self.armed_time = 0.0;

                crate::rules::broadcast(w.eng, "BOMBPL");
                w.player_mut().has_c4 = false;

                // Vanilla (`!REGAMEDLL_FIXES`): planted bomb angles are zero.
                let origin = w.eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
                let bomb_id =
                    crate::grenade::shoot_satchel_charge(w.g, w.eng, id, origin, Vec3::ZERO);
                let bomb_origin =
                    bomb_id.and_then(|b| w.eng.ent(b)).map_or(origin, |v| v.origin);

                w.eng.send_usermsg(
                    cs_engine::MsgDest::All,
                    &CsUserMsg::BombDrop {
                        origin: bomb_origin,
                        planted: true,
                    },
                );
                crate::menus::print_center_all(w.eng, "#Bomb_Planted");
                crate::rules::on_bomb_planted(w.g, w.eng);

                w.eng.emit_sound(
                    id,
                    CHAN_WEAPON,
                    "weapons/c4_plant.wav",
                    1.0,
                    ATTN_NORM,
                    PITCH_NORM,
                );
                if let Some(v) = w.eng.ent_mut(id) {
                    v.body = 0;
                }
                w.reset_max_speed(self);
                crate::buy::set_bomb_icon(w.g, w.eng, w.slot, false);

                w.add_ammo(ammo_type, -1);
                if w.ammo(ammo_type) <= 0 {
                    retire_weapon(self, w);
                    return;
                }
            } else if self.armed_time - 0.75 <= w.now() && !self.bomb_placed_animation {
                self.bomb_placed_animation = true;
                w.send_weapon_anim(C4_DROP, true);
                w.set_animation(PlayerAnim::HoldBomb);
            }
        } else {
            let msg = if in_bomb_zone {
                "#C4_Plant_Must_Be_On_Ground"
            } else {
                "#C4_Arming_Cancelled"
            };
            client_print(w.eng, w.slot, HUD_PRINTCENTER, msg, &[]);
            self.started_arming = false;
            w.weapon.next_primary_attack = w.get_next_attack_delay(1.5);
            w.reset_max_speed(self);
            crate::player::set_progress_bar_time(w.g, w.eng, w.slot, 0);
            w.set_animation(PlayerAnim::HoldBomb);
            let anim = if self.bomb_placed_animation { C4_DRAW } else { C4_IDLE1 };
            w.send_weapon_anim(anim, true);
            return;
        }

        w.weapon.next_primary_attack = w.get_next_attack_delay(0.3);
        w.weapon.time_weapon_idle = w.random_float(10.0, 15.0);
    }

    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if self.started_arming {
            self.started_arming = false;
            w.reset_max_speed(self);
            w.weapon.next_primary_attack = w.get_next_attack_delay(1.0);
            crate::player::set_progress_bar_time(w.g, w.eng, w.slot, 0);
            let anim = if self.bomb_placed_animation { C4_DRAW } else { C4_IDLE1 };
            w.send_weapon_anim(anim, true);
        }

        if w.weapon.time_weapon_idle <= 0.0 {
            if w.ammo(w.weapon.primary_ammo_type) <= 0 {
                retire_weapon(self, w);
                return;
            }
            // Vanilla (`!REGAMEDLL_FIXES`): plays `C4_DRAW` then `C4_IDLE1` without bumping
            // `time_weapon_idle`.
            w.send_weapon_anim(C4_DRAW, true);
            w.send_weapon_anim(C4_IDLE1, true);
        }
    }

    fn max_speed(&self, _w: &Weapon) -> f32 {
        C4_MAX_SPEED
    }

    fn can_deploy(&self, w: &WeaponCtx<'_, '_>) -> bool {
        w.ammo(w.weapon.primary_ammo_type) != 0
    }

    fn can_drop(&self, _w: &WeaponCtx<'_, '_>) -> bool {
        true
    }
}
