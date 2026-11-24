//! Environment, sprite, beam, explosion, fog, weather, lighting, and sound entities
//! ported from ReGameDLL_CS (`effects.cpp`, `lights.cpp`, `sound.cpp`, `globals.cpp`).

use cs_engine::consts::{
    MOVETYPE_NONE, SOLID_BBOX, SOLID_NOT,
};
use cs_engine::{EdictId, EngineApi, MoveFilter, MsgDest};
use cs_proto48::messages::SND_FL_STOP;
use cs_proto48::tempent::{self, BeamParams, explflag};
use glam::Vec3;

use crate::GameState;
use crate::entity::{GameEntity, UseType};
use crate::grenade::radius_damage;
use crate::menus::atoi;
use crate::messages;
use crate::player::{ATTN_NORM, EF_NODRAW, PITCH_NORM, dmg, player_slot};
use crate::targets::{find_by_targetname, use_targets};
use crate::world::atof;

const SF_SPRITE_STARTON: u32 = 1 << 0;
const SF_SPRITE_ONCE: u32 = 1 << 1;

const SF_BEAM_STARTON: u32 = 1 << 0;
const SF_BEAM_TOGGLE: u32 = 1 << 1;
const SF_BEAM_SPARKSTART: u32 = 1 << 4;
const SF_BEAM_SPARKEND: u32 = 1 << 5;

const SF_SPARK_TOGGLE: u32 = 1 << 5;
const SF_SPARK_STARTON: u32 = 1 << 6;

const SF_SHAKE_EVERYONE: u32 = 1 << 0;

const SF_FADE_IN: u32 = 1 << 0;
const SF_FADE_MODULATE: u32 = 1 << 1;
const SF_FADE_ONLYONE: u32 = 1 << 2;

const SF_EXPL_NODAMAGE: u32 = 1 << 0;
const SF_EXPL_REPEATABLE: u32 = 1 << 1;
const SF_EXPL_NOFIREBALL: u32 = 1 << 2;
const SF_EXPL_NOSMOKE: u32 = 1 << 3;
const SF_EXPL_NOSPARKS: u32 = 1 << 5;

const SF_RENDER_MASKFX: u32 = 1 << 0;
const SF_RENDER_MASKAMT: u32 = 1 << 1;
const SF_RENDER_MASKMODE: u32 = 1 << 2;
const SF_RENDER_MASKCOLOR: u32 = 1 << 3;

const SF_MESSAGE_ONCE: u32 = 1 << 0;
const SF_MESSAGE_ALL: u32 = 1 << 1;

const SF_GIBSHOOTER_REPEATABLE: u32 = 1 << 0;

const SF_BLOOD_STREAM: u32 = 1 << 1;

const SF_BUBBLES_STARTOFF: u32 = 1 << 0;

const SF_LIGHT_START_OFF: u32 = 1 << 0;

const SF_AMBIENT_START_SILENT: u32 = 1 << 4;
const SF_AMBIENT_NOT_LOOPING: u32 = 1 << 5;

const SF_SPEAKER_START_SILENT: u32 = 1 << 0;

const CHAN_STATIC: u8 = 6;
const CHAN_VOICE: u8 = 2;

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

pub(crate) fn parse_rgb(s: &str) -> [u8; 3] {
    let mut parts = s.split_whitespace().map(|p| atoi(p).clamp(0, 255) as u8);
    [
        parts.next().unwrap_or(255),
        parts.next().unwrap_or(255),
        parts.next().unwrap_or(255),
    ]
}

fn precache_nonempty_model(eng: &mut EngineApi<'_>, model: &str) {
    if !model.is_empty() {
        let _ = eng.precache_model(model);
    }
}

fn precache_nonempty_sound(eng: &mut EngineApi<'_>, sound: &str) {
    if !sound.is_empty() && !sound.starts_with('!') {
        let _ = eng.precache_sound(sound);
    }
}

/// Animated / toggleable map sprite (`env_sprite`, `CSprite`).
#[derive(Default)]
pub(crate) struct EnvSprite {
    start_on: bool,
    active: bool,
    once: bool,
    framerate: f32,
}

