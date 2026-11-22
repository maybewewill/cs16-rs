//! Footstep sounds: `PM_UpdateStepSound`, `PM_PlayStepSound`, `PM_CatagorizeTextureType`,
//! `PM_MapTextureTypeStepType`. stub: Task 7.

use super::consts::STEP_CONCRETE;
use super::types::PmCtx;

/// `PM_UpdateStepSound`. stub: Task 7.
pub(crate) fn pm_update_step_sound(_c: &mut PmCtx<'_>) {}

/// `PM_PlayStepSound(step, fvol)`. stub: Task 7.
pub(crate) fn pm_play_step_sound(_c: &mut PmCtx<'_>, _step: i32, _fvol: f32) {}

/// `PM_CatagorizeTextureType` (sic). stub: Task 7.
pub(crate) fn pm_catagorize_texture_type(_c: &mut PmCtx<'_>) {}

/// `PM_MapTextureTypeStepType(chTextureType)`. stub: Task 7 (always concrete).
pub(crate) fn pm_map_texture_type_step_type(_ch: u8) -> i32 {
    STEP_CONCRETE
}
