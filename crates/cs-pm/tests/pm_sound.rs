//! Step sounds and texture materials of `cs_pm::pm` (Task 7) through `pm_move` on `BoxWorld`.
//! Expected values come from `pm_shared.cpp` (ReGameDLL_CS, read 2026-11-22); the formula is in
//! each test's comment.

mod pm_support;

use cs_pm::pm::consts::*;
use cs_pm::pm::{PlayerMove, PmShared, TextureTable, pm_find_texture_type, pm_init};
use glam::Vec3;
use pm_support::{BoxWorld, Call, run_cmd};

const MATERIALS: &[u8] = b"// comment\nM METAL1\nD DIRT1\nV VENT1\nG GRATE1\nT TILE1\nS SLOSH1\nN SNOW1\nW WOOD1\n";

/// A world with one floor box (ent 0, top z = 0) with the given texture, and a runner standing on
/// it at 200 units/s (above the 150 threshold of `PM_UpdateStepSound`), the step timer expired.
fn runner(texture: &str) -> (PlayerMove, PmShared, BoxWorld) {
    let mut w = BoxWorld::new();
    w.solid(Vec3::new(-1000.0, -1000.0, -100.0), Vec3::new(1000.0, 1000.0, 0.0), texture);
    let pm = PlayerMove {
        origin: Vec3::new(0.0, 0.0, 36.0),
        velocity: Vec3::new(200.0, 0.0, 0.0),
        maxspeed: 250.0,
        ..PlayerMove::default()
    };
    (pm, pm_init(Some(MATERIALS)), w)
}

/// One command with the step timer expired and the speed restored.
fn step(pm: &mut PlayerMove, sh: &mut PmShared, w: &mut BoxWorld) {
    pm.fl_time_step_sound = 0;
    pm.velocity = Vec3::new(200.0, 0.0, 0.0);
    run_cmd(pm, sh, w, 10);
}

fn samples(w: &BoxWorld) -> Vec<&str> {
    w.sounds.iter().map(|s| s.sample.as_str()).collect()
}

#[test]
fn materials_parse_like_pm_init_texture_types() {
    let find = |t: &TextureTable, n: &str| t.find(n);

    // Comment lines ('/' first) and blank lines are skipped; so is a line whose first non-blank
    // is not a letter (here a digit and punctuation), with its would-be name.
    let t = TextureTable::parse(b"// M SKIPPED\n\n   \n1 DIGIT\n- DASH\nM KEPT\n");
    assert_eq!(t.len(), 1);
    assert_eq!(find(&t, "KEPT"), b'M');
    assert_eq!(find(&t, "SKIPPED"), b'C');
    assert_eq!(find(&t, "DIGIT"), b'C');
    assert_eq!(find(&t, "DASH"), b'C');

    // Leading blanks are fine; the type letter is upper-cased; the name ends at a blank; CRLF files
    // work ('\r' is a space).
    let t = TextureTable::parse(b"  \t m  lower_case   trailing words\r\nt tile_b\r\n");
    assert_eq!(t.len(), 2);
    assert_eq!(find(&t, "lower_case"), b'M');
    assert_eq!(find(&t, "tile_b"), b'T');
    assert_eq!(find(&t, "trailing"), b'C');

    // A type letter without a name is skipped (end of line, or only blanks before it).
    let t = TextureTable::parse(b"M\nD   \nV VENT\n");
    assert_eq!(t.len(), 1);
    assert_eq!(find(&t, "VENT"), b'V');

    // QUIRK: a name that runs to the very end of the file (no newline after it) is skipped, since
    // the scan needs one more character after the name.
    let t = TextureTable::parse(b"M FIRST\nD LAST_NO_NEWLINE");
    assert_eq!(t.len(), 1);
    assert_eq!(find(&t, "FIRST"), b'M');
    assert_eq!(find(&t, "LAST_NO_NEWLINE"), b'C');

    // Names are cut to 16 characters; lookups compare 16 characters, so a longer query matches too.
    let t = TextureTable::parse(b"G 0123456789ABCDEFGHIJ\n");
    assert_eq!(find(&t, "0123456789ABCDEF"), b'G');
    assert_eq!(find(&t, "0123456789abcdefXYZ"), b'G', "case-insensitive, 16 characters");
    assert_eq!(find(&t, "0123456789ABCDE"), b'C', "a 15 character prefix is a different name");

    // QUIRK of memfgets: a line is read in chunks of at most 510 bytes. A long name line is cut:
    // the first chunk ends inside the name (no character after it -> skipped); the rest of the
    // line is read as a new line of its own, starting with a letter.
    let mut long = b"M ".to_vec();
    long.extend(std::iter::repeat_n(b'a', 600));
    long.push(b'\n');
    let t = TextureTable::parse(&long);
    assert_eq!(t.len(), 1);
    assert_eq!(find(&t, "aaaaaaaaaaaaaaaa"), b'A', "type = first letter of the tail chunk");

    // At most 1024 entries (MAX_TEXTURES) are read.
    let mut many = Vec::new();
    for i in 0..1100 {
        many.extend(format!("M name{i:04}\n").bytes());
    }
    let t = TextureTable::parse(&many);
    assert_eq!(t.len(), 1024);
    assert_eq!(find(&t, "name1023"), b'M');
    assert_eq!(find(&t, "name1024"), b'C');

    // Binary search over a table sorted with a case-insensitive compare, whatever the file order.
    let t = TextureTable::parse(b"M zebra\nD Apple\nV mango\nG BANANA\nT cherry\n");
    for (n, ty) in [("ZEBRA", b'M'), ("apple", b'D'), ("MANGO", b'V'), ("banana", b'G'), ("Cherry", b'T')] {
        assert_eq!(find(&t, n), ty, "{n}");
    }

    // A miss, and an empty table (a failed or absent load), answer concrete.
    assert_eq!(find(&t, "nothing"), b'C');
    assert_eq!(find(&TextureTable::parse(b""), "METAL1"), b'C');
    assert!(TextureTable::default().is_empty());
    assert_eq!(pm_find_texture_type(&pm_init(None), "METAL1"), b'C');
    assert_eq!(pm_find_texture_type(&pm_init(Some(b"M METAL1\n")), "METAL1"), b'M');
}