impl GameEntity for EnvSprite {
    fn classname(&self) -> &'static str {
        "env_sprite"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let model = key_str(keys, "model").unwrap_or("sprites/glow01.spr");
        precache_nonempty_model(eng, model);
        eng.set_model(id, model);
        self.framerate = key_str(keys, "framerate").map(atof).unwrap_or(10.0);
        let scale = key_str(keys, "scale").map(atof).unwrap_or(1.0);
        let rendermode = atoi(key_str(keys, "rendermode").unwrap_or("0")).clamp(0, 255) as u8;
        let renderamt = atoi(key_str(keys, "renderamt").unwrap_or("255")).clamp(0, 255) as u8;
        let rendercolor = key_str(keys, "rendercolor").map_or([255, 255, 255], parse_rgb);

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        v.scale = scale;
        v.framerate = self.framerate;
        v.rendermode = rendermode;
        v.renderamt = renderamt;
        v.rendercolor = rendercolor;
        self.once = v.spawnflags & SF_SPRITE_ONCE != 0;
        self.start_on = v.spawnflags & SF_SPRITE_STARTON != 0 || v.targetname.is_empty();
        self.active = self.start_on;
        if !self.active {
            v.effects |= EF_NODRAW;
        } else if self.framerate > 0.0 {
            v.nextthink = v.ltime + 0.1;
        }
        eng.link_entity(id, false);
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.active = match use_type {
            UseType::On => true,
            UseType::Off => false,
            UseType::Toggle | UseType::Set => !self.active,
        };
        let active = self.active;
        let rate = self.framerate;
        if let Some(v) = eng.ent_mut(id) {
            if active {
                v.effects &= !EF_NODRAW;
                if rate > 0.0 {
                    v.nextthink = v.ltime + 0.1;
                }
            } else {
                v.effects |= EF_NODRAW;
                v.nextthink = 0.0;
            }
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if !self.active {
            return;
        }
        let once = self.once;
        let rate = self.framerate;
        if let Some(v) = eng.ent_mut(id) {
            v.frame += rate * 0.1;
            if v.frame >= 16.0 {
                if once {
                    self.active = false;
                    v.effects |= EF_NODRAW;
                    v.nextthink = 0.0;
                    return;
                }
                v.frame = 0.0;
            }
            v.nextthink = v.ltime + 0.1;
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.active = self.start_on;
        let active = self.active;
        let rate = self.framerate;
        if let Some(v) = eng.ent_mut(id) {
            v.frame = 0.0;
            if active {
                v.effects &= !EF_NODRAW;
                v.nextthink = if rate > 0.0 { v.ltime + 0.1 } else { 0.0 };
            } else {
                v.effects |= EF_NODRAW;
                v.nextthink = 0.0;
            }
        }
    }
}

/// Static glowing sprite (`env_glow`, `CGlow`).
pub(crate) struct EnvGlow;

impl GameEntity for EnvGlow {
    fn classname(&self) -> &'static str {
        "env_glow"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let model = key_str(keys, "model").unwrap_or("sprites/glow01.spr");
        precache_nonempty_model(eng, model);
        eng.set_model(id, model);
        let scale = key_str(keys, "scale").map(atof).unwrap_or(1.0);
        let rendermode = atoi(key_str(keys, "rendermode").unwrap_or("3")).clamp(0, 255) as u8;
        let renderamt = atoi(key_str(keys, "renderamt").unwrap_or("255")).clamp(0, 255) as u8;
        let rendercolor = key_str(keys, "rendercolor").map_or([255, 255, 255], parse_rgb);
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        v.scale = scale;
        v.rendermode = rendermode;
        v.renderamt = renderamt;
        v.rendercolor = rendercolor;
        eng.link_entity(id, false);
        true
    }
}

/// Bomb LED glow sprite entity (`env_bombglow`).
#[derive(Default)]
pub(crate) struct EnvBombGlow {
    active: bool,
}

impl GameEntity for EnvBombGlow {
    fn classname(&self) -> &'static str {
        "env_bombglow"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        precache_nonempty_model(eng, "sprites/ledglow.spr");
        eng.set_model(id, "sprites/ledglow.spr");
        self.active = true;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
            v.scale = 0.35;
        }
        eng.link_entity(id, false);
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.active = match use_type {
            UseType::On => true,
            UseType::Off => false,
            UseType::Toggle | UseType::Set => !self.active,
        };
        if let Some(v) = eng.ent_mut(id) {
            if self.active {
                v.effects &= !EF_NODRAW;
            } else {
                v.effects |= EF_NODRAW;
            }
        }
    }
}

/// Decorative cycling model or sprite (`cycler`, `cycler_sprite`).
pub(crate) struct Cycler {
    class: &'static str,
}

impl Cycler {
    pub(crate) fn new(class: &'static str) -> Self {
        Self { class }
    }
}

impl GameEntity for Cycler {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn takes_damage(&self) -> bool {
        self.class == "cycler"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let model = key_str(keys, "model").unwrap_or("models/player.mdl");
        precache_nonempty_model(eng, model);
        eng.set_model(id, model);
        let solid = if self.class == "cycler" {
            SOLID_BBOX
        } else {
            SOLID_NOT
        };
        if let Some(v) = eng.ent_mut(id) {
            v.solid = solid;
            v.movetype = MOVETYPE_NONE;
            v.framerate = 1.0;
        }
        eng.link_entity(id, false);
        true
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
            v.sequence = (v.sequence + 1) % 8;
        }
    }

    fn take_damage(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _inflictor: Option<EdictId>,
        attacker: Option<EdictId>,
        _damage: f32,
        _bits: u32,
    ) -> bool {
        self.use_(g, eng, id, attacker, attacker, UseType::Toggle, 0.0);
        true
    }
}

