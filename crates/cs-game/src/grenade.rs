//! The thrown grenades: `CGrenade` (the entity `grenade`, ReGameDLL_CS `ggrenade.cpp`), the
//! blast and flash of `combat.cpp` (`RadiusDamage`, `RadiusFlash`, `PlayerBlind`) and the
//! `spark_shower` the explosions throw (`explode.cpp`, `CShower`), read 2026-11-22, the vanilla
//! build (`CLIENT_WEAPONS`, `REGAMEDLL_API`, none of `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`,
//! `BUILD_LATEST`).
//!
//! A weapon throws a grenade by [`throw_grenade`] (`CBasePlayer::ThrowGrenade`): the HE grenade
//! ([`shoot_timed2`], damage 100, gravity 0.55, friction 0.7), the flashbang ([`shoot_timed`],
//! damage 35) and the smoke grenade ([`shoot_smoke_grenade`], damage 35) each make a `grenade`
//! entity that flies by the engine's `MOVETYPE_BOUNCE`, bounces ([`Grenade::bounce_touch`]),
//! tumbles in the air until its fuse has run out (a think every tenth of a second) and then
//! goes off. The fuse is 1.5 seconds, but a grenade changes its think when the fuse is spent
//! and the new think runs one tenth later, so it goes off 0.1 to 0.2 seconds after it.
//!
//! - HE ([`Grenade::explode3`]): two `TE_EXPLOSION` to the potentially audible set,
//!   [`radius_damage`] (damage 100, radius `100 * 3.5`, falling off linearly with the distance
//!   from the grenade's origin to the victim's origin, through walls), a scorch decal, a debris
//!   sound and up to three spark showers, then a smoke puff ([`Grenade::smoke3_c`]) 0.55 s
//!   later. Damage bits `DMG_EXPLOSION`, inflictor the grenade, attacker the thrower, so the
//!   death notice names `grenade` and the kill is the thrower's.
//! - Flash ([`Grenade::explode_flashbang`]): [`radius_flash`] (radius 1500, "damage" 4), a
//!   scorch decal, a flashbang sound, sparks, a puff 0.3 s later. A player the flash can see
//!   is blinded: `ScreenFade` (white), and the blind fields of the player
//!   ([`crate::player::Player::blind`]).
//! - Smoke ([`Grenade::sg_detonate`]): the grenade waits to lie on the ground (`SG_TumbleThink`),
//!   then plays `events/createsmoke.sc` at its origin and, every second for 21 seconds, once
//!   more with the direction of a puff ([`Grenade::sg_smoke`]); the client draws the cloud.
//!
//! Not ported, and named:
//! - `StudioFrameAdvance` of the tumble thinks (it moves `pev->frame` and `pev->animtime` along
//!   the model's sequence; the client animates the model by `sequence` and `framerate` by
//!   itself) and the world's sound list (`CSoundEnt::InsertSound`, only the monsters listen).
//! - The bots (`TheBots->OnEvent`, `AddGrenade`, `RemoveGrenade`) and the career task
//!   (`REGAMEDLL_ADD`'s `RecordDamage`).
//! - `pev->takedamage = DAMAGE_NO` of an exploding grenade: the entity variables of the engine
//!   have no such field and nothing in this game damages a grenade.
//! - `CGrenade::Spawn` sets `m_fRegisteredSound` and `ShootTimed*` set `m_bJustBlew`: no code of
//!   the vanilla build reads either (the first was for the danger sound, the second for the C4).
//! - `ShootContact`/`ExplodeTouch`/`DangerSoundThink` (the M203 grenade), `SG_Explode`
//!   (`NOXREF`), `DetonateUse`/`PreDetonate` (nothing sets them), `Explode2`/`Detonate2` and
//!   `ShootSatchelCharge` (the C4, phase 2E) and `RadiusDamage2` (`CBaseMonster::RadiusDamage`
//!   uses it for a damage of 80 or less; the HE grenade's is 100).
//! - `m_iTeam` (the thrower's team, set by `ShootTimed2`) travels with the damage: the explosion
//!   passes it down [`radius_damage`] to `CBasePlayer::TakeDamage`, which compares it with the
//!   victim's team (the entity is checked out of its edict while it explodes).

use cs_engine::collision::CONTENTS_WATER;
use cs_engine::consts::{
    FL_ONGROUND, MOVETYPE_BOUNCE, MOVETYPE_TOSS, SOLID_BBOX, SOLID_NOT,
};
use cs_engine::{EdictId, EngineApi, EntTrace, MoveFilter, MsgDest};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::messages::SND_FL_STOP;
use cs_proto48::tempent::{self, explflag};
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use crate::GameState;
use crate::bullets::{decal_trace, random_float};
use crate::chat::{HUD_PRINTCENTER, client_print};
use crate::entity::{FCAP_CONTINUOUS_USE, GameEntity, UseType, attach, use_entity, util_remove};
use crate::observer::{FADETOBLACK_STAY, FFADE_IN, screen_fade_color};
use crate::player::{
    ATTN_NORM, CHAN_ITEM, CHAN_VOICE, DEAD_NO, EF_NODRAW, PITCH_NORM, Team, dmg, player_slot,
    take_damage_from_grenade,
};
use crate::weapons::core::CHAN_WEAPON;
use crate::weapons::WeaponId;

const ATTN_NONE: f32 = 0.0;
const SF_DETONATE: u32 = 1 << 0;
const NEXT_DEFUSE_TIME: f32 = 0.5;

/// `DECAL_SCORCH1` and `DECAL_SCORCH2` (`decals.h`): the numbers in the game's decal table.
const DECAL_SCORCH1: usize = 11;
const DECAL_SCORCH2: usize = 12;
/// `TE_BUBBLES` (`const.h`).
const TE_BUBBLES: u8 = 113;
/// The radius `RadiusFlash` looks in, and the damage the flashbang passes it.
const FLASH_RADIUS: f32 = 1500.0;
const FLASH_DAMAGE: f32 = 4.0;
/// `kRenderNormal`.
const RENDER_NORMAL: u8 = 0;

/// Which think of `CGrenade` runs next (`SetThink`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Think {
    /// No think function (a freshly made entity).
    None,
    Tumble,
    SgTumble,
    Detonate,
    Detonate2,
    Detonate3,
    SgDetonate,
    SgSmoke,
    Smoke,
    Smoke3C,
    Smoke2,
    #[allow(dead_code)]
    Smoke3B,
    #[allow(dead_code)]
    Smoke3A,
    C4,
}

/// Which touch of `CGrenade` runs (`SetTouch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Touch {
    None,
    Bounce,
    // Unused in vanilla C4 (`C4Touch` is empty); kept for `CGrenade::SlideTouch`.
    #[allow(dead_code)]
    Slide,
}

/// The entity `grenade` (`CGrenade`): thrown grenades and the planted C4.
#[derive(Debug, Clone)]
pub struct Grenade {
    think: Think,
    touch: Touch,
    /// `m_iBounceCount`.
    bounce_count: i32,
    /// `m_usEvent`: the event of the explosion (HE) or the smoke (smoke grenade).
    event: u16,
    /// `m_bLightSmoke`: another smoke grenade went off close by, so the cloud is thinner.
    light_smoke: bool,
    /// `m_bDetonated`.
    detonated: bool,
    /// `m_vSmokeDetonate`.
    smoke_detonate: Vec3,
    /// `m_SGSmoke`: the puffs of the cloud so far.
    sg_smoke: i32,
    /// `m_angle`: the turn of the next puff, degrees.
    angle: i32,
    /// `m_iTeam`: the team id of the thrower when it threw (HE only), what
    /// `CBasePlayer::TakeDamage` compares with the victim's team.
    team: i32,
    /// `m_bIsC4`: this entity is a planted C4.
    pub(crate) is_c4: bool,
    /// `m_bJustBlew`.
    just_blew: bool,
    /// `m_bStartDefuse`.
    start_defuse: bool,
    /// `m_flC4Blow`: server time the bomb detonates.
    c4_blow: f32,
    /// `m_flNextFreq`: next beep wave transition time.
    next_freq: f32,
    /// `m_flNextFreqInterval`: spacing between beep wave transitions.
    next_freq_interval: f32,
    /// `m_iCurWave`: beep wave index (0..=4).
    cur_wave: i32,
    /// `m_fAttenu`: attenuation of the current beep sound.
    attenu: f32,
    /// `m_sBeepName`: current beep sound sample.
    beep_name: Option<&'static str>,
    /// `m_flNextBeep`: server time of the next beep sound.
    next_beep: f32,
    /// `m_flNextBlink`: server time of the next `TE_GLOWSPRITE` LED blink.
    next_blink: f32,
    /// `m_flDefuseCountDown`: server time defusal completes.
    defuse_countdown: f32,
    /// `m_fNextDefuse`: server time defusal aborts if the CT is no longer holding `+use`.
    next_defuse: f32,
    /// `m_pBombDefuser`: player slot currently defusing this bomb.
    bomb_defuser: Option<u8>,
    /// `m_pentCurBombTarget`: `func_bomb_target` under the bomb when planted.
    cur_bomb_target: Option<EdictId>,
}

