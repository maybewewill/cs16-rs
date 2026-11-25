//! A synthetic map, a server on it (`cs-engine` + `cs-game` in this process) and a
//! `cs-clientcore` connection over a loopback, for the prediction tests.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use cs_assets::bsp::{
    BspMap, BspModel, ClipNode, Entity, Leaf, LightmapAtlas, Node, Plane,
};
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{EdictId, EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_game::player::Team;
use cs_pm::collision::{Hull, WorldCollision};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
use glam::Vec3;

pub const SOLID: i32 = -2;
pub const EMPTY: i32 = -1;
pub const WATER: i32 = -3;
const LEAF_CONTENTS: [i32; 3] = [SOLID, EMPTY, WATER];

/// Brush model 1 of the map: a block, `func_wall` in the entity list.
pub const MODEL_BLOCK: usize = 1;
pub const BLOCK_HALF: Vec3 = Vec3::new(16.0, 16.0, 32.0);
/// The wall of the world: solid for `x >= WALL_X`.
pub const WALL_X: f32 = 420.0;
/// The step of the world: `x` in 200..260, `z` below 24.
pub const STEP_X: (f32, f32) = (200.0, 260.0);
pub const STEP_Z: f32 = 24.0;

enum Tree {
    Leaf(i32),
    /// `front` is the side with `p[axis] >= dist`.
    Split {
        axis: usize,
        dist: f32,
        front: Box<Tree>,
        back: Box<Tree>,
    },
}

fn split(axis: usize, dist: f32, front: Tree, back: Tree) -> Tree {
    Tree::Split {
        axis,
        dist,
        front: Box::new(front),
        back: Box::new(back),
    }
}

/// The brush `mins..maxs` grown by `e` on every side, `inside` within, empty outside.
fn box_tree(mins: Vec3, maxs: Vec3, inside: i32, e: Vec3) -> Tree {
    let z = split(
        2,
        mins.z - e.z,
        split(2, maxs.z + e.z, Tree::Leaf(EMPTY), Tree::Leaf(inside)),
        Tree::Leaf(EMPTY),
    );
    let y = split(
        1,
        mins.y - e.y,
        split(1, maxs.y + e.y, Tree::Leaf(EMPTY), z),
        Tree::Leaf(EMPTY),
    );
    split(
        0,
        mins.x - e.x,
        split(0, maxs.x + e.x, Tree::Leaf(EMPTY), y),
        Tree::Leaf(EMPTY),
    )
}

/// The world: solid below `z = 0`, a wall at `x = WALL_X`, a step.
fn world_tree(e: Vec3) -> Tree {
    let step = split(
        0,
        STEP_X.0 - e.x,
        split(
            0,
            STEP_X.1 + e.x,
            Tree::Leaf(EMPTY),
            split(
                2,
                STEP_Z + e.z,
                Tree::Leaf(EMPTY),
                Tree::Leaf(SOLID),
            ),
        ),
        Tree::Leaf(EMPTY),
    );
    split(
        2,
        e.z,
        split(0, WALL_X - e.x, Tree::Leaf(SOLID), step),
        Tree::Leaf(SOLID),
    )
}

#[derive(Default)]
struct Builder {
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    clipnodes: Vec<ClipNode>,
}

impl Builder {
    fn plane(&mut self, axis: usize, dist: f32) -> usize {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        self.planes.push(Plane {
            normal,
            dist,
            plane_type: axis as i32,
        });
        self.planes.len() - 1
    }

    fn emit_nodes(&mut self, tree: &Tree) -> i16 {
        match tree {
            Tree::Leaf(c) => {
                let leaf = LEAF_CONTENTS.iter().position(|l| l == c).expect("leaf");
                -(leaf as i16) - 1
            }
            Tree::Split {
                axis,
                dist,
                front,
                back,
            } => {
                let plane = self.plane(*axis, *dist) as u32;
                let index = self.nodes.len();
                self.nodes.push(Node {
                    plane,
                    children: [0, 0],
                    mins: [0; 3],
                    maxs: [0; 3],
                    first_face: 0,
                    num_faces: 0,
                });
                let f = self.emit_nodes(front);
                let b = self.emit_nodes(back);
                self.nodes[index].children = [f, b];
                index as i16
            }
        }
    }

    fn emit_clip(&mut self, tree: &Tree) -> i16 {
        match tree {
            Tree::Leaf(c) => *c as i16,
            Tree::Split {
                axis,
                dist,
                front,
                back,
            } => {
                let plane = self.plane(*axis, *dist) as i32;
                let index = self.clipnodes.len();
                self.clipnodes.push(ClipNode {
                    plane,
                    children: [0, 0],
                });
                let f = self.emit_clip(front);
                let b = self.emit_clip(back);
                self.clipnodes[index].children = [f, b];
                index as i16
            }
        }
    }

    fn model(&mut self, mins: Vec3, maxs: Vec3, tree_for: impl Fn(Vec3) -> Tree) -> BspModel {
        let point = self.emit_nodes(&tree_for(Vec3::ZERO));
        let standing = self.emit_clip(&tree_for(Hull::Standing.half_extents()));
        let large = self.emit_clip(&tree_for(Hull::Large.half_extents()));
        let crouched = self.emit_clip(&tree_for(Hull::Crouched.half_extents()));
        BspModel {
            mins,
            maxs,
            origin: Vec3::ZERO,
            head_nodes: [
                i32::from(point),
                i32::from(standing),
                i32::from(large),
                i32::from(crouched),
            ],
            vis_leafs: 1,
            first_face: 0,
            num_faces: 0,
        }
    }
}

pub fn entity(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

/// The synthetic map: model 0 the world, model 1 a block. `entities` are its entity lump.
pub fn synthetic_bsp(entities: Vec<Entity>) -> BspMap {
    let mut b = Builder::default();
    let mut models = vec![b.model(Vec3::splat(-4096.0), Vec3::splat(4096.0), world_tree)];
    models.push(b.model(-BLOCK_HALF, BLOCK_HALF, |e| {
        box_tree(-BLOCK_HALF, BLOCK_HALF, SOLID, e)
    }));
    let leaf = |contents: i32| Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    };
    BspMap {
        name: "predtest".into(),
        skyname: String::new(),
        entities,
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes: b.planes,
        vertices_raw: Vec::new(),
        visibility: Vec::new(),
        nodes: b.nodes,
        texinfos: Vec::new(),
        faces: Vec::new(),
        lighting: Vec::new(),
        clipnodes: b.clipnodes,
        leafs: LEAF_CONTENTS.iter().map(|&c| leaf(c)).collect(),
        mark_surfaces: Vec::new(),
        edges: Vec::new(),
        surf_edges: Vec::new(),
        models,
        textures: Vec::new(),
        lightmap_atlas: LightmapAtlas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            packed_faces: 0,
        },
        render_vertices: Vec::new(),
        render_indices: Vec::new(),
        batches: Vec::new(),
    }
}