/// Corpse queue ring slot (`bodyque`, `CBodyQue`).
pub(crate) struct BodyQue;

impl GameEntity for BodyQue {
    fn classname(&self) -> &'static str {
        "bodyque"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
            v.effects |= EF_NODRAW;
        }
        true
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if let Some(v) = eng.ent_mut(id) {
            v.modelindex = 0;
            v.effects |= EF_NODRAW;
            v.solid = SOLID_NOT;
        }
    }
}

/// Beam / lightning / laser entity (`beam`, `env_beam`, `env_lightning`, `env_laser`).
pub(crate) struct BeamEntity {
    class: &'static str,
    texture: String,
    start_target: String,
    end_target: String,
    width: u8,
    noise: u8,
    color: [u8; 3],
    brightness: u8,
    life: f32,
    strike_time: f32,
    damage: f32,
    start_on: bool,
    active: bool,
    toggle: bool,
}

impl BeamEntity {
    pub(crate) fn new(class: &'static str) -> Self {
        Self {
            class,
            texture: "sprites/laserbeam.spr".to_string(),
            start_target: String::new(),
            end_target: String::new(),
            width: 20,
            noise: 0,
            color: [255, 255, 255],
            brightness: 255,
            life: 0.0,
            strike_time: 1.0,
            damage: 0.0,
            start_on: false,
            active: false,
            toggle: false,
        }
    }

    fn fire_beam(&self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(origin) = eng.ent(id).map(|v| v.origin) else {
            return;
        };
        let sf = eng.ent(id).map_or(0, |v| v.spawnflags);
        let start = if !self.start_target.is_empty() {
            find_by_targetname(eng, None, &self.start_target)
                .and_then(|e| eng.ent(e).map(|v| v.origin))
                .unwrap_or(origin)
        } else {
            origin
        };
        let end = if !self.end_target.is_empty() {
            find_by_targetname(eng, None, &self.end_target)
                .and_then(|e| eng.ent(e).map(|v| v.origin))
                .unwrap_or(start + Vec3::X * 256.0)
        } else {
            start + Vec3::X * 256.0
        };
        let sprite = eng.model_index(&self.texture).unwrap_or(0);
        let life_tenths = if self.life > 0.0 {
            (self.life * 10.0).clamp(1.0, 255.0) as u8
        } else {
            10
        };
        let params = BeamParams {
            sprite,
            start_frame: 0,
            frame_rate: 10,
            life: life_tenths,
            width: self.width,
            noise: self.noise,
            color: self.color,
            brightness: self.brightness,
            speed: 0,
        };
        let te = tempent::beam_points(start, end, &params);
        eng.send_temp_entity(MsgDest::Pvs(start), &te);
        if sf & SF_BEAM_SPARKSTART != 0 {
            eng.send_temp_entity(MsgDest::Pvs(start), &tempent::sparks(start));
        }
        if sf & SF_BEAM_SPARKEND != 0 {
            eng.send_temp_entity(MsgDest::Pvs(end), &tempent::sparks(end));
        }
        if self.damage > 0.0 {
            let tr = eng.trace_line(start, end, MoveFilter::Normal, Some(id));
            if let Some(hit) = tr.ent {
                crate::entity::take_damage_of(
                    g,
                    eng,
                    hit,
                    Some(id),
                    Some(id),
                    self.damage,
                    dmg::GENERIC,
                );
            }
        }
    }
}

impl GameEntity for BeamEntity {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.texture = key_str(keys, "texture")
            .unwrap_or("sprites/laserbeam.spr")
            .to_string();
        precache_nonempty_model(eng, &self.texture);
        self.start_target = key_str(keys, "LightningStart").unwrap_or("").to_string();
        self.end_target = key_str(keys, "LightningEnd")
            .or_else(|| key_str(keys, "LaserTarget"))
            .unwrap_or("")
            .to_string();
        self.width = (atof(key_str(keys, "BoltWidth").unwrap_or("20")) * 1.0).clamp(1.0, 255.0) as u8;
        self.noise = atoi(key_str(keys, "NoiseAmplitude").unwrap_or("0")).clamp(0, 255) as u8;
        self.color = key_str(keys, "rendercolor").map_or([255, 255, 255], parse_rgb);
        self.brightness = atoi(key_str(keys, "renderamt").unwrap_or("255")).clamp(0, 255) as u8;
        self.life = key_str(keys, "life").map(atof).unwrap_or(0.0);
        self.strike_time = key_str(keys, "StrikeTime").map(atof).unwrap_or(1.0).max(0.1);
        self.damage = key_str(keys, "damage").map(atof).unwrap_or(0.0);