impl Default for Grenade {
    /// `CGrenade::Spawn`'s own fields: no bounce yet, nothing set (the C++ memory is zeroed).
    fn default() -> Self {
        Self {
            think: Think::None,
            touch: Touch::None,
            bounce_count: 0,
            event: 0,
            light_smoke: false,
            detonated: false,
            smoke_detonate: Vec3::ZERO,
            sg_smoke: 0,
            angle: 0,
            team: 0,
            is_c4: false,
            just_blew: false,
            start_defuse: false,
            c4_blow: 0.0,
            next_freq: 0.0,
            next_freq_interval: 0.0,
            cur_wave: 0,
            attenu: 0.0,
            beep_name: None,
            next_beep: 0.0,
            next_blink: 0.0,
            defuse_countdown: 0.0,
            next_defuse: 0.0,
            bomb_defuser: None,
            cur_bomb_target: None,
        }
    }
}

/// `gpGlobals->time`, the `float` it is in C.
fn now_f32(eng: &EngineApi<'_>) -> f32 {
    eng.time() as f32
}

/// `pev->nextthink = gpGlobals->time + delay`, in `float`s.
fn schedule(eng: &mut EngineApi<'_>, id: EdictId, delay: f32) {
    let at = f64::from(now_f32(eng) + delay);
    if let Some(v) = eng.ent_mut(id) {
        v.nextthink = at;
    }
}

/// `CBaseEntity::IsInWorld` (`cbase.cpp` 864, the vanilla branch): the origin is inside
/// `+-4096` on every axis and no velocity component reaches 2000.
pub fn is_in_world(origin: Vec3, velocity: Vec3) -> bool {
    if origin.x >= 4096.0 || origin.y >= 4096.0 || origin.z >= 4096.0 {
        return false;
    }
    if origin.x <= -4096.0 || origin.y <= -4096.0 || origin.z <= -4096.0 {
        return false;
    }
    if velocity.x >= 2000.0 || velocity.y >= 2000.0 || velocity.z >= 2000.0 {
        return false;
    }
    if velocity.x <= -2000.0 || velocity.y <= -2000.0 || velocity.z <= -2000.0 {
        return false;
    }
    true
}

/// `UTIL_WaterLevel(position, minz, maxz)` (`util.cpp` 1369, read 2026-11-22): the height of the
/// water's surface between `minz` and `maxz` above `position`, by bisection.
fn water_level(eng: &EngineApi<'_>, position: Vec3, minz: f32, maxz: f32) -> f32 {
    let mut minz = minz;
    let mut maxz = maxz;
    let mut mid_up = position;
    mid_up.z = minz;
    if eng.point_contents_ents(mid_up) != CONTENTS_WATER {
        return minz;
    }
    mid_up.z = maxz;
    if eng.point_contents_ents(mid_up) == CONTENTS_WATER {
        return maxz;
    }
    let mut diff = maxz - minz;
    while diff > 1.0 {
        mid_up.z = minz + diff / 2.0;
        if eng.point_contents_ents(mid_up) == CONTENTS_WATER {
            minz = mid_up.z;
        } else {
            maxz = mid_up.z;
        }
        diff = maxz - minz;
    }
    mid_up.z
}

/// `UTIL_Bubbles(mins, maxs, count)` (`util.cpp` 1399, read 2026-11-22): `TE_BUBBLES` to the
/// potentially audible set of the middle of the box.
fn bubbles(eng: &mut EngineApi<'_>, mins: Vec3, maxs: Vec3, count: u8) {
    let mid = (mins + maxs) * 0.5;
    let height = water_level(eng, mid, mid.z, mid.z + 1024.0) - mins.z;
    let sprite = eng.model_index("sprites/bubble.spr").unwrap_or(0);
    let mut w = MsgWriter::new();
    for c in [mins, maxs] {
        w.write_coord(c.x);
        w.write_coord(c.y);
        w.write_coord(c.z);
    }
    w.write_coord(height);
    w.write_u16(sprite);
    w.write_u8(count);
    w.write_coord(8.0);
    eng.temp_entity(MsgDest::Pas(mid), TE_BUBBLES, w.as_slice());
}

/// The sprite `name` of `WeaponsPrecache` (`g_sModelIndex*`), 0 when it was not precached.
fn sprite(eng: &EngineApi<'_>, name: &str) -> u16 {
    eng.model_index(name).unwrap_or(0)
}

/// `UTIL_Sparks(position)`.
fn sparks(eng: &mut EngineApi<'_>, position: Vec3) {
    crate::bullets::sparks(eng, position);
}

impl Grenade {
    /// `SetThink(think)` and the same for the touch are plain assignments here.
    fn set_think(&mut self, think: Think) {
        self.think = think;
    }

    /// The trace every `Detonate*` makes: from 8 above the origin 40 down, `ignore_monsters`,
    /// the grenade itself passed.
    fn trace_down(eng: &EngineApi<'_>, id: EdictId) -> Option<EntTrace> {
        let origin = eng.ent(id)?.origin;
        let spot = origin + Vec3::new(0.0, 0.0, 8.0);
        Some(eng.trace_line(spot, spot + Vec3::new(0.0, 0.0, -40.0), MoveFilter::NoMonsters, Some(id)))
    }

    /// The beginning every explosion shares (`pev->model = iStringNull`, `pev->solid =
    /// SOLID_NOT`, the pull out of the wall): the grenade is invisible and intangible, and
    /// when the trace hit something its origin moves off the wall by `(dmg - 24) * 0.6` along
    /// the plane's normal. (`pev->takedamage = DAMAGE_NO` has no variable here.)
    fn begin_explosion(eng: &mut EngineApi<'_>, id: EdictId, tr: &EntTrace) {
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        v.model.clear(); // invisible
        v.solid = SOLID_NOT; // intangible
        // Pull out of the wall a bit.
        if tr.trace.fraction != 1.0 {
            v.origin = tr.trace.end_pos + (tr.trace.plane_normal * (v.dmg - 24.0) * 0.6);
        }
    }

    /// The scorch mark of an explosion: `RANDOM_FLOAT(0, 1) < 0.5` picks the first of two.
    fn scorch(g: &mut GameState, eng: &mut EngineApi<'_>, tr: &EntTrace) {
        let number = if random_float(g, 0.0, 1.0) < 0.5 { DECAL_SCORCH1 } else { DECAL_SCORCH2 };
        decal_trace(eng, tr, number);
    }

    /// `Create("spark_shower", pev->origin, pTrace->vecPlaneNormal, nullptr)` `RANDOM_LONG(0, 3)`
    /// times.
    fn spark_showers(g: &mut GameState, eng: &mut EngineApi<'_>, origin: Vec3, normal: Vec3) {
        let count = g.random_range(0, 3);
        for _ in 0..count {
            create_spark_shower(g, eng, origin, normal);
        }
    }

    /// `CGrenade::Explode(pTrace, bitsDamageType)` (`ggrenade.cpp` 34-95, the flashbang's):
    /// quirks kept: the grenade's owner is cleared before the flash (so a trace ignoring the
    /// grenade no longer ignores the thrower); `flRndSound` is drawn and never used; the sparks
    /// are made only out of the water.
    fn explode_flashbang(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, tr: &EntTrace, bits: u32) {
        Self::begin_explosion(eng, id, tr);
        let Some(v) = eng.ent(id) else {
            return;
        };
        let (origin, owner) = (v.origin, v.owner);
        let contents = eng.point_contents_ents(origin);
        // Can't traceline attack owner if this is set.
        if let Some(v) = eng.ent_mut(id) {
            v.owner = None;
        }
        radius_flash(g, eng, origin, id, owner, FLASH_DAMAGE, bits);
        Self::scorch(g, eng, tr);
        // `flRndSound = RANDOM_FLOAT(0, 1)`: unused, but it draws a number.
        let _ = random_float(g, 0.0, 1.0);
        let sample = match g.random_range(0, 1) {
            0 => "weapons/flashbang-2.wav",
            _ => "weapons/flashbang-1.wav",
        };
        eng.emit_sound(id, CHAN_VOICE, sample, 0.55, ATTN_NORM, PITCH_NORM);
        if let Some(v) = eng.ent_mut(id) {
            v.effects |= EF_NODRAW;
            v.velocity = Vec3::ZERO;
        }
        self.set_think(Think::Smoke);
        schedule(eng, id, 0.3);
        // Draw sparks.
        if contents != CONTENTS_WATER {
            Self::spark_showers(g, eng, origin, tr.trace.plane_normal);
        }
    }

    /// `CGrenade::Explode3(pTrace, bitsDamageType)` (`ggrenade.cpp` 254-329, the HE grenade's).
    fn explode3(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, tr: &EntTrace, bits: u32) {
        Self::begin_explosion(eng, id, tr);
        let Some(v) = eng.ent(id) else {
            return;
        };
        let (origin, owner, damage) = (v.origin, v.owner, v.dmg);
        let team = self.team;
        // The first fireball, then a second one at a random place near it.
        let te = tempent::explosion(
            Vec3::new(origin.x, origin.y, origin.z + 20.0),
            sprite(eng, "sprites/fexplo.spr"),
            25,
            30,
            explflag::NONE,
        );
        eng.send_temp_entity(MsgDest::Pas(origin), &te);
        let x = origin.x + random_float(g, -64.0, 64.0);
        let y = origin.y + random_float(g, -64.0, 64.0);
        let z = origin.z + random_float(g, 30.0, 35.0);
        let te = tempent::explosion(
            Vec3::new(x, y, z),
            sprite(eng, "sprites/eexplo.spr"),
            30,
            30,
            explflag::NONE,
        );
        eng.send_temp_entity(MsgDest::Pas(origin), &te);

        if let Some(v) = eng.ent_mut(id) {
            v.owner = None;
        }
        grenade_radius_damage(g, eng, origin, id, owner, damage, team, bits);
        Self::scorch(g, eng, tr);
        // `flRndSound = RANDOM_FLOAT(0, 1)`: unused, but it draws a number.
        let _ = random_float(g, 0.0, 1.0);
        let sample = match g.random_range(0, 2) {
            0 => "weapons/debris1.wav",
            1 => "weapons/debris2.wav",
            _ => "weapons/debris3.wav",
        };
        eng.emit_sound(id, CHAN_VOICE, sample, 0.55, ATTN_NORM, PITCH_NORM);
        if let Some(v) = eng.ent_mut(id) {
            v.effects |= EF_NODRAW;
            v.velocity = Vec3::ZERO;
        }
        self.set_think(Think::Smoke3C);
        schedule(eng, id, 0.55);
        Self::spark_showers(g, eng, origin, tr.trace.plane_normal);
    }