/// A BSP file that has nothing but a valid header (the checksum reads only that).
fn header_only() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

/// The entities of the map: the world, a spawn, and (when `wall`) a `func_wall` at `wall`.
pub fn map_entities(wall: Option<Vec3>) -> Vec<Entity> {
    let mut v = vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[
            ("classname", "info_player_start"),
            ("origin", "0 0 37"),
            ("angles", "0 0 0"),
        ]),
        entity(&[
            ("classname", "info_player_deathmatch"),
            ("origin", "0 100 37"),
            ("angles", "0 0 0"),
        ]),
    ];
    if let Some(o) = wall {
        v.push(entity(&[
            ("classname", "func_wall"),
            ("model", "*1"),
            ("origin", &format!("{} {} {}", o.x, o.y, o.z)),
        ]));
    }
    v
}

pub fn server_map(entities: Vec<Entity>) -> (ServerMap, Arc<WorldCollision>) {
    let bsp = synthetic_bsp(entities);
    let coll = Arc::new(WorldCollision::from_bsp(&bsp));
    let map = ServerMap::from_bsp("predtest", &header_only(), Arc::new(bsp)).expect("header");
    (map, coll)
}

type Action = Box<dyn FnOnce(&mut CsGame, &mut EngineApi<'_>)>;

/// `CsGame` plus closures the test queued, which run at the start of the next frame.
pub struct Rig {
    pub game: CsGame,
    pub actions: Vec<Action>,
}

impl GameDll for Rig {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.game.usermsgs()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_activate(eng);
    }
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_deactivate(eng);
    }
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.game.spawn_entity(eng, id, classname, keys)
    }
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.game.client_connect(eng, slot)
    }
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_put_in_server(eng, slot);
    }
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_disconnect(eng, slot);
    }
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.game.client_command(eng, slot, argv);
    }
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_userinfo_changed(eng, slot);
    }
    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        self.game.run_usercmd(eng, slot, cmd, random_seed);
    }
    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        self.game.post_usercmd(eng, slot, cmd);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        for action in std::mem::take(&mut self.actions) {
            action(&mut self.game, eng);
        }
        self.game.start_frame(eng, frametime);
    }
    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.game.think(eng, id);
    }
    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        self.game.touch(eng, toucher, other);
    }
    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        self.game.blocked(eng, pusher, other);
    }
    fn server_command(&mut self, eng: &mut EngineApi<'_>, argv: &[String]) -> Option<String> {
        self.game.server_command(eng, argv)
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.game.update_client_data(eng, slot)
    }
    fn get_weapon_data(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        out: &mut [GoldSrcWeaponData; 64],
    ) -> bool {
        self.game.get_weapon_data(eng, slot, out)
    }
    fn setup_visibility(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Option<Vec3> {
        self.game.setup_visibility(eng, slot)
    }
    fn add_to_full_pack(
        &mut self,
        eng: &mut EngineApi<'_>,
        ctx: &cs_engine::FullPackCtx<'_>,
        state: &mut cs_proto48::delta::GoldSrcEntityState,
    ) -> bool {
        self.game.add_to_full_pack(eng, ctx, state)
    }
    fn delta_encode(
        &mut self,
        encoder: &str,
        current_player: u8,
        from: &cs_proto48::delta::GoldSrcEntityState,
        to: &cs_proto48::delta::GoldSrcEntityState,
        marks: &mut cs_proto48::delta::DeltaMarks<'_>,
    ) {
        self.game.delta_encode(encoder, current_player, from, to, marks);
    }
}

