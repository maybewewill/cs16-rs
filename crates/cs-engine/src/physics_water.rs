//! The water of an entity: `SV_CheckWater`, `SV_CheckWaterTransition`, `SV_Submerged` (with
//! `SV_RecursiveWaterLevel`) and `PF_WaterMove`, ported from ReHLDS `rehlds/engine/sv_phys.cpp`
//! (read 2026-11-21).

use glam::Vec3;

use crate::collision::{
    CONTENTS_CURRENT_0, CONTENTS_CURRENT_DOWN, CONTENTS_EMPTY, CONTENTS_LAVA, CONTENTS_SLIME,
    CONTENTS_WATER,
};
use crate::edict::EdictId;
use crate::game::GameDll;
use crate::physics::vector_ma;
use crate::server::Server;
use crate::world::consts::{
    FL_GODMODE, FL_IMMUNE_LAVA, FL_IMMUNE_SLIME, FL_IMMUNE_WATER, FL_INWATER, FL_SWIM,
    FL_WATERJUMP, MOVETYPE_NOCLIP,
};

/// `CONTENTS_TRANSLUCENT` (`rehlds/common/const.h`, read 2026-11-21).
const CONTENTS_TRANSLUCENT: i32 = -15;
/// `DEAD_NO`.
const DEAD_NO: i32 = 0;
/// Sound channels and attenuation the water code plays on.
const CHAN_AUTO: u8 = 0;
const CHAN_BODY: u8 = 4;
const ATTN_NORM: f32 = 0.8;

/// Whether `contents` is one `SV_CheckWater` calls water: from water down to, not including,
/// translucent.
pub(crate) fn in_liquid(contents: i32) -> bool {
    contents <= CONTENTS_WATER && contents > CONTENTS_TRANSLUCENT
}

/// `SV_RecursiveWaterLevel`, as a loop: the height, relative to `origin`, of the water surface
/// found by 6 bisections of `mins..maxs`.
fn water_level_between(
    contents_at: &mut impl FnMut(Vec3) -> i32,
    origin: Vec3,
    mut mins: f32,
    mut maxs: f32,
) -> f32 {
    let mut level = (mins - maxs) * 0.5 + maxs;
    for _ in 0..5 {
        let test = Vec3::new(origin.x, origin.y, origin.z + level);
        if contents_at(test) == CONTENTS_WATER {
            maxs = level;
        } else {
            mins = level;
        }
        level = (mins - maxs) * 0.5 + maxs;
    }
    level
}

impl<G: GameDll> Server<G> {
    /// `SV_PointContents` at `p`.
    pub(crate) fn contents(&mut self, p: Vec3) -> i32 {
        self.with_engine(|eng| eng.point_contents_ents(p))
    }

    /// `SV_StartSound(0, ent, channel, sample, 255, attenuation, 0, PITCH_NORM)`.
    pub(crate) fn start_sound(&mut self, id: EdictId, channel: u8, sample: &str, attenuation: f32) {
        if self.with_engine(|eng| eng.emit_sound(id, channel, sample, 1.0, attenuation, 100)) {
            self.drain_messages();
        }
    }

