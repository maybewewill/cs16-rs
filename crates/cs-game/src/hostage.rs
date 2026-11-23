//! Classic Half-Life / Counter-Strike hostages (`CHostage`, ReGameDLL_CS
//! `hostage/hostage.cpp`, `hostage/hostage.h`, read 2026-11-23), vanilla non-CZ (`!m_improv`,
//! `!hostage_ai_enable`) behaviour.
//!
//! Each `hostage_entity` (or legacy `monster_scientist`) precaches its model and sounds inside
//! its own [`GameEntity::spawn`] so maps without hostages (such as `de_dust2`) keep their
//! exact precache table.

use cs_engine::collision::Hull;
use cs_engine::consts::{
    EF_NODRAW, FL_MONSTER, FL_MONSTERCLIP, FL_ONGROUND, MOVETYPE_FLY, MOVETYPE_NONE, MOVETYPE_STEP,
    MOVETYPE_TOSS, SOLID_NOT, SOLID_SLIDEBOX,
};
use cs_engine::{EdictId, EngineApi, EntTrace, MoveFilter, MsgDest};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::bullets::{BLOOD_COLOR_RED, MultiDamage, add_multi_damage, random_float, spawn_blood, trace_bleed};
use crate::entity::{FCAP_ACROSS_TRANSITION, FCAP_ONOFF_USE, GameEntity, UseType, with_entity};
use crate::hint::{dhf, first_time, hint, hint_message};
use crate::hostage_localnav::{
    HOSTAGE_STEPSIZE, LocalNav, MAX_NODES, MAX_UNIT_Z_SLOPE, PathTraversable,
};
use crate::intro::vec_to_angles;
use crate::menus::print_center;
use crate::money::add_account;
use crate::player::{
    ATTN_NORM, CHAN_VOICE, DEAD_DEAD, DEAD_NO, DEAD_RESPAWNABLE, HITGROUP_CHEST, HITGROUP_GENERIC,
    HITGROUP_HEAD, HITGROUP_LEFTARM, HITGROUP_LEFTLEG, HITGROUP_RIGHTARM, HITGROUP_RIGHTLEG,
    HITGROUP_SHIELD, HITGROUP_STOMACH, PITCH_NORM, Team, player_slot,
};
use crate::player_anim::{
    ACT_DIEBACKWARD, ACT_DIEFORWARD, ACT_DIE_HEADSHOT, ACT_DIESIMPLE, ACT_IDLE, ACT_INVALID,
    ACT_RUN, ACT_SMALL_FLINCH, ACT_WALK, STUDIO_LOOPING, get_sequence_flags, get_sequence_info,
    lookup_activity,
};
use crate::random_in;

/// `VEC_HOSTAGE_VIEW` (`hostage.h`).
const VEC_HOSTAGE_VIEW: Vec3 = Vec3::new(0.0, 0.0, 12.0);
/// `VEC_HOSTAGE_HULL_MIN` (`hostage.h`).
const VEC_HOSTAGE_HULL_MIN: Vec3 = Vec3::new(-10.0, -10.0, 0.0);
/// `VEC_HOSTAGE_HULL_MAX` (`hostage.h`).
const VEC_HOSTAGE_HULL_MAX: Vec3 = Vec3::new(10.0, 10.0, 62.0);
/// `MAX_HOSTAGES_RESCUE_RADIUS` (`hostage.h`).
const MAX_HOSTAGES_RESCUE_RADIUS: f32 = 256.0;
/// `SF_MONSTER_HITMONSTERCLIP` (`monsters.h`: bit 7).
const SF_MONSTER_HITMONSTERCLIP: u32 = 128;
/// Initial/reset hostage health (`pev->max_health` = 100 in `CHostage::Spawn`).
const HOSTAGE_MAX_HEALTH: f32 = 100.0;

/// `REWARD_TAKEN_HOSTAGE` (`gamerules.h`): rescuer individual bonus when a hostage reaches rescue.
const REWARD_TAKEN_HOSTAGE: i32 = 1000;
/// `REWARD_TOOK_HOSTAGE_ACC` (`gamerules.h`): CT team pool bonus on first hostage touch.
const REWARD_TOOK_HOSTAGE_ACC: i32 = 100;
/// `REWARD_TOOK_HOSTAGE` (`gamerules.h`): individual CT bonus on first hostage touch.
const REWARD_TOOK_HOSTAGE: i32 = 150;

/// Hostage think cadence (`IdleThink` 30 Hz, full follow/rescue logic 10 Hz).
const GIVE_UP_TIME: f64 = 1.0 / 30.0;
const UPDATE_RATE: f64 = 0.1;

/// `CHostage::state` (`hostage.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Stand,
    Follow,
}

