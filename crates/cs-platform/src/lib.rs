//! Platform seams: time source abstraction shared by native and wasm32 builds.

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic time source, in seconds.
pub trait Clock {
    fn now(&self) -> f64;
}

/// Clock driven by hand; starts at `0.0` and moves only through [`ManualClock::advance`].
#[derive(Debug, Default)]
pub struct ManualClock {
    /// Current time as `f64` bits.
    bits: AtomicU64,
}

impl ManualClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the clock forward by `dt` seconds.
    pub fn advance(&self, dt: f64) {
        let mut current = self.bits.load(Ordering::Relaxed);
        loop {
            let next = (f64::from_bits(current) + dt).to_bits();
            match self.bits.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Clock for ManualClock {
    fn now(&self) -> f64 {
        f64::from_bits(self.bits.load(Ordering::Relaxed))
    }
}

/// Wall clock measuring seconds elapsed since its creation.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct NativeClock {
    start: std::time::Instant,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeClock {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for NativeClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Clock for NativeClock {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_advances() {
        let c = ManualClock::new();
        c.advance(0.5);
        c.advance(0.25);
        assert_eq!(c.now(), 0.75);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_clock_is_monotonic() {
        let c = NativeClock::new();
        let first = c.now();
        let second = c.now();
        assert!(second >= first);
    }
}