    /// A `TE_SMOKE` puff at `origin` (to the potentially visible set), or bubbles when the
    /// origin is under water: what the `Smoke*` thinks have in common.
    fn smoke_puff(eng: &mut EngineApi<'_>, origin: Vec3, at: Vec3, scale: u8, framerate: u8) {
        if eng.point_contents_ents(origin) == CONTENTS_WATER {
            bubbles(eng, origin - Vec3::new(64.0, 64.0, 64.0), origin + Vec3::new(64.0, 64.0, 64.0), 100);
        } else {
            let te = tempent::smoke(at, sprite(eng, "sprites/steam1.spr"), scale, framerate);
            eng.send_temp_entity(MsgDest::Pvs(origin), &te);
        }
    }

    /// `CGrenade::Smoke3_C`: the last puff of the HE grenade, then the grenade is gone.
    fn smoke3_c(g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(origin) = eng.ent(id).map(|v| v.origin) else {
            return;
        };
        // `WRITE_BYTE(35 + RANDOM_FLOAT(0, 10))`: the float is cut to a byte.
        let scale = (35.0 + random_float(g, 0.0, 10.0)) as i32 as u8;
        Self::smoke_puff(eng, origin, origin - Vec3::new(0.0, 0.0, 5.0), scale, 5);
        eng.remove_entity(id);
    }

    /// `CGrenade::Smoke3_B` and `Smoke3_A`: two puffs at random places around the origin, the
    /// second 0.15 s after the first (`a_and_b` is true for `Smoke3_B`, which schedules `A`).
    fn smoke3_ab(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, b: bool) {
        let Some(origin) = eng.ent(id).map(|v| v.origin) else {
            return;
        };
        let x = origin.x + random_float(g, -128.0, 128.0);
        let y = origin.y + random_float(g, -128.0, 128.0);
        let z = origin.z + random_float(g, -10.0, 10.0);
        let scale = (15.0 + random_float(g, 0.0, 10.0)) as i32 as u8;
        Self::smoke_puff(eng, origin, Vec3::new(x, y, z), scale, if b { 10 } else { 12 });
        if b {
            schedule(eng, id, 0.15);
            self.set_think(Think::Smoke3A);
        }
    }

    /// `CGrenade::Smoke2` (`TE_SMOKE`, scale 150, framerate 8) and `CGrenade::Smoke` (scale 25,
    /// framerate 6): one puff at the origin, then the grenade is gone.
    fn smoke_once(eng: &mut EngineApi<'_>, id: EdictId, scale: u8, framerate: u8) {
        let Some(origin) = eng.ent(id).map(|v| v.origin) else {
            return;
        };
        Self::smoke_puff(eng, origin, origin, scale, framerate);
        eng.remove_entity(id);
    }

    /// `CGrenade::Detonate`: the flashbang goes off (`Explode(&tr, DMG_BLAST)`).
    fn detonate(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if let Some(tr) = Self::trace_down(eng, id) {
            self.explode_flashbang(g, eng, id, &tr, dmg::BLAST);
        }
    }

    /// `CGrenade::Detonate2`: the planted C4 goes off (`Explode2(&tr, DMG_BLAST)`).
    fn detonate2(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if let Some(tr) = Self::trace_down(eng, id) {
            self.explode2(g, eng, id, &tr, dmg::BLAST);
        }
    }

    /// `CGrenade::Explode2(pTrace, bitsDamageType)` (`ggrenade.cpp` 99-250, vanilla):
    /// shakes players on the ground within 3000 units, sets `m_bTargetBombed` / `m_bJustBlew` and
    /// checks win conditions before pulling out of the wall and firing 4 `TE_SPRITE`s,
    /// `weapons/c4_explode1.wav`, `CBaseMonster::RadiusDamage(m_flBombRadius)`, scorch decal,
    /// debris sound, `Smoke2` 0.85 s later and sparks if out of water.
    fn explode2(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        tr: &EntTrace,
        bits: u32,
    ) {
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        v.model.clear();
        v.solid = SOLID_NOT;

        screen_shake(g, eng, tr.trace.end_pos, 25.0, 150.0, 1.0, 3000.0);

        self.just_blew = true;
        crate::rules::on_bomb_exploded(g, eng);

        // Pull out of the wall a bit.
        if let Some(v) = eng.ent_mut(id)
            && tr.trace.fraction != 1.0
        {
            v.origin = tr.trace.end_pos + (tr.trace.plane_normal * (v.dmg - 24.0) * 0.6);
        }
        let Some(v) = eng.ent(id) else {
            return;
        };
        let (origin, owner, dmg_val) = (v.origin, v.owner, v.dmg);
        let contents = eng.point_contents_ents(origin);
        let scale = ((dmg_val - 275.0) * 0.6) as i32 as u8;

        for (pos, spr, bright) in [
            (origin - Vec3::new(0.0, 0.0, 10.0), "sprites/fexplo.spr", 150u8),
            (
                Vec3::new(
                    origin.x + random_float(g, -512.0, 512.0),
                    origin.y + random_float(g, -512.0, 512.0),
                    origin.z + random_float(g, -10.0, 10.0),
                ),
                "sprites/eexplo.spr",
                150,
            ),
            (
                Vec3::new(
                    origin.x + random_float(g, -512.0, 512.0),
                    origin.y + random_float(g, -512.0, 512.0),
                    origin.z + random_float(g, -10.0, 10.0),
                ),
                "sprites/fexplo.spr",
                150,
            ),
            (
                Vec3::new(
                    origin.x + random_float(g, -512.0, 512.0),
                    origin.y + random_float(g, -512.0, 512.0),
                    origin.z + random_float(g, -10.0, 10.0),
                ),
                "sprites/zerogxplode.spr",
                17,
            ),
        ] {
            let te = tempent::sprite(pos, sprite(eng, spr), scale, bright);
            eng.send_temp_entity(MsgDest::Pas(origin), &te);
        }

        // Sound! for everyone
        eng.emit_sound(id, CHAN_WEAPON, "weapons/c4_explode1.wav", 1.0, 0.25, PITCH_NORM);

        if let Some(v) = eng.ent_mut(id) {
            v.owner = None;
        }
        let bomb_radius = g.rules.bomb_radius();
        monster_radius_damage(g, eng, origin, id, owner, bomb_radius, bits);

        Self::scorch(g, eng, tr);
        // `flRndSound = RANDOM_FLOAT(0, 1)`: unused, but draws a number.
        let _ = random_float(g, 0.0, 1.0);
        let sample = match g.random_range(0, 2) {
            0 => "weapons/debris1.wav",
            1 => "weapons/debris2.wav",
            _ => "weapons/debris3.wav",
        };
        eng.emit_sound(id, CHAN_VOICE, sample, 0.55, ATTN_NORM, PITCH_NORM);

        if let Some(v) = eng.ent_mut(id) {
            v.effects |= EF_NODRAW;
            v.velocity = Vec3::ZERO;
        }
        self.set_think(Think::Smoke2);
        schedule(eng, id, 0.85);

        if contents != CONTENTS_WATER {
            Self::spark_showers(g, eng, origin, tr.trace.plane_normal);
        }
    }

    /// `CGrenade::DefuseBombStart(CBasePlayer *pPlayer)` (`ggrenade.cpp` 1027-1083).
    fn defuse_bomb_start(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
        crate::player::freeze_max_speed(eng, slot);
        let has_defuser = g
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .is_some_and(|p| p.has_defuser);
        let now = now_f32(eng);
        let (msg, secs) = if has_defuser {
            ("#Defusing_Bomb_With_Defuse_Kit", 5)
        } else {
            ("#Defusing_Bomb_Without_Defuse_Kit", 10)
        };
        client_print(eng, slot, HUD_PRINTCENTER, msg, &[]);
        self.defuse_countdown = now + secs as f32;
        crate::player::set_progress_bar_time(g, eng, slot, secs);

        if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.is_defusing = true;
        }
        self.bomb_defuser = Some(slot);
        self.start_defuse = true;
        self.next_defuse = now + NEXT_DEFUSE_TIME;

