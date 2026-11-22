//! The game's conditional delta encoders, `Entity_Encode`, `Player_Encode` and `Custom_Encode`
//! (ReGameDLL_CS `regamedll/dlls/client.cpp`, vanilla branch, read 2026-11-22), registered by
//! `RegisterEncoders` and named in `delta.lst` (`gamedll Entity_Encode`, ...). Each runs on an
//! entity delta after the fields that differ are marked, and sets or clears marks, so that the
//! engine writes (or leaves out) fields the plain comparison would not.
//!
//! The source finds the fields by name once (`DELTA_FINDFIELD`, in `Entity_FieldInit` and its
//! two siblings) and keeps the indices in statics. `Player_Encode` takes the indices of the
//! `entity_state_t` description (`entity_field_alias`, set by `Entity_FieldInit`), not those of
//! the player description it is given; the two descriptions of `delta.lst` hold `origin[0..2]` at
//! the same indices, so looking the names up in the description at hand is the same. (In the
//! source the aliases are 0, the `animtime` field, until `Entity_Encode` has run once; the world
//! entity's baseline is always the first delta the engine writes, so it has.)

use cs_engine::consts::MOVETYPE_FOLLOW;
use cs_proto48::delta::{DeltaMarks, GoldSrcEntityState};

/// `BEAM_POINTS` (`customentity.h` of the SDK, `#define BEAM_POINTS 0x00`): a beam between two
/// points.
const BEAM_POINTS: u8 = 0;
/// `BEAM_ENTPOINT`: a beam from an entity to a point.
const BEAM_ENTPOINT: u8 = 1;
/// `BEAM_ENTS`: a beam between two entities.
const BEAM_ENTS: u8 = 2;

const ORIGIN: [&str; 3] = ["origin[0]", "origin[1]", "origin[2]"];
const ANGLES: [&str; 3] = ["angles[0]", "angles[1]", "angles[2]"];

/// Runs the encoder named `encoder`; any other name is not the game's and changes nothing.
/// `current_player` is `ENGINE_CURRENT_PLAYER()`: the slot of the client the packet is for.
pub fn delta_encode(
    encoder: &str,
    current_player: u8,
    from: &GoldSrcEntityState,
    to: &GoldSrcEntityState,
    marks: &mut DeltaMarks<'_>,
) {
    match encoder {
        "Entity_Encode" => entity_encode(current_player, from, to, marks),
        "Player_Encode" => player_encode(current_player, from, to, marks),
        "Custom_Encode" => custom_encode(from, to, marks),
        _ => {}
    }
}

/// `DELTA_UNSETBYINDEX` of each field of `names` the description has.
fn unset(marks: &mut DeltaMarks<'_>, names: &[&str]) {
    for name in names {
        if let Some(index) = marks.find_field(name) {
            marks.unset(index);
        }
    }
}

/// `DELTA_SETBYINDEX` of each field of `names` the description has.
fn set(marks: &mut DeltaMarks<'_>, names: &[&str]) {
    for name in names {
        if let Some(index) = marks.find_field(name) {
            marks.set(index);
        }
    }
}

/// `localplayer = (t->number - 1) == ENGINE_CURRENT_PLAYER()`.
fn is_local_player(current_player: u8, to: &GoldSrcEntityState) -> bool {
    i32::from(to.number) - 1 == i32::from(current_player)
}

/// The tail `Entity_Encode` and `Player_Encode` share: an entity that follows another has no
/// origin of its own on the wire; one that began or stopped following has it sent again.
fn follow_origin(from: &GoldSrcEntityState, to: &GoldSrcEntityState, marks: &mut DeltaMarks<'_>) {
    if to.movetype == MOVETYPE_FOLLOW && to.aiment != 0 {
        unset(marks, &ORIGIN);
    } else if to.aiment != from.aiment {
        set(marks, &ORIGIN);
    }
}