/// Classic `CHostage` entity (`hostage_entity` / `monster_scientist`).
pub struct Hostage {
    state: State,
    index: u8,
    model: String,
    activity: i32,
    frame_rate: f32,
    sequence_loops: bool,
    sequence_finished: bool,
    takedamage: bool,
    last_hit_group: i32,
    start_origin: Vec3,
    start_angles: Vec3,
    old_pos: Vec3,
    target_ent: Option<EdictId>,
    stopped_target_ent: Option<EdictId>,
    local_nav: LocalNav,
    next_change: f64,
    next_full_think: f64,
    next_radar_time: f64,
    last_path_check: f64,
    path_acquired: f64,
    path_check_interval: f64,
    flinch_time: f64,
    stuck_time: f64,
    dead_remove_at: Option<f64>,
    stuck: bool,
    touched: bool,
    rescue_me: bool,
    rescued: bool,
    has_path: bool,
    target_node: Option<usize>,
    path_nodes: [Vec3; MAX_NODES],
    num_path_nodes: usize,
}

impl Default for Hostage {
    fn default() -> Self {
        Self {
            state: State::Stand,
            index: 0,
            model: String::new(),
            activity: ACT_INVALID,
            frame_rate: 0.0,
            sequence_loops: false,
            sequence_finished: false,
            takedamage: false,
            last_hit_group: HITGROUP_GENERIC,
            start_origin: Vec3::ZERO,
            start_angles: Vec3::ZERO,
            old_pos: Vec3::splat(9999.0),
            target_ent: None,
            stopped_target_ent: None,
            local_nav: LocalNav::default(),
            next_change: 0.0,
            next_full_think: 0.0,
            next_radar_time: 0.0,
            last_path_check: -1.0,
            path_acquired: -1.0,
            path_check_interval: 0.1,
            flinch_time: 0.0,
            stuck_time: 0.0,
            dead_remove_at: None,
            stuck: false,
            touched: false,
            rescue_me: false,
            rescued: false,
            has_path: false,
            target_node: None,
            path_nodes: [Vec3::ZERO; MAX_NODES],
            num_path_nodes: 0,
        }
    }
}

impl Hostage {
    fn set_activity(&mut self, eng: &mut EngineApi<'_>, id: EdictId, act: i32, rng: &mut u32) {
        if self.activity == act {
            return;
        }
        let Some(modelindex) = eng.ent(id).map(|v| v.modelindex) else {
            return;
        };
        let sequence = lookup_activity(eng.studio_model(modelindex).as_deref(), act, rng);
        if sequence == ACT_INVALID {
            return;
        }
        let prev_act = self.activity;
        if let Some(v) = eng.ent_mut(id)
            && i32::from(v.sequence) != sequence
        {
            if (prev_act != ACT_WALK && prev_act != ACT_RUN) || (act != ACT_WALK && act != ACT_RUN) {
                v.frame = 0.0;
            }
            v.sequence = sequence.max(0) as u16;
        }
        self.activity = act;
        self.reset_sequence_info(eng, id);
    }

    fn reset_sequence_info(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some((modelindex, sequence)) = eng.ent(id).map(|v| (v.modelindex, v.sequence)) else {
            return;
        };
        let model = eng.studio_model(modelindex);
        if let Some((rate, _speed)) = get_sequence_info(model.as_deref(), i32::from(sequence)) {
            self.frame_rate = rate / 256.0;
        }
        self.sequence_loops =
            (get_sequence_flags(model.as_deref(), i32::from(sequence)) & STUDIO_LOOPING) != 0;
        self.sequence_finished = false;
        let now = eng.time() as f32;
        if let Some(v) = eng.ent_mut(id) {
            v.framerate = 1.0;
            v.animtime = now;
        }
    }

    fn studio_frame_advance(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let now = eng.time() as f32;
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        let mut interval = now - v.animtime;
        if interval <= 0.001 {
            v.animtime = now;
            return;
        }
        if v.animtime == 0.0 {
            interval = 0.0;
        }
        v.frame += interval * self.frame_rate * v.framerate;
        v.animtime = now;
        if v.frame < 0.0 || f64::from(v.frame) >= 256.0 {
            if self.sequence_loops {
                let frame = f64::from(v.frame);
                v.frame = (frame - f64::from((frame / 256.0) as i32) * 256.0) as f32;
            } else {
                v.frame = if v.frame < 0.0 { 0.0 } else { 255.0 };
            }
            self.sequence_finished = true;
        }
    }

    fn remove_self(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.takedamage = false;
        self.next_full_think = -1.0;
        self.dead_remove_at = None;
        if let Some(v) = eng.ent_mut(id) {
            v.movetype = MOVETYPE_NONE;
            v.solid = SOLID_NOT;
            v.nextthink = 0.0;
        }
        eng.set_size(id, Vec3::ZERO, Vec3::ZERO);
    }

