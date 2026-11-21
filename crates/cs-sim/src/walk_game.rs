//! `WalkGame`: a minimal game behind the server engine's [`GameDll`] boundary. Players spawn
//! at the map's spawn points and move with [`step_player_move`]; there are no weapons, rounds
//! or animations. Scaffolding until the full game rules move onto the engine.

use std::collections::BTreeMap;

use cs_assets::bsp::{SpawnPoint, SpawnTeam};
use cs_engine::{EdictId, EngineApi, GameDll};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;

use crate::pmove::{
    MoveInputCmd, MoveParams, MoveWorld, PlayerMoveState, VEC_VIEW, snap_spawn_to_floor,
    step_player_move,
};
use crate::trace::{CONTENTS_SOLID, Hull, TraceResult};

/// `usercmd_t::buttons` bits.
pub mod buttons {
    pub const IN_ATTACK: u16 = 1;
    pub const IN_JUMP: u16 = 2;
    pub const IN_DUCK: u16 = 4;
    pub const IN_USE: u16 = 32;
    pub const IN_ATTACK2: u16 = 2048;
    pub const IN_RELOAD: u16 = 8192;
    pub const IN_SCORE: u16 = 32768;
}

/// `entvars_t::flags` bits.
pub mod ent_flags {
    pub const FL_CLIENT: u32 = 1 << 3;
    pub const FL_ONGROUND: u32 = 1 << 9;
    pub const FL_DUCKING: u32 = 1 << 14;
}

/// Running speed of every player: there are no weapons to slow anyone down.
pub const WALK_MAX_SPEED: f32 = 250.0;

const PLAYER_MODEL: &str = "models/player.mdl";
const PLAYER_SKINS: [&str; 8] = [
    "urban", "gsg9", "sas", "gign", "terror", "leet", "arctic", "guerilla",
];
/// `SOLID_SLIDEBOX`.
const SOLID_SLIDEBOX: u8 = 3;
/// `MOVETYPE_WALK`.
const MOVETYPE_WALK: u8 = 3;
const STANDING_MINS: Vec3 = Vec3::new(-16.0, -16.0, -36.0);
const STANDING_MAXS: Vec3 = Vec3::new(16.0, 16.0, 36.0);
const DUCKED_MINS: Vec3 = Vec3::new(-16.0, -16.0, -18.0);
const DUCKED_MAXS: Vec3 = Vec3::new(16.0, 16.0, 18.0);
const FOV: f32 = 90.0;

/// Classnames of the brush entities players cannot walk through.
const SOLID_BRUSH_CLASSES: [&str; 8] = [
    "func_wall",
    "func_door",
    "func_door_rotating",
    "func_breakable",
    "func_button",
    "func_plat",
    "func_train",
    "func_pushable",
];

/// A brush entity the game remembers from the map's entity lump.
#[derive(Debug, Clone, Copy)]
struct BrushRef {
    /// Brush model number `N` of the entity's `*N` model.
    model: usize,
    /// The entity's `origin` when it spawned; the engine places the model there.
    ///
    /// Limitation: this is the spawn-time origin, never updated, so a door that has moved is
    /// not followed (entity linking belongs to a later plan item). The BSP model's own `origin`
    /// field, which the legacy `BspCollision` adds, is not reachable through `EngineApi` and is
    /// not added here.
    origin: Vec3,
    /// World-space bounds of the model at `origin`; `None` when the engine gave none.
    bounds: Option<(Vec3, Vec3)>,
}

impl BrushRef {
    /// Whether a box of half-extents `half` swept from `a` to `b` can touch the brush.
    fn near_sweep(&self, a: Vec3, b: Vec3, half: Vec3) -> bool {
        let Some((mins, maxs)) = self.bounds else {
            return true;
        };
        let pad = half + Vec3::splat(4.0);
        let (lo, hi) = (a.min(b) - pad, a.max(b) + pad);
        lo.cmple(maxs).all() && hi.cmpge(mins).all()
    }
}