        // Vanilla (`!REGAMEDLL_FIXES`): emits on the defuser's edict rather than the bomb.
        let player_edict = eng.client_edict(slot);
        eng.emit_sound(player_edict, CHAN_ITEM, "weapons/c4_disarm.wav", 1.0, ATTN_NORM, PITCH_NORM);
    }

    /// `CGrenade::DefuseBombEnd(CBasePlayer *pPlayer, bool bDefused)` (`ggrenade.cpp` 1087-1217).
    fn defuse_bomb_end(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        slot: u8,
        defused: bool,
    ) {
        let player_edict = eng.client_edict(slot);
        if defused {
            let alive = eng.ent(player_edict).is_some_and(|v| v.deadflag == DEAD_NO);
            if alive {
                crate::rules::broadcast(eng, "BOMBDEF");

                let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
                eng.emit_ambient_sound(
                    id,
                    origin,
                    "weapons/c4_beep5.wav",
                    0.0,
                    ATTN_NONE,
                    SND_FL_STOP as i32,
                    0,
                );
                eng.emit_sound(
                    player_edict,
                    CHAN_WEAPON,
                    "weapons/c4_disarmed.wav",
                    1.0,
                    ATTN_NORM,
                    PITCH_NORM,
                );
                util_remove(eng, id);

                self.just_blew = true;
                crate::player::reset_max_speed(g, eng, slot);
                if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
                    p.is_defusing = false;
                }

                eng.send_usermsg(
                    MsgDest::All,
                    &CsUserMsg::Scenario {
                        active: false,
                        sprite: String::new(),
                        alpha: 0,
                        flash: None,
                    },
                );

                crate::rules::on_bomb_defused(g, eng);

                if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
                    p.frags += 3;
                }

                eng.send_usermsg(MsgDest::All, &CsUserMsg::BombPickup);
                g.rules.bomb_dropped = false;
                self.bomb_defuser = None;
                self.start_defuse = false;
            } else {
                // Vanilla (`!REGAMEDLL_FIXES`): no `SetProgressBarTime(0)` when defuser died.
                crate::player::reset_max_speed(g, eng, slot);
                if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
                    p.is_defusing = false;
                }
                self.start_defuse = false;
                self.bomb_defuser = None;
            }
        } else {
            let on_ground = eng.ent(player_edict).is_some_and(|v| v.flags & FL_ONGROUND != 0);
            if !on_ground {
                client_print(eng, slot, HUD_PRINTCENTER, "#C4_Defuse_Must_Be_On_Ground", &[]);
            }
            crate::player::reset_max_speed(g, eng, slot);
            if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
                p.is_defusing = false;
            }
            crate::player::set_progress_bar_time(g, eng, slot, 0);
            self.bomb_defuser = None;
            self.start_defuse = false;
            self.defuse_countdown = 0.0;
        }
    }

    /// `CGrenade::C4Think()` (`ggrenade.cpp` 1403-1591, vanilla).
    fn c4_think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some((origin, velocity, owner, waterlevel)) =
            eng.ent(id).map(|v| (v.origin, v.velocity, v.owner, v.waterlevel))
        else {
            return;
        };
        if !is_in_world(origin, velocity) {
            util_remove(eng, id);
            return;
        }

        // Vanilla (`!REGAMEDLL_FIXES`): 0.12 s think interval.
        schedule(eng, id, 0.12);
        let now = now_f32(eng);

        if now >= self.next_freq {
            self.next_freq = now + self.next_freq_interval;
            self.next_freq_interval *= 0.9;
            match self.cur_wave {
                0 => {
                    self.beep_name = Some("weapons/c4_beep1.wav");
                    self.attenu = 1.5;
                }
                1 => {
                    self.beep_name = Some("weapons/c4_beep2.wav");
                    self.attenu = 1.0;
                }
                2 => {
                    self.beep_name = Some("weapons/c4_beep3.wav");
                    self.attenu = 0.8;
                }
                3 => {
                    self.beep_name = Some("weapons/c4_beep4.wav");
                    self.attenu = 0.5;
                }
                4 => {
                    self.beep_name = Some("weapons/c4_beep5.wav");
                    self.attenu = 0.2;
                }
                _ => {}
            }
            self.cur_wave += 1;
        }

        if now >= self.next_beep {
            self.next_beep = now + 1.4;
            if let Some(sample) = self.beep_name {
                eng.emit_sound(id, CHAN_VOICE, sample, 1.0, self.attenu, PITCH_NORM);
            }
        }

        if now >= self.next_blink {
            self.next_blink = now + 2.0;
            let glow = sprite(eng, "sprites/ledglow.spr");
            let te = tempent::glow_sprite(origin + Vec3::new(0.0, 0.0, 5.0), glow, 1, 3, 255);
            eng.send_temp_entity(MsgDest::Pvs(origin), &te);
        }

        // Vanilla (`!REGAMEDLL_FIXES`): ignores ongoing defusal; fires target before `Detonate2`,
        // and silently deletes the bomb if underwater.
        if now >= self.c4_blow {
            eng.send_usermsg(
                MsgDest::All,
                &CsUserMsg::Scenario {
                    active: false,
                    sprite: String::new(),
                    alpha: 0,
                    flash: None,
                },
            );

            if let Some(target) = self.cur_bomb_target {
                use_entity(g, eng, target, owner, Some(id), UseType::Toggle, 0.0);
            }

            if let Some(owner_slot) = owner.and_then(|o| player_slot(g, eng, o))
                && let Some(owner_player) = g.players[owner_slot].as_mut()
            {
                owner_player.frags += 3;
            }

            eng.send_usermsg(MsgDest::All, &CsUserMsg::BombPickup);
            g.rules.bomb_dropped = false;

            if waterlevel != 0 {
                util_remove(eng, id);
            } else {
                self.set_think(Think::Detonate2);
            }
        }

        if self.start_defuse
            && let Some(defuser_slot) = self.bomb_defuser
        {
            if g.players
                .get(usize::from(defuser_slot))
                .is_none_or(Option::is_none)
            {
                self.start_defuse = false;
                self.bomb_defuser = None;
                self.defuse_countdown = 0.0;
                return;
            }
            let player_edict = eng.client_edict(defuser_slot);
            if now < self.defuse_countdown {
                let on_ground =
                    eng.ent(player_edict).is_some_and(|v| v.flags & FL_ONGROUND != 0);
                if now > self.next_defuse || !on_ground {
                    self.defuse_bomb_end(g, eng, id, defuser_slot, false);
                }
            } else {
                self.defuse_bomb_end(g, eng, id, defuser_slot, true);
            }
        }
    }

    /// `CGrenade::Detonate3`: the HE grenade goes off (`Explode3(&tr, DMG_EXPLOSION)`).
    fn detonate3(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if let Some(tr) = Self::trace_down(eng, id) {
            self.explode3(g, eng, id, &tr, dmg::EXPLOSION);
        }
    }

    /// `CGrenade::SG_Detonate` (`ggrenade.cpp` 578-631): the smoke grenade lies still. The
    /// `sg_explode` sound, then a look at the other grenades: one that is already past its fuse
    /// and within 250 units makes this cloud a light one. The first event (the big puff of the
    /// cloud's start), and the grenade is thrown up a little (random sideways, 250 to 350 up)
    /// for the smoke to rise from; the puffs follow from [`Grenade::sg_smoke`] every tenth of a
    /// second. QUIRK: the trace the source makes first is never used.
    fn sg_detonate(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(origin) = eng.ent(id).map(|v| v.origin) else {
            return;
        };
        eng.emit_sound(id, CHAN_WEAPON, "weapons/sg_explode.wav", 1.0, ATTN_NORM, PITCH_NORM);
        let now = now_f32(eng);
        let mut find = None;
        while let Some(other) = eng.find_by_classname(find, "grenade") {
            find = Some(other);
            if let Some(v) = eng.ent(other) {
                let distance = (v.origin - origin).length();
                if distance != 0.0 && distance <= 250.0 && now > v.dmgtime {
                    self.light_smoke = true;
                }
            }
        }
        self.detonated = true;
        self.smoke_detonate = origin;
        // `PLAYBACK_EVENT_FULL(0, nullptr, m_usEvent, 0, m_vSmokeDetonate, g_vecZero, 0, 0, 0,
        // 1, m_bLightSmoke, FALSE)`.
        eng.playback_event(
            0,
            None,
            self.event,
            0.0,
            self.smoke_detonate,
            Vec3::ZERO,
            0.0,
            0.0,
            0,
            1,
            self.light_smoke,
            false,
        );
        let vx = random_float(g, -175.0, 175.0);
        let vy = random_float(g, -175.0, 175.0);
        let vz = random_float(g, 250.0, 350.0);
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::new(vx, vy, vz);
        }
        schedule(eng, id, 0.1);
        self.set_think(Think::SgSmoke);
    }

    /// `CGrenade::SG_Smoke` (`ggrenade.cpp` 493-541): one puff of the smoke grenade's cloud. In
    /// the water bubbles instead. A puff is an event at the grenade's origin that carries the
    /// place of the detonation as its angles and the direction of the puff (a vector of 3 to 8
    /// units along the grenade's forward vector, turned 30 degrees more each time) as
    /// `fparam1` and `fparam2`; `flSmokeInterval` (150 to 350) is `iparam1`, 4 is `iparam2`.
    /// 21 puffs a second apart, then the grenade is gone. Assumptions: `Q_cos` and `Q_sin` take
    /// a double (computed in double and stored in a float); the last argument of the event,
    /// `6`, is a `bparam2` that the event's delta encodes in one bit, so it reads 0 on the wire,
    /// and is sent as `false`.
    fn sg_smoke(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        const MAX_SMOKE_PUFFS: i32 = 100;
        let Some((origin, angles)) = eng.ent(id).map(|v| (v.origin, v.angles)) else {
            return;
        };
        if eng.point_contents_ents(origin) == CONTENTS_WATER {
            bubbles(eng, origin - Vec3::new(64.0, 64.0, 64.0), origin + Vec3::new(64.0, 64.0, 64.0), 100);
        } else {
            let (forward, _, _) = angle_vectors(angles);
            let along = forward * random_float(g, 3.0, 8.0);
            let smoke_interval = random_float(g, 1.5, 3.5) * MAX_SMOKE_PUFFS as f32;
            // `R_angle = m_angle / (180.00433335 / M_PI)`: a double quotient, kept in a float.
            let r_angle = (f64::from(self.angle) / (180.004_333_35 / std::f64::consts::PI)) as f32;
            let x_old = f64::from(r_angle).cos() as f32;
            let y_old = f64::from(r_angle).sin() as f32;
            let angle_x = along.x * x_old - along.y * y_old;
            let angle_y = along.x * y_old + along.y * x_old;
            self.angle = (self.angle + 30) % 360;
            eng.playback_event(
                0,
                None,
                self.event,
                0.0,
                origin,
                self.smoke_detonate,
                angle_x,
                angle_y,
                smoke_interval as i32,
                4,
                self.light_smoke,
                false,
            );
        }
        if self.sg_smoke <= 20 {
            schedule(eng, id, 1.0);
            self.set_think(Think::SgSmoke);
            self.sg_smoke += 1;
        } else {
            if let Some(v) = eng.ent_mut(id) {
                v.effects |= EF_NODRAW;
            }
            eng.remove_entity(id);
        }
    }

    /// `CGrenade::TumbleThink` (`ggrenade.cpp` 790-832), the flashbang's and the HE grenade's
    /// think in the air: out of the world it is removed; every tenth of a second; when the
    /// fuse is spent the next think is the explosion (damage 40 or less: the flashbang's,
    /// else the HE grenade's); in the water it slows by half and its model animates slowly.
    fn tumble_think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(v) = eng.ent(id) else {
            return;
        };
        if !is_in_world(v.origin, v.velocity) {
            eng.remove_entity(id);
            return;
        }
        // `StudioFrameAdvance()`: not ported (see the module notes).
        schedule(eng, id, 0.1);
        let now = now_f32(eng);
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        if v.dmgtime <= now {
            self.think = if v.dmg <= 40.0 { Think::Detonate } else { Think::Detonate3 };
        }
        if v.waterlevel != 0 {
            v.velocity *= 0.5;
            v.framerate = 0.2;
        }
    }

    /// `CGrenade::SG_TumbleThink` (`ggrenade.cpp` 834-877): the smoke grenade's. On the ground
    /// it slows by 5% a tenth of a second; once its fuse is spent it goes off, but only when it
    /// lies on the ground.
    fn sg_tumble_think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        if !is_in_world(v.origin, v.velocity) {
            eng.remove_entity(id);
            return;
        }
        if v.flags & FL_ONGROUND != 0 {
            v.velocity *= 0.95;
        }
        // `StudioFrameAdvance()`: not ported (see the module notes).
        schedule(eng, id, 0.1);
        let now = now_f32(eng);
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        if v.dmgtime <= now && v.flags & FL_ONGROUND != 0 {
            self.think = Think::SgDetonate;
        }
        if v.waterlevel != 0 {
            v.velocity *= 0.5;
            v.framerate = 0.2;
        }
    }

    /// `CGrenade::BounceSound` (`ggrenade.cpp` 774): the HE grenade's (damage over 50) thud, or
    /// one of three hits for the others.
    fn bounce_sound(g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(damage) = eng.ent(id).map(|v| v.dmg) else {
            return;
        };
        if damage > 50.0 {
            eng.emit_sound(id, CHAN_VOICE, "weapons/he_bounce-1.wav", 0.25, ATTN_NORM, PITCH_NORM);
            return;
        }
        let sample = match g.random_range(0, 2) {
            0 => "weapons/grenade_hit1.wav",
            1 => "weapons/grenade_hit2.wav",
            _ => "weapons/grenade_hit3.wav",
        };
        eng.emit_sound(id, CHAN_VOICE, sample, 0.25, ATTN_NORM, PITCH_NORM);
    }

    /// `CGrenade::BounceTouch` (`ggrenade.cpp` 686-755, the vanilla branch). The grenade ignores
    /// the one that threw it; a see-through `func_breakable` throws it back at twice the speed;
    /// on the ground it loses a fifth of its speed to static friction; in the air it makes a
    /// sound for each of the first five bounces, and after ten it lies still. Its model animates
    /// by its speed: full rate at 200 units a second or more, none below 100. QUIRKS kept:
    /// `pev->sequence = RANDOM_LONG(1, 1)` draws a number; the sound list's danger entry
    /// (`m_fRegisteredSound`) is not ported.
    pub fn bounce_touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        let Some(v) = eng.ent(id) else {
            return;
        };
        // Don't hit the guy that launched this grenade.
        if v.owner == Some(other) {
            return;
        }
        if let Some(o) = eng.ent(other)
            && o.classname == "func_breakable"
            && o.rendermode != RENDER_NORMAL
        {
            if let Some(v) = eng.ent_mut(id) {
                v.velocity *= -2.0;
            }
            return;
        }
        if eng.ent(id).is_some_and(|v| v.flags & FL_ONGROUND != 0) {
            // Add a bit of static friction.
            let sequence = g.random_range(1, 1);
            if let Some(v) = eng.ent_mut(id) {
                v.velocity *= 0.8;
                v.sequence = sequence as u16;
            }
        } else {
            if self.bounce_count < 5 {
                // Play bounce sound.
                Self::bounce_sound(g, eng, id);
            }
            if self.bounce_count >= 10 {
                let world = eng.world();
                if let Some(v) = eng.ent_mut(id) {
                    v.groundentity = Some(world);
                    v.flags |= FL_ONGROUND;
                    v.velocity = Vec3::ZERO;
                }
            }
            self.bounce_count += 1;
        }
        if let Some(v) = eng.ent_mut(id) {
            v.framerate = v.velocity.length() / 200.0;
            if v.framerate > 1.0 {
                v.framerate = 1.0;
            } else if v.framerate < 0.5 {
                v.framerate = 0.0;
            }
        }
    }

    /// `CGrenade::SlideTouch` (`ggrenade.cpp` 757-772): the C4's touch. Ignores the thrower; on
    /// the ground a little static friction (5%), in the air a bounce sound.
    pub fn slide_touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        if v.owner == Some(other) {
            return;
        }
        if v.flags & FL_ONGROUND != 0 {
            v.velocity *= 0.95;
        } else {
            Self::bounce_sound(g, eng, id);
        }
    }
}

