//! `BoxWorld`: a test world for `cs_sim::pm`. Solid axis-aligned boxes with texture names, water
//! volumes, ladder moveents, a sound log, queued `random_long` values, a settable clock, a
//! `physent_by_info` list and a call log. The trace is a hull-expanded AABB sweep with the
//! `DIST_EPSILON` back-off of `PM_RecursiveHullCheck`; it is not bit exact, only a faithful test
//! world for the logic of `pm`. A miss has a zero plane normal and `ent` -1.
//!
//! Entity numbers: a solid box's `ent` is its index in `boxes` (the first box is "the world", ent 0).

#![allow(dead_code)] // every test binary uses a different part of this module

use std::cell::RefCell;
use std::collections::VecDeque;

use cs_sim::pm::consts::{CONTENTS_EMPTY, CONTENTS_SOLID, PLAYER_HULL_MAXS, PLAYER_HULL_MINS};
use cs_sim::pm::{PlayerMove, PmShared, PmTrace, PmWorld, pm_init, pm_move};
use glam::Vec3;

/// `DIST_EPSILON` of the engine's hull check.
pub const DIST_EPSILON: f32 = 0.03125;

/// An axis-aligned solid with a texture name.
#[derive(Debug, Clone)]
pub struct SolidBox {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub texture: String,
}

/// A water (or other non-solid) volume. `contents` is what `PM_PointContents` returns,
/// `truecontents` the second output (a current type for moving water).
#[derive(Debug, Clone)]
pub struct WaterVolume {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub contents: i32,
    pub truecontents: i32,
}

/// A ladder moveent: its bounds and the plane normal `trace_model` reports.
#[derive(Debug, Clone)]
pub struct LadderEnt {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub normal: Vec3,
}

/// One played sound.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundEvent {
    pub channel: i32,
    pub sample: String,
    pub volume: f32,
    pub attenuation: f32,
    pub flags: i32,
    pub pitch: i32,
}

/// One call into the world, in call order.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    PlayerTrace { start: Vec3, end: Vec3, usehull: i32 },
    TestPlayerPosition { pos: Vec3 },
    PointContents(Vec3),
    NumMoveents,
    LadderContains(usize),
    MoveentBounds(usize),
    MoveentOrigin(usize),
    TraceModel(usize),
    TraceTexture(i32),
    PlaySound(String),
    RandomLong(i32, i32),
    SysFloatTime,
    PhysentByInfo(i32),
}

impl Call {
    /// A short stable name of the call kind, for order assertions.
    pub fn kind(&self) -> &'static str {
        match self {
            Call::PlayerTrace { .. } => "PT",
            Call::TestPlayerPosition { .. } => "TPP",
            Call::PointContents(_) => "PC",
            Call::NumMoveents => "NME",
            Call::LadderContains(_) => "LC",
            Call::MoveentBounds(_) => "MB",
            Call::MoveentOrigin(_) => "MO",
            Call::TraceModel(_) => "TM",
            Call::TraceTexture(_) => "TT",
            Call::PlaySound(_) => "PS",
            Call::RandomLong(..) => "RL",
            Call::SysFloatTime => "SFT",
            Call::PhysentByInfo(_) => "PBI",
        }
    }
}

/// The test world.
pub struct BoxWorld {
    pub boxes: Vec<SolidBox>,
    pub water: Vec<WaterVolume>,
    pub ladders: Vec<LadderEnt>,
    /// Hull extents by `usehull` (the engine's four hulls by default).
    pub hull_mins: [Vec3; 4],
    pub hull_maxs: [Vec3; 4],
    /// Every `play_sound` call.
    pub sounds: Vec<SoundEvent>,
    /// Values `random_long` returns, front first; an empty queue returns `low`.
    pub random_values: VecDeque<i32>,
    /// What `sys_float_time` returns (seconds).
    pub time: f64,
    /// `(info, origin, angles)` of the physents `physent_by_info` finds.
    pub infos: Vec<(i32, Vec3, Vec3)>,
    /// Entities `physent_is_player` answers true for.
    pub player_ents: Vec<i32>,
    /// Entities `physent_has_model` answers true for (the world, ent 0, always has one).
    pub model_ents: Vec<i32>,
    /// The call log (interior mutability: `&self` trait methods log too).
    log: RefCell<Vec<Call>>,
}

