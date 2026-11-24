//! GoldSrc client particle, weather (`ReceiveW`), and beam (`TE_BEAM*` / `TE_KILLBEAM`)
//! simulation systems emitting geometry batches for [`FxRenderer`].

use cs_clientcore::tempents::ParticleEffect;
use cs_proto48::tempent::{BeamParams, TempEffect};
use glam::Vec3;

use crate::render::fx_renderer::{FxBeam, FxPoint};

const MAX_PARTICLES: usize = 4096;

/// GoldSrc / Quake 256-entry RGB palette (normalized 0..1) for `color: u8` particle effects.
pub fn goldsrc_palette_rgb(idx: u8) -> [f32; 3] {
    let g = f32::from(idx & 15) / 15.0;
    let ramp = idx >> 4;
    match ramp {
        0 => [0.06 + g * 0.85, 0.06 + g * 0.85, 0.06 + g * 0.85], // gray
        1 => [0.22 + g * 0.55, 0.14 + g * 0.35, 0.08 + g * 0.22], // brown
        2 => [0.18 + g * 0.55, 0.22 + g * 0.62, 0.32 + g * 0.65], // steel blue
        3 => [0.12 + g * 0.35, 0.28 + g * 0.55, 0.10 + g * 0.28], // olive green
        4 => [0.45 + g * 0.55, 0.08 + g * 0.25, 0.05 + g * 0.15], // red / blood
        5 => [0.55 + g * 0.45, 0.42 + g * 0.45, 0.12 + g * 0.25], // gold / brass
        6 => [0.65 + g * 0.35, 0.25 + g * 0.55, 0.04 + g * 0.30], // fire orange
        7 => [0.48 + g * 0.42, 0.28 + g * 0.38, 0.18 + g * 0.30], // peach / skin
        8 => [0.42 + g * 0.45, 0.26 + g * 0.40, 0.48 + g * 0.45], // lavender
        9 => [0.45 + g * 0.45, 0.20 + g * 0.40, 0.38 + g * 0.45], // purple
        10 => [0.35 + g * 0.45, 0.22 + g * 0.35, 0.14 + g * 0.28], // tan
        11 => [0.15 + g * 0.40, 0.45 + g * 0.45, 0.48 + g * 0.48], // cyan
        12 => [0.25 + g * 0.70, 0.25 + g * 0.70, 0.10 + g * 0.20], // yellow
        13 => [0.10 + g * 0.35, 0.25 + g * 0.65, 0.65 + g * 0.35], // electric blue
        14 => [0.85 + g * 0.15, 0.35 + g * 0.60, 0.08 + g * 0.35], // bright ember
        _ => [0.85 + g * 0.15, 0.78 + g * 0.22, 0.55 + g * 0.45],  // bright spark
    }
}

#[derive(Debug, Clone)]
pub struct Particle {
    pub origin: Vec3,
    pub vel: Vec3,
    pub accel: Vec3,
    pub color: [f32; 4],
    pub size: f32,
    pub born: f64,
    pub die: f64,
    pub additive: bool,
    pub streak_len: f32,
}

#[derive(Debug, Clone)]
struct ParticleRng(u64);

impl ParticleRng {
    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        ((x >> 40) as f32) / ((1u32 << 24) as f32)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    fn vec3(&mut self, half: f32) -> Vec3 {
        Vec3::new(
            self.range(-half, half),
            self.range(-half, half),
            self.range(-half, half),
        )
    }
}

fn push_particle(particles: &mut Vec<Particle>, p: Particle) {
    if particles.len() >= MAX_PARTICLES {
        particles.remove(0);
    }
    particles.push(p);
}

#[derive(Debug, Clone)]
pub struct ParticleSystem {
    particles: Vec<Particle>,
    rng: ParticleRng,
    weather_accum: f32,
}

