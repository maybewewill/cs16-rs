//! Entity I/O (`multi_manager`, `multisource`, `env_global`), `game_*` map rule entities,
//! and `player_weaponstrip` ported from ReGameDLL_CS (`subs.cpp`, `triggers.cpp`, `maprules.cpp`).

use cs_engine::consts::{FL_KILLME, SOLID_NOT};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::entity::{FCAP_ACROSS_TRANSITION, GameEntity, UseType, util_remove};
use crate::inventory::remove_all_items;
use crate::menus::atoi;
use crate::messages;
use crate::player::{self, Team, dmg, player_slot};
use crate::rules;
use crate::targets::{fire_targets, is_master_triggered, use_targets};
use crate::teams::team_name;
use crate::world::atof;

const SF_MULTIMAN_THREAD: u32 = 1 << 0;
const SF_MULTIMAN_CLONE: u32 = 1 << 31;

const SF_GLOBAL_SET: u32 = 1 << 0;

const SF_GAMECOUNT_FIREONCE: u32 = 1 << 0;
const SF_GAMECOUNT_RESET: u32 = 1 << 1;

const SF_PKILL_FIREONCE: u32 = 1 << 0;

const SF_PTEAM_KILL: u32 = 1 << 0;
const SF_PTEAM_GIB: u32 = 1 << 1;
const SF_PTEAM_FIREONCE: u32 = 1 << 2;

const SF_SCORE_NEGATIVE: u32 = 1 << 0;
const SF_SCORE_TEAM: u32 = 1 << 1;

const SF_TEAMMASTER_FIREONCE: u32 = 1 << 0;

const SF_ENVTEXT_ALLPLAYERS: u32 = 1 << 0;

const SF_GZP_NO_CLIENTS: u32 = 1 << 0;

const STANDARD_KEYS: &[&str] = &[
    "classname",
    "origin",
    "targetname",
    "target",
    "spawnflags",
    "angles",
    "angle",
    "delay",
    "killtarget",
    "master",
    "wait",
    "model",
];

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn parse_rgb(s: &str) -> [u8; 3] {
    let mut parts = s.split_whitespace().map(|p| atoi(p).clamp(0, 255) as u8);
    [
        parts.next().unwrap_or(255),
        parts.next().unwrap_or(255),
        parts.next().unwrap_or(255),
    ]
}

fn parse_team(s: &str) -> Option<Team> {
    let trim = s.trim();
    match trim {
        "1" => Some(Team::Terrorist),
        "2" => Some(Team::Ct),
        "3" => Some(Team::Spectator),
        _ => {
            if trim.eq_ignore_ascii_case("TERRORIST") {
                Some(Team::Terrorist)
            } else if trim.eq_ignore_ascii_case("CT") {
                Some(Team::Ct)
            } else if trim.eq_ignore_ascii_case("SPECTATOR") {
                Some(Team::Spectator)
            } else {
                None
            }
        }
    }
}

/// `multi_manager` (`CMultiManager`): schedules sorted target activations at per-key delays.
#[derive(Default)]
pub(crate) struct MultiManager {
    entries: Vec<(String, f32)>,
    index: usize,
    start_time: f64,
    activator: Option<EdictId>,
    active: bool,
}

impl GameEntity for MultiManager {
    fn classname(&self) -> &'static str {
        "multi_manager"
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        let mut entries = Vec::new();
        for (k, v) in keys {
            if STANDARD_KEYS.iter().any(|s| s.eq_ignore_ascii_case(k)) {
                continue;
            }
            let target = k.split('#').next().unwrap_or(k).to_string();
            if target.is_empty() {
                continue;
            }
            entries.push((target, atof(v).max(0.0)));
        }
        entries.sort_by(|a, b| a.1.total_cmp(&b.1));
        self.entries = entries;
        self.index = 0;
        self.active = false;
        self.activator = None;
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
        let spawnflags = eng.ent(id).map_or(0, |v| v.spawnflags);
        if self.active && (spawnflags & (SF_MULTIMAN_THREAD | SF_MULTIMAN_CLONE)) == 0 {
            return;
        }
        if self.entries.is_empty() {
            return;
        }
        self.active = true;
        self.index = 0;
        self.start_time = eng.time();
        self.activator = activator;
        self.fire_due(g, eng, id);
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if self.active {
            self.fire_due(g, eng, id);
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.active = false;
        self.index = 0;
        self.activator = None;
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = 0.0;
        }
    }
}