    fn check_rescue(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) -> bool {
        let Some(leader_id) = self.target_ent else {
            return false;
        };
        let Some(leader_slot) = player_slot(g, eng, leader_id) else {
            return false;
        };
        if g.players[leader_slot].as_ref().is_none_or(|p| p.team != Team::Ct) {
            return false;
        }

        if !self.rescue_me && !g.rules.map_has_rescue_zone() {
            let Some(my_origin) = eng.ent(id).map(|v| v.origin) else {
                return false;
            };
            let mut has_rescue_pt = false;
            let mut cursor = None;
            while let Some(spot) = eng.find_by_classname(cursor, "info_hostage_rescue") {
                has_rescue_pt = true;
                if let Some(sv) = eng.ent(spot)
                    && (sv.origin - my_origin).length() < MAX_HOSTAGES_RESCUE_RADIUS
                {
                    self.rescue_me = true;
                    break;
                }
                cursor = Some(spot);
            }
            if !has_rescue_pt && !self.rescue_me {
                let mut cursor = None;
                while let Some(spot) = eng.find_by_classname(cursor, "info_player_start") {
                    if let Some(sv) = eng.ent(spot)
                        && (sv.origin - my_origin).length() < MAX_HOSTAGES_RESCUE_RADIUS
                    {
                        self.rescue_me = true;
                        break;
                    }
                    cursor = Some(spot);
                }
            }
        }

        if !self.rescue_me {
            return false;
        }

        self.rescued = true;
        self.state = State::Stand;
        self.target_ent = None;
        self.stopped_target_ent = None;
        if let Some(v) = eng.ent_mut(id) {
            v.deadflag = DEAD_RESPAWNABLE;
            v.effects |= EF_NODRAW;
        }
        add_account(g, eng, leader_slot as u8, REWARD_TAKEN_HOSTAGE, true);
        send_hostage_event_msg(g, eng, self.index);
        self.remove_self(eng, id);
        g.rules.hostages_rescued += 1;
        crate::rules::on_hostage_rescued(g, eng);
        crate::rules::broadcast(eng, "rescued");
        true
    }

    fn do_follow(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(leader_id) = self.target_ent else {
            return;
        };
        if eng.cvar_f32("hostage_stop") > 0.0 {
            self.state = State::Stand;
            self.target_ent = None;
            self.stopped_target_ent = None;
            return;
        }

        self.local_nav.set_target_ent(Some(leader_id));
        let (Some(hv), Some(lv)) = (eng.ent(id), eng.ent(leader_id)) else {
            return;
        };
        let now = eng.time();
        let my_origin = hv.origin;
        let my_flags = hv.flags;
        let my_movetype = hv.movetype;
        let my_vel = hv.velocity;
        let alive = hv.deadflag == DEAD_NO && hv.health > 0.0;
        let leader_vel = lv.velocity;
        let mut vec_dest = lv.origin + Vec3::new(0.0, 0.0, lv.mins.z);
        let dist_to_dest = (vec_dest - my_origin).length();
        let step_size = g.hostage_nav.step_size;

        if dist_to_dest < 80.0
            && (self.has_path
                || self.local_nav.path_traversable(
                    eng,
                    id,
                    step_size,
                    my_origin,
                    &mut vec_dest,
                    true,
                ) != PathTraversable::Empty)
        {
            return;
        }

        if my_flags & FL_ONGROUND != 0
            && self.last_path_check + self.path_check_interval < now
            && (!self.has_path || leader_vel.truncate().length() > 1.0)
        {
            self.last_path_check = now;
            if g.hostage_nav.can_run_immediate() {
                self.nav_ready(g, eng, id);
            } else {
                g.hostage_nav.enqueue_nav(id);
            }
        }

        if self.has_path {
            self.target_node = self.local_nav.get_furthest_traversable_node(
                eng,
                id,
                step_size,
                my_origin,
                &self.path_nodes,
                self.num_path_nodes,
                true,
            );
            if self.target_node == Some(0)
                && (self.path_nodes[0] - my_origin).truncate().length() < HOSTAGE_STEPSIZE
            {
                self.target_node = None;
            }
            if self.target_node.is_none() {
                self.has_path = false;
                self.path_check_interval = 0.1;
            }
        }

        if now < self.flinch_time {
            return;
        }

        if let Some(node_idx) = self.target_node {
            let target = self.path_nodes[node_idx];
            if my_flags & FL_ONGROUND != 0 {
                let look = vec_to_angles(target - my_origin);
                if let Some(v) = eng.ent_mut(id) {
                    v.angles = Vec3::new(0.0, look.y, 0.0);
                }
            }
            if my_movetype == MOVETYPE_FLY
                && let Some(v) = eng.ent_mut(id)
            {
                v.v_angle.x = -60.0;
            }
            self.move_toward(eng, id, step_size, leader_id, target);
            self.stuck = false;
        } else if alive && self.state == State::Follow && !self.stuck && dist_to_dest > 200.0 {
            self.stuck = true;
            self.stuck_time = now;
        }

        let current_vel = eng.ent(id).map_or(my_vel, |v| v.velocity);
        if my_flags & FL_ONGROUND != 0
            && self.path_acquired >= 0.0
            && self.path_acquired + 2.0 > now
            && (current_vel.truncate().length() < 1.0 || self.target_node.is_none())
        {
            self.wiggle(g, eng, id, step_size);
        }
    }