impl Default for ParticleSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self {
            particles: Vec::new(),
            rng: ParticleRng(0x5EED_C516_9E37_79B9),
            weather_accum: 0.0,
        }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.weather_accum = 0.0;
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// Spawn GoldSrc particle burst/trail matching any `ParticleEffect` from `cs-clientcore::tempents`.
    pub fn spawn_effect(&mut self, now: f64, effect: &ParticleEffect) {
        let rng = &mut self.rng;
        let particles = &mut self.particles;
        match *effect {
            ParticleEffect::Gunshot { position, count } => {
                let n = usize::from(count.clamp(4, 32));
                for _ in 0..n {
                    let vel = rng.vec3(90.0) + Vec3::new(0.0, 0.0, rng.range(20.0, 90.0));
                    let col = if rng.next_f32() < 0.45 {
                        [1.0, 0.78, 0.35, 0.9]
                    } else {
                        [0.55, 0.52, 0.48, 0.7]
                    };
                    push_particle(
                        particles,
                        Particle {
                            origin: position + rng.vec3(3.0),
                            vel,
                            accel: Vec3::new(0.0, 0.0, -400.0),
                            color: col,
                            size: 1.6,
                            born: now,
                            die: now + f64::from(rng.range(0.15, 0.4)),
                            additive: col[0] > 0.9,
                            streak_len: 0.02,
                        },
                    );
                }
            }
            ParticleEffect::BulletImpact { position } => {
                for _ in 0..10 {
                    let vel = rng.vec3(110.0) + Vec3::new(0.0, 0.0, 40.0);
                    push_particle(
                        particles,
                        Particle {
                            origin: position,
                            vel,
                            accel: Vec3::new(0.0, 0.0, -320.0),
                            color: [0.85, 0.72, 0.42, 0.85],
                            size: 1.5,
                            born: now,
                            die: now + f64::from(rng.range(0.12, 0.3)),
                            additive: true,
                            streak_len: 0.025,
                        },
                    );
                }
            }
            ParticleEffect::Sparks { position } | ParticleEffect::ArmorRicochet { position } => {
                for _ in 0..14 {
                    let vel = rng.vec3(160.0) + Vec3::new(0.0, 0.0, 70.0);
                    push_particle(
                        particles,
                        Particle {
                            origin: position,
                            vel,
                            accel: Vec3::new(0.0, 0.0, -480.0),
                            color: [1.0, 0.88, 0.48, 1.0],
                            size: 1.4,
                            born: now,
                            die: now + f64::from(rng.range(0.2, 0.5)),
                            additive: true,
                            streak_len: 0.035,
                        },
                    );
                }
            }
            ParticleEffect::Flicker { position } => {
                for _ in 0..12 {
                    let vel = rng.vec3(32.0) + Vec3::new(0.0, 0.0, 28.0);
                    push_particle(
                        particles,
                        Particle {
                            origin: position + rng.vec3(8.0),
                            vel,
                            accel: Vec3::new(0.0, 0.0, 20.0),
                            color: [1.0, 0.65, 0.25, 0.9],
                            size: 2.2,
                            born: now,
                            die: now + 0.25,
                            additive: true,
                            streak_len: 0.0,
                        },
                    );
                }
            }
            ParticleEffect::Explosion2 {
                position,
                color,
                count,
            } => {
                let n = usize::from(count).clamp(8, 128);
                for i in 0..n {
                    let c = goldsrc_palette_rgb(color.wrapping_add((i & 7) as u8));
                    let vel = rng.vec3(220.0) + Vec3::new(0.0, 0.0, 60.0);
                    push_particle(
                        particles,
                        Particle {
                            origin: position + rng.vec3(8.0),
                            vel,
                            accel: Vec3::new(0.0, 0.0, -300.0),
                            color: [c[0], c[1], c[2], 0.95],
                            size: 2.4,
                            born: now,
                            die: now + f64::from(rng.range(0.35, 0.85)),
                            additive: false,
                            streak_len: 0.0,
                        },
                    );
                }
            }
            ParticleEffect::SvcParticle {
                origin: position,
                color,
                count,
                ..
            } => {
                let n = (count as usize).clamp(8, 128);
                for i in 0..n {
                    let c = goldsrc_palette_rgb(color.wrapping_add((i & 7) as u8));
                    let vel = rng.vec3(220.0) + Vec3::new(0.0, 0.0, 60.0);
                    push_particle(
                        particles,
                        Particle {
                            origin: position + rng.vec3(8.0),
                            vel,
                            accel: Vec3::new(0.0, 0.0, -300.0),
                            color: [c[0], c[1], c[2], 0.95],
                            size: 2.4,
                            born: now,
                            die: now + f64::from(rng.range(0.35, 0.85)),
                            additive: false,
                            streak_len: 0.0,
                        },
                    );
                }
            }
            ParticleEffect::BloodStream {
                origin,
                direction,
                color,
                count,
            } => {
                let c = goldsrc_palette_rgb(color);
                let dir = direction.normalize_or_zero();
                let n = usize::from(count.clamp(8, 48));
                for _ in 0..n {
                    let speed = rng.range(80.0, 220.0);
                    let vel = dir * speed + rng.vec3(45.0);
                    push_particle(
                        particles,
                        Particle {
                            origin,
                            vel,
                            accel: Vec3::new(0.0, 0.0, -520.0),
                            color: [c[0].max(0.55), c[1] * 0.25, c[2] * 0.25, 0.92],
                            size: 2.0,
                            born: now,
                            die: now + f64::from(rng.range(0.3, 0.7)),
                            additive: false,
                            streak_len: 0.015,
                        },
                    );
                }
            }
            ParticleEffect::Blood {
                origin,
                direction,
                color,
                speed,
            } => {
                let c = goldsrc_palette_rgb(color);
                let dir = direction.normalize_or_zero();
                let base_speed = f32::from(speed).max(30.0);
                for _ in 0..16 {
                    let vel = dir * base_speed + rng.vec3(50.0);
                    push_particle(
                        particles,
                        Particle {
                            origin,
                            vel,
                            accel: Vec3::new(0.0, 0.0, -450.0),
                            color: [c[0].max(0.5), c[1] * 0.3, c[2] * 0.25, 0.9],
                            size: 1.8,
                            born: now,
                            die: now + f64::from(rng.range(0.25, 0.6)),
                            additive: false,
                            streak_len: 0.0,
                        },
                    );
                }
            }
            ParticleEffect::StreakSplash {
                start,
                direction,
                color,
                count,
                speed,
                random_velocity,
            } => {
                let c = goldsrc_palette_rgb(color);
                let dir = direction.normalize_or_zero();
                let base = f32::from(speed).max(40.0);
                let jitter = f32::from(random_velocity).max(20.0);
                let n = usize::from(count.clamp(4, 64));
                for _ in 0..n {
                    let vel = dir * base + rng.vec3(jitter);
                    push_particle(
                        particles,
                        Particle {
                            origin: start,
                            vel,
                            accel: Vec3::new(0.0, 0.0, -600.0),
                            color: [c[0], c[1], c[2], 1.0],
                            size: 1.3,
                            born: now,
                            die: now + f64::from(rng.range(0.25, 0.55)),
                            additive: true,
                            streak_len: 0.04,
                        },
                    );
                }
            }
            ParticleEffect::TarExplosion { position } => {
                for _ in 0..48 {
                    let vel = rng.vec3(180.0) + Vec3::new(0.0, 0.0, 90.0);
                    let ember = rng.next_f32() < 0.35;
                    push_particle(
                        particles,
                        Particle {
                            origin: position + rng.vec3(6.0),
                            vel,
                            accel: Vec3::new(0.0, 0.0, -400.0),
                            color: if ember {
                                [1.0, 0.5, 0.15, 0.95]
                            } else {
                                [0.18, 0.16, 0.18, 0.9]
                            },
                            size: 2.5,
                            born: now,
                            die: now + f64::from(rng.range(0.4, 1.0)),
                            additive: ember,
                            streak_len: 0.0,
                        },
                    );
                }
            }
            ParticleEffect::LavaSplash { position } => {
                for _ in 0..48 {
                    let vel = rng.vec3(120.0) + Vec3::new(0.0, 0.0, rng.range(120.0, 260.0));
                    push_particle(
                        particles,
                        Particle {
                            origin: position + Vec3::new(rng.range(-24.0, 24.0), rng.range(-24.0, 24.0), 0.0),
                            vel,
                            accel: Vec3::new(0.0, 0.0, -350.0),
                            color: [1.0, rng.range(0.25, 0.7), 0.08, 0.95],
                            size: 2.6,
                            born: now,
                            die: now + f64::from(rng.range(0.6, 1.4)),
                            additive: true,
                            streak_len: 0.02,
                        },
                    );
                }
            }
            ParticleEffect::Teleport { position } => {
                for _ in 0..40 {
                    let angle = rng.range(0.0, std::f32::consts::TAU);
                    let r = rng.range(12.0, 32.0);
                    let origin = position + Vec3::new(angle.cos() * r, angle.sin() * r, rng.range(-24.0, 24.0));
                    let vel = Vec3::new(angle.cos() * 40.0, angle.sin() * 40.0, rng.range(-20.0, 40.0));
                    push_particle(
                        particles,
                        Particle {
                            origin,
                            vel,
                            accel: Vec3::ZERO,
                            color: [0.45, 0.85, 1.0, 0.9],
                            size: 2.2,
                            born: now,
                            die: now + f64::from(rng.range(0.3, 0.65)),
                            additive: true,
                            streak_len: 0.0,
                        },
                    );
                }
            }
            ParticleEffect::Implosion {
                position,
                radius,
                count,
                life,
            } => {
                let rad = f32::from(radius).max(16.0);
                let dur = life.max(0.2);
                let n = usize::from(count.clamp(8, 128));
                for _ in 0..n {
                    let dir = rng.vec3(1.0).normalize_or(Vec3::X);
                    let origin = position + dir * rad;
                    let vel = (position - origin) / dur;
                    push_particle(
                        particles,
                        Particle {
                            origin,
                            vel,
                            accel: Vec3::ZERO,
                            color: [0.65, 0.9, 1.0, 0.95],
                            size: 2.0,
                            born: now,
                            die: now + f64::from(dur),
                            additive: true,
                            streak_len: 0.03,
                        },
                    );
                }
            }
            ParticleEffect::LargeFunnel { position, flags } => {
                let reverse = flags != 0;
                for _ in 0..48 {
                    let angle = rng.range(0.0, std::f32::consts::TAU);
                    let rad = rng.range(8.0, 64.0);
                    let origin = position + Vec3::new(angle.cos() * rad, angle.sin() * rad, rng.range(0.0, 64.0));
                    let dir = if reverse { origin - position } else { position - origin };
                    let vel = dir.normalize_or_zero() * 90.0;
                    push_particle(
                        particles,
                        Particle {
                            origin,
                            vel,
                            accel: Vec3::ZERO,
                            color: [0.25, 0.95, 0.4, 0.9],
                            size: 2.2,
                            born: now,
                            die: now + 0.8,
                            additive: true,
                            streak_len: 0.02,
                        },
                    );
                }
            }
            ParticleEffect::ParticleBurst {
                position,
                radius,
                color,
                duration,
            } => {
                let c = goldsrc_palette_rgb(color);
                let rad = f32::from(radius).max(8.0);
                let dur = f64::from(duration.max(0.2));
                for _ in 0..48 {
                    let dir = rng.vec3(1.0).normalize_or(Vec3::Z);
                    push_particle(
                        particles,
                        Particle {
                            origin: position + dir * rng.range(0.0, rad * 0.25),
                            vel: dir * (rad * 1.6),
                            accel: Vec3::new(0.0, 0.0, -120.0),
                            color: [c[0], c[1], c[2], 0.95],
                            size: 2.4,
                            born: now,
                            die: now + dur,
                            additive: false,
                            streak_len: 0.0,
                        },
                    );
                }
            }
            ParticleEffect::UserTracer {
                origin,
                velocity,
                life,
                color,
                ..
            } => {
                let c = goldsrc_palette_rgb(color);
                push_particle(
                    particles,
                    Particle {
                        origin,
                        vel: velocity,
                        accel: Vec3::ZERO,
                        color: [c[0].max(0.65), c[1].max(0.55), c[2], 0.95],
                        size: 1.5,
                        born: now,
                        die: now + f64::from(life.max(0.05)),
                        additive: true,
                        streak_len: 0.06,
                    },
                );
            }
            ParticleEffect::MultiGunshot {
                origin,
                direction,
                x_noise,
                y_noise,
                count,
            } => {
                let dir = direction.normalize_or(Vec3::X);
                let n = usize::from(count.clamp(1, 24));
                for _ in 0..n {
                    let spread = dir
                        + Vec3::new(
                            rng.range(-x_noise, x_noise),
                            rng.range(-y_noise, y_noise),
                            rng.range(-x_noise, x_noise),
                        );
                    push_particle(
                        particles,
                        Particle {
                            origin,
                            vel: spread.normalize_or_zero() * 900.0,
                            accel: Vec3::ZERO,
                            color: [1.0, 0.85, 0.45, 0.85],
                            size: 1.3,
                            born: now,
                            die: now + 0.14,
                            additive: true,
                            streak_len: 0.04,
                        },
                    );
                }
            }
            ParticleEffect::Line {
                start,
                end,
                life,
                color,
            } => {
                let col = [
                    f32::from(color[0]) / 255.0,
                    f32::from(color[1]) / 255.0,
                    f32::from(color[2]) / 255.0,
                    1.0,
                ];
                push_particle(
                    particles,
                    Particle {
                        origin: start,
                        vel: end - start,
                        accel: Vec3::ZERO,
                        color: col,
                        size: 1.2,
                        born: now,
                        die: now + f64::from(life.max(0.1)),
                        additive: true,
                        streak_len: 1.0,
                    },
                );
            }
            ParticleEffect::ShowLine { start, end } => {
                push_particle(
                    particles,
                    Particle {
                        origin: start,
                        vel: end - start,
                        accel: Vec3::ZERO,
                        color: [1.0, 0.35, 0.35, 0.9],
                        size: 1.2,
                        born: now,
                        die: now + 1.0,
                        additive: true,
                        streak_len: 1.0,
                    },
                );
            }
            ParticleEffect::Box {
                min,
                max,
                life,
                color,
            } => {
                let col = [
                    f32::from(color[0]) / 255.0,
                    f32::from(color[1]) / 255.0,
                    f32::from(color[2]) / 255.0,
                    1.0,
                ];
                let die = now + f64::from(life.max(0.1));
                let corners = [
                    Vec3::new(min.x, min.y, min.z),
                    Vec3::new(max.x, min.y, min.z),
                    Vec3::new(max.x, max.y, min.z),
                    Vec3::new(min.x, max.y, min.z),
                    Vec3::new(min.x, min.y, max.z),
                    Vec3::new(max.x, min.y, max.z),
                    Vec3::new(max.x, max.y, max.z),
                    Vec3::new(min.x, max.y, max.z),
                ];
                let edges = [
                    (0, 1), (1, 2), (2, 3), (3, 0),
                    (4, 5), (5, 6), (6, 7), (7, 4),
                    (0, 4), (1, 5), (2, 6), (3, 7),
                ];
                for (a, b) in edges {
                    push_particle(
                        particles,
                        Particle {
                            origin: corners[a],
                            vel: corners[b] - corners[a],
                            accel: Vec3::ZERO,
                            color: col,
                            size: 1.1,
                            born: now,
                            die,
                            additive: true,
                            streak_len: 1.0,
                        },
                    );
                }
            }
            ParticleEffect::SmokeTrail { position } => {
                push_particle(
                    particles,
                    Particle {
                        origin: position + rng.vec3(3.0),
                        vel: rng.vec3(10.0) + Vec3::new(0.0, 0.0, rng.range(16.0, 32.0)),
                        accel: Vec3::new(0.0, 0.0, 12.0),
                        color: [0.58, 0.58, 0.58, 0.45],
                        size: 4.5,
                        born: now,
                        die: now + f64::from(rng.range(0.4, 0.9)),
                        additive: false,
                        streak_len: 0.0,
                    },
                );
            }
        }
    }

    /// Spawn client-side weather particles around the camera from `ReceiveW` (`0` off, `1` rain, `2` snow).
    pub fn step_weather(&mut self, now: f64, dt: f32, weather: u8, camera_pos: Vec3) {
        if weather == 0 || dt <= 0.0 {
            return;
        }
        let rate = if weather == 1 { 180.0 } else { 90.0 };
        self.weather_accum += dt * rate;
        let count = (self.weather_accum.floor() as usize).min(48);
        self.weather_accum -= count as f32;
        let rng = &mut self.rng;
        let particles = &mut self.particles;
        for _ in 0..count {
            let offset = Vec3::new(
                rng.range(-384.0, 384.0),
                rng.range(-384.0, 384.0),
                rng.range(64.0, 256.0),
            );
            if weather == 1 {
                let vx = rng.range(-18.0, 18.0);
                let vy = rng.range(-18.0, 18.0);
                push_particle(
                    particles,
                    Particle {
                        origin: camera_pos + offset,
                        vel: Vec3::new(vx, vy, -650.0),
                        accel: Vec3::ZERO,
                        color: [0.68, 0.78, 0.9, 0.5],
                        size: 0.9,
                        born: now,
                        die: now + 0.65,
                        additive: false,
                        streak_len: 0.03,
                    },
                );
            } else {
                let vx = rng.range(-28.0, 28.0);
                let vy = rng.range(-28.0, 28.0);
                push_particle(
                    particles,
                    Particle {
                        origin: camera_pos + offset,
                        vel: Vec3::new(vx, vy, -75.0),
                        accel: Vec3::ZERO,
                        color: [0.95, 0.97, 1.0, 0.75],
                        size: 1.8,
                        born: now,
                        die: now + 2.5,
                        additive: false,
                        streak_len: 0.0,
                    },
                );
            }
        }
    }

    pub fn step(&mut self, now: f64, dt: f32) {
        let step_dt = dt.max(0.0);
        self.particles.retain_mut(|p| {
            if now >= p.die {
                return false;
            }
            if p.streak_len < 0.99 {
                p.vel += p.accel * step_dt;
                p.origin += p.vel * step_dt;
            }
            true
        });
    }

    pub fn emit_fx(&self, now: f64, beams: &mut Vec<FxBeam>, points: &mut Vec<FxPoint>) {
        for p in &self.particles {
            let total = (p.die - p.born).max(0.01);
            let frac = (1.0 - (now - p.born) / total).clamp(0.0, 1.0) as f32;
            let mut color = p.color;
            color[3] *= frac;
            if p.streak_len > 0.0 {
                let end = if p.streak_len >= 0.99 {
                    p.origin + p.vel
                } else {
                    p.origin + p.vel * p.streak_len
                };
                beams.push(FxBeam {
                    start: p.origin,
                    end,
                    width: p.size,
                    color,
                    additive: p.additive,
                });
            } else {
                points.push(FxPoint {
                    origin: p.origin,
                    size: p.size,
                    color,
                    additive: p.additive,
                });
            }
        }
    }
}