impl MultiManager {
    fn fire_due(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let elapsed = (eng.time() - self.start_time) as f32;
        while self.index < self.entries.len() && self.entries[self.index].1 <= elapsed + 1e-4 {
            let target = self.entries[self.index].0.clone();
            self.index += 1;
            fire_targets(g, eng, &target, self.activator, Some(id), UseType::Toggle, 0.0);
        }
        if self.index < self.entries.len() {
            let next_delay = f64::from(self.entries[self.index].1);
            if let Some(v) = eng.ent_mut(id) {
                v.nextthink = self.start_time + next_delay;
            }
        } else {
            self.active = false;
            if let Some(v) = eng.ent_mut(id) {
                v.nextthink = 0.0;
            }
        }
    }
}

/// `multisource` (`CMultiSource`): multi-input master gate that requires all callers active.
#[derive(Default)]
pub(crate) struct MultiSource {
    globalstate: String,
    entities: Vec<EdictId>,
    triggered: u32,
}

impl GameEntity for MultiSource {
    fn classname(&self) -> &'static str {
        "multisource"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.globalstate = key_str(keys, "globalstate").unwrap_or("").to_string();
        self.entities.clear();
        self.triggered = 0;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        let source = caller.unwrap_or(id);
        let idx = if let Some(pos) = self.entities.iter().position(|&e| e == source) {
            pos
        } else if self.entities.len() < 32 {
            self.entities.push(source);
            self.entities.len() - 1
        } else {
            0
        };
        let mask = 1_u32 << idx;
        match use_type {
            UseType::On => self.triggered |= mask,
            UseType::Off => self.triggered &= !mask,
            UseType::Toggle => self.triggered ^= mask,
            UseType::Set => {}
        }
        if self.is_triggered(g, eng, id, activator) {
            use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
        }
    }

    fn is_triggered(
        &self,
        g: &GameState,
        _eng: &EngineApi<'_>,
        _id: EdictId,
        _activator: Option<EdictId>,
    ) -> bool {
        if !self.globalstate.is_empty() && g.global_states.get(&self.globalstate).copied().unwrap_or(0) != 1 {
            return false;
        }
        if self.entities.is_empty() {
            return !self.globalstate.is_empty();
        }
        let expected = if self.entities.len() >= 32 {
            u32::MAX
        } else {
            (1_u32 << self.entities.len()) - 1
        };
        (self.triggered & expected) == expected
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.triggered = 0;
    }
}

/// `env_global` (`CEnvGlobal`): reads/writes persistent named global states in `GameState::global_states`.
#[derive(Default)]
pub(crate) struct EnvGlobal {
    globalstate: String,
    triggermode: i32,
    initialstate: i32,
    spawn_init_done: bool,
}

impl GameEntity for EnvGlobal {
    fn classname(&self) -> &'static str {
        "env_global"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.globalstate = key_str(keys, "globalstate").unwrap_or("").to_string();
        self.triggermode = key_str(keys, "triggermode").map_or(0, atoi);
        self.initialstate = key_str(keys, "initialstate").map_or(0, atoi);
        if self.globalstate.is_empty() {
            return false;
        }
        let due = eng.time() + 0.05;
        if let Some(v) = eng.ent_mut(id)
            && v.spawnflags & SF_GLOBAL_SET != 0
        {
            v.nextthink = due;
        }
        self.spawn_init_done = false;
        true
    }

    fn think(&mut self, g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        if !self.spawn_init_done {
            self.spawn_init_done = true;
            g.global_states.insert(self.globalstate.clone(), self.initialstate);
        }
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        let old = g.global_states.get(&self.globalstate).copied().unwrap_or(0);
        if old == 2 {
            return;
        }
        let next = match use_type {
            UseType::Off => 0,
            UseType::On => 1,
            UseType::Set => old,
            UseType::Toggle => match self.triggermode {
                0 => 0,
                1 => 1,
                2 => 2,
                _ => i32::from(old == 0),
            },
        };
        g.global_states.insert(self.globalstate.clone(), next);
        if !eng.ent(id).is_some_and(|v| v.target.is_empty()) {
            use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
        }
    }
}

