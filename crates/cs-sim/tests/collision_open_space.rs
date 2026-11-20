//! Second characterization pass of `BspCollision`, aimed at real traces and true zone and
//! ladder answers: trace starts are drawn by rejection until they lie in open space, and
//! zone / ladder queries are also sampled around the map's spawns, buy zones, bomb targets
//! and ladders. Run on de_dust2 and on de_nuke (a map with `func_ladder`). Uses the public
//! `BspCollision` API only, so the same file builds against the code before and after the
//! hull-tracing core moved into `cs-engine`. Each test returns without assertions when the
//! installed game is absent; with the game installed, a map that fails to load is a failure.
//!
//! Call sequence for one map (one LCG stream per map, consumed strictly in this order):
//!
//! - LCG: `state = state * 6364136223846793005 + 1442695040888963407` (wrapping, `u64`),
//!   seed `0xC516`. `unit()` steps once and returns `(state >> 40) as f32 / 2^24`.
//!   `point()` is `mins + (maxs - mins) * unit()` per axis x, y, z over the world bounds;
//!   `in_box(mins, maxs)` the same over a given box; `offset(r)` is `(unit() * 2 - 1) * r`
//!   per axis. `open(hull)` repeats `point()` until `point_contents(p, hull) == CONTENTS_EMPTY`
//!   (at most 10 000 draws).
//! - Hulls cycle `Point, Standing, Large, Crouched` by `i % 4`.
//! - Traces: for `i in 0..2000`, `start = open(hull)`, `end = start + offset(512)`,
//!   `trace_hull(start, end, hull)`; hashed as `fraction`, `end_pos`, `plane_normal` raw
//!   `f32` bits (little endian) and a flag byte
//!   `all_solid | start_solid << 1 | hit_sky << 2 | hit_solid << 3`.
//! - Contents: 2000 `point_contents(point(), hull)`, hashing the `i32` (little endian).
//! - Ladders: 500 `is_on_ladder(open(hull), hull)`; then, if the map has ladders, 500
//!   `is_on_ladder(in_box(l.mins - 32, l.maxs + 32), hull)` with `l` the `i % n`-th
//!   `func_ladder` brush in `BspMap::brush_entities` order. One byte per answer.
//! - Line of sight: 500 `has_line_of_sight(a, a + offset(512))` with `a = open(Point)`.
//! - Zones: 500 positions `open(Standing)`, then 500 targeted positions taken by `i % 3`
//!   from: `spawn + offset(256)` with spawn `i % n` of `BspMap::spawns`; `in_box` of buy-zone
//!   brush `i % n`; `in_box` of bomb-target brush `i % n` (a missing kind falls back to the
//!   spawn rule). For each position: `is_in_buy_zone(p, Terrorist)`,
//!   `is_in_buy_zone(p, CounterTerrorist)`, `is_in_bomb_target(p)`, `is_in_hostage_rescue(p)`,
//!   one byte each.
//!
//! Each of the five sections has its own FNV-1a 64 hash. The constants were captured on the
//! project owner's machine (Steam CS 1.6) against the pre-move sources of commit `a029927`.

use cs_assets::bsp::{BrushEntity, BrushVolumeKind, BspMap, SpawnTeam};
use cs_assets::materials::MaterialTable;
use cs_sim::trace::{BspCollision, CONTENTS_EMPTY, Hull, TraceResult};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const DUST2: Hashes = Hashes {
    trace: 0x841ea81f55ac8194,
    contents: 0x5b829d8950d41981,
    ladder: 0x1fe3fce62bd816b5,
    sight: 0xbcc698dd0a3e560f,
    zone: 0xb03ff0ca4c6dd281,
};

/// de_nuke: the first of the installed maps checked (`de_nuke`, `cs_assault`, `de_train`)
/// that has `func_ladder` brushes.
const NUKE: Hashes = Hashes {
    trace: 0xb9a3998da5b79ecc,
    contents: 0x10713892049499e5,
    ladder: 0xb1d1b18b29265f2e,
    sight: 0xda6c9b0a68bcb829,
    zone: 0x03eb5e24fbb58a30,
};