        let now = eng.time();
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        self.toggle = v.spawnflags & SF_BEAM_TOGGLE != 0;
        self.start_on = v.spawnflags & SF_BEAM_STARTON != 0 || v.targetname.is_empty();
        self.active = self.start_on;
        if self.active {
            v.nextthink = now + 0.1;
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        if self.toggle || matches!(use_type, UseType::On | UseType::Off) {
            self.active = match use_type {
                UseType::On => true,
                UseType::Off => false,
                _ => !self.active,
            };
            if self.active {
                self.fire_beam(g, eng, id);
                let due = eng.time() + f64::from(self.strike_time);
                if let Some(v) = eng.ent_mut(id) {
                    v.nextthink = due;
                }
            } else {
                if let Some(v) = eng.ent_mut(id) {
                    v.nextthink = 0.0;
                }
                eng.send_temp_entity(MsgDest::All, &tempent::kill_beam(id.index));
            }
        } else {
            self.fire_beam(g, eng, id);
        }
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if !self.active {
            return;
        }
        self.fire_beam(g, eng, id);
        let due = eng.time() + f64::from(self.strike_time);
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.active = self.start_on;
        let due = if self.active { eng.time() + 0.1 } else { 0.0 };
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
    }
}

/// Repeating or triggered electrical spark emitter (`env_spark`, `CEnvSpark`).
#[derive(Default)]
pub(crate) struct EnvSpark {
    max_delay: f32,
    start_on: bool,
    active: bool,
}

impl GameEntity for EnvSpark {
    fn classname(&self) -> &'static str {
        "env_spark"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        precache_nonempty_sound(eng, "buttons/spark1.wav");
        self.max_delay = key_str(keys, "MaxDelay").map(atof).unwrap_or(1.5).max(0.1);
        let now = eng.time();
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        self.start_on = v.spawnflags & SF_SPARK_STARTON != 0 || v.targetname.is_empty();
        self.active = self.start_on;
        if self.active {
            v.nextthink = now + 0.1;
        }
        true
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
        let toggle = eng.ent(id).is_some_and(|v| v.spawnflags & SF_SPARK_TOGGLE != 0);
        if toggle {
            self.active = !self.active;
            let due = if self.active { eng.time() + 0.1 } else { 0.0 };
            if let Some(v) = eng.ent_mut(id) {
                v.nextthink = due;
            }
        } else if let Some(origin) = eng.ent(id).map(|v| v.origin) {
            eng.send_temp_entity(MsgDest::Pvs(origin), &tempent::sparks(origin));
            eng.emit_sound(id, CHAN_VOICE, "buttons/spark1.wav", 1.0, ATTN_NORM, PITCH_NORM);
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if !self.active {
            return;
        }
        if let Some(origin) = eng.ent(id).map(|v| v.origin) {
            eng.send_temp_entity(MsgDest::Pvs(origin), &tempent::sparks(origin));
            eng.emit_sound(id, CHAN_VOICE, "buttons/spark1.wav", 1.0, ATTN_NORM, PITCH_NORM);
        }
        let due = eng.time() + f64::from(self.max_delay);
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.active = self.start_on;
        let due = if self.active { eng.time() + 0.1 } else { 0.0 };
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
    }
}

/// Screen shake trigger (`env_shake`, `CShake`).
#[derive(Default)]
pub(crate) struct EnvShake {
    amplitude: f32,
    radius: f32,
    duration: f32,
    frequency: f32,
}

impl GameEntity for EnvShake {
    fn classname(&self) -> &'static str {
        "env_shake"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.amplitude = key_str(keys, "amplitude").map(atof).unwrap_or(4.0);
        self.radius = key_str(keys, "radius").map(atof).unwrap_or(500.0);
        self.duration = key_str(keys, "duration").map(atof).unwrap_or(1.0);
        self.frequency = key_str(keys, "frequency").map(atof).unwrap_or(2.5);
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
    }

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
        let Some((origin, flags)) = eng.ent(id).map(|v| (v.origin, v.spawnflags)) else {
            return;
        };
        if flags & SF_SHAKE_EVERYONE != 0 {
            messages::screen_shake(eng, MsgDest::All, self.amplitude, self.duration, self.frequency);
            return;
        }
        let max_sq = self.radius * self.radius;
        for slot in 0..g.players.len() {
            if g.players[slot].is_none() {
                continue;
            }
            let pe = eng.client_edict(slot as u8);
            if eng.ent(pe).is_some_and(|v| (v.origin - origin).length_squared() <= max_sq) {
                messages::screen_shake(
                    eng,
                    MsgDest::One(slot as u8),
                    self.amplitude,
                    self.duration,
                    self.frequency,
                );
            }
        }
    }
}

/// Screen fade trigger (`env_fade`, `CFade`).
#[derive(Default)]
pub(crate) struct EnvFade {
    duration: f32,
    hold_time: f32,
    color: [u8; 4],
}