impl GameEntity for Grenade {
    fn classname(&self) -> &'static str {
        "grenade"
    }

    fn object_caps(&self) -> i32 {
        if self.is_c4 { FCAP_CONTINUOUS_USE } else { 0 }
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        _id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        if !self.is_c4 {
            return;
        }
        let Some(slot) = activator.and_then(|a| player_slot(g, eng, a)) else {
            return;
        };
        if g.players[slot].as_ref().is_none_or(|p| p.team != Team::Ct) {
            return;
        }
        if self.start_defuse {
            // Vanilla (`!REGAMEDLL_FIXES`): any CT using the bomb refreshes `next_defuse`.
            self.next_defuse = now_f32(eng) + NEXT_DEFUSE_TIME;
            return;
        }
        self.defuse_bomb_start(g, eng, slot as u8);
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        match self.think {
            Think::None => {}
            Think::Tumble => self.tumble_think(eng, id),
            Think::SgTumble => self.sg_tumble_think(eng, id),
            Think::Detonate => self.detonate(g, eng, id),
            Think::Detonate2 => self.detonate2(g, eng, id),
            Think::Detonate3 => self.detonate3(g, eng, id),
            Think::SgDetonate => self.sg_detonate(g, eng, id),
            Think::SgSmoke => self.sg_smoke(g, eng, id),
            Think::Smoke => Self::smoke_once(eng, id, 25, 6),
            Think::Smoke2 => Self::smoke_once(eng, id, 150, 8),
            Think::Smoke3C => Self::smoke3_c(g, eng, id),
            Think::Smoke3B => self.smoke3_ab(g, eng, id, true),
            Think::Smoke3A => self.smoke3_ab(g, eng, id, false),
            Think::C4 => self.c4_think(g, eng, id),
        }
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        match self.touch {
            Touch::None => {}
            Touch::Bounce => self.bounce_touch(g, eng, id, other),
            Touch::Slide => self.slide_touch(g, eng, id, other),
        }
    }
}

// ---- making a grenade ----

/// `CBasePlayer::ThrowGrenade(pWeapon, vecSrc, vecThrow, time, usEvent)` (`player.cpp` 3538):
/// the grenade of weapon `id` thrown by the player of `slot`; `None` for any other weapon.
/// `event` is the event of the explosion (HE) or of the smoke, found by name here.
pub(crate) fn throw_grenade(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    id: WeaponId,
    src: Vec3,
    velocity: Vec3,
    time: f32,
) -> Option<EdictId> {
    let owner = eng.client_edict(slot);
    match id {
        WeaponId::HeGrenade => {
            let team = g.players.get(usize::from(slot))?.as_ref().map_or(0, |p| i32::from(p.team.id()));
            let event = eng.precache_event("events/createexplo.sc");
            shoot_timed2(g, eng, owner, src, velocity, time, team, event)
        }
        WeaponId::Flashbang => shoot_timed(g, eng, owner, src, velocity, time),
        WeaponId::SmokeGrenade => {
            let event = eng.precache_event("events/createsmoke.sc");
            shoot_smoke_grenade(g, eng, owner, src, velocity, time, event)
        }
        _ => None,
    }
}

