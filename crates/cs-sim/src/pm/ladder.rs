//! Ladders: `PM_Ladder`, `PM_LadderMove`. stub: Task 6.

use super::types::PmCtx;

/// `PM_Ladder`: the index of the ladder moveent the player is in. stub: Task 6 (always `None`).
pub(crate) fn pm_ladder(_c: &mut PmCtx<'_>) -> Option<usize> {
    None
}

/// `PM_LadderMove(pLadder)`. stub: Task 6.
pub(crate) fn pm_ladder_move(_c: &mut PmCtx<'_>, _ladder: usize) {}