/// Whether the player's box (hull half-extents plus 3 units in x and y) overlaps the bounds of
/// any ladder: the check of the legacy `BspCollision::is_on_ladder`. A ladder the engine gave
/// no bounds never matches.
fn touches_ladder(ladders: &[(Vec3, Vec3)], pos: Vec3, hull: Hull) -> bool {
    let ext = hull.half_extents() + Vec3::new(3.0, 3.0, 0.0);
    let (lo, hi) = (pos - ext, pos + ext);
    ladders
        .iter()
        .any(|&(mins, maxs)| lo.cmple(maxs).all() && hi.cmpge(mins).all())
}

/// The world the players move in: the engine's geometry through [`EngineApi`] plus the game's
/// own lists of solid and ladder brush entities.
struct EngineWorld<'a, 'e> {
    eng: &'a EngineApi<'e>,
    solids: &'a [BrushRef],
    ladders: &'a [(Vec3, Vec3)],
}

impl MoveWorld for EngineWorld<'_, '_> {
    /// Mirrors `BspCollision::trace_hull`: the world trace, returned at once when it is
    /// all-solid; then each solid brush entity's trace, where an all-solid brush trace is
    /// returned at once, and a trace replaces the best wholesale when it is nearer or starts in
    /// solid while the best does not.
    fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
        let mut best = self.eng.trace_hull(start, end, hull);
        if best.fraction <= 0.0 && best.all_solid {
            return best;
        }
        let half = hull.half_extents();
        for brush in self.solids {
            if !brush.near_sweep(start, end, half) {
                continue;
            }
            let res = self
                .eng
                .trace_model(brush.model, brush.origin, start, end, hull);
            if res.all_solid {
                return res;
            }
            if res.fraction < best.fraction || (res.start_solid && !best.start_solid) {
                best = res;
            }
        }
        best
    }

    fn is_on_ladder(&self, pos: Vec3, hull: Hull) -> bool {
        touches_ladder(self.ladders, pos, hull)
    }

    /// Mirrors `BspCollision::point_contents`: the world's contents unless that is solid
    /// anyway, else solid when a solid brush entity near `point` (bounds padded by the hull's
    /// half-extents plus 2) holds it. The legacy precise query is the brush model's contents
    /// for `hull`; `EngineApi` has no such call, so a zero-length trace of `hull` through the
    /// model stands in: it starts in solid exactly when that point is solid.
    fn point_contents(&self, point: Vec3, hull: Hull) -> i32 {
        let c = self.eng.point_contents(point, hull);
        if c == CONTENTS_SOLID {
            return c;
        }
        let pad = hull.half_extents() + Vec3::splat(2.0);
        let in_solid_brush = self.solids.iter().any(|b| {
            b.bounds
                .is_none_or(|(mins, maxs)| (mins - pad).cmple(point).all() && point.cmple(maxs + pad).all())
                && self
                    .eng
                    .trace_model(b.model, b.origin, point, point, hull)
                    .start_solid
        });
        if in_solid_brush { CONTENTS_SOLID } else { c }
    }
}

/// A question put to the game's movement world, answered at the start of the next frame.
/// For tests only (cargo feature `test-hooks`).
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum WorldQuery {
    /// `MoveWorld::trace_hull`.
    Trace { start: Vec3, end: Vec3, hull: Hull },
    /// `MoveWorld::point_contents`.
    Contents { point: Vec3, hull: Hull },
    /// `snap_spawn_to_floor`.
    Snap(Vec3),
}

/// The answer to a [`WorldQuery`]. For tests only (cargo feature `test-hooks`).
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum WorldAnswer {
    Trace(TraceResult),
    Contents(i32),
    Snap(Vec3),
}