impl GameEntity for EnvFade {
    fn classname(&self) -> &'static str {
        "env_fade"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.duration = key_str(keys, "duration").map(atof).unwrap_or(2.0);
        self.hold_time = key_str(keys, "holdtime").map(atof).unwrap_or(0.0);
        let rgb = key_str(keys, "rendercolor").map_or([0, 0, 0], parse_rgb);
        let alpha = atoi(key_str(keys, "renderamt").unwrap_or("255")).clamp(0, 255) as u8;
        self.color = [rgb[0], rgb[1], rgb[2], alpha];
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        let sf = eng.ent(id).map_or(0, |v| v.spawnflags);
        let mut flags = 0_u16;
        if sf & SF_FADE_IN == 0 {
            flags |= 0x0001; // FFADE_OUT
        }
        if sf & SF_FADE_MODULATE != 0 {
            flags |= 0x0002;
        }
        let dest = if sf & SF_FADE_ONLYONE != 0
            && let Some(slot) = activator.and_then(|a| player_slot(g, eng, a))
        {
            MsgDest::One(slot as u8)
        } else {
            MsgDest::All
        };
        messages::screen_fade(eng, dest, self.duration, self.hold_time, flags, self.color);
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }
}

/// Explosion effect + radius damage trigger (`env_explosion`, `CEnvExplosion`).
#[derive(Default)]
pub(crate) struct EnvExplosion {
    magnitude: f32,
    spent: bool,
}

impl GameEntity for EnvExplosion {
    fn classname(&self) -> &'static str {
        "env_explosion"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        precache_nonempty_model(eng, "sprites/zerogxplode.spr");
        precache_nonempty_model(eng, "sprites/steam1.spr");
        self.magnitude = key_str(keys, "iMagnitude").map(atof).unwrap_or(100.0);
        self.spent = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        let Some((origin, sf)) = eng.ent(id).map(|v| (v.origin, v.spawnflags)) else {
            return;
        };
        if self.spent && sf & SF_EXPL_REPEATABLE == 0 {
            return;
        }
        self.spent = true;
        if sf & SF_EXPL_NOFIREBALL == 0 {
            let sprite = eng.model_index("sprites/zerogxplode.spr").unwrap_or(0);
            let te = tempent::explosion(origin, sprite, 25, 30, explflag::NONE);
            eng.send_temp_entity(MsgDest::Pvs(origin), &te);
        }
        if sf & SF_EXPL_NOSMOKE == 0 {
            let smoke = eng.model_index("sprites/steam1.spr").unwrap_or(0);
            eng.send_temp_entity(MsgDest::Pvs(origin), &tempent::smoke(origin, smoke, 30, 10));
        }
        if sf & SF_EXPL_NOSPARKS == 0 {
            eng.send_temp_entity(MsgDest::Pvs(origin), &tempent::sparks(origin));
        }
        if sf & SF_EXPL_NODAMAGE == 0 && self.magnitude > 0.0 {
            radius_damage(
                g,
                eng,
                origin,
                Some(id),
                None,
                activator,
                self.magnitude,
                self.magnitude * 2.5,
                dmg::BLAST,
            );
        }
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.spent = false;
    }
}

/// Entity render property mutator (`env_render`, `CRenderFxManager`).
#[derive(Default)]
pub(crate) struct EnvRender {
    rendermode: u8,
    renderamt: u8,
    renderfx: u8,
    rendercolor: [u8; 3],
}

impl GameEntity for EnvRender {
    fn classname(&self) -> &'static str {
        "env_render"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.rendermode = atoi(key_str(keys, "rendermode").unwrap_or("0")).clamp(0, 255) as u8;
        self.renderamt = atoi(key_str(keys, "renderamt").unwrap_or("255")).clamp(0, 255) as u8;
        self.renderfx = atoi(key_str(keys, "renderfx").unwrap_or("0")).clamp(0, 255) as u8;
        self.rendercolor = key_str(keys, "rendercolor").map_or([0, 0, 0], parse_rgb);
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
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
        let Some((target, sf)) = eng.ent(id).map(|v| (v.target.clone(), v.spawnflags)) else {
            return;
        };
        if target.is_empty() {
            return;
        }
        let mut cursor = None;
        while let Some(tid) = find_by_targetname(eng, cursor, &target) {
            cursor = Some(tid);
            if let Some(tv) = eng.ent_mut(tid) {
                if sf & SF_RENDER_MASKFX == 0 {
                    tv.renderfx = self.renderfx;
                }
                if sf & SF_RENDER_MASKAMT == 0 {
                    tv.renderamt = self.renderamt;
                }
                if sf & SF_RENDER_MASKMODE == 0 {
                    tv.rendermode = self.rendermode;
                }
                if sf & SF_RENDER_MASKCOLOR == 0 {
                    tv.rendercolor = self.rendercolor;
                }
            }
        }
    }
}

/// HUD text + optional sound trigger (`env_message`, `CMessage`).
#[derive(Default)]
pub(crate) struct EnvMessage {
    message: String,
    sound: String,
    volume: f32,
    used: bool,
}

