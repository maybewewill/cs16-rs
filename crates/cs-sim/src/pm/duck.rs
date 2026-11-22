//! Ducking: `PM_Duck`, `PM_UnDuck`, `PM_FixPlayerCrouchStuck`, `PM_SplineFraction`.
//! stub: Task 5.

use super::types::PmCtx;

/// `PM_Duck`. stub: Task 5.
pub(crate) fn pm_duck(_c: &mut PmCtx<'_>) {}

/// `PM_UnDuck`. stub: Task 5.
#[allow(dead_code)] // called by the Task 5 body of `pm_duck`
pub(crate) fn pm_unduck(_c: &mut PmCtx<'_>) {}

/// `PM_FixPlayerCrouchStuck(direction)`. stub: Task 5.
#[allow(dead_code)] // called by the Task 5 body of `pm_duck`
pub(crate) fn pm_fix_player_crouch_stuck(_c: &mut PmCtx<'_>, _direction: i32) {}

/// `PM_SplineFraction(value, scale)`. stub: Task 5 (returns 0).
#[allow(dead_code)] // called by the Task 5 body of `pm_duck`
pub(crate) fn pm_spline_fraction(_value: f32, _scale: f32) -> f32 {
    0.0
}