pub const FRAME: f64 = 0.01;

/// A server on `map` and a `cs-clientcore` connection in its slot 0, playing as a spawned
/// counter-terrorist, on one clock.
pub struct Loop {
    pub server: Server<Rig>,
    pub hub: LoopbackHub,
    pub conn: Connection<LoopbackEndpoint>,
    pub now: f64,
}

impl Loop {
    pub fn new(map: ServerMap, world: Arc<WorldCollision>) -> Self {
        let rig = Rig {
            game: CsGame::new(),
            actions: Vec::new(),
        };
        let mut server = Server::new(
            ServerConfig {
                max_clients: 4,
                max_edicts: 256,
                max_update_rate: 102,
                ..ServerConfig::default()
            },
            map,
            rig,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        let mut hub = LoopbackHub::new();
        let endpoint = hub.connect();
        let cfg = ConnectionConfig::new(
            "predictor",
            76_561_197_960_265_730,
            &md5_hex(b"cs-clientcore prediction"),
        );
        let mut conn = Connection::new(endpoint, PeerAddr::Loopback(0), cfg);
        conn.set_cvar("cl_updaterate", "60");
        conn.frames_mut().set_remote_is_local(true);
        conn.connect();
        let mut now = 0.0;
        for _ in 0..600 {
            now += FRAME;
            conn.pump(now);
            server.frame(&mut hub, now);
            if conn.state() == &ConnState::InGame {
                break;
            }
        }
        assert_eq!(conn.state(), &ConnState::InGame, "signed on");
        // The shell hands the map's collision over once the map is loaded.
        conn.set_world_collision(Some(world));
        let mut this = Self {
            server,
            hub,
            conn,
            now,
        };
        this.server.game_mut().actions.push(Box::new(|game, eng| {
            assert!(game.spawn_player(eng, 0, Team::Ct, 0));
            // The weapons set the speed; the movement takes the entity's `maxspeed`.
            eng.ent_mut(eng.client_edict(0)).unwrap().max_speed = 250.0;
        }));
        for _ in 0..30 {
            this.step(Some(idle(10)));
        }
        this
    }

    /// One frame: the command (if any) is made, the client pumps, the server runs.
    pub fn step(&mut self, cmd: Option<GoldSrcUserCmd>) {
        if let Some(cmd) = cmd {
            self.conn.queue_cmd(cmd);
        }
        self.now += FRAME;
        self.conn.pump(self.now);
        self.server.frame(&mut self.hub, self.now);
    }

    /// Drops the next client packet and the next server frame (one step of total loss).
    pub fn lose_one_step(&mut self, cmd: Option<GoldSrcUserCmd>) {
        self.conn.transport_mut().set_faults(cs_transport::Faults {
            drop_every: 1,
            ..cs_transport::Faults::default()
        });
        self.step(cmd);
        self.conn
            .transport_mut()
            .set_faults(cs_transport::Faults::default());
    }

    /// The origin of the player on the server.
    pub fn server_origin(&mut self) -> Vec3 {
        let out = std::rc::Rc::new(std::cell::Cell::new(Vec3::ZERO));
        let sink = std::rc::Rc::clone(&out);
        self.server.game_mut().actions.push(Box::new(move |_, eng| {
            sink.set(eng.ent(eng.client_edict(0)).unwrap().origin);
        }));
        self.step(None);
        out.get()
    }
}

pub const IN_JUMP: u16 = 1 << 1;
pub const IN_DUCK: u16 = 1 << 2;
pub const IN_FORWARD: u16 = 1 << 3;
pub const IN_MOVELEFT: u16 = 1 << 9;

pub fn idle(msec: u8) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        ..GoldSrcUserCmd::default()
    }
}

pub fn walk(msec: u8, buttons: u16, forward: f32, side: f32, yaw: f32) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        buttons,
        forwardmove: forward,
        sidemove: side,
        viewangles: Vec3::new(0.0, yaw, 0.0),
        ..GoldSrcUserCmd::default()
    }
}