pub struct WalkGame {
    spawns: Vec<SpawnPoint>,
    /// Brush entities of the map that block movement, from `spawn_entity`.
    solids: Vec<BrushRef>,
    /// World-space bounds of the map's `func_ladder` entities: all the overlap test reads.
    ladders: Vec<(Vec3, Vec3)>,
    params: MoveParams,
    /// Model index of [`PLAYER_MODEL`], known after `server_activate`.
    player_model: u16,
    /// Movement state of every player put in the server, by slot.
    players: BTreeMap<u8, PlayerMoveState>,
    /// Queries tests queued for the next frame, and their answers.
    #[cfg(feature = "test-hooks")]
    queries: Vec<WorldQuery>,
    #[cfg(feature = "test-hooks")]
    answers: Vec<WorldAnswer>,
}

impl WalkGame {
    /// A game with no map: spawn points and brush entities arrive through `spawn_entity` as the
    /// engine activates a map, and `server_deactivate` drops them again.
    pub fn new(params: MoveParams) -> Self {
        Self {
            spawns: Vec::new(),
            solids: Vec::new(),
            ladders: Vec::new(),
            params,
            player_model: 0,
            players: BTreeMap::new(),
            #[cfg(feature = "test-hooks")]
            queries: Vec::new(),
            #[cfg(feature = "test-hooks")]
            answers: Vec::new(),
        }
    }

    /// Whether a player of `hull` at `pos` touches a ladder of the current map; the answer the
    /// movement code gets.
    pub fn is_on_ladder(&self, pos: Vec3, hull: Hull) -> bool {
        touches_ladder(&self.ladders, pos, hull)
    }

    /// The spawn points of the current map, in the order of the entity lump.
    pub fn spawns(&self) -> &[SpawnPoint] {
        &self.spawns
    }

    /// Queues `query`; its answer is available through [`take_answers`](Self::take_answers)
    /// after the next server frame. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn query_world(&mut self, query: WorldQuery) {
        self.queries.push(query);
    }

    /// The answers to the queries run so far, in order, removed from the game. For tests only
    /// (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn take_answers(&mut self) -> Vec<WorldAnswer> {
        std::mem::take(&mut self.answers)
    }

    /// Moves the movement state of the player in `slot` to `origin` and stops it, to place a
    /// player at a chosen spot in tests; the edict follows with the player's next command.
    /// `false` when nobody is in that slot. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_player_origin(&mut self, slot: u8, origin: Vec3) -> bool {
        let Some(state) = self.players.get_mut(&slot) else {
            return false;
        };
        state.origin = origin;
        state.velocity = Vec3::ZERO;
        true
    }
}

/// The brush model number `N` of a model named `*N`; `None` for the world (`*0`) and for any
/// other name.
fn brush_model_number(model: &str) -> Option<usize> {
    model
        .strip_prefix('*')?
        .parse::<usize>()
        .ok()
        .filter(|&n| n > 0)
}

/// Spawn of `slot`: even slots are counter-terrorists, odd slots terrorists, the `slot / 2`th
/// spawn of the team (wrapping). Falls back to the other team's spawns, then to none.
fn spawn_for(spawns: &[SpawnPoint], slot: u8) -> Option<&SpawnPoint> {
    let (team, other) = if slot.is_multiple_of(2) {
        (SpawnTeam::CounterTerrorist, SpawnTeam::Terrorist)
    } else {
        (SpawnTeam::Terrorist, SpawnTeam::CounterTerrorist)
    };
    let of = |t: SpawnTeam| -> Vec<&SpawnPoint> { spawns.iter().filter(|s| s.team == t).collect() };
    let mut candidates = of(team);
    if candidates.is_empty() {
        candidates = of(other);
    }
    if candidates.is_empty() {
        return None;
    }
    Some(candidates[usize::from(slot / 2) % candidates.len()])
}

/// Movement input of a client command: move speeds as fractions of `max_speed`.
pub fn usercmd_to_move_input(cmd: &GoldSrcUserCmd, max_speed: f32) -> MoveInputCmd {
    MoveInputCmd {
        forward: (cmd.forwardmove / max_speed).clamp(-1.0, 1.0),
        right: (cmd.sidemove / max_speed).clamp(-1.0, 1.0),
        yaw_deg: cmd.viewangles.y,
        pitch_deg: cmd.viewangles.x,
        jump: cmd.buttons & buttons::IN_JUMP != 0,
        duck: cmd.buttons & buttons::IN_DUCK != 0,
        walk: false,
    }
}

