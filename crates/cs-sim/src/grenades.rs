//! CS 1.6 grenade physics (HE, Flashbang, Smoke) bouncing against BSP geometry.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::trace::{BspCollision, Hull};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrenadeKind {
    He,
    Flashbang,
    Smoke,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveGrenade {
    pub id: u32,
    pub owner_id: u32,
    pub kind: GrenadeKind,
    pub position: Vec3,
    pub velocity: Vec3,
    pub fuse_remaining: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmokeCloud {
    pub position: Vec3,
    pub radius: f32,
    pub remaining: f32,
}

#[derive(Debug, Clone)]
pub enum GrenadeEvent {
    Bounce { pos: Vec3, kind: GrenadeKind },
    Detonate { owner_id: u32, kind: GrenadeKind, pos: Vec3 },
}

pub fn step_grenades(
    grenades: &mut Vec<ActiveGrenade>,
    smokes: &mut Vec<SmokeCloud>,
    collision: &BspCollision,
    gravity: f32,
    dt: f32,
) -> Vec<GrenadeEvent> {
    let mut events = Vec::new();

    for cloud in smokes.iter_mut() {
        cloud.remaining -= dt;
    }
    smokes.retain(|c| c.remaining > 0.0);

    let mut i = 0;
    while i < grenades.len() {
        let g = &mut grenades[i];
        g.fuse_remaining -= dt;

        // Gravity (classic CS 1.6 grenade gravity ~0.55x player gravity)
        g.velocity.z -= gravity * 0.55 * dt;
        let next = g.position + g.velocity * dt;
        let tr = collision.trace_hull(g.position, next, Hull::Point);

        if tr.fraction < 1.0 && !tr.all_solid {
            g.position = tr.end_pos + tr.plane_normal * 2.0;
            let vn = g.velocity.dot(tr.plane_normal);
            let restitution = if tr.plane_normal.z > 0.7 { 0.38 } else { 0.48 };
            g.velocity -= tr.plane_normal * ((1.0 + restitution) * vn);
            if tr.plane_normal.z > 0.7 {
                // Ground friction when rolling
                g.velocity.x *= 0.75;
                g.velocity.y *= 0.75;
                if g.velocity.length_squared() < 25.0 {
                    g.velocity = Vec3::ZERO;
                }
            }
            if vn.abs() > 45.0 {
                events.push(GrenadeEvent::Bounce {
                    pos: g.position,
                    kind: g.kind,
                });
            }
        } else {
            g.position = next;
        }

        // Smoke waits until it slows down on the floor or fuse expires
        let detonate = match g.kind {
            GrenadeKind::Smoke => g.fuse_remaining <= 0.0 && (g.velocity.length_squared() < 400.0 || g.fuse_remaining <= -1.5),
            GrenadeKind::He | GrenadeKind::Flashbang => g.fuse_remaining <= 0.0,
        };

        if detonate {
            let popped = grenades.swap_remove(i);
            if popped.kind == GrenadeKind::Smoke {
                smokes.push(SmokeCloud {
                    position: popped.position + Vec3::new(0.0, 0.0, 32.0),
                    radius: 135.0,
                    remaining: 18.0,
                });
            }
            events.push(GrenadeEvent::Detonate {
                owner_id: popped.owner_id,
                kind: popped.kind,
                pos: popped.position,
            });
        } else {
            i += 1;
        }
    }

    events
}

/// Check whether line segment `a -> b` passes through any active smoke cloud (blocks bot LOS).
pub fn segment_blocked_by_smoke(a: Vec3, b: Vec3, smokes: &[SmokeCloud]) -> bool {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-4 {
        return false;
    }
    for s in smokes {
        let t = ((s.position - a).dot(ab) / len_sq).clamp(0.0, 1.0);
        let closest = a + ab * t;
        if closest.distance_squared(s.position) <= (s.radius * 0.85).powi(2) {
            return true;
        }
    }
    false
}
