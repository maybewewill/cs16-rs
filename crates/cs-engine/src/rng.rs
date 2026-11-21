//! The engine's random numbers: the replacement of the original's `RandomLong`.
//!
//! The original's generator is a time-seeded global, so its sequence cannot be reproduced. This
//! one is a xorshift32 that starts from the same constant whenever a map's world is created
//! (it lives in `WorldLinks`, the per-map state every `EngineApi` carries), so what the engine
//! does with random numbers on a map is repeatable and tests can count on it.

/// Start of every map's sequence.
const SEED: u32 = 0x2545_F491;

/// A xorshift32 generator.
#[derive(Debug, Clone)]
pub(crate) struct EngineRng(u32);

impl EngineRng {
    pub(crate) fn new() -> Self {
        Self(SEED)
    }

    /// `RandomLong(lo, hi)`: a number in `lo..=hi` (`lo` when `hi <= lo`).
    pub(crate) fn random_long(&mut self, lo: i32, hi: i32) -> i32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        if hi <= lo {
            return lo;
        }
        let span = i64::from(hi) - i64::from(lo) + 1;
        (i64::from(lo) + i64::from(x >> 8) % span) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::EngineRng;

    #[test]
    fn same_seed_same_sequence_and_in_range() {
        let (mut a, mut b) = (EngineRng::new(), EngineRng::new());
        let mut seen = [false; 2];
        for _ in 0..64 {
            let v = a.random_long(0, 1);
            assert_eq!(v, b.random_long(0, 1));
            seen[v as usize] = true;
        }
        assert_eq!(seen, [true, true]);
        assert_eq!(a.random_long(5, 5), 5);
    }
}