#[test]
fn materials_parse_hostile_bytes() {
    // Embedded NUL: the line is a C string and ends there. "M AB\0CD\n" has a NUL right after
    // "AB" so the name run hits the terminator and the line is skipped.
    let t = TextureTable::parse(b"M AB\0CD\nD OK\n");
    assert_eq!(t.len(), 1);
    assert_eq!(t.find("OK"), b'D');
    assert_eq!(t.find("AB"), b'C');

    // 0xFF is neither a letter nor a space: a line starting with it is skipped; inside a name it
    // is an ordinary byte (the lookup string is lossy UTF-8, so only a pure-ASCII name is checked).
    let t = TextureTable::parse(b"\xffM SKIP\nM A\xffB\nT PLAIN\n");
    assert_eq!(t.len(), 2);
    assert_eq!(t.find("PLAIN"), b'T');

    // A lone '\r' is whitespace but not a line break: both entries sit on one line, the second
    // is never read as a type.
    let t = TextureTable::parse(b"M ONE\rD TWO\nV THREE\n");
    assert_eq!(t.len(), 2);
    assert_eq!(t.find("ONE"), b'M');
    assert_eq!(t.find("TWO"), b'C');
    assert_eq!(t.find("THREE"), b'V');

    // Lines of exactly 510 and 511 bytes (newline included). 510 fits one chunk: the name is
    // followed by '\n' and is kept. 511 splits: the first 510 bytes end inside the name
    // (skipped), the tail is a lone "\n", a blank line.
    let mut f = b"M ".to_vec();
    f.extend(std::iter::repeat_n(b'x', 507));
    f.push(b'\n'); // 2 + 507 + 1 = 510 bytes
    let t = TextureTable::parse(&f);
    assert_eq!(t.len(), 1);
    assert_eq!(t.find("xxxxxxxxxxxxxxxx"), b'M');

    let mut f = b"M ".to_vec();
    f.extend(std::iter::repeat_n(b'x', 508));
    f.push(b'\n'); // 511 bytes
    f.extend(b"D NEXT\n");
    let t = TextureTable::parse(&f);
    assert_eq!(t.len(), 1);
    assert_eq!(t.find("NEXT"), b'D');
}