/// `CGrenade::Spawn` and the lines `ShootTimed2`, `ShootTimed` and `ShootSmokeGrenade` have in
/// common: a `grenade` entity that bounces, 30 damage until the caller says otherwise, at
/// `src`, going `velocity`, owned by `owner` and facing the way its owner's angles do.
fn make_grenade(
    eng: &mut EngineApi<'_>,
    owner: EdictId,
    src: Vec3,
    velocity: Vec3,
) -> Option<EdictId> {
    let id = eng.create_entity("grenade")?;
    // `Spawn`.
    if let Some(v) = eng.ent_mut(id) {
        v.movetype = MOVETYPE_BOUNCE;
        v.solid = SOLID_BBOX;
        v.dmg = 30.0;
    }
    eng.set_model(id, "models/grenade.mdl");
    eng.set_size(id, Vec3::ZERO, Vec3::ZERO);
    eng.set_origin(id, src);
    let angles = eng.ent(owner).map_or(Vec3::ZERO, |v| v.angles);
    if let Some(v) = eng.ent_mut(id) {
        v.velocity = velocity;
        v.angles = angles;
        v.owner = Some(owner);
    }
    Some(id)
}

/// The fuse and the look every thrown grenade gets: `dmgtime`, the first think a tenth of a
/// second away, a random one of the four tumbling sequences (3 to 6), the model's frame rate.
fn arm_fuse(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    id: EdictId,
    time: f32,
    stop_at_zero: bool,
) {
    let now = now_f32(eng);
    if let Some(v) = eng.ent_mut(id) {
        v.dmgtime = now + time;
    }
    schedule(eng, id, 0.1);
    if stop_at_zero && time < 0.1 {
        schedule(eng, id, 0.0);
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
        }
    }
    let sequence = g.random_range(3, 6);
    if let Some(v) = eng.ent_mut(id) {
        v.sequence = sequence as u16;
        v.framerate = 1.0;
    }
}

/// `CGrenade::ShootTimed2(pevOwner, vecStart, vecVelocity, time, iTeam, usEvent)`
/// (`ggrenade.cpp` 926-968): the HE grenade. QUIRK: unlike the other two there is no
/// `time < 0.1` shortcut.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shoot_timed2(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    owner: EdictId,
    src: Vec3,
    velocity: Vec3,
    time: f32,
    team: i32,
    event: u16,
) -> Option<EdictId> {
    let id = make_grenade(eng, owner, src, velocity)?;
    arm_fuse(g, eng, id, time, false);
    if let Some(v) = eng.ent_mut(id) {
        v.gravity = 0.55;
        v.friction = 0.7;
    }
    eng.set_model(id, "models/w_hegrenade.mdl");
    if let Some(v) = eng.ent_mut(id) {
        v.dmg = 100.0;
    }
    attach(
        eng,
        id,
        Box::new(Grenade {
            think: Think::Tumble,
            touch: Touch::Bounce,
            event,
            team,
            ..Grenade::default()
        }),
    );
    Some(id)
}

/// `CGrenade::ShootTimed(pevOwner, vecStart, vecVelocity, time)` (`ggrenade.cpp` 972-1023):
/// the flashbang.
pub(crate) fn shoot_timed(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    owner: EdictId,
    src: Vec3,
    velocity: Vec3,
    time: f32,
) -> Option<EdictId> {
    let id = make_grenade(eng, owner, src, velocity)?;
    arm_fuse(g, eng, id, time, true);
    if let Some(v) = eng.ent_mut(id) {
        v.gravity = 0.5;
        v.friction = 0.8;
    }
    eng.set_model(id, "models/w_flashbang.mdl");
    if let Some(v) = eng.ent_mut(id) {
        v.dmg = 35.0;
    }
    attach(
        eng,
        id,
        Box::new(Grenade { think: Think::Tumble, touch: Touch::Bounce, ..Grenade::default() }),
    );
    Some(id)
}

/// `CGrenade::ShootSmokeGrenade(pevOwner, vecStart, vecVelocity, time, usEvent)`
/// (`ggrenade.cpp` 1338-1384): the smoke grenade.
pub(crate) fn shoot_smoke_grenade(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    owner: EdictId,
    src: Vec3,
    velocity: Vec3,
    time: f32,
    event: u16,
) -> Option<EdictId> {
    let id = make_grenade(eng, owner, src, velocity)?;
    arm_fuse(g, eng, id, time, true);
    if let Some(v) = eng.ent_mut(id) {
        v.gravity = 0.5;
        v.friction = 0.8;
    }
    eng.set_model(id, "models/w_smokegrenade.mdl");
    if let Some(v) = eng.ent_mut(id) {
        v.dmg = 35.0;
    }
    attach(
        eng,
        id,
        Box::new(Grenade {
            think: Think::SgTumble,
            touch: Touch::Bounce,
            event,
            light_smoke: false,
            detonated: false,
            sg_smoke: 0,
            ..Grenade::default()
        }),
    );
    Some(id)
}

/// `CGrenade::ShootSatchelCharge(pevOwner, vecStart, vecAngles)` (`ggrenade.cpp` 1268-1334):
/// spawns the planted C4 as a `"grenade"` entity using `models/w_c4.mdl` with a fuse of
/// `CSGameRules()->m_iC4Timer`.
pub(crate) fn shoot_satchel_charge(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    owner: EdictId,
    start: Vec3,
    angles: Vec3,
) -> Option<EdictId> {
    let id = eng.create_entity("grenade")?;
    let c4_timer = g.rules.c4_timer;
    let now = now_f32(eng);
    let cur_bomb_target = player_slot(g, eng, owner)
        .and_then(|s| g.players[s].as_ref())
        .and_then(|p| p.cur_bomb_target);
    if let Some(v) = eng.ent_mut(id) {
        v.movetype = MOVETYPE_TOSS;
        v.solid = SOLID_BBOX;
        v.dmg = 100.0;
        v.velocity = Vec3::ZERO;
        v.angles = angles;
        v.owner = Some(owner);
        v.spawnflags = SF_DETONATE;
        v.friction = 0.9;
    }
    eng.set_model(id, "models/w_c4.mdl");
    eng.set_size(id, Vec3::new(-3.0, -6.0, 0.0), Vec3::new(3.0, 6.0, 8.0));
    eng.set_origin(id, start);
    schedule(eng, id, 0.1);
    attach(
        eng,
        id,
        Box::new(Grenade {
            think: Think::C4,
            touch: Touch::None,
            c4_blow: now + c4_timer as f32,
            next_freq_interval: (c4_timer / 4) as f32,
            next_freq: now,
            cur_wave: 0,
            attenu: 0.0,
            beep_name: None,
            next_beep: now + 0.5,
            is_c4: true,
            next_defuse: 0.0,
            start_defuse: false,
            next_blink: now + 2.0,
            just_blew: false,
            cur_bomb_target,
            ..Grenade::default()
        }),
    );
    Some(id)
}

/// `UTIL_ScreenShake(center, amplitude, frequency, duration, radius)` (`util.cpp` 497-539):
/// shakes every player standing on the ground within `radius` (or all grounded players if
/// `radius <= 0`).
fn screen_shake(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    center: Vec3,
    amplitude: f32,
    frequency: f32,
    duration: f32,
    radius: f32,
) {
    let fixed =
        |val: f32, scale: i32| (f64::from(val) * f64::from(scale)).clamp(0.0, 65535.0) as u16;
    let dur = fixed(duration, 1 << 12);
    let freq = fixed(frequency, 1 << 8);
    for (slot, slot_entry) in g.players.iter().enumerate() {
        if slot_entry.is_none() {
            continue;
        }
        let Some(v) = eng.ent(eng.client_edict(slot as u8)) else {
            continue;
        };
        if v.flags & FL_ONGROUND == 0 {
            continue;
        }
        let local_amp = if radius > 0.0 {
            if (center - v.origin).length() < radius { amplitude } else { 0.0 }
        } else {
            amplitude
        };
        if local_amp != 0.0 {
            eng.send_usermsg(
                MsgDest::One(slot as u8),
                &CsUserMsg::ScreenShake {
                    amplitude: fixed(local_amp, 1 << 12),
                    duration: dur,
                    frequency: freq,
                },
            );
        }
    }
}

/// `CBaseMonster::RadiusDamage(pevInflictor, pevAttacker, flDamage, iClassIgnore,
/// bitsDamageType)` (`basemonster.cpp` 553-559): if `damage > 80`, runs [`radius_damage`] with
/// radius `damage * 3.5`; otherwise runs [`radius_damage2`] with radius `damage * (RANDOM_FLOAT(0.5, 1.5) + 3)`.
#[allow(clippy::too_many_arguments)]
fn monster_radius_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    origin: Vec3,
    inflictor: EdictId,
    attacker: Option<EdictId>,
    damage: f32,
    bits: u32,
) {
    if damage > 80.0 {
        let radius = (f64::from(damage) * 3.5) as f32;
        radius_damage(g, eng, origin, Some(inflictor), None, attacker, damage, radius, bits);
    } else {
        let mult = random_float(g, 0.5, 1.5) + 3.0;
        let radius = (f64::from(damage) * f64::from(mult)) as f32;
        radius_damage2(g, eng, origin, Some(inflictor), attacker, damage, radius, bits);
    }
}

// ---- combat.cpp ----