impl Default for BoxWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl BoxWorld {
    /// An empty world with the engine's player hulls.
    pub fn new() -> Self {
        Self {
            boxes: Vec::new(),
            water: Vec::new(),
            ladders: Vec::new(),
            hull_mins: PLAYER_HULL_MINS,
            hull_maxs: PLAYER_HULL_MAXS,
            sounds: Vec::new(),
            random_values: VecDeque::new(),
            time: 0.0,
            infos: Vec::new(),
            player_ents: Vec::new(),
            model_ents: Vec::new(),
            log: RefCell::new(Vec::new()),
        }
    }

    /// A world with one big floor (ent 0): top at z = 0, x/y in -1000..1000, texture "FLOOR".
    pub fn with_floor() -> Self {
        let mut w = Self::new();
        w.solid(Vec3::new(-1000.0, -1000.0, -100.0), Vec3::new(1000.0, 1000.0, 0.0), "FLOOR");
        w
    }

    /// Adds a solid box and returns its entity number.
    pub fn solid(&mut self, mins: Vec3, maxs: Vec3, texture: &str) -> i32 {
        self.boxes.push(SolidBox { mins, maxs, texture: texture.to_string() });
        (self.boxes.len() - 1) as i32
    }

    /// Adds a water volume.
    pub fn water_volume(&mut self, mins: Vec3, maxs: Vec3, contents: i32, truecontents: i32) {
        self.water.push(WaterVolume { mins, maxs, contents, truecontents });
    }

    /// Adds a ladder moveent and returns its moveent index.
    pub fn ladder(&mut self, mins: Vec3, maxs: Vec3, normal: Vec3) -> usize {
        self.ladders.push(LadderEnt { mins, maxs, origin: Vec3::ZERO, normal });
        self.ladders.len() - 1
    }

    /// The call log so far.
    pub fn calls(&self) -> Vec<Call> {
        self.log.borrow().clone()
    }

    /// The kinds of the calls so far (see [`Call::kind`]).
    pub fn call_kinds(&self) -> Vec<&'static str> {
        self.log.borrow().iter().map(Call::kind).collect()
    }

    /// Empties the call log.
    pub fn clear_calls(&self) {
        self.log.borrow_mut().clear();
    }

    fn record(&self, call: Call) {
        self.log.borrow_mut().push(call);
    }

    fn hull(&self, usehull: i32) -> usize {
        usehull.clamp(0, 3) as usize
    }

    /// `PM_PlayerTrace` over the boxes: the closest hit wins, the first box wins ties.
    fn trace_boxes(&self, start: Vec3, end: Vec3, usehull: i32, ignore_pe: i32) -> PmTrace {
        if !start.is_finite() || !end.is_finite() {
            // Test-world safety: a non-finite sweep is "stuck in solid" and goes nowhere.
            let mut t = PmTrace::miss(start);
            t.allsolid = true;
            t.startsolid = true;
            t.fraction = 0.0;
            return t;
        }
        let h = self.hull(usehull);
        let mut best = PmTrace::miss(end);
        for (ent, b) in self.boxes.iter().enumerate() {
            if ent as i32 == ignore_pe {
                continue;
            }
            // The hull-expanded box: the player origin must stay outside of it.
            let emins = b.mins - self.hull_maxs[h];
            let emaxs = b.maxs - self.hull_mins[h];
            let Some(hit) = sweep(emins, emaxs, start, end) else {
                continue;
            };
            if hit.fraction < best.fraction {
                let mut t = PmTrace::miss(end);
                t.ent = ent as i32;
                t.fraction = hit.fraction;
                t.plane_normal = hit.normal;
                t.startsolid = hit.startsolid;
                t.allsolid = hit.allsolid;
                t.inopen = false;
                t.endpos = start + (end - start) * hit.fraction;
                best = t;
            }
        }
        best
    }

    fn solid_at(&self, p: Vec3) -> Option<usize> {
        self.boxes.iter().position(|b| inside(b.mins, b.maxs, p))
    }
}