    fn move_toward(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        step_size: f32,
        leader_id: EdictId,
        target: Vec3,
    ) {
        let (Some(hv), Some(lv)) = (eng.ent(id), eng.ent(leader_id)) else {
            return;
        };
        let origin = hv.origin;
        let on_ground = hv.flags & FL_ONGROUND != 0;
        let leader_feet = lv.origin + Vec3::new(0.0, 0.0, lv.mins.z);

        let vec_move = target - origin;
        let yaw = vec_to_angles(vec_move).y;
        let (fwd, _, _) = cs_sim::pm::math::angle_vectors(Vec3::new(0.0, yaw, 0.0));
        let move_2d = vec_move.truncate().length();
        let step_2d = (fwd * step_size).truncate().length();
        let dist = if step_2d <= move_2d { step_2d } else { move_2d };
        let mut big_dest = origin + fwd * dist;

        let fwd_move = self
            .local_nav
            .path_traversable(eng, id, step_size, origin, &mut big_dest, false);
        if fwd_move != PathTraversable::Empty {
            let mut speed = 250.0_f32;
            if on_ground {
                speed = (leader_feet - origin).length();
                if speed >= 110.0 {
                    speed = if speed >= 250.0 { 400.0 } else { 300.0 };
                }
            }
            if let Some(v) = eng.ent_mut(id) {
                v.velocity.x = fwd.x * speed;
                v.velocity.y = fwd.y * speed;
                if fwd_move == PathTraversable::StepJumpable && v.flags & FL_ONGROUND != 0 {
                    v.velocity.z = 270.0;
                }
            }
        }
    }

    fn wiggle(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, step_size: f32) {
        const DIRS: [Vec3; 8] = [
            Vec3::new(50.0, 0.0, 0.0),
            Vec3::new(-50.0, 0.0, 0.0),
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(0.0, -50.0, 0.0),
            Vec3::new(50.0, 50.0, 0.0),
            Vec3::new(50.0, -50.0, 0.0),
            Vec3::new(-50.0, 50.0, 0.0),
            Vec3::new(-50.0, -50.0, 0.0),
        ];
        let Some(origin) = eng.ent(id).map(|v| v.origin) else {
            return;
        };
        let mut push = Vec3::ZERO;
        for dir in DIRS {
            let mut dest = origin + dir;
            if self
                .local_nav
                .path_traversable(eng, id, step_size, origin, &mut dest, true)
                == PathTraversable::Empty
            {
                push -= dir;
            }
        }
        push.x += random_float(g, -3.0, 3.0);
        push.y += random_float(g, -3.0, 3.0);
        let dir = push.normalize_or_zero();
        if let Some(v) = eng.ent_mut(id) {
            v.velocity += dir * 100.0;
        }
    }

    fn nav_ready(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(leader_id) = self.target_ent else {
            return;
        };
        let (Some(hv), Some(lv)) = (eng.ent(id), eng.ent(leader_id)) else {
            return;
        };
        let my_origin = hv.origin;
        let mut dest = lv.origin;
        let leader_mins_z = lv.mins.z;
        if lv.flags & FL_ONGROUND == 0 {
            let drop_dest = dest - Vec3::new(0.0, 0.0, 300.0);
            let tr = eng.trace_hull_ents(
                dest,
                drop_dest,
                Hull::Standing,
                MoveFilter::NoMonsters,
                Some(leader_id),
            );
            if tr.trace.start_solid || tr.trace.fraction == 1.0 {
                return;
            }
            dest = tr.trace.end_pos;
        }
        dest.z += leader_mins_z;
        self.local_nav.set_target_ent(Some(leader_id));

        let path_node = self
            .local_nav
            .find_path(eng, id, &mut g.hostage_nav, my_origin, dest, 40.0, true);
        if let Some(last) = path_node {
            self.has_path = true;
            self.target_node = None;
            self.path_acquired = eng.time();
            self.path_check_interval = 0.5;
            self.num_path_nodes = self.local_nav.setup_path_nodes(last, &mut self.path_nodes);
        } else if !self.has_path {
            self.path_check_interval = (self.path_check_interval + 0.1).min(0.5);
        }
    }

    fn play_follow_sound(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let sample = match g.random_range(0, 4) {
            0 => "hostage/hos1.wav",
            1 => "hostage/hos2.wav",
            2 => "hostage/hos3.wav",
            3 => "hostage/hos4.wav",
            _ => "hostage/hos5.wav",
        };
        eng.emit_sound(id, CHAN_VOICE, sample, 1.0, ATTN_NORM, PITCH_NORM);
    }

    fn give_ct_touch_bonus(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        slot: u8,
    ) {
        if self.touched {
            return;
        }
        self.touched = true;
        g.rules.add_ct_account(REWARD_TOOK_HOSTAGE_ACC);
        g.rules.hostages_touched += 1;
        add_account(g, eng, slot, REWARD_TOOK_HOSTAGE, true);
    }
}