/// `CBaseMonster::RadiusDamage(pevInflictor, pevAttacker, flDamage, iClassIgnore,
/// bitsDamageType)` (`basemonster.cpp` 553, read 2026-11-22) as the grenade calls it from
/// `origin`: a damage over 80 is [`radius_damage`] with the radius `flDamage * 3.5` (a
/// `double` product). The other branch (`RadiusDamage2`) is not ported: no grenade has it.
#[allow(clippy::too_many_arguments)]
fn grenade_radius_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    origin: Vec3,
    inflictor: EdictId,
    attacker: Option<EdictId>,
    damage: f32,
    team: i32,
    bits: u32,
) {
    let radius = (f64::from(damage) * 3.5) as f32;
    radius_damage(g, eng, origin, Some(inflictor), Some(team), attacker, damage, radius, bits);
}

/// `RadiusDamage(vecSrc, pevInflictor, pevAttacker, flDamage, flRadius, iClassIgnore,
/// bitsDamageType)` (`combat.cpp` 243-335, read 2026-11-22), `iClassIgnore` being `CLASS_NONE`:
/// every entity with `takedamage` within `radius` of `src` (one unit up, "in case grenade is
/// lying on the ground"), a player or any other class (a hostage, a breakable), takes
/// `damage - distance * (damage / radius)` of damage (`0` at the farthest, never negative: the
/// vanilla build hurts with 0), `distance` from `src` to the entity's origin, through walls. A
/// blast does not travel into or out of the water: a blast in the water hurts only entities
/// with their feet in it, one out of it only those whose head is out (`waterlevel` 3 is
/// "under"). The Condition Zero line-of-sight variant (`AreRunningCZero`) is not ported. A
/// missing attacker is the inflictor.
#[allow(clippy::too_many_arguments)]
pub(crate) fn radius_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    src: Vec3,
    inflictor: Option<EdictId>,
    inflictor_team: Option<i32>,
    attacker: Option<EdictId>,
    damage: f32,
    radius: f32,
    bits: u32,
) {
    let falloff = if radius != 0.0 { damage / radius } else { 1.0 };
    let in_water = eng.point_contents_ents(src) == CONTENTS_WATER;
    // In case grenade is lying on the ground.
    let src = Vec3::new(src.x, src.y, src.z + 1.0);
    let attacker = attacker.or(inflictor);
    // Iterate on all entities in the vicinity.
    for entity in eng.entities_in_sphere(src, radius) {
        let slot = player_slot(g, eng, entity);
        let takes_damage = match slot {
            Some(s) => g.players[s].as_ref().is_some_and(|p| p.takes_damage),
            None => crate::entity::entity_takes_damage(eng, entity),
        };
        if !takes_damage {
            continue;
        }
        let Some(v) = eng.ent(entity) else {
            continue;
        };
        // Blasts don't travel into or out of water.
        if in_water && v.waterlevel == 0 {
            continue;
        }
        if !in_water && v.waterlevel == 3 {
            continue;
        }
        let length = (src - v.origin).length();
        let mut adjusted = damage - length * falloff;
        if adjusted < 0.0 {
            adjusted = 0.0;
        }
        match slot {
            Some(s) => {
                take_damage_from_grenade(g, eng, s as u8, inflictor, inflictor_team, attacker, adjusted, bits);
            }
            None => {
                crate::entity::take_damage_of(g, eng, entity, inflictor, attacker, adjusted, bits);
            }
        }
    }
}

/// `RadiusDamage2(vecSrc, pevInflictor, pevAttacker, flDamage, flRadius, iClassIgnore,
/// bitsDamageType)` (`combat.cpp` 337-402, read 2026-11-22), `iClassIgnore` being `CLASS_NONE`
/// (the blast the planted C4 uses): as [`radius_damage`], but an entity is hurt only if a line
/// from the blast to its `BodyTarget` (passing the inflictor) reaches the point or ends on the
/// entity, and the damage is capped at 75. A line that reaches the point hurts through
/// `TakeDamage`; one that ends on the entity hurts through `TraceAttack` and `ApplyMultiDamage`,
/// so the entity's own hit handling (a player's hit group, its armour) sees the shot. A start
/// inside a solid counts as a fraction of 0 at the blast's position. A missing attacker is the
/// inflictor. A player's `BodyTarget` draws `RANDOM_FLOAT(0.5, 1.1)` once per player.
#[allow(clippy::too_many_arguments)]
pub fn radius_damage2(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    src: Vec3,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
    damage: f32,
    radius: f32,
    bits: u32,
) {
    let falloff = if radius != 0.0 { damage / radius } else { 1.0 };
    let in_water = eng.point_contents_ents(src) == CONTENTS_WATER;
    // In case grenade is lying on the ground.
    let src = Vec3::new(src.x, src.y, src.z + 1.0);
    let attacker = attacker.or(inflictor);
    for entity in eng.entities_in_sphere(src, radius) {
        let slot = player_slot(g, eng, entity);
        let takes_damage = match slot {
            Some(s) => g.players[s].as_ref().is_some_and(|p| p.takes_damage),
            None => crate::entity::entity_takes_damage(eng, entity),
        };
        if !takes_damage {
            continue;
        }
        let Some(v) = eng.ent(entity) else {
            continue;
        };
        // Blasts don't travel into or out of water.
        if in_water && v.waterlevel == 0 {
            continue;
        }
        if !in_water && v.waterlevel == 3 {
            continue;
        }
        let origin = v.origin;
        let spot = crate::entity::body_target_of(g, eng, entity, src);
        let mut tr = eng.trace_line(src, spot, MoveFilter::Normal, inflictor);
        if tr.trace.fraction == 1.0 || tr.ent == Some(entity) {
            if tr.trace.start_solid {
                tr.trace.end_pos = src;
                tr.trace.fraction = 0.0;
            }
            let mut adjusted = damage - (src - origin).length() * falloff;
            // `< 0` gives 0, `> 75` gives 75 (a NaN passes both tests, as it does `clamp`).
            adjusted = adjusted.clamp(0.0, 75.0);
            if tr.trace.fraction == 1.0 {
                crate::entity::take_damage_of(g, eng, entity, inflictor, attacker, adjusted, bits);
            } else {
                // `ClearMultiDamage(); pEntity->TraceAttack(pevInflictor, ...)`.
                let mut mm = crate::bullets::MultiDamage::new(None);
                mm.clear();
                let dir = crate::bullets::normalize_reciprocal(tr.trace.end_pos - src);
                crate::bullets::trace_attack(g, eng, entity, inflictor, &mut mm, adjusted, dir, &tr, bits);
                crate::bullets::apply_multi_damage(g, eng, &mm, inflictor, attacker);
            }
        }
    }
}

/// `PlayerBlind(pPlayer, pevInflictor, pevAttacker, fadeTime, fadeHold, alpha, color)`
/// (`combat.cpp` 3-40): the white `ScreenFade` to the player, and to the observers that watch it
/// in first person (unless `mp_fadetoblack` is 1), and the player remembers the flash for a third
/// of its fade ([`crate::player::Player::blind`]).
pub(crate) fn player_blind(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    fade_time: f32,
    fade_hold: f32,
    alpha: i32,
) {
    const WHITE: [u8; 3] = [255, 255, 255];
    let alpha_byte = alpha.clamp(0, 255) as u8;
    screen_fade_color(eng, slot, WHITE, fade_time, fade_hold, alpha_byte, FFADE_IN);
    if eng.cvar_f32("mp_fadetoblack") != FADETOBLACK_STAY as f32 {
        for observer in crate::hud_misc::followers(g, eng, slot) {
            screen_fade_color(eng, observer, WHITE, fade_time, fade_hold, alpha_byte, FFADE_IN);
        }
    }
    // `float flDurationTime = fadeTime * 0.33` (a `double` product).
    let duration = (f64::from(fade_time) * 0.33) as f32;
    let now = now_f32(eng);
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.blind(now, duration, fade_hold, fade_time, alpha);
    }
}