/// Strictly inside the box (a point on a plane is on the empty side, as in a clip hull).
fn inside(mins: Vec3, maxs: Vec3, p: Vec3) -> bool {
    (0..3).all(|a| p[a] > mins[a] && p[a] < maxs[a])
}

struct Sweep {
    fraction: f32,
    normal: Vec3,
    startsolid: bool,
    allsolid: bool,
}

/// Sweeps the point `start -> end` against the box `[emins, emaxs]` the way
/// `PM_RecursiveHullCheck` does: the entry plane is approached to `DIST_EPSILON`, a start inside is
/// `startsolid` with fraction 0 (`allsolid` when the end is inside too).
fn sweep(emins: Vec3, emaxs: Vec3, start: Vec3, end: Vec3) -> Option<Sweep> {
    if inside(emins, emaxs, start) {
        return Some(Sweep { fraction: 0.0, normal: Vec3::ZERO, startsolid: true, allsolid: inside(emins, emaxs, end) });
    }
    let mut f_enter = -1.0f32;
    let mut f_exit = f32::INFINITY;
    let mut normal = Vec3::ZERO;
    for a in 0..3 {
        let s = start[a];
        let e = end[a];
        if s == e {
            // Parallel to the slab: must be strictly inside it.
            if !(s > emins[a] && s < emaxs[a]) {
                return None;
            }
            continue;
        }
        let mut axis = Vec3::ZERO;
        let (d1, d2, exit) = if e > s {
            axis[a] = -1.0;
            (emins[a] - s, emins[a] - e, (emaxs[a] - s) / (e - s))
        } else {
            axis[a] = 1.0;
            (s - emaxs[a], e - emaxs[a], (s - emins[a]) / (s - e))
        };
        f_exit = f_exit.min(exit);
        if d1 >= 0.0 {
            let f = ((d1 - DIST_EPSILON) / (d1 - d2)).max(0.0);
            if f > f_enter {
                f_enter = f;
                normal = axis;
            }
        }
    }
    if f_enter < 0.0 || f_enter >= f_exit || f_enter >= 1.0 {
        return None;
    }
    Some(Sweep { fraction: f_enter, normal, startsolid: false, allsolid: false })
}

impl PmWorld for BoxWorld {
    fn player_trace(&mut self, start: Vec3, end: Vec3, usehull: i32, _trace_flags: i32, ignore_pe: i32) -> PmTrace {
        self.record(Call::PlayerTrace { start, end, usehull });
        self.trace_boxes(start, end, usehull, ignore_pe)
    }

    fn test_player_position(&mut self, pm_origin: Vec3, pos: Vec3, usehull: i32) -> (i32, PmTrace) {
        self.record(Call::TestPlayerPosition { pos });
        // QUIRK of the engine: the returned trace is from `pm_origin` to `pm_origin`, not `pos`.
        let trace = self.trace_boxes(pm_origin, pm_origin, usehull, -1);
        let h = self.hull(usehull);
        for (ent, b) in self.boxes.iter().enumerate() {
            if inside(b.mins - self.hull_maxs[h], b.maxs - self.hull_mins[h], pos) {
                return (ent as i32, trace);
            }
        }
        (-1, trace)
    }