/// `game_counter` (`CGameCounter`): increments count on `use_` and fires `target` at threshold.
#[derive(Default)]
pub(crate) struct GameCounter {
    initial_count: f32,
    threshold: f32,
    master: String,
    disabled: bool,
}

impl GameEntity for GameCounter {
    fn classname(&self) -> &'static str {
        "game_counter"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.threshold = key_str(keys, "frags").map_or(1.0, atof);
        self.initial_count = key_str(keys, "health").map_or(0.0, atof);
        self.disabled = false;
        if let Some(v) = eng.ent_mut(id) {
            v.health = self.initial_count;
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
        use_type: UseType,
        value: f32,
    ) {
        if self.disabled || !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        match use_type {
            UseType::Set => v.health = value,
            UseType::Off => v.health -= 1.0,
            UseType::On | UseType::Toggle => v.health += 1.0,
        }
        let count = v.health;
        let spawnflags = v.spawnflags;
        if count >= self.threshold {
            if spawnflags & SF_GAMECOUNT_FIREONCE != 0 {
                self.disabled = true;
            }
            if spawnflags & SF_GAMECOUNT_RESET != 0 {
                if let Some(v) = eng.ent_mut(id) {
                    v.health = self.initial_count;
                }
            }
            use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.disabled = false;
        if let Some(v) = eng.ent_mut(id) {
            v.health = self.initial_count;
        }
    }
}

/// `game_counter_set` (`CGameCounterSet`): sets `target` `game_counter` value via `UseType::Set`.
#[derive(Default)]
pub(crate) struct GameCounterSet {
    master: String,
    set_value: f32,
}

impl GameEntity for GameCounterSet {
    fn classname(&self) -> &'static str {
        "game_counter_set"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.set_value = key_str(keys, "frags").map_or(0.0, atof);
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
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        use_targets(g, eng, id, activator, UseType::Set, self.set_value);
    }
}

/// `game_end` (`CGameEnd`): ends the current match and enters intermission.
#[derive(Default)]
pub(crate) struct GameEnd {
    master: String,
}

impl GameEntity for GameEnd {
    fn classname(&self) -> &'static str {
        "game_end"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        true
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
        if is_master_triggered(g, eng, &self.master, activator) {
            rules::go_to_intermission(g, eng);
        }
    }
}

/// `game_player_hurt` (`CGamePlayerHurt`): hurts the activating player when used.
#[derive(Default)]
pub(crate) struct GamePlayerHurt {
    master: String,
    damage: f32,
    disabled: bool,
}

impl GameEntity for GamePlayerHurt {
    fn classname(&self) -> &'static str {
        "game_player_hurt"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.damage = key_str(keys, "dmg").map_or(10.0, atof);
        self.disabled = false;
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
        if self.disabled || !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        let Some(act) = activator else {
            return;
        };
        if let Some(slot) = player_slot(g, eng, act) {
            player::take_damage(g, eng, slot as u8, Some(id), Some(id), self.damage, dmg::GENERIC);
        }
        if eng.ent(id).is_some_and(|v| v.spawnflags & SF_PKILL_FIREONCE != 0) {
            self.disabled = true;
        }
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.disabled = false;
    }
}

/// `game_player_team` (`CGamePlayerTeam`): moves the activating player to the team named by `target`.
#[derive(Default)]
pub(crate) struct GamePlayerTeam {
    master: String,
    disabled: bool,
}

