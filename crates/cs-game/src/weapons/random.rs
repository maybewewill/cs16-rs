//! The shared random generator of the weapons: `UTIL_SharedRandomLong` and
//! `UTIL_SharedRandomFloat` with `U_Random`/`U_Srand` and the 256-entry `seed_table`.
//!
//! The client predicts the spread and the recoil with the same functions and the same seed, so
//! every quirk of the C code is kept (ReGameDLL_CS `regamedll/dlls/util.cpp`, read 2026-11-22).
//!
//! The C code keeps the state in the global `glSeed`, but every public function calls `U_Srand`
//! first and draws within the same call, so no state survives a call: the state here is a local
//! of the call ([`URandom`]).

/// `seed_table[256]` (`util.cpp`, read 2026-11-22), verbatim.
pub(crate) const SEED_TABLE: [u32; 256] = [
    28985, 27138, 26457, 9451, 17764, 10909, 28790, 8716, 6361, 4853, 17798, 21977, 19643, 20662, 10834, 20103,
    27067, 28634, 18623, 25849, 8576, 26234, 23887, 18228, 32587, 4836, 3306, 1811, 3035, 24559, 18399, 315,
    26766, 907, 24102, 12370, 9674, 2972, 10472, 16492, 22683, 11529, 27968, 30406, 13213, 2319, 23620, 16823,
    10013, 23772, 21567, 1251, 19579, 20313, 18241, 30130, 8402, 20807, 27354, 7169, 21211, 17293, 5410, 19223,
    10255, 22480, 27388, 9946, 15628, 24389, 17308, 2370, 9530, 31683, 25927, 23567, 11694, 26397, 32602, 15031,
    18255, 17582, 1422, 28835, 23607, 12597, 20602, 10138, 5212, 1252, 10074, 23166, 19823, 31667, 5902, 24630,
    18948, 14330, 14950, 8939, 23540, 21311, 22428, 22391, 3583, 29004, 30498, 18714, 4278, 2437, 22430, 3439,
    28313, 23161, 25396, 13471, 19324, 15287, 2563, 18901, 13103, 16867, 9714, 14322, 15197, 26889, 19372, 26241,
    31925, 14640, 11497, 8941, 10056, 6451, 28656, 10737, 13874, 17356, 8281, 25937, 1661, 4850, 7448, 12744,
    21826, 5477, 10167, 16705, 26897, 8839, 30947, 27978, 27283, 24685, 32298, 3525, 12398, 28726, 9475, 10208,
    617, 13467, 22287, 2376, 6097, 26312, 2974, 9114, 21787, 28010, 4725, 15387, 3274, 10762, 31695, 17320,
    18324, 12441, 16801, 27376, 22464, 7500, 5666, 18144, 15314, 31914, 31627, 6495, 5226, 31203, 2331, 4668,
    12650, 18275, 351, 7268, 31319, 30119, 7600, 2905, 13826, 11343, 13053, 15583, 30055, 31093, 5067, 761,
    9685, 11070, 21369, 27155, 3663, 26542, 20169, 12161, 15411, 30401, 7580, 31784, 8985, 29367, 20989, 14203,
    29694, 21167, 10337, 1706, 28578, 887, 3373, 19477, 14382, 675, 7033, 15111, 26138, 12252, 30996, 21409,
    25678, 18555, 13256, 23316, 22407, 16727, 991, 9236, 5373, 29402, 6117, 15241, 27715, 19291, 19888, 19847,
];

/// The state of `glSeed` for one call: `U_Srand` then `U_Random`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct URandom {
    seed: u32,
}

impl URandom {
    /// `U_Srand(seed)`: `glSeed = seed_table[seed & 0xFF]`. Only the low byte of the seed counts.
    pub(crate) fn new(seed: u32) -> Self {
        Self { seed: SEED_TABLE[(seed & 0xFF) as usize] }
    }

    /// `U_Random()`: `glSeed *= 69069; glSeed += seed_table[glSeed & 0xFF] + 1;` (all `unsigned int`,
    /// wrapping), returns `glSeed & 0xFFFFFFF` (28 bits).
    pub(crate) fn random(&mut self) -> u32 {
        self.seed = self.seed.wrapping_mul(69069);
        self.seed = self
            .seed
            .wrapping_add(SEED_TABLE[(self.seed & 0xFF) as usize].wrapping_add(1));
        self.seed & 0x0FFF_FFFF
    }
}

/// `UTIL_SharedRandomLong` (`util.cpp`, read 2026-11-22).
///
/// Quirks kept: `range = high - low + 1` is an `unsigned int`; the seed is `high + low + seed`
/// in `unsigned` arithmetic (wrapping); when `range == 1` the result is `low` without drawing;
/// the draw is `U_Random() % range` with the 28-bit value taken as `int` then converted to
/// `unsigned` by the modulo.
///
/// Deviation: for `high == low - 1` the range is 0 and the C code divides by zero; here the result
/// is `low` (no panic on game data).
pub fn shared_random_long(seed: u32, low: i32, high: i32) -> i32 {
    let range = (high.wrapping_sub(low).wrapping_add(1)) as u32;
    let mut r = URandom::new((high as u32).wrapping_add(low as u32).wrapping_add(seed));
    if range != 1 {
        if range == 0 {
            return low;
        }
        let rnum = r.random();
        let offset = rnum % range;
        return low.wrapping_add(offset as i32);
    }
    low
}

/// `UTIL_SharedRandomFloat` (`util.cpp`, read 2026-11-22).
///
/// Quirks kept: `range` is an `unsigned int` made by truncating `high - low` (so a span below 1
/// gives 0 and the function returns `low`; a negative span wraps, as the x86 conversion of the
/// original does: assumption, the C standard leaves it undefined); the seed adds the raw bits of
/// `low` and `high`; two draws are thrown away before the third is used; the offset is the low
/// 16 bits of the draw over 65536, times the integer range. Float arithmetic is `f32`
/// (assumption: the original build uses SSE, not x87 extended precision).
pub fn shared_random_float(seed: u32, low: f32, high: f32) -> f32 {
    // `unsigned int range = high - low;`: float to integer truncates toward zero, the low 32 bits
    // are kept. NaN and out-of-range values become 0 (`as` saturates): undefined in C.
    let range = ((high - low) as i64) as u32;
    let mut r = URandom::new(seed.wrapping_add(low.to_bits()).wrapping_add(high.to_bits()));
    r.random();
    r.random();
    if range != 0 {
        let tensixrand = r.random() & 0xFFFF;
        let offset = tensixrand as f32 / 65536.0_f32;
        return low + offset * range as f32;
    }
    low
}