const SEED: u64 = 0xC516;
const HULLS: [Hull; 4] = [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched];

#[derive(Debug, PartialEq, Eq)]
struct Hashes {
    trace: u64,
    contents: u64,
    ladder: u64,
    sight: u64,
    zone: u64,
}

/// How much of the pass exercised something real.
#[derive(Debug, Default)]
struct Coverage {
    open_starts: usize,
    trace_hits: usize,
    ladders_open: usize,
    ladders_near: usize,
    sight_true: usize,
    zones_open: [usize; 4],
    zones_targeted: [usize; 4],
}

struct Lcg {
    state: u64,
    mins: Vec3,
    maxs: Vec3,
}

impl Lcg {
    fn unit(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 40) as f32 / 16_777_216.0
    }

    fn in_box(&mut self, mins: Vec3, maxs: Vec3) -> Vec3 {
        let x = mins.x + (maxs.x - mins.x) * self.unit();
        let y = mins.y + (maxs.y - mins.y) * self.unit();
        let z = mins.z + (maxs.z - mins.z) * self.unit();
        Vec3::new(x, y, z)
    }

    fn point(&mut self) -> Vec3 {
        self.in_box(self.mins, self.maxs)
    }

    fn offset(&mut self, r: f32) -> Vec3 {
        let x = (self.unit() * 2.0 - 1.0) * r;
        let y = (self.unit() * 2.0 - 1.0) * r;
        let z = (self.unit() * 2.0 - 1.0) * r;
        Vec3::new(x, y, z)
    }

    fn open(&mut self, collision: &BspCollision, hull: Hull) -> Vec3 {
        for _ in 0..10_000 {
            let p = self.point();
            if collision.point_contents(p, hull) == CONTENTS_EMPTY {
                return p;
            }
        }
        panic!("no open point for {hull:?} in 10 000 draws");
    }
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn f32(&mut self, v: f32) {
        self.bytes(&v.to_bits().to_le_bytes());
    }

    fn vec3(&mut self, v: Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }

    /// Hashes `b` and returns it as `0`/`1` for counting.
    fn bool(&mut self, b: bool) -> usize {
        self.bytes(&[u8::from(b)]);
        usize::from(b)
    }

    fn trace(&mut self, tr: &TraceResult) {
        self.f32(tr.fraction);
        self.vec3(tr.end_pos);
        self.vec3(tr.plane_normal);
        let flags = u8::from(tr.all_solid)
            | u8::from(tr.start_solid) << 1
            | u8::from(tr.hit_sky) << 2
            | u8::from(tr.hit_solid) << 3;
        self.bytes(&[flags]);
    }
}

fn brushes(bsp: &BspMap, pick: impl Fn(&BrushVolumeKind) -> bool) -> Vec<&BrushEntity> {
    bsp.brush_entities
        .iter()
        .filter(|b| pick(&b.kind))
        .collect()
}

/// Hashes the four zone answers at `p`, adding the `true` ones to `counts`.
fn zones(h: &mut Fnv, collision: &BspCollision, p: Vec3, counts: &mut [usize; 4]) {
    counts[0] += h.bool(collision.is_in_buy_zone(p, SpawnTeam::Terrorist));
    counts[1] += h.bool(collision.is_in_buy_zone(p, SpawnTeam::CounterTerrorist));
    counts[2] += h.bool(collision.is_in_bomb_target(p));
    counts[3] += h.bool(collision.is_in_hostage_rescue(p));
}

