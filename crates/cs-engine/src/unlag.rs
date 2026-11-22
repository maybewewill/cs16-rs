//! Lag compensation, the original's `sv_unlag`: while the game runs a client's user command the
//! other players are moved to where that client saw them, and moved back afterwards.
//!
//! Ported from ReHLDS `rehlds/engine/sv_user.cpp` (`SV_IsLagCompensationActive`,
//! `SV_GetUnlagTargetTime`, `SV_FindUnlagFrames`, `SV_FlagNonInterpolatedPlayers`,
//! `SV_CalcUnlagFraction`, `SV_UnlagCheckTeleport`, `SV_SaveTruePositions`,
//! `SV_ApplyAdjustedPositions`, `SV_SetupMove`, `SV_RestoreMove`, `SV_GetMovedPosition`) and
//! `rehlds/engine/sv_main.cpp` (`SV_CheckUserInfo` for `cl_lw` and `cl_lc`), read 2026-11-21.
//!
//! The history comes from the client's own frames, as in the original: the players as the last
//! updates sent to this client showed them, with the real time each update was sent (see
//! `frame.rs`). The latency is the average round trip of the acknowledged updates
//! (`SV_CalcClientTime`, `sv_unlagsamples` of them).
//!
//! Differences, kept on purpose:
//! - `pfnAllowLagCompensation` of the game does not exist here; the game always allows it.
//! - The target time is computed in `f64` (the original's `REHLDS_FIXES` branch); the latency
//!   and the interpolation time are `f32` values as in the original.
//! - A moved player is put back when the game left it where the rewind put it; the original
//!   checks the same (`initial_correction_org`) but only when the shooter is still active, this
//!   always restores, so a shooter dropped by its own command leaves nobody displaced.
//! - `next_messageinterval` is `1 / cl_updaterate` clamped to the server's update rates
//!   (`update_interval`), 0.05 without `cl_updaterate`.
//! - The `health` of a player in a frame is the edict's, taken when the update was built.
//! - When the frame a client acknowledges is no longer among the 64 kept, its latency stays at
//!   the last value measured (see `note_accepted`); HLDS computes a wrong small one from the
//!   newer frame in that slot.
//! - HLDS skips `SV_SetupMove` and `SV_RestoreMove` for fake clients (`SV_RunCmd`,
//!   `sv_user.cpp`, read 2026-11-23): `lag_compensation_active` is false for them, and
//!   `run_player_cmd` does not call `setup_move` or `restore_move` for them at all.

use crate::world::consts::EF_NOINTERP;
use std::collections::BTreeSet;

use glam::Vec3;

use crate::client::ClientState;
use crate::edict::EdictId;
use crate::frame::{ClientFrame, UPDATE_BACKUP, update_interval};
use crate::game::GameDll;
use crate::server::Server;

/// Longest time, in seconds, the latency counts for in the target time.
const MAX_LATENCY: f32 = 1.5;
/// Longest interpolation time, in seconds, taken from a command's `lerp_msec`.
const MAX_LERP: f32 = 0.1;
/// Frames older than the target time by more than this are too old to rewind from, in seconds.
const MAX_FRAME_AGE: f64 = 1.0;
/// A distance on any axis above this between two updates of a player is a teleport.
const TELEPORT_DISTANCE: f32 = 128.0;

/// A player the rewind moved, with what it takes to put it back.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MovedPlayer {
    pub(crate) id: EdictId,
    /// Where the player is for real (`oldorg`), and its absolute box there.
    pub(crate) true_origin: Vec3,
    pub(crate) true_absmin: Vec3,
    pub(crate) true_absmax: Vec3,
    /// Where the rewind put it (`initial_correction_org`).
    pub(crate) rewound_origin: Vec3,
}

/// `SV_UnlagCheckTeleport`: whether two origins are more than 128 units apart on any axis.
pub(crate) fn unlag_check_teleport(a: Vec3, b: Vec3) -> bool {
    let d = (a - b).abs();
    d.x > TELEPORT_DISTANCE || d.y > TELEPORT_DISTANCE || d.z > TELEPORT_DISTANCE
}

/// `SV_CalcUnlagFraction`: where `target` lies between two sent times, 0 at the older and 1 at
/// the newer; 0 when they are the same, and clamped to `0..=1`.
pub(crate) fn calc_unlag_fraction(older_sent: f64, newer_sent: f64, target: f64) -> f32 {
    let diff = (newer_sent - older_sent) as f32;
    if diff == 0.0 {
        return 0.0;
    }
    let frac = ((target - older_sent) / f64::from(diff)) as f32;
    frac.clamp(0.0, 1.0)
}

/// C's `atoi` on a userinfo value: the leading integer, 0 for a missing or non-numeric one.
pub(crate) fn info_atoi(value: Option<&str>) -> i64 {
    let s = value.unwrap_or("").trim_start();
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            n.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    if negative { -n } else { n }
}