impl GameEntity for Hostage {
    fn classname(&self) -> &'static str {
        "hostage_entity"
    }

    fn is_hostage(&self) -> bool {
        true
    }

    fn set_hostage_index(&mut self, idx: u8) {
        self.index = idx;
    }

    fn hostage_index(&self) -> Option<u8> {
        (self.index != 0).then_some(self.index)
    }

    fn mark_hostage_rescue(&mut self) {
        self.rescue_me = true;
    }

    fn takes_damage(&self) -> bool {
        self.takedamage
    }

    fn blood_color(&self) -> i32 {
        i32::from(BLOOD_COLOR_RED)
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION | FCAP_ONOFF_USE
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let key_model = keys
            .iter()
            .find(|(k, _)| k == "model")
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty());
        let existing_model = eng
            .ent(id)
            .map(|v| v.model.as_str())
            .filter(|v| !v.is_empty());
        let model = key_model
            .or(existing_model)
            .unwrap_or("models/scientist.mdl")
            .to_string();

        let _ = eng.precache_model(&model);
        let _ = eng.precache_sound("hostage/hos1.wav");
        let _ = eng.precache_sound("hostage/hos2.wav");
        let _ = eng.precache_sound("hostage/hos3.wav");
        let _ = eng.precache_sound("hostage/hos4.wav");
        let _ = eng.precache_sound("hostage/hos5.wav");
        let _ = eng.precache_model("sprites/smoke.spr");
        self.model = model.clone();

        if let Some(v) = eng.ent_mut(id) {
            v.classname = "hostage_entity".to_string();
            v.movetype = MOVETYPE_STEP;
            v.solid = SOLID_SLIDEBOX;
            v.flags |= FL_MONSTER;
            if v.spawnflags & SF_MONSTER_HITMONSTERCLIP != 0 {
                v.flags |= FL_MONSTERCLIP;
            }
            if v.skin < 0 {
                v.skin = 0;
            }
            v.deadflag = DEAD_NO;
            v.health = HOSTAGE_MAX_HEALTH;
            v.gravity = 1.0;
            v.view_ofs = VEC_HOSTAGE_VIEW;
            v.velocity = Vec3::ZERO;
        }
        self.takedamage = true;
        self.state = State::Stand;
        self.target_ent = None;
        self.stopped_target_ent = None;
        self.next_change = 0.0;
        self.flinch_time = 0.0;
        self.rescue_me = false;
        self.rescued = false;
        self.dead_remove_at = None;

        eng.set_model(id, &model);
        eng.set_size(id, VEC_HOSTAGE_HULL_MIN, VEC_HOSTAGE_HULL_MAX);
        let _ = eng.drop_to_floor(id);

        let now = eng.time();
        let mut spawn_rng = u32::from(id.index) ^ 0x9e37_79b9;
        self.set_activity(eng, id, ACT_IDLE, &mut spawn_rng);

        if let Some(v) = eng.ent_mut(id) {
            self.start_origin = v.origin;
            self.start_angles = v.angles;
            v.nextthink = now + 0.1;
        }
        self.old_pos = Vec3::splat(9999.0);
        self.next_full_think = now + 0.1;
        self.next_radar_time = now + 0.5;
        self.last_path_check = -1.0;
        self.path_acquired = -1.0;
        self.path_check_interval = 0.1;
        true
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let now = eng.time();
        if self.dead_remove_at.is_some_and(|at| now >= at) {
            self.remove_self(eng, id);
            return;
        }

        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = now + GIVE_UP_TIME;
        }

        self.studio_frame_advance(eng, id);

        if self.next_full_think < 0.0 || self.next_full_think > now {
            return;
        }
        self.next_full_think = now + UPDATE_RATE;

        let Some((deadflag, effects, origin, vel)) = eng
            .ent(id)
            .map(|v| (v.deadflag, v.effects, v.origin, v.velocity))
        else {
            return;
        };

        if deadflag == DEAD_DEAD {
            eng.set_size(id, Vec3::ZERO, Vec3::ZERO);
            return;
        }

        if let Some(leader_id) = self.target_ent {
            let leader_dead = eng.ent(leader_id).is_none_or(|lv| lv.deadflag != DEAD_NO);
            if (self.stuck && now - self.stuck_time > 5.0) || leader_dead {
                self.state = State::Stand;
                self.target_ent = None;
                self.stuck = false;
            }
        }

        if self.check_rescue(g, eng, id) {
            return;
        }

        self.do_follow(g, eng, id);

        if now >= self.flinch_time {
            let speed = eng.ent(id).map_or(vel, |v| v.velocity).length();
            let act = if speed > 160.0 {
                ACT_RUN
            } else if speed > 15.0 {
                ACT_WALK
            } else {
                ACT_IDLE
            };
            let mut rng = g.rng;
            self.set_activity(eng, id, act, &mut rng);
            g.rng = rng;
        }

        let current_origin = eng.ent(id).map_or(origin, |v| v.origin);
        if deadflag != DEAD_DEAD && effects & EF_NODRAW == 0 && self.next_radar_time <= now {
            if (self.old_pos - current_origin).length() > 1.0 {
                self.old_pos = current_origin;
                if !g.rules.is_restarting() {
                    send_hostage_position_msg(g, eng, self.index, current_origin);
                }
            }
            self.next_radar_time = now + 1.0;
        }
    }

    fn hostage_nav_ready(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.nav_ready(g, eng, id);
    }

    fn hostage_pre_think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(hv) = eng.ent(id) else {
            return;
        };
        if hv.flags & FL_ONGROUND == 0 || hv.velocity.truncate().length() < 1.0 {
            return;
        }
        let step_size = g.hostage_nav.step_size;
        let frametime = eng.frametime();
        let src = hv.origin;
        let vel = hv.velocity;
        let mut dest = src + vel * frametime;
        dest.z = src.z;

        let tr = eng.trace_monster_hull(id, src, dest, MoveFilter::Normal, Some(id));
        if tr.trace.start_solid || tr.trace.fraction == 1.0 || tr.trace.plane_normal.z > MAX_UNIT_Z_SLOPE
        {
            return;
        }

        let orig_dist = (tr.trace.end_pos - src).truncate().length();
        let up_src = src + Vec3::new(0.0, 0.0, step_size);
        let mut up_dest = up_src + vel.normalize_or_zero() * 0.1;
        up_dest.z = up_src.z;

        let tr_up = eng.trace_monster_hull(id, up_src, up_dest, MoveFilter::Normal, Some(id));
        if tr_up.trace.start_solid {
            return;
        }
        let down_src = tr_up.trace.end_pos;
        let down_dest = down_src - Vec3::new(0.0, 0.0, step_size);
        let tr_down = eng.trace_monster_hull(id, down_src, down_dest, MoveFilter::Normal, Some(id));
        if tr_down.trace.plane_normal.z < MAX_UNIT_Z_SLOPE {
            return;
        }
        let raised_dist = (tr_down.trace.end_pos - src).truncate().length();
        if raised_dist > orig_dist {
            let new_origin = Vec3::new(src.x, src.y, tr_down.trace.end_pos.z);
            eng.set_origin(id, new_origin);
            let gravity = eng.cvar_f32("sv_gravity");
            if let Some(v) = eng.ent_mut(id) {
                v.velocity.z += v.gravity * gravity * frametime;
            }
        }
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
        let Some(act_id) = activator else {
            return;
        };
        let Some(slot) = player_slot(g, eng, act_id) else {
            return;
        };
        if !self.takedamage {
            return;
        }
        let team = match g.players[slot].as_ref() {
            Some(p) => p.team,
            None => return,
        };
        let slot_u8 = slot as u8;
        if team != Team::Ct {
            if first_time(g, slot_u8, dhf::HOSTAGE_CTMOVE) {
                hint_message(g, eng, slot_u8, "#Only_CT_Can_Move_Hostages", false, true);
            }
            return;
        }

        let now = eng.time();
        if now < self.next_change {
            return;
        }
        self.next_change = now + 1.0;
        self.path_acquired = now;

        if self.target_ent != Some(act_id) {
            self.state = State::Follow;
            self.target_ent = Some(act_id);
            self.stopped_target_ent = None;
        } else if self.state == State::Follow {
            self.state = State::Stand;
            self.target_ent = None;
            self.stopped_target_ent = Some(act_id);
        } else {
            self.state = State::Follow;
        }

        if self.state == State::Follow {
            self.play_follow_sound(g, eng, id);
        }

        self.give_ct_touch_bonus(g, eng, slot_u8);
        if first_time(g, slot_u8, dhf::HOSTAGE_USED) {
            hint(g, eng, slot_u8, "#Hint_lead_hostage_to_rescue_point");
        }
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        const PUSH_FORCE: f32 = 50.0;
        let is_ct_player = player_slot(g, eng, other)
            .and_then(|s| g.players[s].as_ref())
            .is_some_and(|p| p.team == Team::Ct);
        let is_other_hostage = eng
            .ent(other)
            .is_some_and(|v| v.classname == "hostage_entity");
        if !is_ct_player && !is_other_hostage {
            return;
        }
        let (Some(hv), Some(ov)) = (eng.ent(id), eng.ent(other)) else {
            return;
        };
        let mut push = hv.origin - ov.origin;
        push.z = 0.0;
        let dir = push.normalize_or_zero();
        if dir == Vec3::ZERO {
            return;
        }
        if let Some(v) = eng.ent_mut(id) {
            v.velocity.x += dir.x * PUSH_FORCE;
            v.velocity.y += dir.y * PUSH_FORCE;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_attack(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        attacker: Option<EdictId>,
        mm: &mut MultiDamage,
        damage: f32,
        dir: Vec3,
        tr: &EntTrace,
        bits: u32,
    ) {
        if !self.takedamage {
            return;
        }
        self.last_hit_group = tr.trace.hitgroup;
        let scaled = match tr.trace.hitgroup {
            HITGROUP_HEAD => damage * 3.0,
            HITGROUP_CHEST | HITGROUP_STOMACH => damage * 1.5,
            HITGROUP_LEFTARM | HITGROUP_RIGHTARM => damage,
            HITGROUP_LEFTLEG | HITGROUP_RIGHTLEG => damage * 0.75,
            HITGROUP_SHIELD => 0.0,
            _ => damage,
        };
        add_multi_damage(g, eng, mm, attacker, id, scaled, bits);
        let spot = tr.trace.end_pos - dir * 4.0;
        spawn_blood(eng, spot, BLOOD_COLOR_RED, scaled);
        trace_bleed(g, eng, id, scaled, dir, tr.trace.end_pos, bits);
    }

    #[allow(clippy::too_many_arguments)]
    fn take_damage(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _inflictor: Option<EdictId>,
        attacker: Option<EdictId>,
        damage: f32,
        _bits: u32,
    ) -> bool {
        if !self.takedamage {
            return false;
        }
        let mut actual_damage = modified_damage(damage, self.last_hit_group);
        let Some(health) = eng.ent(id).map(|v| v.health) else {
            return false;
        };
        if actual_damage > health {
            actual_damage = health;
        }
        let new_health = health - actual_damage;
        if let Some(v) = eng.ent_mut(id) {
            v.health = new_health;
        }

        if self.last_hit_group == HITGROUP_HEAD {
            let sample = if g.random_range(0, 1) == 0 {
                "player/headshot1.wav"
            } else {
                "player/headshot2.wav"
            };
            eng.emit_sound(id, CHAN_VOICE, sample, 1.0, ATTN_NORM, PITCH_NORM);
        }

        let attacker_slot = attacker.and_then(|a| player_slot(g, eng, a));
        let now = eng.time();

        if new_health > 0.0 {
            self.flinch_time = now + 0.75;
            let mut rng = g.rng;
            self.set_activity(eng, id, ACT_SMALL_FLINCH, &mut rng);
            g.rng = rng;

            if let Some(slot) = attacker_slot {
                let slot_u8 = slot as u8;
                add_account(g, eng, slot_u8, -20 * (actual_damage as i32), true);
                print_center(eng, slot_u8, "#Injured_Hostage");
                if first_time(g, slot_u8, dhf::HOSTAGE_INJURED) {
                    hint(g, eng, slot_u8, "#Hint_careful_around_hostages");
                }
                return true;
            }
            return false;
        }

        if let Some(v) = eng.ent_mut(id) {
            v.health = 0.0;
            v.movetype = MOVETYPE_TOSS;
            v.flags &= !FL_ONGROUND;
            v.deadflag = DEAD_DEAD;
            v.solid = SOLID_NOT;
            v.nextthink = now + 3.0;
        }
        self.takedamage = false;
        self.target_ent = None;
        self.state = State::Stand;
        self.dead_remove_at = Some(now + 3.0);

        let death_act = match self.last_hit_group {
            HITGROUP_GENERIC | HITGROUP_HEAD => ACT_DIE_HEADSHOT,
            HITGROUP_CHEST | HITGROUP_RIGHTARM => ACT_DIESIMPLE,
            HITGROUP_STOMACH | HITGROUP_RIGHTLEG => ACT_DIEFORWARD,
            HITGROUP_LEFTARM | HITGROUP_LEFTLEG => ACT_DIEBACKWARD,
            _ => ACT_DIESIMPLE,
        };
        let mut rng = g.rng;
        self.set_activity(eng, id, death_act, &mut rng);
        g.rng = rng;

        if let Some(slot) = attacker_slot {
            let slot_u8 = slot as u8;
            add_account(
                g,
                eng,
                slot_u8,
                20 * (-25 - (actual_damage as i32)),
                true,
            );
            print_center(eng, slot_u8, "#Killed_Hostage");
            if first_time(g, slot_u8, dhf::HOSTAGE_KILLED) {
                hint(g, eng, slot_u8, "#Hint_lost_money");
            }
            apply_hostage_penalty(g, eng, slot_u8);
        }

        if !g.rules.is_restarting() {
            send_hostage_event_msg(g, eng, self.index);
        }
        crate::rules::on_hostage_killed(g, eng);
        false
    }

    fn restart(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.takedamage = true;
        self.state = State::Stand;
        self.target_ent = None;
        self.stopped_target_ent = None;
        self.touched = false;
        self.rescue_me = false;
        self.rescued = false;
        self.dead_remove_at = None;
        self.has_path = false;
        self.target_node = None;
        self.stuck = false;
        self.last_hit_group = HITGROUP_GENERIC;
        self.last_path_check = -1.0;
        self.path_acquired = -1.0;
        self.path_check_interval = 0.1;
        self.next_radar_time = 0.0;
        self.old_pos = Vec3::splat(9999.0);
        self.flinch_time = 0.0;

        if let Some(v) = eng.ent_mut(id) {
            v.health = HOSTAGE_MAX_HEALTH;
            v.movetype = MOVETYPE_STEP;
            v.solid = SOLID_SLIDEBOX;
            v.deadflag = DEAD_NO;
            v.velocity = Vec3::ZERO;
            v.angles = self.start_angles;
            v.effects &= !EF_NODRAW;
        }
        eng.set_origin(id, self.start_origin);
        eng.set_size(id, VEC_HOSTAGE_HULL_MIN, VEC_HOSTAGE_HULL_MAX);
        let _ = eng.drop_to_floor(id);

        let now = eng.time();
        self.activity = ACT_INVALID;
        let mut rng = g.rng;
        self.set_activity(eng, id, ACT_IDLE, &mut rng);
        g.rng = rng;

        let delay = f64::from(random_float(g, 0.1, 0.2));
        self.next_full_think = now + delay;
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = now + delay;
        }
    }
}