impl GameDll for WalkGame {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }

    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        let models = std::iter::once(PLAYER_MODEL.to_string()).chain(
            PLAYER_SKINS
                .iter()
                .map(|m| format!("models/player/{m}/{m}.mdl")),
        );
        for name in models {
            match eng.precache_model(&name) {
                Ok(index) if name == PLAYER_MODEL => self.player_model = index,
                Ok(_) => {}
                Err(err) if name == PLAYER_MODEL => {
                    self.player_model = 0;
                    tracing::error!(
                        model = PLAYER_MODEL,
                        ?err,
                        "player model not precached; players get model index 0 and are invisible"
                    );
                }
                Err(err) => tracing::error!(model = %name, ?err, "model not precached"),
            }
        }
    }

    fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {
        self.players.clear();
        self.spawns.clear();
        self.solids.clear();
        self.ladders.clear();
    }

    /// Remembers spawn points and brush entities from the map's entity lump and refuses every
    /// entity: the game keeps none of its own.
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        let Some(vars) = eng.ent(id) else {
            return false;
        };
        let team = match classname {
            "info_player_start" => Some(SpawnTeam::CounterTerrorist),
            "info_player_deathmatch" => Some(SpawnTeam::Terrorist),
            _ => None,
        };
        if let Some(team) = team {
            // The map's own spawn list (`BspMap::spawns`) skips entries without an origin.
            if keys.iter().any(|(k, _)| k == "origin") {
                self.spawns.push(SpawnPoint {
                    position: vars.origin,
                    yaw_deg: vars.angles.y,
                    team,
                });
            }
            return false;
        }
        let solid = SOLID_BRUSH_CLASSES.contains(&classname);
        if !solid && classname != "func_ladder" {
            return false;
        }
        // The engine ran `set_model` for a precached `*N` model before this call. The brush
        // model number comes from the name: `modelindex` is the model-table index, one more
        // than `N` (slot 1 holds the map itself).
        let Some(model) = brush_model_number(&vars.model) else {
            return false;
        };
        let origin = vars.origin;
        let bounds = (vars.maxs != vars.mins).then_some((origin + vars.mins, origin + vars.maxs));
        if solid {
            self.solids.push(BrushRef {
                model,
                origin,
                bounds,
            });
        } else if let Some(bounds) = bounds {
            self.ladders.push(bounds);
        }
        false
    }

    fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
        Ok(())
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        let (origin, yaw) = {
            let world = EngineWorld {
                eng,
                solids: &self.solids,
                ladders: &self.ladders,
            };
            match spawn_for(&self.spawns, slot) {
                Some(spawn) => (snap_spawn_to_floor(spawn.position, &world), spawn.yaw_deg),
                None => (Vec3::ZERO, 0.0),
            }
        };
        self.players.insert(slot, PlayerMoveState::at_spawn(origin));
        let model = if slot.is_multiple_of(2) {
            "gign"
        } else {
            "leet"
        };
        eng.set_client_key(slot, "model", model);

        let id = eng.client_edict(slot);
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        v.origin = origin;
        v.velocity = Vec3::ZERO;
        v.v_angle = Vec3::new(0.0, yaw, 0.0);
        v.angles = Vec3::new(0.0, yaw, 0.0);
        v.modelindex = self.player_model;
        v.solid = SOLID_SLIDEBOX;
        v.movetype = MOVETYPE_WALK;
        v.health = 100.0;
        v.flags = ent_flags::FL_CLIENT;
        v.mins = STANDING_MINS;
        v.maxs = STANDING_MAXS;
        v.view_ofs = Vec3::new(0.0, 0.0, VEC_VIEW);
        v.max_speed = WALK_MAX_SPEED;
        v.fov = FOV;
    }

    fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, slot: u8) {
        self.players.remove(&slot);
    }

    fn client_command(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _argv: &[String]) {}

    fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        _random_seed: u32,
    ) {
        if cmd.msec == 0 {
            return;
        }
        let Some(state) = self.players.get_mut(&slot) else {
            return;
        };
        let input = usercmd_to_move_input(cmd, WALK_MAX_SPEED);
        let dt = f32::from(cmd.msec) / 1000.0;
        let world = EngineWorld {
            eng,
            solids: &self.solids,
            ladders: &self.ladders,
        };
        step_player_move(state, &input, WALK_MAX_SPEED, &self.params, &world, dt);

        let id = eng.client_edict(slot);
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
    }

    #[cfg(not(feature = "test-hooks"))]
    fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {}

    #[cfg(feature = "test-hooks")]
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, _frametime: f32) {
        if self.queries.is_empty() {
            return;
        }
        let world = EngineWorld {
            eng,
            solids: &self.solids,
            ladders: &self.ladders,
        };
        for query in self.queries.drain(..) {
            self.answers.push(match query {
                WorldQuery::Trace { start, end, hull } => {
                    WorldAnswer::Trace(world.trace_hull(start, end, hull))
                }
                WorldQuery::Contents { point, hull } => {
                    WorldAnswer::Contents(world.point_contents(point, hull))
                }
                WorldQuery::Snap(raw) => WorldAnswer::Snap(snap_spawn_to_floor(raw, &world)),
            });
        }
    }

    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(x: f32, team: SpawnTeam) -> SpawnPoint {
        SpawnPoint {
            position: Vec3::new(x, 0.0, 0.0),
            yaw_deg: 0.0,
            team,
        }
    }

    fn spawn_x(spawns: &[SpawnPoint], slot: u8) -> Option<f32> {
        spawn_for(spawns, slot).map(|s| s.position.x)
    }

    #[test]
    fn spawns_alternate_teams_and_wrap() {
        let spawns = [
            spawn(1.0, SpawnTeam::Terrorist),
            spawn(10.0, SpawnTeam::CounterTerrorist),
            spawn(2.0, SpawnTeam::Terrorist),
            spawn(20.0, SpawnTeam::CounterTerrorist),
        ];
        let xs: Vec<_> = (0..6).map(|slot| spawn_x(&spawns, slot)).collect();
        assert_eq!(xs, [10.0, 1.0, 20.0, 2.0, 10.0, 1.0].map(Some).to_vec());
    }

    #[test]
    fn spawns_of_one_team_serve_both_teams() {
        let spawns = [
            spawn(1.0, SpawnTeam::Terrorist),
            spawn(2.0, SpawnTeam::Terrorist),
        ];
        // Counter-terrorists fall back to the terrorist spawns, by their own `slot / 2`.
        let xs: Vec<_> = (0..4).map(|slot| spawn_x(&spawns, slot)).collect();
        assert_eq!(xs, [1.0, 1.0, 2.0, 2.0].map(Some).to_vec());
    }

    #[test]
    fn no_spawns_give_no_spawn() {
        assert_eq!(spawn_x(&[], 0), None);
        assert_eq!(spawn_x(&[], 1), None);
    }

    #[test]
    fn usercmd_maps_to_move_input() {
        let cmd = GoldSrcUserCmd {
            forwardmove: 130.0,
            sidemove: -500.0,
            buttons: buttons::IN_JUMP | buttons::IN_DUCK,
            viewangles: Vec3::new(10.0, 90.0, 0.0),
            ..Default::default()
        };
        let m = usercmd_to_move_input(&cmd, 250.0);
        assert!((m.forward - 0.52).abs() < 1e-6);
        assert_eq!(m.right, -1.0);
        assert_eq!(
            (m.yaw_deg, m.pitch_deg, m.jump, m.duck),
            (90.0, 10.0, true, true)
        );
        assert!(!m.walk);
    }
}
