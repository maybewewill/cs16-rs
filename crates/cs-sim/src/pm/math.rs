//! The functions of `pm_math.cpp` that `pm_shared` needs. Ported from ReGameDLL_CS
//! `regamedll/pm_shared/pm_math.cpp` and `pm_math.h`, read 2026-11-22.
//!
//! Float semantics: `real_t` is `float` in the vanilla build, so every value is an `f32`. Where the
//! C source mixes in a `double` literal the expression is computed in `f64` and cast back where C
//! stores it (named at the spot).

use glam::Vec3;

/// `PITCH` / `YAW` / `ROLL` indices of an angle vector.
pub const PITCH: usize = 0;
pub const YAW: usize = 1;
pub const ROLL: usize = 2;

/// `IS_NAN(x)`: the exponent bits are all ones. QUIRK: infinities match too, so the callers reset
/// them to zero as well.
pub fn is_nan(x: f32) -> bool {
    const NANMASK: i32 = 255 << 23;
    (x.to_bits() as i32 & NANMASK) == NANMASK
}

/// `AngleVectors`: forward, right and up vectors of `angles` (pitch, yaw, roll in degrees).
/// `angle = real_t(angles[i] * (M_PI * 2 / 360))`: the product is a `double`, then cast to `float`;
/// `sin`/`cos` are taken on that `float`.
#[allow(clippy::neg_multiply)] // `-1 * x` is kept as in the source
pub fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    const TO_RAD: f64 = std::f64::consts::PI * 2.0 / 360.0;
    let rad = |a: f32| (f64::from(a) * TO_RAD) as f32;

    let angle = rad(angles[YAW]);
    let sy = angle.sin();
    let cy = angle.cos();
    let angle = rad(angles[PITCH]);
    let sp = angle.sin();
    let cp = angle.cos();
    let angle = rad(angles[ROLL]);
    let sr = angle.sin();
    let cr = angle.cos();

    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(
        -1.0 * sr * sp * cy + -1.0 * cr * -sy,
        -1.0 * sr * sp * sy + -1.0 * cr * cy,
        -1.0 * sr * cp,
    );
    let up = Vec3::new(cr * sp * cy + -sr * -sy, cr * sp * sy + -sr * cy, cr * cp);
    (forward, right, up)
}

/// `DotProduct` / `_DotProduct`: `v1[0]*v2[0] + v1[1]*v2[1] + v1[2]*v2[2]` in `float`.
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `CrossProduct`.
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )
}

/// `Length`: accumulates `v[i] * v[i]` in a `real_t` starting at zero, then the square root.
#[allow(clippy::needless_range_loop)]
pub fn length(v: Vec3) -> f32 {
    let mut len = 0.0f32;
    for i in 0..3 {
        len += v[i] * v[i];
    }
    len.sqrt()
}

/// `VectorNormalize`: scales `v` in place by `ilength = 1.0 / length` (a `double` quotient stored in
/// a `real_t`) and returns the length. A zero-length vector is left untouched.
pub fn vector_normalize(v: &mut Vec3) -> f32 {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len != 0.0 {
        let ilength = (1.0f64 / f64::from(len)) as f32;
        v[0] *= ilength;
        v[1] *= ilength;
        v[2] *= ilength;
    }
    len
}

/// `VectorMA(veca, scale, vecb, vecc)`: `veca + scale * vecb`.
pub fn vector_ma(a: Vec3, scale: f32, b: Vec3) -> Vec3 {
    Vec3::new(a[0] + scale * b[0], a[1] + scale * b[1], a[2] + scale * b[2])
}

/// `VectorScale(in, scale, out)`: `scale * in`.
pub fn vector_scale(v: Vec3, scale: f32) -> Vec3 {
    Vec3::new(scale * v[0], scale * v[1], scale * v[2])
}

/// `VectorAdd`.
pub fn vector_add(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a[0] + b[0], a[1] + b[1], a[2] + b[2])
}

/// `VectorSubtract`.
pub fn vector_subtract(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a[0] - b[0], a[1] - b[1], a[2] - b[2])
}

/// `VectorCompare`: exact component equality.
pub fn vector_compare(a: Vec3, b: Vec3) -> bool {
    a[0] == b[0] && a[1] == b[1] && a[2] == b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_vectors_of_zero_are_the_axes() {
        let (f, r, u) = angle_vectors(Vec3::ZERO);
        assert_eq!(f, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(r, Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(u, Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn angle_vectors_yaw_90_looks_along_y() {
        let (f, r, _) = angle_vectors(Vec3::new(0.0, 90.0, 0.0));
        assert!((f - Vec3::new(0.0, 1.0, 0.0)).abs().max_element() < 1e-6);
        assert!((r - Vec3::new(1.0, 0.0, 0.0)).abs().max_element() < 1e-6);
    }

    #[test]
    fn nan_mask_matches_nan_and_infinity_only() {
        assert!(is_nan(f32::NAN));
        assert!(is_nan(f32::INFINITY));
        assert!(is_nan(f32::NEG_INFINITY));
        assert!(!is_nan(1e30));
        assert!(!is_nan(0.0));
    }

    #[test]
    fn normalize_returns_length_and_leaves_zero_alone() {
        let mut v = Vec3::new(3.0, 4.0, 0.0);
        assert_eq!(vector_normalize(&mut v), 5.0);
        assert!((v.length() - 1.0).abs() < 1e-6);
        let mut z = Vec3::ZERO;
        assert_eq!(vector_normalize(&mut z), 0.0);
        assert_eq!(z, Vec3::ZERO);
    }
}