impl GameEntity for GamePlayerTeam {
    fn classname(&self) -> &'static str {
        "game_player_team"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.disabled = false;
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
        if self.disabled || !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        let Some(act) = activator else {
            return;
        };
        let Some(slot) = player_slot(g, eng, act) else {
            return;
        };
        let Some((target, spawnflags)) = eng.ent(id).map(|v| (v.target.clone(), v.spawnflags)) else {
            return;
        };
        let Some(new_team) = g
            .global_states
            .get(&format!("__team_master:{target}"))
            .copied()
            .and_then(|t| match t {
                1 => Some(Team::Terrorist),
                2 => Some(Team::Ct),
                3 => Some(Team::Spectator),
                _ => None,
            })
            .or_else(|| parse_team(&target))
        else {
            return;
        };

        if let Some(p) = g.players[slot].as_mut() {
            p.team = new_team;
        }
        eng.send_usermsg(
            MsgDest::All,
            &CsUserMsg::TeamInfo {
                player_id: slot as u8 + 1,
                team_name: team_name(new_team).to_string(),
            },
        );
        if let Some(p) = g.players[slot].as_ref() {
            messages::score_info(eng, act.index, p);
        }
        if spawnflags & (SF_PTEAM_KILL | SF_PTEAM_GIB) != 0 {
            player::take_damage(g, eng, slot as u8, Some(id), Some(id), 10000.0, dmg::GENERIC);
        }
        if spawnflags & SF_PTEAM_FIREONCE != 0 {
            self.disabled = true;
        }
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.disabled = false;
    }
}

/// `game_score` (`CGameScore`): awards frags to a player or their team.
#[derive(Default)]
pub(crate) struct GameScore {
    master: String,
    points: i32,
}

impl GameEntity for GameScore {
    fn classname(&self) -> &'static str {
        "game_score"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.points = key_str(keys, "points").map_or(1, atoi);
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
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        let Some(act) = activator else {
            return;
        };
        let Some(slot) = player_slot(g, eng, act) else {
            return;
        };
        let spawnflags = eng.ent(id).map_or(0, |v| v.spawnflags);
        let delta = if spawnflags & SF_SCORE_NEGATIVE != 0 {
            -self.points
        } else {
            self.points
        };
        if spawnflags & SF_SCORE_TEAM != 0 {
            let team = g.players[slot].as_ref().map(|p| p.team);
            if let Some(team) = team {
                match team {
                    Team::Terrorist => g.num_terrorist_wins += delta,
                    Team::Ct => g.num_ct_wins += delta,
                    _ => {}
                }
                let score = match team {
                    Team::Terrorist => g.num_terrorist_wins,
                    Team::Ct => g.num_ct_wins,
                    _ => 0,
                }
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                eng.send_usermsg(
                    MsgDest::All,
                    &CsUserMsg::TeamScore {
                        team_name: team_name(team).to_string(),
                        score,
                    },
                );
            }
        } else if let Some(p) = g.players[slot].as_mut() {
            p.frags += delta;
            let edict_idx = eng.client_edict(slot as u8).index;
            messages::score_info(eng, edict_idx, p);
        }
    }
}

/// `game_team_master` (`CGameTeamMaster`): master entity checking whether the activator is on a specific team.
#[derive(Default)]
pub(crate) struct GameTeamMaster {
    master: String,
    team_index: i32,
    trigger_type: UseType,
    disabled: bool,
}

