//! Characterization of `BspCollision` on de_dust2: hashes of its answers to a fixed,
//! pseudo-random set of queries, recorded before the hull-tracing core moved into
//! `cs-engine`. The move must not change a single bit. Every test returns without
//! assertions when the installed game is absent.
//!
//! Call sequence (one LCG stream, consumed strictly in this order):
//!
//! - LCG: `state = state * 6364136223846793005 + 1442695040888963407` (wrapping, `u64`),
//!   seed `0xC516`. `unit()` steps once and returns `(state >> 40) as f32 / 2^24`, in `[0, 1)`.
//!   `point()` is `mins + (maxs - mins) * unit()` per axis x, y, z (world model bounds);
//!   `offset(r)` is `(unit() * 2 - 1) * r` per axis x, y, z.
//! - Hulls are taken in the order `Point, Standing, Large, Crouched` by `i % 4`.
//! - 2000 traces: `start = point()`, `end = start + offset(512)`, `trace_hull(start, end, hull)`.
//!   The trace hash covers `fraction`, `end_pos`, `plane_normal` (raw `f32` bits, little
//!   endian) and a flag byte `all_solid | start_solid << 1 | hit_sky << 2 | hit_solid << 3`.
//! - 2000 `point_contents(point(), hull)`, hashing the `i32` (little endian).
//! - 500 `is_on_ladder(point(), hull)`, hashing one byte per answer.
//! - 500 `has_line_of_sight(a, a + offset(512))` with `a = point()`, one byte per answer.
//! - 500 positions `p = point()`, hashing `is_in_buy_zone(p, Terrorist)` then
//!   `is_in_buy_zone(p, CounterTerrorist)`, one byte each.
//!
//! Each section has its own FNV-1a 64 hash. `WORLD_ONLY_TRACE_HASH` replays the 2000 traces
//! (fresh seed) on a clone of the collision with every brush model (`1..models.len()`) in
//! `disabled_models`, i.e. against the world model alone.
//!
//! The hash constants were captured on the project owner's machine at commit `7fa58e2`
//! (before the move), with the Steam install of CS 1.6.

use cs_assets::bsp::{BspMap, SpawnTeam};
use cs_assets::materials::MaterialTable;
use cs_sim::trace::{BspCollision, Hull, TraceResult};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const TRACE_HASH: u64 = 0xb9b78d4a0fc6b943;
const CONTENTS_HASH: u64 = 0xbb0cab94fb0fe044;
const LADDER_HASH: u64 = 0x1fe3fce62bd816b5;
const SIGHT_HASH: u64 = 0x2af166312bd1b982;
const BUY_ZONE_HASH: u64 = 0x35318b2399e15770;
const WORLD_ONLY_TRACE_HASH: u64 = 0xc2187f037968731c;

const SEED: u64 = 0xC516;
const HULLS: [Hull; 4] = [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched];

struct Lcg {
    state: u64,
    mins: Vec3,
    maxs: Vec3,
}

impl Lcg {
    fn new(bounds: (Vec3, Vec3)) -> Self {
        Self {
            state: SEED,
            mins: bounds.0,
            maxs: bounds.1,
        }
    }

    fn unit(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.state >> 40) as f32 / 16_777_216.0
    }

    fn point(&mut self) -> Vec3 {
        let x = self.mins.x + (self.maxs.x - self.mins.x) * self.unit();
        let y = self.mins.y + (self.maxs.y - self.mins.y) * self.unit();
        let z = self.mins.z + (self.maxs.z - self.mins.z) * self.unit();
        Vec3::new(x, y, z)
    }

    fn offset(&mut self, r: f32) -> Vec3 {
        let x = (self.unit() * 2.0 - 1.0) * r;
        let y = (self.unit() * 2.0 - 1.0) * r;
        let z = (self.unit() * 2.0 - 1.0) * r;
        Vec3::new(x, y, z)
    }
}