impl GameEntity for EnvMessage {
    fn classname(&self) -> &'static str {
        "env_message"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.message = key_str(keys, "message").unwrap_or("").to_string();
        self.sound = key_str(keys, "messagesound").unwrap_or("").to_string();
        self.volume = key_str(keys, "messagevolume").map(atof).unwrap_or(10.0) / 10.0;
        precache_nonempty_sound(eng, &self.sound);
        self.used = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        let sf = eng.ent(id).map_or(0, |v| v.spawnflags);
        if sf & SF_MESSAGE_ONCE != 0 && self.used {
            return;
        }
        self.used = true;
        if !self.message.is_empty() {
            let dest = if sf & SF_MESSAGE_ALL != 0 {
                MsgDest::All
            } else if let Some(slot) = activator.and_then(|a| player_slot(g, eng, a)) {
                MsgDest::One(slot as u8)
            } else {
                MsgDest::All
            };
            messages::hud_text(eng, dest, &self.message);
        }
        if !self.sound.is_empty() {
            eng.emit_sound(id, CHAN_VOICE, &self.sound, self.volume.clamp(0.0, 1.0), ATTN_NORM, PITCH_NORM);
        }
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.used = false;
    }
}

/// Gib / debris / model burst emitter (`env_shooter`, `gibshooter`, `env_debris`).
pub(crate) struct EnvShooter {
    class: &'static str,
    model: String,
    gibs: u8,
    velocity: f32,
    life: f32,
    spent: bool,
}

impl EnvShooter {
    pub(crate) fn new(class: &'static str) -> Self {
        Self {
            class,
            model: "models/woodgibs.mdl".to_string(),
            gibs: 3,
            velocity: 200.0,
            life: 4.0,
            spent: false,
        }
    }
}

impl GameEntity for EnvShooter {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.model = key_str(keys, "shootmodel")
            .unwrap_or("models/woodgibs.mdl")
            .to_string();
        precache_nonempty_model(eng, &self.model);
        self.gibs = atoi(key_str(keys, "m_iGibs").unwrap_or("3")).clamp(1, 32) as u8;
        self.velocity = key_str(keys, "m_flVelocity").map(atof).unwrap_or(200.0);
        self.life = key_str(keys, "m_flGibLife").map(atof).unwrap_or(4.0);
        self.spent = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
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
        let Some((origin, sf)) = eng.ent(id).map(|v| (v.origin, v.spawnflags)) else {
            return;
        };
        if self.spent && sf & SF_GIBSHOOTER_REPEATABLE == 0 {
            return;
        }
        self.spent = true;
        let model_idx = eng.model_index(&self.model).unwrap_or(0);
        let life_tenths = (self.life * 10.0).clamp(1.0, 255.0) as u8;
        let te = tempent::break_model(
            origin,
            Vec3::splat(8.0),
            Vec3::new(0.0, 0.0, self.velocity),
            10,
            model_idx,
            self.gibs,
            life_tenths,
            0,
        );
        eng.send_temp_entity(MsgDest::Pvs(origin), &te);
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.spent = false;
    }
}

/// Triggered blood burst emitter (`env_blood`, `CEnvBlood`).
#[derive(Default)]
pub(crate) struct EnvBlood {
    color: u8,
    amount: u8,
}

impl GameEntity for EnvBlood {
    fn classname(&self) -> &'static str {
        "env_blood"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.color = match atoi(key_str(keys, "color").unwrap_or("0")) {
            1 => 195, // yellow blood
            _ => 247, // red blood
        };
        self.amount = atoi(key_str(keys, "amount").unwrap_or("100")).clamp(1, 255) as u8;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
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
        let Some((origin, sf)) = eng.ent(id).map(|v| (v.origin, v.spawnflags)) else {
            return;
        };
        let te = if sf & SF_BLOOD_STREAM != 0 {
            tempent::blood_stream(origin, Vec3::Z, self.color, self.amount)
        } else {
            let spray = eng.model_index("sprites/bloodspray.spr").unwrap_or(0);
            let drop = eng.model_index("sprites/blood.spr").unwrap_or(0);
            tempent::blood_sprite(origin, spray, drop, self.color, (self.amount / 16).max(3))
        };
        eng.send_temp_entity(MsgDest::Pvs(origin), &te);
    }
}

/// Bubble volume emitter (`env_bubbles`, `CBubbling`).
#[derive(Default)]
pub(crate) struct EnvBubbles {
    active: bool,
    start_on: bool,
}

impl GameEntity for EnvBubbles {
    fn classname(&self) -> &'static str {
        "env_bubbles"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        precache_nonempty_model(eng, "sprites/bubble.spr");
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        v.effects |= EF_NODRAW;
        self.start_on = v.spawnflags & SF_BUBBLES_STARTOFF == 0;
        self.active = self.start_on;
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.active = match use_type {
            UseType::On => true,
            UseType::Off => false,
            _ => !self.active,
        };
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.active = self.start_on;
    }
}