/// `cl->lw` of ReHLDS (`SV_ExtractFromUserinfo`): `cl_lw` read as `atoi(..) != 0`, false when
/// the key is missing or empty. The one place `cl_lw` is read.
pub(crate) fn userinfo_lw(info: &cs_proto48::info::InfoString) -> bool {
    info_atoi(info.get("cl_lw")) != 0
}

/// The frames a rewind interpolates between, and the players it must not move.
struct UnlagFrames {
    older: u32,
    /// `None` when `older` is the latest frame sent.
    newer: Option<u32>,
    /// Players that died, teleported or were flagged `EF_NOINTERP` in the frames walked.
    non_interpolated: BTreeSet<u16>,
}

impl<G: GameDll> Server<G> {
    /// A cvar as the original's `cvar_t::value`: 0 when it is missing or not a finite number.
    fn unlag_cvar(&self, name: &str) -> f32 {
        let value = self.cvars.get_f32(name);
        if value.is_finite() { value } else { 0.0 }
    }

    /// `SV_IsLagCompensationActive`: `sv_unlag` is on, there is more than one player slot and
    /// the client is active with both `cl_lw` and `cl_lc` set in its userinfo.
    #[doc(hidden)]
    pub fn lag_compensation_active(&self, slot: u8) -> bool {
        let index = usize::from(slot);
        if index >= self.clients.len() {
            return false;
        }
        let info = &self.views[index].userinfo;
        self.unlag_cvar("sv_unlag") != 0.0
            && self.cfg.max_clients > 1
            && self.clients[index].state == ClientState::Active
            && !self.clients[index].fake
            && userinfo_lw(info)
            && info_atoi(info.get("cl_lc")) != 0
    }

    /// `SV_GetUnlagTargetTime`: the real time the other players are rewound to, the client's
    /// latency (at most 1.5 s and `sv_maxunlag`) and interpolation delay before now, plus
    /// `sv_unlagpush`, never later than now. A negative `sv_maxunlag` is reset to 0.
    fn unlag_target_time(&mut self, slot: u8) -> f64 {
        let index = usize::from(slot);
        let mut latency = (self.clients[index].latency as f32).min(MAX_LATENCY);
        let mut max_unlag = self.unlag_cvar("sv_maxunlag");
        if max_unlag != 0.0 {
            if max_unlag < 0.0 {
                self.cvars.set("sv_maxunlag", "0");
                max_unlag = 0.0;
            }
            if latency >= max_unlag {
                latency = max_unlag;
            }
        }
        let mut interp = f32::from(self.clients[index].last_cmd.lerp_msec) / 1000.0;
        if interp > MAX_LERP {
            interp = MAX_LERP;
        }
        let message_interval =
            update_interval(self.views[index].userinfo.get("cl_updaterate"), &self.cfg);
        if message_interval > f64::from(interp) {
            interp = message_interval as f32;
        }
        let target = self.real_time - f64::from(latency) - f64::from(interp)
            + f64::from(self.unlag_cvar("sv_unlagpush"));
        target.min(self.real_time)
    }

    /// `SV_FindUnlagFrames` with `SV_FlagNonInterpolatedPlayers`: walks the frames sent to
    /// `slot`, newest first, to the first one sent before `target`, flagging the players that
    /// must not be interpolated on the way. That frame is `older`, the one sent after it
    /// `newer`. `None` when no frame is kept that is at most a second older than the target.
    fn find_unlag_frames(&self, slot: u8, target: f64) -> Option<UnlagFrames> {
        let client = &self.clients[usize::from(slot)];
        let max_clients = u16::from(self.cfg.max_clients);
        let mut flagged = BTreeSet::new();
        // The last origin seen of each player, to find teleports between two updates.
        let mut last_origin: Vec<Option<Vec3>> = vec![None; usize::from(max_clients) + 1];
        let mut prev: Option<&ClientFrame> = None;
        for i in 0..UPDATE_BACKUP as u32 {
            let seq = client.chan.outgoing_sequence.wrapping_sub(i + 1);
            let Some(frame) = client.frames.get(seq) else {
                continue;
            };
            for p in &frame.players {
                if !(1..=max_clients).contains(&p.number) || flagged.contains(&p.number) {
                    continue;
                }
                if p.health <= 0.0 || p.effects & EF_NOINTERP != 0 {
                    flagged.insert(p.number);
                }
                let seen = &mut last_origin[usize::from(p.number)];
                if seen.is_some_and(|o| unlag_check_teleport(p.origin, o)) {
                    flagged.insert(p.number);
                }
                *seen = Some(p.origin);
            }
            if target > frame.sent_at {
                if target - frame.sent_at > MAX_FRAME_AGE {
                    return None;
                }
                return Some(UnlagFrames {
                    older: frame.seq,
                    newer: prev.map(|f| f.seq),
                    non_interpolated: flagged,
                });
            }
            prev = Some(frame);
        }
        None
    }