impl GameEntity for GameTeamMaster {
    fn classname(&self) -> &'static str {
        "game_team_master"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.team_index = key_str(keys, "teamindex").map_or(-1, atoi);
        self.trigger_type = match key_str(keys, "triggerstate").map_or(0, atoi) {
            1 => UseType::On,
            2 => UseType::Toggle,
            _ => UseType::Off,
        };
        self.disabled = false;
        let _ = (eng, id);
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        value: f32,
    ) {
        if self.disabled || !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        if use_type == UseType::Set {
            self.team_index = value as i32;
            if let Some(name) = eng.ent(id).map(|v| v.targetname.clone())
                && !name.is_empty()
            {
                g.global_states.insert(format!("__team_master:{name}"), self.team_index);
            }
            return;
        }
        if let Some(name) = eng.ent(id).map(|v| v.targetname.clone())
            && !name.is_empty()
        {
            g.global_states.insert(format!("__team_master:{name}"), self.team_index);
        }
        if !self.is_triggered(g, eng, id, activator) {
            return;
        }
        if eng.ent(id).is_some_and(|v| v.spawnflags & SF_TEAMMASTER_FIREONCE != 0) {
            self.disabled = true;
        }
        use_targets(g, eng, id, activator, self.trigger_type, 0.0);
    }

    fn is_triggered(
        &self,
        g: &GameState,
        eng: &EngineApi<'_>,
        _id: EdictId,
        activator: Option<EdictId>,
    ) -> bool {
        if !is_master_triggered(g, eng, &self.master, activator) {
            return false;
        }
        if self.team_index < 0 {
            return true;
        }
        let Some(act) = activator else {
            return false;
        };
        let Some(slot) = player_slot(g, eng, act) else {
            return false;
        };
        let team_num = g.players[slot].as_ref().map_or(0, |p| match p.team {
            Team::Terrorist => 1,
            Team::Ct => 2,
            Team::Spectator => 3,
            Team::Unassigned => 0,
        });
        team_num == self.team_index
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.disabled = false;
    }
}

/// `game_team_set` (`CGameTeamSet`): sets `target` `game_team_master` to the activator's team.
#[derive(Default)]
pub(crate) struct GameTeamSet {
    master: String,
    disabled: bool,
}

impl GameEntity for GameTeamSet {
    fn classname(&self) -> &'static str {
        "game_team_set"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.disabled = false;
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
        if self.disabled || !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        let team_num = activator
            .and_then(|act| player_slot(g, eng, act))
            .and_then(|slot| g.players[slot].as_ref())
            .map_or(0, |p| match p.team {
                Team::Terrorist => 1,
                Team::Ct => 2,
                Team::Spectator => 3,
                Team::Unassigned => 0,
            });
        use_targets(g, eng, id, activator, UseType::Set, team_num as f32);
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.disabled = false;
    }
}

/// `game_text` (`CGameText`): emits `TE_TEXTMESSAGE` (`te_type = 29`) to activator or all clients.
#[derive(Default)]
pub(crate) struct GameText {
    master: String,
    message: String,
    channel: u8,
    x: f32,
    y: f32,
    effect: u8,
    color1: [u8; 4],
    color2: [u8; 4],
    fadein: f32,
    fadeout: f32,
    holdtime: f32,
    fxtime: f32,
}

impl GameEntity for GameText {
    fn classname(&self) -> &'static str {
        "game_text"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.message = key_str(keys, "message").unwrap_or("").to_string();
        self.channel = key_str(keys, "channel").map_or(1, atoi).clamp(0, 255) as u8;
        self.x = key_str(keys, "x").map_or(-1.0, atof);
        self.y = key_str(keys, "y").map_or(-1.0, atof);
        self.effect = key_str(keys, "effect").map_or(0, atoi).clamp(0, 2) as u8;
        let c1 = key_str(keys, "color").map_or([255, 255, 255], parse_rgb);
        let c2 = key_str(keys, "color2").map_or([240, 110, 0], parse_rgb);
        self.color1 = [c1[0], c1[1], c1[2], 0];
        self.color2 = [c2[0], c2[1], c2[2], 255];
        self.fadein = key_str(keys, "fadein").map_or(1.5, atof);
        self.fadeout = key_str(keys, "fadeout").map_or(0.5, atof);
        self.holdtime = key_str(keys, "holdtime").map_or(1.2, atof);
        self.fxtime = key_str(keys, "fxtime").map_or(0.25, atof);
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
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        let all = eng.ent(id).is_some_and(|v| v.spawnflags & SF_ENVTEXT_ALLPLAYERS != 0);
        let dest = if all {
            MsgDest::All
        } else if let Some(slot) = activator.and_then(|a| player_slot(g, eng, a)) {
            MsgDest::One(slot as u8)
        } else {
            return;
        };
        let mut w = MsgWriter::new();
        w.write_u8(self.channel);
        w.write_i16((self.x * 8192.0).round() as i16);
        w.write_i16((self.y * 8192.0).round() as i16);
        w.write_u8(self.effect);
        w.write_bytes(&self.color1);
        w.write_bytes(&self.color2);
        w.write_u16((self.fadein.max(0.0) * 256.0).round() as u16);
        w.write_u16((self.fadeout.max(0.0) * 256.0).round() as u16);
        w.write_u16((self.holdtime.max(0.0) * 256.0).round() as u16);
        if self.effect == 2 {
            w.write_u16((self.fxtime.max(0.0) * 256.0).round() as u16);
        }
        w.write_str(&self.message);
        eng.temp_entity(dest, 29, &w.into_bytes());
    }
}