/// `Entity_Encode`: "Callback for sending entity_state_t info over network."
///
/// - The local player's origin is never sent: "it's sent with more resolution in clientdata_t
///   structure".
/// - An entity with both `impacttime` and `starttime` (a parametric mover) sends neither origin
///   nor angles.
/// - [`follow_origin`].
pub fn entity_encode(
    current_player: u8,
    from: &GoldSrcEntityState,
    to: &GoldSrcEntityState,
    marks: &mut DeltaMarks<'_>,
) {
    if is_local_player(current_player, to) {
        unset(marks, &ORIGIN);
    }
    if to.impacttime != 0.0 && to.starttime != 0.0 {
        unset(marks, &ORIGIN);
        unset(marks, &ANGLES);
    }
    follow_origin(from, to, marks);
}

/// `Player_Encode`: "Callback for sending entity_state_t for players info over network." The
/// local player's origin is not sent, then [`follow_origin`].
pub fn player_encode(
    current_player: u8,
    from: &GoldSrcEntityState,
    to: &GoldSrcEntityState,
    marks: &mut DeltaMarks<'_>,
) {
    if is_local_player(current_player, to) {
        unset(marks, &ORIGIN);
    }
    follow_origin(from, to, marks);
}

/// `Custom_Encode`: "Callback for sending entity_state_t info ( for custom entities ) over
/// network." The beam type is `rendermode & 0x0F`.
///
/// - Not `BEAM_POINTS`: no angles; and no origin either unless it is `BEAM_ENTPOINT`.
/// - Neither `BEAM_ENTS` nor `BEAM_ENTPOINT`: no `skin` and no `sequence`.
/// - `animtime` is sent only when it differs after truncation to an integer ("animtime is
///   compared by rounding first").
pub fn custom_encode(
    from: &GoldSrcEntityState,
    to: &GoldSrcEntityState,
    marks: &mut DeltaMarks<'_>,
) {
    let beam_type = to.rendermode & 0x0F;
    if beam_type != BEAM_POINTS {
        if beam_type != BEAM_ENTPOINT {
            unset(marks, &ORIGIN);
        }
        unset(marks, &ANGLES);
    }
    if beam_type != BEAM_ENTS && beam_type != BEAM_ENTPOINT {
        unset(marks, &["skin", "sequence"]);
    }
    if from.animtime as i32 == to.animtime as i32 {
        unset(marks, &["animtime"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::delta::{DeltaRegistry, DeltaSchema};

    fn schema(name: &str) -> DeltaSchema {
        DeltaRegistry::cs16_default().get(name).expect("in delta.lst").clone()
    }

    /// Marks with every field of `names` set.
    fn marked<'a>(schema: &'a DeltaSchema, names: &[&str]) -> DeltaMarks<'a> {
        let mut marks = DeltaMarks::new(schema, 0);
        for name in names {
            marks.set(marks.find_field(name).expect("a field of the schema"));
        }
        marks
    }

    fn is_set(marks: &DeltaMarks<'_>, name: &str) -> bool {
        marks.is_set(marks.find_field(name).expect("a field of the schema"))
    }

    fn state(number: u16) -> GoldSrcEntityState {
        GoldSrcEntityState { number, ..GoldSrcEntityState::default() }
    }

    const ORIGINS: [&str; 3] = ["origin[0]", "origin[1]", "origin[2]"];

    #[test]
    fn the_local_players_origin_is_unset_and_other_players_keep_theirs() {
        let schema = schema("entity_state_player_t");
        // Client slot 2 is edict 3.
        for (current, local) in [(2, true), (0, false)] {
            let mut marks = marked(&schema, &["origin[0]", "origin[1]", "origin[2]", "angles[1]"]);
            delta_encode("Player_Encode", current, &state(3), &state(3), &mut marks);
            for name in ORIGINS {
                assert_eq!(is_set(&marks, name), !local, "{name} for current player {current}");
            }
            assert!(is_set(&marks, "angles[1]"), "the angles are the engine's to send");
        }
    }

    #[test]
    fn an_entity_that_follows_another_sends_no_origin_until_it_stops() {
        let schema = schema("entity_state_t");
        let follower = GoldSrcEntityState { movetype: MOVETYPE_FOLLOW, aiment: 5, ..state(20) };
        let mut marks = marked(&schema, &ORIGINS);
        delta_encode("Entity_Encode", 0, &state(20), &follower, &mut marks);
        assert!(ORIGINS.iter().all(|n| !is_set(&marks, n)), "following: no origin");
        // It followed entity 5 and stops: the origin goes out again though it did not change.
        let stopped = GoldSrcEntityState { aiment: 0, ..state(20) };
        let mut marks = DeltaMarks::new(&schema, 0);
        delta_encode("Entity_Encode", 0, &follower, &stopped, &mut marks);
        assert!(ORIGINS.iter().all(|n| is_set(&marks, n)), "aiment changed: origin sent");
        // The same aiment changes nothing.
        let mut marks = DeltaMarks::new(&schema, 0);
        delta_encode("Entity_Encode", 0, &stopped, &stopped, &mut marks);
        assert_eq!(marks.mask(), 0);
    }

    #[test]
    fn a_parametric_mover_sends_neither_origin_nor_angles() {
        let schema = schema("entity_state_t");
        let mover = GoldSrcEntityState { impacttime: 1.0, starttime: 2.0, ..state(20) };
        let all = ["origin[0]", "origin[1]", "origin[2]", "angles[0]", "angles[1]", "angles[2]", "frame"];
        let mut marks = marked(&schema, &all);
        delta_encode("Entity_Encode", 0, &state(20), &mover, &mut marks);
        assert!(is_set(&marks, "frame"));
        assert!(all[..6].iter().all(|n| !is_set(&marks, n)));
        // Only one of the two times: the plain delta.
        let half = GoldSrcEntityState { impacttime: 1.0, ..state(20) };
        let mut marks = marked(&schema, &all);
        delta_encode("Entity_Encode", 0, &state(20), &half, &mut marks);
        assert!(all.iter().all(|n| is_set(&marks, n)));
    }

    #[test]
    fn a_beam_sends_the_fields_its_type_uses() {
        let schema = schema("custom_entity_state_t");
        let all = [
            "origin[0]", "origin[1]", "origin[2]", "angles[0]", "angles[1]", "angles[2]", "skin",
            "sequence", "animtime",
        ];
        let with = |beam: u8, from_animtime: f32, to_animtime: f32| {
            let from = GoldSrcEntityState { animtime: from_animtime, ..state(30) };
            let to = GoldSrcEntityState { rendermode: beam | 0x40, animtime: to_animtime, ..state(30) };
            let mut marks = marked(&schema, &all);
            delta_encode("Custom_Encode", 0, &from, &to, &mut marks);
            all.map(|n| is_set(&marks, n))
        };
        // BEAM_POINTS: positions and angles, no skin and sequence.
        assert_eq!(
            with(BEAM_POINTS, 0.0, 5.0),
            [true, true, true, true, true, true, false, false, true]
        );
        // BEAM_ENTPOINT: origin but no angles; skin and sequence.
        assert_eq!(
            with(BEAM_ENTPOINT, 0.0, 5.0),
            [true, true, true, false, false, false, true, true, true]
        );
        // BEAM_ENTS: no origin, no angles; skin and sequence.
        assert_eq!(
            with(BEAM_ENTS, 0.0, 5.0),
            [false, false, false, false, false, false, true, true, true]
        );
        // A hose (3): nothing positional, no skin.
        assert_eq!(with(3, 0.0, 5.0), [false, false, false, false, false, false, false, false, true]);
        // `animtime` is compared after truncation to an integer.
        assert!(!with(BEAM_POINTS, 5.2, 5.9)[8]);
        assert!(with(BEAM_POINTS, 5.9, 6.1)[8]);
    }

    #[test]
    fn an_encoder_the_game_does_not_know_changes_nothing() {
        let schema = schema("entity_state_t");
        let mut marks = marked(&schema, &ORIGINS);
        let before = marks.mask();
        delta_encode("Other_Encode", 0, &state(1), &state(1), &mut marks);
        assert_eq!(marks.mask(), before);
    }
}