/// FNV-1a 64, plus how many `true` answers it has hashed (printed, to show coverage).
struct Fnv(u64, usize);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325, 0)
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

    fn bool(&mut self, b: bool) {
        self.1 += usize::from(b);
        self.bytes(&[u8::from(b)]);
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

/// de_dust2's collision and its model count, or `None` without the installed game.
fn dust2() -> Option<(BspCollision, usize)> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let bsp = BspMap::load(&vfs, "de_dust2").ok()?;
    let collision = BspCollision::from_map(&bsp, MaterialTable::load(&vfs));
    Some((collision, bsp.models.len()))
}

/// The 2000 trace queries of the sequence, in order.
fn trace_queries(lcg: &mut Lcg) -> Vec<(Vec3, Vec3, Hull)> {
    (0..2000)
        .map(|i| {
            let start = lcg.point();
            let end = start + lcg.offset(512.0);
            (start, end, HULLS[i % 4])
        })
        .collect()
}

fn hash_traces(collision: &BspCollision, queries: &[(Vec3, Vec3, Hull)]) -> u64 {
    let mut h = Fnv::new();
    for &(start, end, hull) in queries {
        h.trace(&collision.trace_hull(start, end, hull));
    }
    h.0
}

#[test]
fn bsp_collision_answers_are_unchanged() {
    let Some((collision, _)) = dust2() else {
        return;
    };
    let mut lcg = Lcg::new(collision.world_bounds());

    let queries = trace_queries(&mut lcg);
    let trace = hash_traces(&collision, &queries);

    let mut h = Fnv::new();
    for i in 0..2000 {
        let c = collision.point_contents(lcg.point(), HULLS[i % 4]);
        h.bytes(&c.to_le_bytes());
    }
    let contents = h.0;

    let mut h = Fnv::new();
    for i in 0..500 {
        h.bool(collision.is_on_ladder(lcg.point(), HULLS[i % 4]));
    }
    let (ladder, ladders) = (h.0, h.1);

    let mut h = Fnv::new();
    for _ in 0..500 {
        let a = lcg.point();
        let b = a + lcg.offset(512.0);
        h.bool(collision.has_line_of_sight(a, b));
    }
    let (sight, sights) = (h.0, h.1);

    let mut h = Fnv::new();
    for _ in 0..500 {
        let p = lcg.point();
        h.bool(collision.is_in_buy_zone(p, SpawnTeam::Terrorist));
        h.bool(collision.is_in_buy_zone(p, SpawnTeam::CounterTerrorist));
    }
    let (buy, buys) = (h.0, h.1);

    let open = queries
        .iter()
        .filter(|&&(start, end, hull)| !collision.trace_hull(start, end, hull).start_solid)
        .count();
    println!(
        "TRACE_HASH {trace:#018x} ({open} of 2000 start outside solid)\n\
         CONTENTS_HASH {contents:#018x}\nLADDER_HASH {ladder:#018x} ({ladders} true)\n\
         SIGHT_HASH {sight:#018x} ({sights} true)\nBUY_ZONE_HASH {buy:#018x} ({buys} true)"
    );
    assert_eq!(trace, TRACE_HASH, "trace_hull");
    assert_eq!(contents, CONTENTS_HASH, "point_contents");
    assert_eq!(ladder, LADDER_HASH, "is_on_ladder");
    assert_eq!(sight, SIGHT_HASH, "has_line_of_sight");
    assert_eq!(buy, BUY_ZONE_HASH, "is_in_buy_zone");
}

#[test]
fn world_only_traces_are_unchanged() {
    let Some((collision, model_count)) = dust2() else {
        return;
    };
    let mut world_only = collision.clone();
    world_only.disabled_models.extend(1..model_count);
    let queries = trace_queries(&mut Lcg::new(collision.world_bounds()));
    let hash = hash_traces(&world_only, &queries);
    println!("WORLD_ONLY_TRACE_HASH {hash:#018x}");
    assert_eq!(hash, WORLD_ONLY_TRACE_HASH);
}