fn modified_damage(damage: f32, hitgroup: i32) -> f32 {
    let mult = match hitgroup {
        HITGROUP_GENERIC => 1.75,
        HITGROUP_HEAD => 2.5,
        HITGROUP_CHEST => 1.5,
        HITGROUP_STOMACH => 1.75,
        HITGROUP_LEFTARM | HITGROUP_RIGHTARM => 0.75,
        HITGROUP_LEFTLEG | HITGROUP_RIGHTLEG => 0.6,
        _ => 1.5,
    };
    damage * mult
}

fn apply_hostage_penalty(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(team) = g.players.get(usize::from(slot)).and_then(Option::as_ref).map(|p| p.team)
    else {
        return;
    };
    if team != Team::Terrorist {
        return;
    }
    let penalty = eng.cvar_f32("mp_hostagepenalty") as i32;
    if penalty <= 0 {
        return;
    }
    let Some(p) = g.players[usize::from(slot)].as_mut() else {
        return;
    };
    let prev = p.hostages_killed;
    p.hostages_killed += 1;
    if prev == penalty {
        hint_message(
            g,
            eng,
            slot,
            "#Hint_removed_for_next_hostage_killed",
            true,
            false,
        );
    } else if prev >= penalty {
        eng.kick(slot, "For killing too many hostages");
    }
}