/// `RadiusFlash(vecSrc, pevInflictor, pevAttacker, flDamage, iClassIgnore, bitsDamageType)`
/// (`combat.cpp` 50-168, read 2026-11-22): every living player within 1500 units of `src`
/// (one up) with `takedamage` that is in the same medium (water rule as [`radius_damage`]) and
/// that a line from the flash sees is blinded.
///
/// "Sees": a trace from the flash to a point of the player's body (`BodyTarget`: its centre
/// plus `view_ofs * RANDOM_FLOAT(0.5, 1.1)`, vanilla; `REGAMEDLL_FIXES` uses the eyes) stops
/// at the player or reaches the point, and a trace back, ignoring the entity the first one hit,
/// reaches the flash. The player's distance is then that of the first trace's end, and the
/// "damage" is `flDamage - distance * (flDamage / 1500)` (4 at the flash, 0 at 1500). A player
/// turned away (the dot product of the vector from its ears to the flash with its forward
/// vector, from `v_angle`, is negative) gets alpha 200, a fade of 1.75 and a hold of 1/3.5
/// of the damage; one that looks at it alpha 255, a fade of 3 and a hold of 1/1.5. A player
/// still blind keeps the longer of the two holds (a facing flash adds what is left of the
/// hold), fade and alpha: the new flash cannot make the flash a player already has lighter.
#[allow(clippy::too_many_arguments)]
pub(crate) fn radius_flash(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    src: Vec3,
    inflictor: EdictId,
    _attacker: Option<EdictId>,
    damage: f32,
    _bits: u32,
) {
    let radius = FLASH_RADIUS;
    let falloff = if radius != 0.0 { damage / radius } else { 1.0 };
    let in_water = eng.point_contents_ents(src) == CONTENTS_WATER;
    let src = Vec3::new(src.x, src.y, src.z + 1.0);
    for entity in eng.entities_in_sphere(src, radius) {
        let Some(slot) = player_slot(g, eng, entity) else {
            continue;
        };
        if !g.players[slot].as_ref().is_some_and(|p| p.takes_damage) {
            continue;
        }
        let Some(v) = eng.ent(entity) else {
            continue;
        };
        if v.deadflag != DEAD_NO {
            continue;
        }
        if in_water && v.waterlevel == 0 {
            continue;
        }
        if !in_water && v.waterlevel == 3 {
            continue;
        }
        let (v_angle, view_ofs, origin) = (v.v_angle, v.view_ofs, v.origin);
        // `BodyTarget`: `Center() + pev->view_ofs * RANDOM_FLOAT(0.5, 1.1)`.
        let center = (v.absmax + v.absmin) * 0.5;
        let spot = center + view_ofs * random_float(g, 0.5, 1.1);

        let mut tr = eng.trace_line(src, spot, MoveFilter::Normal, Some(inflictor));
        if tr.trace.fraction != 1.0 && tr.ent != Some(entity) {
            continue;
        }
        let pass = tr.ent.unwrap_or_else(|| eng.world());
        let tr2 = eng.trace_line(spot, src, MoveFilter::Normal, Some(pass));
        if tr2.trace.fraction < 1.0 {
            continue;
        }
        if tr.trace.start_solid {
            tr.trace.end_pos = src;
            tr.trace.fraction = 0.0;
        }
        let mut adjusted = damage - (src - tr.trace.end_pos).length() * falloff;
        if adjusted < 0.0 {
            adjusted = 0.0;
        }
        let (forward, _, _) = angle_vectors(v_angle);
        let los = src - (origin + view_ofs);
        let dot = los.dot(forward);
        // `fadeTime = flAdjustedDamage * 1.75` and the like are `float * double`.
        let (alpha, mut fade_time, mut fade_hold) = if dot < 0.0 {
            (200, (f64::from(adjusted) * 1.75) as f32, (f64::from(adjusted) / 3.5) as f32)
        } else {
            (255, (f64::from(adjusted) * 3.0) as f32, (f64::from(adjusted) / 1.5) as f32)
        };
        let mut alpha = alpha;
        let now = now_f32(eng);
        let Some(p) = g.players[slot].as_ref() else {
            continue;
        };
        let current_hold = p.blind_start_time + p.blind_hold_time - now;
        if current_hold > 0.0 && alpha == 255 {
            fade_hold += current_hold;
        }
        if p.blind_start_time != 0.0
            && p.blind_fade_time != 0.0
            && p.blind_start_time + p.blind_fade_time + p.blind_hold_time > now
        {
            if p.blind_fade_time > fade_time {
                fade_time = p.blind_fade_time;
            }
            if p.blind_alpha >= alpha {
                alpha = p.blind_alpha;
            }
        }
        player_blind(g, eng, slot as u8, fade_time, fade_hold, alpha);
    }
}

// ---- explode.cpp ----

/// `CShower` (`spark_shower`, `explode.cpp` 1-60): a shower of sparks an explosion throws, a
/// bouncing invisible thing that makes a `TE_SPARKS` every tenth of a second for 0.5 to 1.5
/// seconds. Made by [`create_spark_shower`].
#[derive(Debug, Clone, Default)]
pub struct SparkShower;

/// `CBaseEntity::Create("spark_shower", origin, normal, nullptr)` and `CShower::Spawn`: the
/// shower flies off the surface along `normal` at 200 to 300 units a second, with 100 of random
/// spread sideways and 200 more up (or down, if it points down), bounces with gravity 0.5 and
/// thinks a tenth of a second later.
pub fn create_spark_shower(g: &mut GameState, eng: &mut EngineApi<'_>, origin: Vec3, normal: Vec3) {
    let Some(id) = eng.create_entity("spark_shower") else {
        return;
    };
    // `Create`: owner, origin and angles, then the spawn.
    let mut velocity = normal * random_float(g, 200.0, 300.0);
    velocity.x += random_float(g, -100.0, 100.0);
    velocity.y += random_float(g, -100.0, 100.0);
    if velocity.z >= 0.0 {
        velocity.z += 200.0;
    } else {
        velocity.z -= 200.0;
    }
    let speed = random_float(g, 0.5, 1.5);
    if let Some(v) = eng.ent_mut(id) {
        v.owner = None;
        v.origin = origin;
        v.velocity = velocity;
        v.movetype = MOVETYPE_BOUNCE;
        v.gravity = 0.5;
        v.solid = SOLID_NOT;
        v.effects |= EF_NODRAW;
        v.speed = speed;
        v.angles = Vec3::ZERO;
        // `DispatchSpawn` gives the box around the origin before the entity is linked.
        v.absmin = origin - Vec3::ONE;
        v.absmax = origin + Vec3::ONE;
    }
    schedule(eng, id, 0.1);
    // Need a model, just use the grenade, we don't draw it anyway.
    eng.set_model(id, "models/grenade.mdl");
    eng.set_size(id, Vec3::ZERO, Vec3::ZERO);
    attach(eng, id, Box::new(SparkShower));
}

impl GameEntity for SparkShower {
    fn classname(&self) -> &'static str {
        "spark_shower"
    }

    /// `CShower::Think`: sparks at the origin, the time left shortens by 0.1; while there is
    /// some it thinks again, else it is gone.
    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(v) = eng.ent(id) else {
            return;
        };
        sparks(eng, v.origin);
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        v.speed -= 0.1;
        v.flags &= !FL_ONGROUND;
        if v.speed > 0.0 {
            schedule(eng, id, 0.1);
        } else {
            eng.remove_entity(id);
        }
    }

    /// `CShower::Touch`: it loses 90% of its speed on the ground, 40% in the air; when it has
    /// all but stopped sideways its time is up.
    fn touch(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, _other: EdictId) {
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        if v.flags & FL_ONGROUND != 0 {
            v.velocity *= 0.1;
        } else {
            v.velocity *= 0.6;
        }
        if (v.velocity.x * v.velocity.x + v.velocity.y * v.velocity.y) < 10.0 {
            v.speed = 0.0;
        }
    }
}

// ---- for the tests ----

/// Entry points for the tests (cargo feature `test-hooks`).
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
impl crate::CsGame {
    /// The `grenade` entities in the world, oldest first.
    pub fn grenades(&self, eng: &EngineApi<'_>) -> Vec<EdictId> {
        let mut found = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "grenade") {
            found.push(id);
            after = Some(id);
        }
        found
    }

    /// `RadiusDamage(src, inflictor, attacker, damage, radius, CLASS_NONE, bits)`, the inflictor
    /// being a grenade of `inflictor_team` (a team id).
    #[allow(clippy::too_many_arguments)]
    pub fn test_radius_damage(
        &mut self,
        eng: &mut EngineApi<'_>,
        src: Vec3,
        inflictor: Option<EdictId>,
        inflictor_team: Option<i32>,
        attacker: Option<EdictId>,
        damage: f32,
        radius: f32,
        bits: u32,
    ) {
        radius_damage(
            &mut self.state,
            eng,
            src,
            inflictor,
            inflictor_team,
            attacker,
            damage,
            radius,
            bits,
        );
    }

    /// `TakeDamage` of the entity `id` (a player or a game entity).
    pub fn test_take_damage(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        inflictor: Option<EdictId>,
        attacker: Option<EdictId>,
        damage: f32,
        bits: u32,
    ) -> bool {
        crate::entity::take_damage_of(&mut self.state, eng, id, inflictor, attacker, damage, bits)
    }

    /// `RadiusFlash(src, inflictor, attacker, damage, CLASS_NONE, DMG_BLAST)`.
    pub fn test_radius_flash(
        &mut self,
        eng: &mut EngineApi<'_>,
        src: Vec3,
        inflictor: EdictId,
        attacker: Option<EdictId>,
        damage: f32,
    ) {
        radius_flash(&mut self.state, eng, src, inflictor, attacker, damage, dmg::BLAST);
    }

    /// The blind fields of the player of `slot`:
    /// `(m_blindUntilTime, m_blindStartTime, m_blindHoldTime, m_blindFadeTime, m_blindAlpha)`.
    pub fn blind_state(&self, slot: u8) -> Option<(f32, f32, f32, f32, i32)> {
        let p = self.state.players.get(usize::from(slot))?.as_ref()?;
        Some((p.blind_until_time, p.blind_start_time, p.blind_hold_time, p.blind_fade_time, p.blind_alpha))
    }

    /// Sets the blind fields of the player of `slot` as a flash did at `now`.
    pub fn set_blind(&mut self, slot: u8, now: f32, duration: f32, hold: f32, fade: f32, alpha: i32) {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.blind(now, duration, hold, fade, alpha);
        }
    }

    /// `CGrenade::ShootTimed(owner, src, velocity, time)`: a flashbang, true when it was made.
    pub fn test_shoot_flashbang(
        &mut self,
        eng: &mut EngineApi<'_>,
        owner: EdictId,
        src: Vec3,
        velocity: Vec3,
        time: f32,
    ) -> bool {
        shoot_timed(&mut self.state, eng, owner, src, velocity, time).is_some()
    }

    /// `CGrenade::BounceTouch` on `grenade` touched by `other`.
    pub fn test_bounce_touch(&mut self, eng: &mut EngineApi<'_>, grenade: EdictId, other: EdictId) {
        crate::entity::with_entity(&mut self.state, eng, grenade, |entity, g, eng| {
            entity.touch(g, eng, grenade, other);
        });
    }
}