#[derive(Debug, Clone)]
enum BeamShape {
    Segment {
        start: Vec3,
        end: Vec3,
        start_ent: Option<u16>,
        end_ent: Option<u16>,
    },
    Ring {
        center: Vec3,
        axis_radius: Vec3,
        mode: RingMode,
    },
    FollowTrail {
        entity: u16,
        points: Vec<Vec3>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RingMode {
    Torus,
    Disk,
    Cylinder,
    EntityRing,
}

#[derive(Debug, Clone)]
struct ActiveClientBeam {
    shape: BeamShape,
    width: f32,
    noise: f32,
    color: [f32; 4],
    born: f64,
    die: f64,
    additive: bool,
    seed: u32,
}

/// Manages client temporary and entity beams (`TE_BEAM*`, `TE_LIGHTNING`, `TE_KILLBEAM`).
#[derive(Debug, Clone, Default)]
pub struct BeamSystem {
    beams: Vec<ActiveClientBeam>,
    seed_counter: u32,
}

fn beam_color_from_params(b: &BeamParams) -> [f32; 4] {
    [
        f32::from(b.color[0]) / 255.0,
        f32::from(b.color[1]) / 255.0,
        f32::from(b.color[2]) / 255.0,
        (f32::from(b.brightness) / 255.0).max(0.08),
    ]
}

impl BeamSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.beams.clear();
    }