    /// One of the four wade sounds, as `RandomLong(0, 3)` picks (assumption: any generator
    /// will do; the engine's own is not reproduced).
    pub(crate) fn random_wade_sound(&mut self) -> &'static str {
        let mut x = self.phys_rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.phys_rng = x;
        match x % 4 {
            0 => "player/pl_wade1.wav",
            1 => "player/pl_wade2.wav",
            2 => "player/pl_wade3.wav",
            _ => "player/pl_wade4.wav",
        }
    }

    /// `SV_CheckWater`: sets `watertype` and `waterlevel` (1 feet, 2 waist, 3 eyes) from what
    /// is at the entity's feet, middle and view point, and returns whether it is deeper than
    /// the feet. Water with a current pushes the `basevelocity` (the current contents of the
    /// world are reported as water by `SV_PointContents`, so that branch is out of reach in
    /// the source and here).
    pub(crate) fn check_water(&mut self, id: EdictId) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        let (absmin, absmax, view_ofs) = (v.absmin, v.absmax, v.view_ofs);
        let mut point = Vec3::new(
            (absmax.x + absmin.x) * 0.5,
            (absmax.y + absmin.y) * 0.5,
            absmin.z + 1.0,
        );
        let mut watertype = CONTENTS_EMPTY;
        let mut waterlevel = 0;
        let mut current = None;

        let cont = self.contents(point);
        if in_liquid(cont) {
            watertype = cont;
            waterlevel = 1;
            if absmin.z == absmax.z {
                waterlevel = 3;
            } else {
                point.z = (absmax.z + absmin.z) * 0.5;
                if in_liquid(self.contents(point)) {
                    waterlevel = 2;
                    point += view_ofs;
                    if in_liquid(self.contents(point)) {
                        waterlevel = 3;
                    }
                }
            }
            if (CONTENTS_CURRENT_DOWN..=CONTENTS_CURRENT_0).contains(&cont) {
                const CURRENTS: [Vec3; 6] = [
                    Vec3::X,
                    Vec3::Y,
                    Vec3::NEG_X,
                    Vec3::NEG_Y,
                    Vec3::Z,
                    Vec3::NEG_Z,
                ];
                current = CURRENTS
                    .get((CONTENTS_CURRENT_0 - cont) as usize)
                    .map(|dir| *dir * (150.0 * waterlevel as f32 / 3.0));
            }
        }
        if let Some(v) = self.vars_mut(id) {
            v.watertype = watertype;
            v.waterlevel = waterlevel;
            if let Some(push) = current {
                v.basevelocity += push;
            }
        }
        waterlevel > 1
    }

    /// `SV_CheckWaterTransition`: after a move, tracks the entity's water. A fresh entity
    /// (`watertype` 0) takes what it is in; leaving water plays `player/pl_wade2.wav`; entering
    /// it plays `player/pl_wade1.wav` and halves the vertical velocity; in the water the level
    /// is worked out again.
    pub(crate) fn check_water_transition(&mut self, id: EdictId) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let (absmin, absmax, view_ofs, watertype) = (v.absmin, v.absmax, v.view_ofs, v.watertype);
        let mut point = Vec3::new(
            (absmax.x + absmin.x) * 0.5,
            (absmax.y + absmin.y) * 0.5,
            absmin.z + 1.0,
        );
        let cont = self.contents(point);
        if watertype == 0 {
            // Just spawned here.
            if let Some(v) = self.vars_mut(id) {
                v.watertype = cont;
                v.waterlevel = 1;
            }
            return;
        }
        if !in_liquid(cont) {
            if watertype != CONTENTS_EMPTY {
                self.start_sound(id, CHAN_AUTO, "player/pl_wade2.wav", 1.0);
            }
            if let Some(v) = self.vars_mut(id) {
                v.watertype = CONTENTS_EMPTY;
                v.waterlevel = 0;
            }
            return;
        }
        if watertype == CONTENTS_EMPTY {
            // Just crossed into water.
            self.start_sound(id, CHAN_AUTO, "player/pl_wade1.wav", 1.0);
            if let Some(v) = self.vars_mut(id) {
                v.velocity.z *= 0.5;
            }
        }
        let mut waterlevel = 1;
        if absmin.z == absmax.z {
            // A point entity.
            waterlevel = 3;
        } else {
            point.z = (absmin.z + absmax.z) * 0.5;
            if in_liquid(self.contents(point)) {
                waterlevel = 2;
                if in_liquid(self.contents(point + view_ofs)) {
                    waterlevel = 3;
                }
            }
        }
        if let Some(v) = self.vars_mut(id) {
            v.watertype = cont;
            v.waterlevel = waterlevel;
        }
    }

    /// `SV_Submerged`: how much of the entity's height is under water, from its `waterlevel`.
    pub(crate) fn submerged(&mut self, id: EdictId) -> f32 {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return 0.0;
        };
        let (absmin, absmax, level) = (v.absmin, v.absmax, v.waterlevel);
        let height = v.maxs.z - v.mins.z;
        let center = (absmax + absmin) * 0.5;
        let feet = absmin.z - center.z;
        self.with_engine(|eng| {
            let mut contents_at = |p: Vec3| eng.point_contents_ents(p);
            let top = absmax.z - center.z;
            match level {
                1 => water_level_between(&mut contents_at, center, 0.0, feet) - feet,
                2 => water_level_between(&mut contents_at, center, top, 0.0) - feet,
                3 => {
                    let head = Vec3::new(center.x, center.y, absmax.z);
                    if contents_at(head) == CONTENTS_WATER {
                        height
                    } else {
                        water_level_between(&mut contents_at, center, top, 0.0) - feet
                    }
                }
                _ => 0.0,
            }
        })
    }

    /// `PF_WaterMove` (ReHLDS `sv_phys.cpp`, read 2026-11-21): the water movement of a monster,
    /// before its step. It keeps the drowning clock (`air_finished`, `pain_finished`, `dmg`),
    /// the lava and slime damage clock (`dmgtime`), plays the wade sounds when `FL_INWATER`
    /// changes and slows the entity down in the water by `0.8 * waterlevel` per second. The
    /// game reads the clocks and does the damage.
    pub(crate) fn water_move(&mut self, id: EdictId) {
        let time = self.sv_time;
        let frametime = self.frametime;
        let Some(v) = self.vars_mut(id) else {
            return;
        };
        if v.movetype == MOVETYPE_NOCLIP {
            v.air_finished = (time + 12.0) as f32;
            return;
        }
        if v.health < 0.0 {
            return;
        }
        let drownlevel = if v.deadflag == DEAD_NO { 3.0 } else { 1.0 };
        let (waterlevel, watertype, flags) = (v.waterlevel, v.watertype, v.flags);
        let level = waterlevel as f32;

        if flags & (FL_IMMUNE_WATER | FL_GODMODE) == 0 {
            if (flags & FL_SWIM != 0 && level < drownlevel) || level >= drownlevel {
                if f64::from(v.air_finished) < time && f64::from(v.pain_finished) < time {
                    v.dmg += 2.0;
                    if v.dmg > 15.0 {
                        v.dmg = 10.0;
                    }
                    v.pain_finished = (time + 1.0) as f32;
                }
            } else {
                v.dmg = 2.0;
                v.air_finished = (time + 12.0) as f32;
            }
        }

        if waterlevel == 0 {
            // Leaving the water.
            v.air_finished = (time + 12.0) as f32;
            let was_in_water = flags & FL_INWATER != 0;
            if was_in_water {
                v.flags &= !FL_INWATER;
                let sound = self.random_wade_sound();
                self.start_sound(id, CHAN_BODY, sound, ATTN_NORM);
            }
            return;
        }

        if watertype == CONTENTS_LAVA {
            if flags & (FL_IMMUNE_LAVA | FL_GODMODE) == 0 && f64::from(v.dmgtime) < time {
                v.dmgtime = if time <= f64::from(v.radsuit_finished) {
                    (time + 1.0) as f32
                } else {
                    (time + 0.2) as f32
                };
            }
        } else if watertype == CONTENTS_SLIME
            && flags & (FL_IMMUNE_SLIME | FL_GODMODE) == 0
            && f64::from(v.dmgtime) < time
            && f64::from(v.radsuit_finished) < time
        {
            v.dmgtime = (time + 1.0) as f32;
        }

        if flags & FL_INWATER == 0 {
            v.dmgtime = 0.0;
            v.flags |= FL_INWATER;
            if watertype == CONTENTS_WATER {
                let sound = self.random_wade_sound();
                self.start_sound(id, CHAN_BODY, sound, ATTN_NORM);
            }
        }

        if flags & FL_WATERJUMP == 0
            && let Some(v) = self.vars_mut(id)
        {
            let drag = -0.8 * f64::from(waterlevel) * frametime;
            v.velocity = vector_ma(v.velocity, drag, v.velocity);
        }
    }
}