#[test]
fn texture_prefixes_are_stripped() {
    // (texture name the world reports, name PM keeps)
    for (raw, kept) in [
        ("METAL1", "METAL1"),
        ("-0METAL1", "METAL1"),
        ("+0METAL1", "METAL1"),
        ("+0~METAL1", "METAL1"),
        ("{METAL1", "METAL1"),
        ("!METAL1", "METAL1"),
        ("~METAL1", "METAL1"),
        (" METAL1", "METAL1"),
        ("-0{METAL1", "METAL1"),
        // QUIRK: '-' / '+' drop two characters whatever the second one is.
        ("-xMETAL1", "METAL1"),
        // A '-' that is not at the start is just a character.
        ("A-0METAL1", "A-0METAL1"),
        // 16 characters are kept.
        ("ABCDEFGHIJKLMNOPQRST", "ABCDEFGHIJKLMNOP"),
        // The strip applies before the cut: the name is 16 characters after the prefix.
        ("-0ABCDEFGHIJKLMNOPQRST", "ABCDEFGHIJKLMNOP"),
    ] {
        let (mut pm, mut sh, mut w) = runner(raw);
        step(&mut pm, &mut sh, &mut w);
        assert_eq!(pm.sztexturename, kept, "raw {raw:?}");
    }

    // The category follows the stripped name through the table.
    let (mut pm, mut sh, mut w) = runner("+0~METAL1");
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(pm.chtexturetype, b'M');

    // QUIRK: a bare "+" has fewer than two characters to drop; the source would walk past the
    // terminator, the port clamps: an empty name, concrete.
    let (mut pm, mut sh, mut w) = runner("+");
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(pm.sztexturename, "");
    assert_eq!(pm.chtexturetype, b'C');

    // No texture (PM_TraceTexture returned NULL): the defaults stay, no table lookup.
    let (mut pm, mut sh, mut w) = runner("");
    pm.sztexturename = "stale".into();
    pm.chtexturetype = b'M';
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(pm.sztexturename, "");
    assert_eq!(pm.chtexturetype, b'C');

    // The trace is asked about the ground entity.
    assert!(w.calls().contains(&Call::TraceTexture(0)));
}

#[test]
fn step_sounds_alternate_feet_by_material() {
    // iStepLeft starts at 0 and toggles before the draw: the first step is the left foot
    // (irand = r + 2), the second the right (irand = r), and so on. Sample number for irand
    // 0, 1, 2, 3 is 1, 3, 2, 4.
    for (texture, family) in [
        ("FLOORX", "step"), // not in the table -> concrete
        ("METAL1", "metal"),
        ("DIRT1", "dirt"),
        ("VENT1", "duct"),
        ("GRATE1", "grate"),
        ("SLOSH1", "slosh"),
        ("SNOW1", "snow"),
        ("WOOD1", "step"), // wood maps to concrete steps
    ] {
        let (mut pm, mut sh, mut w) = runner(texture);
        // r = 0, 1, 0, 1 -> irand 2, 1+... computed below.
        w.random_values.extend([0, 1, 0, 1]);
        for _ in 0..4 {
            step(&mut pm, &mut sh, &mut w);
        }
        // call 1: left=1, irand 0+2 = 2 -> 2;  call 2: left=0, irand 1 -> 3;
        // call 3: left=1, irand 0+2 = 2 -> 2;  call 4: left=0, irand 1 -> 3.
        let want: Vec<String> = ["2", "3", "2", "3"].iter().map(|n| format!("player/pl_{family}{n}.wav")).collect();
        assert_eq!(samples(&w), want, "{texture}");
    }

    // The fourth sample: r = 1 on the left foot -> irand 3 -> 4.
    let (mut pm, mut sh, mut w) = runner("METAL1");
    w.random_values.push_back(1);
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(samples(&w), ["player/pl_metal4.wav"]);

    // Volume per material: dirt 0.55 and vent 0.7 (double constants stored into floats), the rest
    // 0.5; the step timer is 300 ms for all of them; channel body, attenuation 0.8, pitch 100.
    for (texture, vol) in [("DIRT1", 0.55_f64 as f32), ("VENT1", 0.7_f64 as f32), ("METAL1", 0.5), ("FLOORX", 0.5)] {
        let (mut pm, mut sh, mut w) = runner(texture);
        step(&mut pm, &mut sh, &mut w);
        let s = &w.sounds[0];
        assert_eq!((s.volume, s.channel, s.attenuation, s.flags, s.pitch), (vol, CHAN_BODY, ATTN_NORM, 0, PITCH_NORM));
        assert_eq!(pm.fl_time_step_sound, 300, "{texture}");
    }

    // Tile: a second RandomLong(0, 4); 0 selects pl_tile5, anything else the foot sample.
    let (mut pm, mut sh, mut w) = runner("TILE1");
    w.random_values.extend([0, 2, 0, 0]);
    step(&mut pm, &mut sh, &mut w);
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(samples(&w), ["player/pl_tile2.wav", "player/pl_tile5.wav"]);
    // The RNG draws are (0,1) then (0,4) per step.
    let draws: Vec<Call> = w.calls().into_iter().filter(|c| matches!(c, Call::RandomLong(..))).collect();
    assert_eq!(draws, [Call::RandomLong(0, 1), Call::RandomLong(0, 4), Call::RandomLong(0, 1), Call::RandomLong(0, 4)]);
}