/// Runs the pass on `map`, or `None` without the installed game. With the game installed, a
/// map that fails to load panics.
fn run_pass(map: &str) -> Option<(Hashes, Coverage)> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let bsp = BspMap::load(&vfs, map).unwrap_or_else(|err| panic!("{map} fails to load: {err:?}"));
    let collision = BspCollision::from_map(&bsp, MaterialTable::load(&vfs));
    let (mins, maxs) = collision.world_bounds();
    let mut lcg = Lcg {
        state: SEED,
        mins,
        maxs,
    };
    let mut cov = Coverage::default();

    let mut h = Fnv::new();
    for i in 0..2000 {
        let hull = HULLS[i % 4];
        let start = lcg.open(&collision, hull);
        let end = start + lcg.offset(512.0);
        let tr = collision.trace_hull(start, end, hull);
        cov.open_starts += usize::from(!tr.start_solid);
        cov.trace_hits += usize::from(tr.fraction < 1.0);
        h.trace(&tr);
    }
    let trace = h.0;

    let mut h = Fnv::new();
    for i in 0..2000 {
        let c = collision.point_contents(lcg.point(), HULLS[i % 4]);
        h.bytes(&c.to_le_bytes());
    }
    let contents = h.0;

    let ladders = brushes(&bsp, |k| matches!(k, BrushVolumeKind::Ladder));
    let mut h = Fnv::new();
    for i in 0..500 {
        let hull = HULLS[i % 4];
        let p = lcg.open(&collision, hull);
        cov.ladders_open += h.bool(collision.is_on_ladder(p, hull));
    }
    if !ladders.is_empty() {
        for i in 0..500 {
            let l = ladders[i % ladders.len()];
            let p = lcg.in_box(l.mins - Vec3::splat(32.0), l.maxs + Vec3::splat(32.0));
            cov.ladders_near += h.bool(collision.is_on_ladder(p, HULLS[i % 4]));
        }
    }
    let ladder = h.0;

    let mut h = Fnv::new();
    for _ in 0..500 {
        let a = lcg.open(&collision, Hull::Point);
        let b = a + lcg.offset(512.0);
        cov.sight_true += h.bool(collision.has_line_of_sight(a, b));
    }
    let sight = h.0;

    let buy_zones = brushes(&bsp, |k| matches!(k, BrushVolumeKind::BuyZone { .. }));
    let bomb_targets = brushes(&bsp, |k| matches!(k, BrushVolumeKind::BombTarget));
    let mut h = Fnv::new();
    for _ in 0..500 {
        let p = lcg.open(&collision, Hull::Standing);
        zones(&mut h, &collision, p, &mut cov.zones_open);
    }
    for i in 0..500 {
        let p = match i % 3 {
            1 if !buy_zones.is_empty() => {
                let z = buy_zones[i % buy_zones.len()];
                lcg.in_box(z.mins, z.maxs)
            }
            2 if !bomb_targets.is_empty() => {
                let z = bomb_targets[i % bomb_targets.len()];
                lcg.in_box(z.mins, z.maxs)
            }
            _ => bsp.spawns[i % bsp.spawns.len()].position + lcg.offset(256.0),
        };
        zones(&mut h, &collision, p, &mut cov.zones_targeted);
    }
    let zone = h.0;

    let hashes = Hashes {
        trace,
        contents,
        ladder,
        sight,
        zone,
    };
    println!("{map}: {hashes:#018x?}\n{map}: {cov:?}");
    Some((hashes, cov))
}

#[test]
fn dust2_open_space_answers_are_unchanged() {
    let Some((hashes, cov)) = run_pass("de_dust2") else {
        return;
    };
    assert_eq!(cov.open_starts, 2000);
    assert_eq!(hashes, DUST2);
}

#[test]
fn nuke_open_space_and_ladder_answers_are_unchanged() {
    let Some((hashes, cov)) = run_pass("de_nuke") else {
        return;
    };
    assert_eq!(cov.open_starts, 2000);
    assert!(cov.ladders_near > 0, "no ladder answer is true: {cov:?}");
    assert_eq!(hashes, NUKE);
}