    /// `SV_SetupMove`: when lag compensation is active for `slot`, moves every other active
    /// player to where `slot`'s client saw it at the target time (interpolated between two of
    /// the updates sent to it), without touching triggers, and remembers how to put them
    /// back. A player flagged as dead, teleported or not interpolated stays where it is. The
    /// game's own movement collisions (`EngineApi::trace_player_move`) still see the true
    /// places, as `SV_CopyEdictToPhysent` does.
    pub(crate) fn setup_move(&mut self, slot: u8) {
        self.world.moved_players.clear();
        if !self.lag_compensation_active(slot) {
            return;
        }
        let target = self.unlag_target_time(slot);
        let Some(frames) = self.find_unlag_frames(slot, target) else {
            return;
        };
        let client = &self.clients[usize::from(slot)];
        let Some(older) = client.frames.get(frames.older) else {
            return;
        };
        let newer = frames.newer.and_then(|seq| client.frames.get(seq));
        let frac = newer.map_or(0.0, |n| calc_unlag_fraction(older.sent_at, n.sent_at, target));
        let newer = newer.unwrap_or(older);
        let mut rewinds = Vec::new();
        for p in &older.players {
            let other = p.number.wrapping_sub(1);
            if other == u16::from(slot)
                || frames.non_interpolated.contains(&p.number)
                || !(0..u16::from(self.cfg.max_clients)).contains(&other)
                || self.clients[usize::from(other)].state != ClientState::Active
            {
                continue;
            }
            let origin = match newer.players.iter().find(|n| n.number == p.number) {
                Some(next) => p.origin + (next.origin - p.origin) * frac,
                None => p.origin,
            };
            if origin.is_finite() {
                rewinds.push((other as u8, origin));
            }
        }
        for (other, origin) in rewinds {
            let id = self.edicts.client(other);
            let Some(vars) = self.edicts.get(id).map(|e| &e.vars) else {
                continue;
            };
            if vars.origin == origin {
                continue;
            }
            let (true_origin, true_absmin, true_absmax) = (vars.origin, vars.absmin, vars.absmax);
            if let Some(v) = self.edicts.get_mut(id) {
                v.vars.origin = origin;
            }
            self.with_engine(|eng| eng.link_entity(id, false));
            self.world.moved_players.push(MovedPlayer {
                id,
                true_origin,
                true_absmin,
                true_absmax,
                rewound_origin: origin,
            });
        }
    }

    /// `SV_RestoreMove`: puts the players [`setup_move`](Self::setup_move) moved back, linked
    /// without touching triggers. One the game moved meanwhile stays where the game put it.
    /// As in HLDS the absolute box is recomputed by the relink, not restored from a copy (a
    /// copy taken before the command would be stale if the player's size changed), and the
    /// entity's place in its area node list changes; queries sort by entity number.
    pub(crate) fn restore_move(&mut self) {
        for moved in std::mem::take(&mut self.world.moved_players) {
            let untouched = self
                .edicts
                .get(moved.id)
                .is_some_and(|e| e.vars.origin == moved.rewound_origin);
            if !untouched {
                continue;
            }
            if let Some(e) = self.edicts.get_mut(moved.id) {
                e.vars.origin = moved.true_origin;
            }
            self.with_engine(|eng| eng.link_entity(moved.id, false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teleport_is_more_than_128_on_any_axis() {
        let o = Vec3::ZERO;
        assert!(!unlag_check_teleport(o, Vec3::new(128.0, -128.0, 128.0)));
        assert!(unlag_check_teleport(o, Vec3::new(128.5, 0.0, 0.0)));
        assert!(unlag_check_teleport(o, Vec3::new(0.0, -129.0, 0.0)));
        assert!(unlag_check_teleport(Vec3::new(0.0, 0.0, 300.0), o));
    }

    #[test]
    fn fraction_is_clamped_and_zero_for_one_instant() {
        assert_eq!(calc_unlag_fraction(1.0, 1.0, 5.0), 0.0);
        assert_eq!(calc_unlag_fraction(1.0, 2.0, 1.25), 0.25);
        assert_eq!(calc_unlag_fraction(1.0, 2.0, 0.5), 0.0);
        assert_eq!(calc_unlag_fraction(1.0, 2.0, 9.0), 1.0);
    }

    #[test]
    fn userinfo_numbers_read_like_atoi() {
        assert_eq!(info_atoi(None), 0);
        assert_eq!(info_atoi(Some("")), 0);
        assert_eq!(info_atoi(Some("1")), 1);
        assert_eq!(info_atoi(Some(" 2abc")), 2);
        assert_eq!(info_atoi(Some("-3")), -3);
        assert_eq!(info_atoi(Some("x1")), 0);
        assert_eq!(info_atoi(Some("99999999999999999999999")), i64::MAX);
    }
}