    pub fn len(&self) -> usize {
        self.beams.len()
    }

    pub fn is_empty(&self) -> bool {
        self.beams.is_empty()
    }

    pub fn kill_entity_beams(&mut self, entity: u16) {
        self.beams.retain(|b| match &b.shape {
            BeamShape::Segment {
                start_ent, end_ent, ..
            } => *start_ent != Some(entity) && *end_ent != Some(entity),
            BeamShape::FollowTrail { entity: e, .. } => *e != entity,
            BeamShape::Ring { .. } => true,
        });
    }

    pub fn spawn_from_temp_effect(
        &mut self,
        now: f64,
        effect: &TempEffect,
        entity_pos: &dyn Fn(u16) -> Option<Vec3>,
    ) {
        self.seed_counter = self.seed_counter.wrapping_add(1);
        let seed = self.seed_counter;
        let life_sec = |tenths: u8| f64::from(tenths).max(1.0) * 0.1;

        match effect {
            TempEffect::KillBeam { entity } => self.kill_entity_beams(*entity),
            TempEffect::BeamPoints { start, end, beam } => {
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Segment {
                        start: *start,
                        end: *end,
                        start_ent: None,
                        end_ent: None,
                    },
                    width: (f32::from(beam.width) * 0.15).max(0.8),
                    noise: f32::from(beam.noise) * 0.25,
                    color: beam_color_from_params(beam),
                    born: now,
                    die: now + life_sec(beam.life),
                    additive: true,
                    seed,
                });
            }
            TempEffect::BeamEntPoint {
                start_entity,
                end,
                beam,
            } => {
                let start = entity_pos(*start_entity).unwrap_or(*end);
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Segment {
                        start,
                        end: *end,
                        start_ent: Some(*start_entity),
                        end_ent: None,
                    },
                    width: (f32::from(beam.width) * 0.15).max(0.8),
                    noise: f32::from(beam.noise) * 0.25,
                    color: beam_color_from_params(beam),
                    born: now,
                    die: now + life_sec(beam.life),
                    additive: true,
                    seed,
                });
            }
            TempEffect::BeamEnts {
                start_entity,
                end_entity,
                beam,
            } => {
                let start = entity_pos(*start_entity).unwrap_or(Vec3::ZERO);
                let end = entity_pos(*end_entity).unwrap_or(start);
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Segment {
                        start,
                        end,
                        start_ent: Some(*start_entity),
                        end_ent: Some(*end_entity),
                    },
                    width: (f32::from(beam.width) * 0.15).max(0.8),
                    noise: f32::from(beam.noise) * 0.25,
                    color: beam_color_from_params(beam),
                    born: now,
                    die: now + life_sec(beam.life),
                    additive: true,
                    seed,
                });
            }
            TempEffect::BeamFollow {
                entity,
                life,
                width,
                color,
                brightness,
                ..
            } => {
                let pts = entity_pos(*entity).into_iter().collect();
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::FollowTrail {
                        entity: *entity,
                        points: pts,
                    },
                    width: (f32::from(*width) * 0.15).max(0.8),
                    noise: 0.0,
                    color: [
                        f32::from(color[0]) / 255.0,
                        f32::from(color[1]) / 255.0,
                        f32::from(color[2]) / 255.0,
                        (f32::from(*brightness) / 255.0).max(0.1),
                    ],
                    born: now,
                    die: now + life_sec(*life),
                    additive: true,
                    seed,
                });
            }
            TempEffect::BeamTorus {
                center,
                axis_radius,
                beam,
            }
            | TempEffect::BeamDisk {
                center,
                axis_radius,
                beam,
            }
            | TempEffect::BeamCylinder {
                center,
                axis_radius,
                beam,
            } => {
                let mode = match effect {
                    TempEffect::BeamTorus { .. } => RingMode::Torus,
                    TempEffect::BeamDisk { .. } => RingMode::Disk,
                    _ => RingMode::Cylinder,
                };
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Ring {
                        center: *center,
                        axis_radius: *axis_radius,
                        mode,
                    },
                    width: (f32::from(beam.width) * 0.15).max(1.0),
                    noise: 0.0,
                    color: beam_color_from_params(beam),
                    born: now,
                    die: now + life_sec(beam.life),
                    additive: true,
                    seed,
                });
            }
            TempEffect::BeamRing {
                start_entity,
                end_entity,
                beam,
            } => {
                let a = entity_pos(*start_entity).unwrap_or(Vec3::ZERO);
                let b = entity_pos(*end_entity).unwrap_or(a + Vec3::X * 32.0);
                let mid = (a + b) * 0.5;
                let rad = ((b - a).length() * 0.5).max(8.0);
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Ring {
                        center: mid,
                        axis_radius: Vec3::new(0.0, 0.0, rad),
                        mode: RingMode::EntityRing,
                    },
                    width: (f32::from(beam.width) * 0.15).max(1.0),
                    noise: f32::from(beam.noise) * 0.2,
                    color: beam_color_from_params(beam),
                    born: now,
                    die: now + life_sec(beam.life),
                    additive: true,
                    seed,
                });
            }
            TempEffect::Lightning {
                start,
                end,
                life,
                width,
                amplitude,
                ..
            } => {
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Segment {
                        start: *start,
                        end: *end,
                        start_ent: None,
                        end_ent: None,
                    },
                    width: (f32::from(*width) * 0.2).max(1.0),
                    noise: f32::from(*amplitude).max(2.0),
                    color: [0.65, 0.85, 1.0, 0.95],
                    born: now,
                    die: now + life_sec(*life),
                    additive: true,
                    seed,
                });
            }
            TempEffect::BeamSprite { start, end, .. } => {
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Segment {
                        start: *start,
                        end: *end,
                        start_ent: None,
                        end_ent: None,
                    },
                    width: 2.0,
                    noise: 0.0,
                    color: [1.0, 0.95, 0.85, 0.9],
                    born: now,
                    die: now + 0.25,
                    additive: true,
                    seed,
                });
            }
            TempEffect::Tracer { start, end } => {
                self.beams.push(ActiveClientBeam {
                    shape: BeamShape::Segment {
                        start: *start,
                        end: *end,
                        start_ent: None,
                        end_ent: None,
                    },
                    width: 0.8,
                    noise: 0.0,
                    color: [1.0, 0.88, 0.45, 0.8],
                    born: now,
                    die: now + 0.08,
                    additive: true,
                    seed,
                });
            }
            _ => {}
        }
    }

    pub fn step(&mut self, now: f64, entity_pos: &dyn Fn(u16) -> Option<Vec3>) {
        self.beams.retain_mut(|b| {
            if now >= b.die {
                return false;
            }
            match &mut b.shape {
                BeamShape::Segment {
                    start,
                    end,
                    start_ent,
                    end_ent,
                } => {
                    if let Some(se) = *start_ent
                        && let Some(p) = entity_pos(se)
                    {
                        *start = p;
                    }
                    if let Some(ee) = *end_ent
                        && let Some(p) = entity_pos(ee)
                    {
                        *end = p;
                    }
                }
                BeamShape::FollowTrail { entity, points } => {
                    if let Some(p) = entity_pos(*entity)
                        && points.last().is_none_or(|last| last.distance_squared(p) > 4.0)
                    {
                        if points.len() >= 32 {
                            points.remove(0);
                        }
                        points.push(p);
                    }
                }
                BeamShape::Ring { .. } => {}
            }
            true
        });
    }

    pub fn emit_fx(&self, now: f64, out: &mut Vec<FxBeam>) {
        for b in &self.beams {
            let total = (b.die - b.born).max(0.01);
            let frac = (1.0 - (now - b.born) / total).clamp(0.0, 1.0) as f32;
            let mut color = b.color;
            color[3] *= frac;
            match &b.shape {
                BeamShape::Segment { start, end, .. } => {
                    if b.noise <= 0.1 {
                        out.push(FxBeam {
                            start: *start,
                            end: *end,
                            width: b.width,
                            color,
                            additive: b.additive,
                        });
                    } else {
                        let segs = 10usize;
                        let mut prev = *start;
                        let dir = (*end - *start).normalize_or(Vec3::X);
                        let ortho_a = dir.cross(Vec3::Z).normalize_or(Vec3::Y);
                        let ortho_b = dir.cross(ortho_a).normalize_or(Vec3::Z);
                        for i in 1..=segs {
                            let t = i as f32 / segs as f32;
                            let base = start.lerp(*end, t);
                            let next = if i == segs {
                                *end
                            } else {
                                let phase = (now as f32) * 18.0 + (b.seed as f32) + (i as f32) * 2.3;
                                let env = (t * std::f32::consts::PI).sin();
                                base + (ortho_a * phase.sin() + ortho_b * phase.cos()) * (b.noise * env)
                            };
                            out.push(FxBeam {
                                start: prev,
                                end: next,
                                width: b.width,
                                color,
                                additive: b.additive,
                            });
                            prev = next;
                        }
                    }
                }
                BeamShape::Ring {
                    center,
                    axis_radius,
                    mode,
                } => {
                    let progress = ((now - b.born) / total).clamp(0.0, 1.0) as f32;
                    let max_r = axis_radius.length().max(16.0);
                    let cur_r = match mode {
                        RingMode::EntityRing => max_r,
                        _ => 4.0 + (max_r - 4.0) * progress,
                    };
                    let segs = 24usize;
                    for i in 0..segs {
                        let a0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
                        let a1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
                        let p0 = *center + Vec3::new(a0.cos() * cur_r, a0.sin() * cur_r, 0.0);
                        let p1 = *center + Vec3::new(a1.cos() * cur_r, a1.sin() * cur_r, 0.0);
                        out.push(FxBeam {
                            start: p0,
                            end: p1,
                            width: b.width,
                            color,
                            additive: b.additive,
                        });
                    }
                }
                BeamShape::FollowTrail { points, .. } => {
                    for w in points.windows(2) {
                        out.push(FxBeam {
                            start: w[0],
                            end: w[1],
                            width: b.width,
                            color,
                            additive: b.additive,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particle_system_spawns_decays_and_weather_works() {
        let mut ps = ParticleSystem::new();
        ps.spawn_effect(
            1.0,
            &ParticleEffect::Gunshot {
                position: Vec3::ZERO,
                count: 10,
            },
        );
        ps.spawn_effect(
            1.0,
            &ParticleEffect::Implosion {
                position: Vec3::new(50.0, 0.0, 0.0),
                radius: 40,
                count: 12,
                life: 0.5,
            },
        );
        ps.spawn_effect(
            1.0,
            &ParticleEffect::SmokeTrail {
                position: Vec3::ZERO,
            },
        );
        ps.step_weather(1.0, 0.1, 1, Vec3::ZERO);
        assert!(ps.len() >= 22);

        let mut beams = Vec::new();
        let mut points = Vec::new();
        ps.emit_fx(1.1, &mut beams, &mut points);
        assert!(!beams.is_empty() && !points.is_empty());

        ps.step(5.0, 0.1);
        assert!(ps.is_empty());
    }

    #[test]
    fn beam_system_subdivides_and_kills() {
        let mut bs = BeamSystem::new();
        bs.spawn_from_temp_effect(
            1.0,
            &TempEffect::Lightning {
                start: Vec3::ZERO,
                end: Vec3::new(100.0, 0.0, 0.0),
                life: 5,
                width: 10,
                amplitude: 8,
                sprite: 1,
            },
            &|_| None,
        );
        let mut out = Vec::new();
        bs.emit_fx(1.1, &mut out);
        assert_eq!(out.len(), 10);

        bs.spawn_from_temp_effect(
            1.0,
            &TempEffect::BeamFollow {
                entity: 7,
                sprite: 1,
                life: 10,
                width: 5,
                color: [255, 100, 50],
                brightness: 255,
            },
            &|_| Some(Vec3::new(10.0, 0.0, 0.0)),
        );
        assert_eq!(bs.len(), 2);
        bs.kill_entity_beams(7);
        assert_eq!(bs.len(), 1);
    }
}