#[test]
fn step_timing_and_volume_as_source() {
    // Order of world calls inside PM_UpdateStepSound: the texture trace first, then the knee and
    // feet water probes, then the RNG of the step.
    let (mut pm, mut sh, mut w) = runner("METAL1");
    step(&mut pm, &mut sh, &mut w);
    let kinds = w.call_kinds();
    let tt = kinds.iter().position(|k| *k == "TT").expect("texture trace");
    assert_eq!(&kinds[tt..tt + 4], ["TT", "PC", "PC", "RL"]);
    // knee = z - 0.3 * 72 = 14.4, feet = z - 0.5 * 72 = 0 (origin z is 36 on the floor).
    let probes: Vec<Vec3> = w.calls().into_iter().filter_map(|c| if let Call::PointContents(p) = c { Some(p) } else { None }).collect();
    let knee = probes.iter().find(|p| (p.z - 14.4).abs() < 1e-3).is_some();
    assert!(knee, "knee probe at origin - 0.3 * height: {probes:?}");

    // The timer 300 ms is set by the update; a running timer is only reduced by the command's msec
    // (PM_ReduceTimers) and blocks the sound.
    assert_eq!(pm.fl_time_step_sound, 300);
    pm.velocity = Vec3::new(200.0, 0.0, 0.0);
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.fl_time_step_sound, 290);
    assert_eq!(w.sounds.len(), 1);

    // At or below 150 units/s: no sound; the timer is re-armed with 400 (then the command's own
    // reduction happened before, so the value after the command is 400).
    let (mut pm, mut sh, mut w) = runner("METAL1");
    pm.velocity = Vec3::new(150.0, 0.0, 0.0);
    // Friction acts after the step update, so 150 is what the update sees.
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert_eq!(pm.fl_time_step_sound, 400);
    assert!(w.sounds.is_empty());

    // FL_FROZEN: the update returns at once.
    let (mut pm, mut sh, mut w) = runner("METAL1");
    pm.flags |= FL_FROZEN;
    step(&mut pm, &mut sh, &mut w);
    assert!(w.sounds.is_empty());
    assert_eq!(pm.fl_time_step_sound, 0);
}

#[test]
fn wade_skips_three_of_four() {
    // Water 0..20 over the floor: the knee (z = 36 - 21.6 = 14.4) is in water -> STEP_WADE,
    // fvol 0.65, 600 ms; the origin (36) is dry so the walk is a normal ground walk.
    // The static iSkipStep: 0 -> skipped (counter 1); then 1, 2, 3 play and the call at 3 resets
    // the counter to 0, so the pattern is skip, play, play, play, skip, ... In the source's own
    // words a call at counter 0 never plays: one skip in four calls (the plan's "3 of 4" counts
    // the plays).
    let (mut pm, mut sh, mut w) = runner("FLOORX");
    w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 20.0), CONTENTS_WATER, CONTENTS_WATER);
    for _ in 0..8 {
        step(&mut pm, &mut sh, &mut w);
        assert_eq!(pm.fl_time_step_sound, 600);
    }
    // Calls 1 and 5 are skipped. iStepLeft toggles on every call: odd calls are the left foot
    // (irand 2 -> wade3), even calls the right foot (irand 0 -> wade1); r is always 0.
    assert_eq!(
        samples(&w),
        [
            "player/pl_wade1.wav", // call 2
            "player/pl_wade3.wav", // call 3
            "player/pl_wade1.wav", // call 4
            "player/pl_wade1.wav", // call 6
            "player/pl_wade3.wav", // call 7
            "player/pl_wade1.wav", // call 8
        ]
    );
    assert!(w.sounds.iter().all(|s| s.volume == 0.65_f64 as f32));
}