    fn point_contents(&mut self, p: Vec3) -> (i32, i32) {
        self.record(Call::PointContents(p));
        if self.solid_at(p).is_some() {
            return (CONTENTS_SOLID, CONTENTS_SOLID);
        }
        for v in &self.water {
            if inside(v.mins, v.maxs, p) {
                return (v.contents, v.truecontents);
            }
        }
        (CONTENTS_EMPTY, CONTENTS_EMPTY)
    }

    fn num_moveents(&self) -> usize {
        self.record(Call::NumMoveents);
        self.ladders.len()
    }

    fn ladder_contains(&mut self, moveent: usize, origin: Vec3, usehull: i32) -> bool {
        self.record(Call::LadderContains(moveent));
        let Some(l) = self.ladders.get(moveent) else {
            return false;
        };
        let h = self.hull(usehull);
        inside(l.mins + l.origin - self.hull_maxs[h], l.maxs + l.origin - self.hull_mins[h], origin)
    }

    fn moveent_bounds(&self, moveent: usize) -> (Vec3, Vec3) {
        self.record(Call::MoveentBounds(moveent));
        self.ladders.get(moveent).map_or((Vec3::ZERO, Vec3::ZERO), |l| (l.mins, l.maxs))
    }

    fn moveent_origin(&self, moveent: usize) -> Vec3 {
        self.record(Call::MoveentOrigin(moveent));
        self.ladders.get(moveent).map_or(Vec3::ZERO, |l| l.origin)
    }

    fn trace_model(&mut self, moveent: usize, start: Vec3, end: Vec3) -> (f32, Vec3) {
        self.record(Call::TraceModel(moveent));
        let Some(l) = self.ladders.get(moveent) else {
            return (1.0, Vec3::ZERO);
        };
        match sweep(l.mins + l.origin, l.maxs + l.origin, start, end) {
            Some(hit) => (hit.fraction, l.normal),
            None => (1.0, Vec3::ZERO),
        }
    }

    fn trace_texture(&mut self, ground: i32, _start: Vec3, _end: Vec3) -> Option<String> {
        self.record(Call::TraceTexture(ground));
        let b = self.boxes.get(usize::try_from(ground).ok()?)?;
        (!b.texture.is_empty()).then(|| b.texture.clone())
    }

    fn play_sound(&mut self, channel: i32, sample: &str, volume: f32, attenuation: f32, flags: i32, pitch: i32) {
        self.record(Call::PlaySound(sample.to_string()));
        self.sounds.push(SoundEvent { channel, sample: sample.to_string(), volume, attenuation, flags, pitch });
    }

    fn random_long(&mut self, low: i32, high: i32) -> i32 {
        self.record(Call::RandomLong(low, high));
        self.random_values.pop_front().unwrap_or(low)
    }

    fn sys_float_time(&mut self) -> f64 {
        self.record(Call::SysFloatTime);
        self.time
    }

    fn physent_by_info(&self, info: i32) -> Option<(Vec3, Vec3)> {
        self.record(Call::PhysentByInfo(info));
        self.infos.iter().find(|(i, _, _)| *i == info).map(|&(_, o, a)| (o, a))
    }

    fn physent_is_player(&self, ent: i32) -> bool {
        self.player_ents.contains(&ent)
    }

    fn physent_has_model(&self, ent: i32) -> bool {
        ent == 0 || self.model_ents.contains(&ent)
    }
}

/// A standing walker at `origin`, server side, no input; a fresh `PmShared` without materials.
pub fn standing_player(origin: Vec3) -> (PlayerMove, PmShared) {
    let pm = PlayerMove { origin, server: true, multiplayer: true, ..PlayerMove::default() };
    (pm, pm_init(None))
}

/// Runs one command of `msec` milliseconds: sets the command, prefills `frametime` as the engine
/// does, and calls `pm_move`.
pub fn run_cmd(pm: &mut PlayerMove, sh: &mut PmShared, w: &mut BoxWorld, msec: u8) {
    pm.cmd.msec = msec;
    pm.frametime = (f64::from(msec) * 0.001) as f32;
    pm_move(pm, sh, w);
}