/// Particle funnel (`env_funnel`, `CEnvFunnel`).
#[derive(Default)]
pub(crate) struct EnvFunnel;

impl GameEntity for EnvFunnel {
    fn classname(&self) -> &'static str {
        "env_funnel"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        precache_nonempty_model(eng, "sprites/flare6.spr");
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
        }
        true
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
        if let Some(origin) = eng.ent(id).map(|v| v.origin) {
            let sprite = eng.model_index("sprites/flare6.spr").unwrap_or(0);
            eng.send_temp_entity(MsgDest::Pvs(origin), &tempent::sprite(origin, sprite, 10, 255));
        }
    }
}

/// Map fog controller (`env_fog`, `CFog`).
#[derive(Default)]
pub(crate) struct EnvFog {
    color: [u8; 3],
    density: f32,
    enabled: bool,
}

impl GameEntity for EnvFog {
    fn classname(&self) -> &'static str {
        "env_fog"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.color = key_str(keys, "rendercolor").map_or([128, 128, 128], parse_rgb);
        self.density = key_str(keys, "density").map(atof).unwrap_or(0.001);
        self.enabled = true;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
            v.effects |= EF_NODRAW;
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        _id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.enabled = match use_type {
            UseType::On => true,
            UseType::Off => false,
            _ => !self.enabled,
        };
        let density = if self.enabled { self.density } else { 0.0 };
        g.map_fog = if self.enabled {
            Some((self.color, self.density))
        } else {
            None
        };
        messages::fog(eng, MsgDest::All, self.color, density);
    }

    fn restart(&mut self, g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.enabled = true;
        g.map_fog = Some((self.color, self.density));
    }
}

/// Map weather marker (`env_rain`, `func_rain`, `env_snow`, `func_snow`).
pub(crate) struct EnvWeather {
    class: &'static str,
}

impl EnvWeather {
    pub(crate) fn new(class: &'static str) -> Self {
        Self { class }
    }
}

impl GameEntity for EnvWeather {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
            v.effects |= EF_NODRAW;
        }
        true
    }
}

/// Map light (`light`, `light_spot`, `CLight`): static unnamed lights free their edict cleanly
/// just like GoldSrc `CLight::Spawn` so large BSPs stay well within `MAX_EDICTS`; switchable
/// named lights update engine `lightstyle` entries.
pub(crate) struct Light {
    class: &'static str,
    style: u8,
    pattern: String,
    start_off: bool,
    on: bool,
}

impl Light {
    pub(crate) fn new(class: &'static str) -> Self {
        Self {
            class,
            style: 0,
            pattern: "m".to_string(),
            start_off: false,
            on: true,
        }
    }
}

impl GameEntity for Light {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let targetname = key_str(keys, "targetname")
            .map(str::to_string)
            .or_else(|| eng.ent(id).map(|v| v.targetname.clone()))
            .unwrap_or_default();
        if targetname.is_empty() {
            // Un-targeted static lights are pre-baked into BSP lightmaps.
            return false;
        }
        self.style = atoi(key_str(keys, "style").unwrap_or("0")).clamp(0, 63) as u8;
        self.pattern = key_str(keys, "pattern").unwrap_or("m").to_string();
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        v.effects |= EF_NODRAW;
        self.start_off = v.spawnflags & SF_LIGHT_START_OFF != 0;
        self.on = !self.start_off;
        if self.style >= 32 {
            eng.light_style(self.style, if self.on { &self.pattern } else { "a" });
        }
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        _id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.on = match use_type {
            UseType::On => true,
            UseType::Off => false,
            _ => !self.on,
        };
        if self.style >= 32 {
            eng.light_style(self.style, if self.on { &self.pattern } else { "a" });
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, _id: EdictId) {
        self.on = !self.start_off;
        if self.style >= 32 {
            eng.light_style(self.style, if self.on { &self.pattern } else { "a" });
        }
    }
}

/// Directional sun light keyholder (`light_environment`, `CEnvLight`): GoldSrc removes this
/// compiler/static entity immediately on `Spawn` (`REMOVE_ENTITY`) so brush/model baseline
/// edict indices match retail HLDS.
pub(crate) struct LightEnvironment;

impl GameEntity for LightEnvironment {
    fn classname(&self) -> &'static str {
        "light_environment"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, _keys: &[(String, String)]) -> bool {
        false
    }
}

/// Ambient map sound emitter (`ambient_generic`, `CAmbientGeneric`).
#[derive(Default)]
pub(crate) struct AmbientGeneric {
    sample: String,
    volume: f32,
    pitch: i32,
    attenuation: f32,
    start_active: bool,
    looping: bool,
    active: bool,
}