fn send_hostage_position_msg(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    hostage_id: u8,
    origin: Vec3,
) {
    for (slot, slot_p) in g.players.iter().enumerate() {
        let Some(p) = slot_p.as_ref() else {
            continue;
        };
        if p.team != Team::Ct {
            continue;
        }
        let id = eng.client_edict(slot as u8);
        if eng.ent(id).is_none_or(|v| v.deadflag != DEAD_NO) {
            continue;
        }
        eng.send_usermsg(
            MsgDest::One(slot as u8),
            &CsUserMsg::HostagePos {
                flag: 0,
                hostage_id,
                origin,
            },
        );
    }
}

fn send_hostage_event_msg(g: &GameState, eng: &mut EngineApi<'_>, hostage_id: u8) {
    for (slot, slot_p) in g.players.iter().enumerate() {
        let Some(p) = slot_p.as_ref() else {
            continue;
        };
        if p.team != Team::Ct {
            continue;
        }
        let id = eng.client_edict(slot as u8);
        if eng.ent(id).is_none_or(|v| v.deadflag != DEAD_NO) {
            continue;
        }
        eng.send_usermsg(MsgDest::One(slot as u8), &CsUserMsg::HostageK(hostage_id));
    }
}

/// `CBasePlayer::SendHostagePos`: tells the spawning player where every hostage is (`flag: 1`).
pub(crate) fn send_hostage_pos_to_player(_g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "hostage_entity") {
        let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        let hostage_id = eng
            .private::<Box<dyn GameEntity>>(id)
            .and_then(|e| e.hostage_index())
            .unwrap_or(0);
        if hostage_id != 0 {
            eng.send_usermsg(
                MsgDest::One(slot),
                &CsUserMsg::HostagePos {
                    flag: 1,
                    hostage_id,
                    origin,
                },
            );
        }
        cursor = Some(id);
    }
}

/// Whether `id` is a `hostage_entity`.
pub(crate) fn entity_is_hostage(eng: &EngineApi<'_>, id: EdictId) -> bool {
    eng.private::<Box<dyn GameEntity>>(id)
        .is_some_and(|e| e.is_hostage())
}

/// `RestartRound` (`CHostage::RePosition` for every `"hostage_entity"`).
pub(crate) fn restart_round(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let mut ids = Vec::new();
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "hostage_entity") {
        ids.push(id);
        cursor = Some(id);
    }
    for id in ids {
        with_entity(g, eng, id, |entity, g, eng| {
            entity.restart(g, eng, id);
        });
    }
}

/// Test/bot helper using [`random_in`] directly when testing damage/sound distribution.
#[allow(dead_code)]
fn _keep_random_in(seed: &mut u32) -> i32 {
    random_in(seed, 0, 1)
}