/// `game_zone_player` (`CGameZonePlayer`): counts living players inside/outside its brush volume.
#[derive(Default)]
pub(crate) struct GameZonePlayer {
    in_target: String,
    out_target: String,
    in_count: String,
    out_count: String,
}

impl GameEntity for GameZonePlayer {
    fn classname(&self) -> &'static str {
        "game_zone_player"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.in_target = key_str(keys, "intarget").unwrap_or("").to_string();
        self.out_target = key_str(keys, "outtarget").unwrap_or("").to_string();
        self.in_count = key_str(keys, "incount").unwrap_or("").to_string();
        self.out_count = key_str(keys, "outcount").unwrap_or("").to_string();
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
        }
        eng.link_entity(id, false);
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
        let Some((mins, maxs, ignore_clients)) =
            eng.ent(id).map(|v| (v.absmin, v.absmax, v.spawnflags & SF_GZP_NO_CLIENTS != 0))
        else {
            return;
        };
        let mut inside = 0_i32;
        let mut outside = 0_i32;
        if !ignore_clients {
            for slot in 0..eng.max_clients() {
                if !g.players[usize::from(slot)].as_ref().is_some_and(|p| p.alive) {
                    continue;
                }
                let pe = eng.client_edict(slot);
                let Some(ev) = eng.ent(pe) else {
                    continue;
                };
                if ev.flags & FL_KILLME != 0 {
                    continue;
                }
                let intersects = intersects_aabb(mins, maxs, ev.absmin, ev.absmax);
                if intersects {
                    inside += 1;
                    if !self.in_target.is_empty() {
                        fire_targets(g, eng, &self.in_target, Some(pe), Some(id), UseType::Toggle, 0.0);
                    }
                } else {
                    outside += 1;
                    if !self.out_target.is_empty() {
                        fire_targets(g, eng, &self.out_target, Some(pe), Some(id), UseType::Toggle, 0.0);
                    }
                }
            }
        }
        if !self.in_count.is_empty() {
            fire_targets(g, eng, &self.in_count, None, Some(id), UseType::Set, inside as f32);
        }
        if !self.out_count.is_empty() {
            fire_targets(g, eng, &self.out_count, None, Some(id), UseType::Set, outside as f32);
        }
    }
}

pub(crate) fn intersects_aabb(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> bool {
    a_min.x <= b_max.x
        && a_max.x >= b_min.x
        && a_min.y <= b_max.y
        && a_max.y >= b_min.y
        && a_min.z <= b_max.z
        && a_max.z >= b_min.z
}

/// `player_weaponstrip` (`CStripWeapons`): strips all carried weapons from the activating player.
#[derive(Default)]
pub(crate) struct PlayerWeaponStrip;

impl GameEntity for PlayerWeaponStrip {
    fn classname(&self) -> &'static str {
        "player_weaponstrip"
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
        let Some(act) = activator else {
            return;
        };
        if let Some(slot) = player_slot(g, eng, act) {
            remove_all_items(g, eng, slot as u8, false);
        }
        let _ = util_remove;
        let _ = id;
    }
}