impl GameEntity for AmbientGeneric {
    fn classname(&self) -> &'static str {
        "ambient_generic"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.sample = key_str(keys, "message").unwrap_or("").to_string();
        precache_nonempty_sound(eng, &self.sample);
        self.volume = (key_str(keys, "health").map(atof).unwrap_or(10.0) / 10.0).clamp(0.0, 1.0);
        self.pitch = atoi(key_str(keys, "pitchstart").unwrap_or("100")).clamp(1, 255);
        let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        let sf = v.spawnflags;
        self.attenuation = if sf & 1 != 0 {
            0.0 // everywhere
        } else if sf & 2 != 0 {
            1.25
        } else if sf & 8 != 0 {
            0.3
        } else {
            0.8
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        v.effects |= EF_NODRAW;
        self.looping = sf & SF_AMBIENT_NOT_LOOPING == 0;
        self.start_active = sf & SF_AMBIENT_START_SILENT == 0 && self.looping;
        self.active = self.start_active;
        if self.active && !self.sample.is_empty() {
            eng.emit_ambient_sound(
                id,
                origin,
                &self.sample,
                self.volume,
                self.attenuation,
                0,
                self.pitch,
            );
        }
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        if self.sample.is_empty() {
            return;
        }
        let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        if !self.looping {
            eng.emit_ambient_sound(
                id,
                origin,
                &self.sample,
                self.volume,
                self.attenuation,
                0,
                self.pitch,
            );
            return;
        }
        self.active = match use_type {
            UseType::On => true,
            UseType::Off => false,
            _ => !self.active,
        };
        let flags = if self.active { 0 } else { SND_FL_STOP as i32 };
        eng.emit_ambient_sound(
            id,
            origin,
            &self.sample,
            self.volume,
            self.attenuation,
            flags,
            self.pitch,
        );
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.active = self.start_active;
        if !self.sample.is_empty() {
            let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
            let flags = if self.active { 0 } else { SND_FL_STOP as i32 };
            eng.emit_ambient_sound(
                id,
                origin,
                &self.sample,
                self.volume,
                self.attenuation,
                flags,
                self.pitch,
            );
        }
    }
}

/// DSP room reverb zone marker (`env_sound`, `CEnvSound`).
#[derive(Default)]
pub(crate) struct EnvSound {
    radius: f32,
    room_type: i32,
}

impl GameEntity for EnvSound {
    fn classname(&self) -> &'static str {
        "env_sound"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.radius = key_str(keys, "radius").map(atof).unwrap_or(128.0);
        self.room_type = atoi(key_str(keys, "roomtype").unwrap_or("0"));
        let now = eng.time();
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
            v.effects |= EF_NODRAW;
            v.nextthink = now + 0.5;
        }
        true
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(origin) = eng.ent(id).map(|v| v.origin) else {
            return;
        };
        let r_sq = self.radius * self.radius;
        for slot in 0..g.players.len() {
            if g.players[slot].is_none() {
                continue;
            }
            let pe = eng.client_edict(slot as u8);
            if eng.ent(pe).is_some_and(|v| (v.origin - origin).length_squared() <= r_sq) {
                eng.set_cvar("room_type", &self.room_type.to_string());
                break;
            }
        }
        let due = eng.time() + 0.75;
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
    }
}

/// Periodic announcement / intercom speaker (`speaker`, `CSpeaker`).
#[derive(Default)]
pub(crate) struct Speaker {
    message: String,
    volume: f32,
    start_active: bool,
    active: bool,
}

impl GameEntity for Speaker {
    fn classname(&self) -> &'static str {
        "speaker"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.message = key_str(keys, "message").unwrap_or("").to_string();
        precache_nonempty_sound(eng, &self.message);
        self.volume = (key_str(keys, "health").map(atof).unwrap_or(5.0) / 10.0).clamp(0.0, 1.0);
        let now = eng.time();
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        v.effects |= EF_NODRAW;
        self.start_active = v.spawnflags & SF_SPEAKER_START_SILENT == 0;
        self.active = self.start_active;
        if self.active && !self.message.is_empty() {
            v.nextthink = now + 5.0;
        }
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.active = match use_type {
            UseType::On => true,
            UseType::Off => false,
            _ => !self.active,
        };
        let due = if self.active { eng.time() + 0.1 } else { 0.0 };
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if !self.active || self.message.is_empty() {
            return;
        }
        eng.emit_sound(id, CHAN_STATIC, &self.message, self.volume, 0.3, PITCH_NORM);
        let due = eng.time() + 15.0;
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.active = self.start_active;
    }
}

/// World sound queue manager (`soundent`, `CSoundEnt`).
pub(crate) struct SoundEnt;

impl GameEntity for SoundEnt {
    fn classname(&self) -> &'static str {
        "soundent"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let now = eng.time();
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_NONE;
            v.effects |= EF_NODRAW;
            v.nextthink = now + 0.3;
        }
        true
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let now = eng.time();
        g.world_sounds.retain(|&(_, _, expiry)| expiry > now);
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = now + 0.3;
        }
    }

    fn restart(&mut self, g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        g.world_sounds.clear();
    }
}
