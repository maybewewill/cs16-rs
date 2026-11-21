//! The stand-in for players until the player task: a client that has entered the server spawns
//! at a spawn point of the map and walks with the `cs-sim` movement. No teams (even slots are
//! counter-terrorists, odd slots terrorists), no weapons, no health, no death.
//!
//! This is the player handling of `cs_sim::walk_game::WalkGame`, moved onto the entities of
//! this crate (the spawn point comes from [`select_spawn_point`], the ladders from
//! `func_ladder` entities); the player task replaces the whole file.

use std::collections::BTreeMap;

use cs_assets::bsp::SpawnTeam;
use cs_engine::consts::{MOVETYPE_WALK, SOLID_SLIDEBOX};
use cs_engine::{EdictId, EngineApi};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_sim::pmove::{PlayerMoveState, VEC_VIEW, snap_spawn_to_floor, step_player_move};
use cs_sim::walk_game::{EngineWorld, WALK_MAX_SPEED, ent_flags, usercmd_to_move_input};
use glam::Vec3;

use crate::GameState;
use crate::world::select_spawn_point;

const PLAYER_MODEL: &str = "models/player.mdl";
const STANDING_MINS: Vec3 = Vec3::new(-16.0, -16.0, -36.0);
const STANDING_MAXS: Vec3 = Vec3::new(16.0, 16.0, 36.0);
const DUCKED_MINS: Vec3 = Vec3::new(-16.0, -16.0, -18.0);
const DUCKED_MAXS: Vec3 = Vec3::new(16.0, 16.0, 18.0);
const FOV: f32 = 90.0;

/// The movement state of every player in the server, by slot.
#[derive(Default)]
pub(crate) struct Walkers {
    states: BTreeMap<u8, PlayerMoveState>,
}

impl Walkers {
    pub(crate) fn clear(&mut self) {
        self.states.clear();
    }

    pub(crate) fn remove(&mut self, slot: u8) {
        self.states.remove(&slot);
    }

    /// Spawns the player of `slot` at a spawn point.
    pub(crate) fn put_in_server(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
        let id = eng.client_edict(slot);
        let team = if slot.is_multiple_of(2) {
            SpawnTeam::CounterTerrorist
        } else {
            SpawnTeam::Terrorist
        };
        let spot = select_spawn_point(g, eng, id, team)
            .and_then(|spot| eng.ent(spot).map(|v| (v.origin, v.angles.y)));
        let (origin, yaw) = match spot {
            Some((raw, yaw)) => {
                let world = EngineWorld::new(eng, id, &g.ladders);
                (snap_spawn_to_floor(raw, &world), yaw)
            }
            None => (Vec3::ZERO, 0.0),
        };
        self.states.insert(slot, PlayerMoveState::at_spawn(origin));
        let model = if team == SpawnTeam::CounterTerrorist {
            "gign"
        } else {
            "leet"
        };
        eng.set_client_key(slot, "model", model);

        let model_index = eng.model_index(PLAYER_MODEL).unwrap_or(0);
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        v.origin = origin;
        v.velocity = Vec3::ZERO;
        v.v_angle = Vec3::new(0.0, yaw, 0.0);
        v.angles = Vec3::new(0.0, yaw, 0.0);
        v.modelindex = model_index;
        v.solid = SOLID_SLIDEBOX;
        v.movetype = MOVETYPE_WALK;
        v.health = 100.0;
        v.flags = ent_flags::FL_CLIENT;
        v.mins = STANDING_MINS;
        v.maxs = STANDING_MAXS;
        v.size = STANDING_MAXS - STANDING_MINS;
        v.view_ofs = Vec3::new(0.0, 0.0, VEC_VIEW);
        v.max_speed = WALK_MAX_SPEED;
        v.fov = FOV;
        // The engine links the player after this call.
    }

    /// Moves the player of `slot` by one command.
    pub(crate) fn run_usercmd(
        &mut self,
        g: &GameState,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
    ) {
        if cmd.msec == 0 {
            return;
        }
        let Some(state) = self.states.get_mut(&slot) else {
            return;
        };
        let input = usercmd_to_move_input(cmd, WALK_MAX_SPEED);
        let dt = f32::from(cmd.msec) / 1000.0;
        let id = eng.client_edict(slot);
        let world = EngineWorld::new(eng, id, &g.ladders);
        step_player_move(state, &input, WALK_MAX_SPEED, &g.move_params, &world, dt);
        // What the movement ran into reaches the engine as impacts, delivered after the command.
        let touched: Vec<EdictId> = world.into_touched();
        for other in touched {
            eng.player_touched(id, other);
        }

        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        v.v_angle = cmd.viewangles;
        v.angles.y = v.v_angle.y;
        v.origin = state.origin;
        v.velocity = state.velocity;
        v.view_ofs.z = state.view_ofs_z;
        v.flags &= !(ent_flags::FL_ONGROUND | ent_flags::FL_DUCKING);
        if state.on_ground {
            v.flags |= ent_flags::FL_ONGROUND;
        }
        if state.ducking {
            v.flags |= ent_flags::FL_DUCKING;
            v.mins = DUCKED_MINS;
            v.maxs = DUCKED_MAXS;
        } else {
            v.mins = STANDING_MINS;
            v.maxs = STANDING_MAXS;
        }
        v.size = v.maxs - v.mins;
        // The engine links the player after the command.
    }
}

/// The client data of the player of `slot`.
pub(crate) fn client_data(eng: &EngineApi<'_>, slot: u8) -> GoldSrcClientData {
    let Some(v) = eng.ent(eng.client_edict(slot)) else {
        return GoldSrcClientData::default();
    };
    GoldSrcClientData {
        origin: v.origin,
        velocity: v.velocity,
        view_ofs: v.view_ofs,
        flags: v.flags,
        health: v.health,
        maxspeed: WALK_MAX_SPEED,
        fov: FOV,
        waterlevel: 0,
        ..Default::default()
    }
}