#[test]
fn footsteps_off_in_multiplayer_is_silent() {
    // mp_footsteps 0 in multiplayer: no sound, but the foot toggled, the RNG was drawn first and
    // the timer is armed.
    let (mut pm, mut sh, mut w) = runner("METAL1");
    pm.multiplayer = true;
    pm.movevars.footsteps = false;
    step(&mut pm, &mut sh, &mut w);
    assert!(w.sounds.is_empty());
    assert_eq!(pm.i_step_left, 1);
    assert!(w.calls().contains(&Call::RandomLong(0, 1)));
    assert_eq!(pm.fl_time_step_sound, 300);

    // Single player ignores the cvar.
    let (mut pm, mut sh, mut w) = runner("METAL1");
    pm.multiplayer = false;
    pm.movevars.footsteps = false;
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(samples(&w), ["player/pl_metal2.wav"]);

    // The tile roll comes after the check: silent steps do not draw it.
    let (mut pm, mut sh, mut w) = runner("TILE1");
    pm.movevars.footsteps = false;
    step(&mut pm, &mut sh, &mut w);
    assert!(!w.calls().contains(&Call::RandomLong(0, 4)));
}

#[test]
fn runfuncs_off_still_toggles_the_foot() {
    // A replayed (client) command: the foot toggles but nothing is drawn or played; the timer is
    // armed as usual.
    let (mut pm, mut sh, mut w) = runner("METAL1");
    pm.runfuncs = false;
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(pm.i_step_left, 1);
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(pm.i_step_left, 0);
    assert!(w.sounds.is_empty());
    assert!(!w.call_kinds().contains(&"RL"));
    assert_eq!(pm.fl_time_step_sound, 300);

    // The next live command plays the foot the toggling left: i_step_left 0 -> 1 -> left foot.
    pm.runfuncs = true;
    step(&mut pm, &mut sh, &mut w);
    assert_eq!(samples(&w), ["player/pl_metal2.wav"]);
}

/// A runner at rest on the floor with a prefilled fall speed; one command lands it.
fn landing(fall: f32, waterlevel_water: bool) -> (PlayerMove, BoxWorld) {
    let (mut pm, mut sh, mut w) = runner("METAL1");
    pm.velocity = Vec3::ZERO;
    pm.fl_fall_velocity = fall;
    if waterlevel_water {
        // Feet in water (waterlevel 1), the knee of a standing hull is not what matters here.
        w.water_volume(Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, 1000.0, 10.0), CONTENTS_WATER, CONTENTS_WATER);
    }
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    (pm, w)
}

#[test]
fn landing_volume_thresholds() {
    // PM_CheckFalling: onground && fall >= 250 (PM_PLAYER_FALL_PUNCH_THRESHHOLD); out of water:
    // > 580 -> 1.0; > 290 (580 / 2, integer division) -> 0.85; otherwise (< 350) -> 0.0, so the
    // 0.5 default is only reachable in water. The 350 bounce speed never separates anything:
    // everything above 290 is already 0.85.
    for (fall, want) in [
        (600.0, Some(1.0_f32)),
        (581.0, Some(1.0)),
        (580.0, Some(0.85)),
        (350.0, Some(0.85)),
        (349.0, Some(0.85)),
        (291.0, Some(0.85)),
        (290.0, None),
        (250.0, None),
        (249.0, None),
        (0.0, None),
    ] {
        let (pm, w) = landing(fall, false);
        match want {
            Some(v) => {
                assert_eq!(w.sounds.len(), 1, "fall {fall}");
                // Metal: left foot first, r = 0 -> irand 2 -> 2.
                assert_eq!(w.sounds[0].sample, "player/pl_metal2.wav", "fall {fall}");
                assert_eq!(w.sounds[0].volume, v, "fall {fall}");
                // timer 300; punch z = fall * 0.013 (double product stored into a float).
                assert_eq!(pm.fl_time_step_sound, 300, "fall {fall}");
                assert_eq!(pm.punchangle[2], (f64::from(fall) * 0.013) as f32, "fall {fall}");
            }
            None => {
                assert!(w.sounds.is_empty(), "fall {fall}");
                assert_eq!(pm.punchangle[2], 0.0, "fall {fall}");
            }
        }
        // On the ground the fall speed is always cleared.
        assert_eq!(pm.fl_fall_velocity, 0.0, "fall {fall}");
    }

    // In water (waterlevel > 0) the fall volume stays at its 0.5 start whatever the speed.
    for fall in [260.0_f32, 700.0] {
        let (pm, w) = landing(fall, true);
        assert!(pm.waterlevel > 0);
        assert_eq!(w.sounds.len(), 1, "fall {fall}");
        assert_eq!(w.sounds[0].volume, 0.5, "fall {fall}");
    }

    // A dead player makes no landing sound.
    let (mut pm, mut sh, mut w) = runner("METAL1");
    pm.velocity = Vec3::ZERO;
    pm.fl_fall_velocity = 600.0;
    pm.dead = true;
    run_cmd(&mut pm, &mut sh, &mut w, 10);
    assert!(w.sounds.is_empty());
}
