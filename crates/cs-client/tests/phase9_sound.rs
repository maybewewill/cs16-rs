use std::collections::HashMap;
use std::io::Cursor;

use cs_client::audio::mixer::{SND_FL_CHANGE_PITCH, SND_FL_CHANGE_VOL};
use cs_client::audio::{
    GoldSrcMixer, ListenerState, MAX_DYNAMIC_CHANNELS, MAX_STATIC_CHANNELS, MIX_SAMPLE_RATE,
    MusicCommand, MusicController, NUM_ROOM_TYPES, ROOM_PRESETS, RoomDsp, SentenceTable,
    VOICE_SAMPLE_RATE, VoiceCodecKind, VoiceManager, build_impact_sounds, decode_ima_adpcm,
    decode_steam_opus_plc, decode_wav, encode_ima_adpcm, encode_steam_opus_plc,
    encode_test_wav_pcm16, frame_speex_nb_packet, material_impact_sound, parse_speex_nb_packet,
    parse_wav_loop_points, reload_sound_path, ricochet_sound,
};
use cs_clientcore::sound::SoundStart;
use cs_clientcore::{SoundCommand, VoiceEvent};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_pm::pm::consts::{
    CHAR_TEX_CONCRETE, CHAR_TEX_FLESH, CHAR_TEX_GLASS, CHAR_TEX_METAL, CHAR_TEX_WOOD,
};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

fn sine_i16(sample_rate: u32, freq_hz: f32, frames: usize, amplitude: f32) -> Vec<i16> {
    (0..frames)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let s = (2.0 * std::f32::consts::PI * freq_hz * t).sin() * amplitude;
            (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16
        })
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

fn stereo_channel_rms(interleaved: &[f32], channel: usize) -> f32 {
    let vals: Vec<f32> = interleaved
        .as_chunks::<2>()
        .0
        .iter()
        .map(|lr| lr[channel])
        .collect();
    rms(&vals)
}

#[test]
fn wav_2d_playback_renders_pcm() {
    let pcm = sine_i16(22_050, 440.0, 4_000, 0.7);
    let wav = encode_test_wav_pcm16(22_050, 1, &pcm, None, None);

    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    mixer.insert_cached_wav("ui/menu_click.wav", &wav);

    let start = SoundStart::by_name("ui/menu_click.wav", 1, 2, Vec3::ZERO, 1.0, 0.0, 100);
    mixer.start_sound(None, &start, false, None);

    let mut out = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 20) * 2];
    mixer.mix_frames(&mut out);
    assert!(rms(&out) > 0.15, "2D WAV playback must render non-silent PCM");

    if let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) {
        let candidate = ["sound/weapons/ak47-1.wav", "sound/items/smallmedkit1.wav"]
            .into_iter()
            .find_map(|p| vfs.read_bytes(p).ok().map(|b| (p, b)));
        if let Some((path, bytes)) = candidate {
            let dec = decode_wav(&bytes).unwrap_or_else(|e| panic!("decode {path}: {e}"));
            assert!(dec.sample_rate >= 8000 && dec.frame_count() > 100);
        }
    }
}

#[test]
fn wav_cue_and_smpl_loop_points_wrap() {
    let pcm = sine_i16(22_050, 440.0, 4_000, 0.7);
    let cue_wav = encode_test_wav_pcm16(22_050, 1, &pcm, Some(1_000), None);
    let smpl_wav = encode_test_wav_pcm16(22_050, 1, &pcm, None, Some((500, 1_500)));

    let dec_cue = decode_wav(&cue_wav).expect("cue wav decodes");
    assert_eq!(dec_cue.loop_start, Some(1_000));
    assert_eq!(dec_cue.loop_end, Some(4_000));

    let (ls, le, sr) = parse_wav_loop_points(&smpl_wav).expect("smpl loop points extracted");
    assert_eq!((ls, le, sr), (500, 1_500, 22_050));

    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    mixer.insert_cached_wav("ambience/loop_hum.wav", &smpl_wav);

    let start = SoundStart::by_name("ambience/loop_hum.wav", 10, 6, Vec3::ZERO, 1.0, 0.0, 100);
    mixer.start_sound(None, &start, false, None);

    // Mix 1 full second (~44,100 frames) so a 1500-frame WAV must wrap cleanly many times.
    let mut out = vec![0.0f32; (MIX_SAMPLE_RATE as usize) * 2];
    mixer.mix_frames(&mut out);

    let tail = &out[out.len() - 4096..];
    assert!(rms(tail) > 0.15, "looped smpl WAV must keep playing seamlessly in tail");
    let (dyn_active, _) = mixer.active_channel_counts();
    assert_eq!(dyn_active, 1);
}

#[test]
fn channels_priorities_alter_and_stopsound() {
    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    let tone = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 330.0, 44_100, 0.6), None, None);
    mixer.insert_cached_wav("weapons/test_fire.wav", &tone);

    // 1. Same (entity, channel != 0) replaces existing slot; CHAN_AUTO (0) layers multiple.
    for _ in 0..4 {
        let s = SoundStart::by_name("weapons/test_fire.wav", 2, 0, Vec3::ZERO, 0.8, 0.0, 100);
        mixer.start_sound(None, &s, false, None);
    }
    assert_eq!(mixer.active_channel_counts(), (4, 0));

    let ch1_a = SoundStart::by_name("weapons/test_fire.wav", 2, 1, Vec3::ZERO, 0.8, 0.0, 100);
    let ch1_b = SoundStart::by_name("weapons/test_fire.wav", 2, 1, Vec3::ZERO, 0.4, 0.0, 100);
    mixer.start_sound(None, &ch1_a, false, None);
    mixer.start_sound(None, &ch1_b, false, None);
    assert_eq!(mixer.active_channel_counts(), (5, 0));

    // 2. SND_CHANGE_VOL and SND_CHANGE_PITCH modify active channel in-place without spawning a duplicate.
    let mut alter = SoundStart::by_name("weapons/test_fire.wav", 2, 1, Vec3::ZERO, 0.25, 0.0, 150);
    alter.flags = SND_FL_CHANGE_VOL | SND_FL_CHANGE_PITCH;
    mixer.start_sound(None, &alter, false, None);
    assert_eq!(mixer.active_channel_counts(), (5, 0));
    let (lg, rg) = mixer.channel_gains(2, 1).expect("altered channel exists");
    assert!((lg - 0.25).abs() < 0.03 && (rg - 0.25).abs() < 0.03);

    // 3. SND_STOP & SoundCommand::Stop kill only that channel; stop_all clears all dynamic + static.
    let mut stop_flag = SoundStart::by_name("weapons/test_fire.wav", 2, 1, Vec3::ZERO, 0.0, 0.0, 100);
    stop_flag.flags = 1 << 5; // SND_STOP
    mixer.start_sound(None, &stop_flag, false, None);
    assert_eq!(mixer.active_channel_counts(), (4, 0));

    // Fill past MAX_DYNAMIC_CHANNELS + MAX_STATIC_CHANNELS to verify capped priority eviction.
    for i in 0..(MAX_DYNAMIC_CHANNELS + 16) {
        let s = SoundStart::by_name("weapons/test_fire.wav", 10 + i as u16, 1, Vec3::ZERO, 0.5, 0.0, 100);
        mixer.start_sound(None, &s, false, None);
    }
    for i in 0..(MAX_STATIC_CHANNELS + 8) {
        let s = SoundStart::by_name("weapons/test_fire.wav", 500 + i as u16, 6, Vec3::ZERO, 0.5, 0.8, 100);
        mixer.start_sound(None, &s, true, None);
    }
    let (dyn_cnt, stat_cnt) = mixer.active_channel_counts();
    assert!(dyn_cnt <= MAX_DYNAMIC_CHANNELS);
    assert!(stat_cnt <= MAX_STATIC_CHANNELS);

    mixer.stop_all();
    assert_eq!(mixer.active_channel_counts(), (0, 0));
}

#[test]
fn goldsrc_distance_attenuation_and_stereo_pan() {
    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    let tone = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 500.0, 22_050, 0.8), None, None);
    mixer.insert_cached_wav("player/step.wav", &tone);

    let listener = ListenerState {
        origin: Vec3::ZERO,
        right: Vec3::Y, // +Y is right ear
        local_entity: Some(1),
        waterlevel: 0,
    };
    mixer.update_listener_and_entities(listener, &HashMap::new(), None);

    // 1. Source directly on listener's right ear -> right channel louder than left.
    let right_src = SoundStart::by_name(
        "player/step.wav",
        10,
        2,
        Vec3::new(0.0, 200.0, 0.0),
        1.0,
        0.8,
        100,
    );
    mixer.start_sound(None, &right_src, false, None);
    let (l_pan, r_pan) = mixer.channel_gains(10, 2).expect("right source active");
    assert!(r_pan > l_pan * 2.0, "right pan gain {r_pan} should dominate left {l_pan}");

    let mut pan_buf = vec![0.0f32; 4096];
    mixer.mix_frames(&mut pan_buf);
    assert!(stereo_channel_rms(&pan_buf, 1) > stereo_channel_rms(&pan_buf, 0) * 2.0);
    mixer.stop_all();

    // 2. Closer source vs farther source under ATTN_NORM (0.8) attenuates monotonically.
    let near_src = SoundStart::by_name("player/step.wav", 11, 2, Vec3::new(100.0, 0.0, 0.0), 1.0, 0.8, 100);
    let far_src = SoundStart::by_name("player/step.wav", 12, 2, Vec3::new(600.0, 0.0, 0.0), 1.0, 0.8, 100);
    mixer.start_sound(None, &near_src, false, None);
    mixer.start_sound(None, &far_src, false, None);
    let (near_l, _) = mixer.channel_gains(11, 2).expect("near active");
    let (far_l, _) = mixer.channel_gains(12, 2).expect("far active");
    assert!(near_l > far_l * 1.5, "near gain {near_l} should exceed far gain {far_l}");
}

#[test]
fn goldsrc_spatialize_no_wall_occlusion_plus_timescale_and_duplicate_jitter() {
    use cs_client::audio::mixer::brush_model_sound_origin;

    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    mixer.set_occlusion_factor(0.3);
    let tone = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 500.0, 22_050, 0.8), None, None);
    mixer.insert_cached_wav("player/step.wav", &tone);

    let listener = ListenerState {
        origin: Vec3::ZERO,
        right: Vec3::Y,
        local_entity: Some(1),
        waterlevel: 0,
    };
    mixer.update_listener_and_entities(listener, &HashMap::new(), None);

    let occ_pos = Vec3::new(300.0, 0.0, 0.0);
    let clear = SoundStart::by_name("player/step.wav", 11, 1, occ_pos, 1.0, 0.8, 100);
    mixer.start_sound(None, &clear, false, Some(&|_| false));
    let (c_l, _) = mixer.channel_gains(11, 1).unwrap();

    // Retail `hw.dll:FUN_101fd050` does not apply a 48-unit wall occlusion gain multiplier.
    let blocked = SoundStart::by_name("player/step.wav", 12, 1, occ_pos, 1.0, 0.8, 100);
    mixer.start_sound(None, &blocked, false, Some(&|p| p.x > 250.0));
    let (b_l, _) = mixer.channel_gains(12, 1).unwrap();
    assert!((b_l - c_l).abs() < 1e-6, "clear {c_l} vs blocked {b_l}");

    // Duplicate dynamic sound receives 0..10% sample-rate jitter (`hw.dll:FUN_101fe3e0`).
    let first_jitter = mixer.channel_rate_jitter(11, 1).unwrap();
    let dup_jitter = mixer.channel_rate_jitter(12, 1).unwrap();
    assert_eq!(first_jitter, 1.0);
    assert!((1.0..=1.10).contains(&dup_jitter) && dup_jitter > 1.0, "{dup_jitter}");

    // Brush-model sound origin uses bbox center `origin + (mins + maxs) * 0.5`.
    assert_eq!(
        brush_model_sound_origin(
            Vec3::new(10.0, 20.0, 30.0),
            Vec3::new(-4.0, -6.0, -8.0),
            Vec3::new(8.0, 10.0, 12.0),
        ),
        Vec3::new(12.0, 22.0, 32.0)
    );
}

#[test]
fn goldsrc_fractional_pitch_shifting() {
    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    let short = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 440.0, 2_205, 0.8), None, None);
    mixer.insert_cached_wav("misc/short.wav", &short);

    mixer.start_sound(
        None,
        &SoundStart::by_name("misc/short.wav", 20, 1, Vec3::ZERO, 1.0, 0.0, 200),
        false,
        None,
    );
    let mut tmp = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 10) * 2];
    mixer.mix_frames(&mut tmp); // 100 ms: at 2x speed, a 100 ms WAV completes (~50 ms)
    assert_eq!(mixer.active_channel_counts().0, 0, "pitch=200 should finish in <=100ms");

    mixer.start_sound(
        None,
        &SoundStart::by_name("misc/short.wav", 21, 1, Vec3::ZERO, 1.0, 0.0, 50),
        false,
        None,
    );
    mixer.mix_frames(&mut tmp); // 100 ms at 0.5x speed -> takes ~200 ms, still active
    assert_eq!(mixer.active_channel_counts().0, 1, "pitch=50 should still be playing at 100ms");

    // Retail `hw.dll:FUN_10202bf0`: `(timescale + 1.0) * 0.5` pitch scaling.
    mixer.stop_all();
    mixer.set_timescale(3.0); // (3.0 + 1.0) * 0.5 = 2.0x speed
    mixer.start_sound(
        None,
        &SoundStart::by_name("misc/short.wav", 22, 1, Vec3::ZERO, 1.0, 0.0, 100),
        false,
        None,
    );
    mixer.mix_frames(&mut tmp); // 100 ms WAV at 2.0x speed completes in ~50 ms
    assert_eq!(mixer.active_channel_counts().0, 0, "timescale=3.0 should play at 2x pitch");
}

#[test]
fn sentences_txt_and_vox_concatenation() {
    let script = r#"
        // GoldSrc sentences.txt sample
        TEST_SEQ vox/(v80 p120 s10 e90) hello, (p95) world
        MRAD_FIREINHOLE radio/ct_fireinhole(v100)
    "#;
    let table = SentenceTable::parse(script);
    let words = table.resolve("!TEST_SEQ").expect("resolve !TEST_SEQ");
    assert_eq!(words[0].path, "vox/hello.wav");
    assert_eq!(words[0].pitch, 120);
    assert!((words[0].volume - 0.8).abs() < 0.01);
    assert_eq!(words[0].start_pct, 10);
    assert_eq!(words[0].end_pct, 90);
    assert!(words[0].pause_sec > 0.0);
    assert_eq!(words[1].path, "vox/world.wav");
    assert_eq!(words[1].pitch, 95);

    assert_eq!(
        table.resolve("%!MRAD_FIREINHOLE").unwrap()[0].path,
        "radio/ct_fireinhole.wav"
    );

    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 1.0);
    mixer.set_room_off(true);
    let w1 = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 300.0, 2_205, 0.6), None, None);
    let w2 = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 600.0, 2_205, 0.6), None, None);
    mixer.insert_cached_wav("vox/hello.wav", &w1);
    mixer.insert_cached_wav("vox/world.wav", &w2);

    let s = SoundStart::by_name("!TEST_SEQ", 0, 2, Vec3::ZERO, 1.0, 0.0, 100);
    mixer.start_vox_words(None, &words, &s, false, None);

    // Mix across ~400ms so first word (0..67ms) + comma pause (67..217ms) + second word (217..322ms) all play.
    let mut buf = vec![0.0f32; ((MIX_SAMPLE_RATE as f32 * 0.40) as usize) * 2];
    mixer.mix_frames(&mut buf);
    let first_rms = rms(&buf[..4000]);
    let mid_start = ((MIX_SAMPLE_RATE as f32 * 0.23) as usize) * 2;
    let mid_end = ((MIX_SAMPLE_RATE as f32 * 0.30) as usize) * 2;
    let second_rms = rms(&buf[mid_start..mid_end]);
    assert!(first_rms > 0.05 && second_rms > 0.05);

    // Retail sentences.txt verification if HL1/CS1.6 is installed.
    if let Ok(vfs) = Vfs::discover(None, VfsConfig::default())
        && let Ok(raw) = vfs.read_string_lossy("sound/sentences.txt")
    {
        let retail = SentenceTable::parse(&raw);
        let hg = retail.resolve("!HG_GREN1");
        assert!(hg.is_some(), "retail sentences.txt should resolve !HG_GREN1");
        assert!(hg.unwrap().iter().all(|w| w.path.starts_with("hgrunt/")));
    }
}

#[test]
fn roomtype_29_presets_modulate_wet_reverb() {
    assert_eq!(NUM_ROOM_TYPES, 29);
    assert_eq!(ROOM_PRESETS.len(), 29);

    let mut dsp = RoomDsp::new(MIX_SAMPLE_RATE);
    for rt in 0..NUM_ROOM_TYPES as u16 {
        dsp.set_room_type(rt);
        assert_eq!(dsp.room_type(), usize::from(rt));
        let (l, r) = dsp.process_frame(0.5, -0.5);
        assert!(l.is_finite() && r.is_finite());
    }

    // Feed an impulse followed by silence in Cavern 2 preset (24) -> measurable wet tail.
    let mut rev = RoomDsp::new(MIX_SAMPLE_RATE);
    rev.set_room_type(24);
    let mut energy_after = 0.0f32;
    for i in 0..4_000 {
        let inp = if i < 200 { 0.8 } else { 0.0 };
        let (l, r) = rev.process_frame(inp, inp);
        if i >= 600 {
            energy_after += l * l + r * r;
        }
    }
    assert!(energy_after > 0.01, "preset 24 should generate reverb tail");

    // `room_off = true` disables wet reverb tail.
    let mut off_dsp = RoomDsp::new(MIX_SAMPLE_RATE);
    off_dsp.set_room_type(14);
    off_dsp.set_room_off(true);
    let mut off_tail = 0.0f32;
    for i in 0..2_000 {
        let inp = if i < 100 { 0.8 } else { 0.0 };
        let (l, _) = off_dsp.process_frame(inp, inp);
        if i >= 200 {
            off_tail += l * l;
        }
    }
    assert!(off_tail < 1e-6, "room_off=1 must bypass reverb");

    // Mixer integration of SoundCommand::RoomType.
    let mut mixer = GoldSrcMixer::new();
    mixer.handle_sound_command(None, &SoundCommand::RoomType(18), None);
    assert_eq!(mixer.room_type(), 18);
}

#[test]
fn underwater_muffling_lowpass_filters_audio() {
    let mut uw_low = RoomDsp::new(MIX_SAMPLE_RATE);
    uw_low.set_underwater(true);
    let low_in: Vec<f32> = (0..4_000)
        .map(|i| (2.0 * std::f32::consts::PI * 180.0 * i as f32 / MIX_SAMPLE_RATE as f32).sin())
        .collect();
    let low_out: Vec<f32> = low_in.iter().map(|&x| uw_low.process_frame(x, x).0).collect();

    let mut uw_high = RoomDsp::new(MIX_SAMPLE_RATE);
    uw_high.set_underwater(true);
    let high_in: Vec<f32> = (0..4_000)
        .map(|i| (2.0 * std::f32::consts::PI * 4_500.0 * i as f32 / MIX_SAMPLE_RATE as f32).sin())
        .collect();
    let high_out: Vec<f32> = high_in.iter().map(|&x| uw_high.process_frame(x, x).0).collect();
    assert!(rms(&low_out[500..]) > rms(&high_out[500..]) * 5.0);
}

#[test]
fn soundfade_attenuates_and_recovers_master_gain() {
    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    let tone = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 300.0, 176_400, 0.8), None, None);
    mixer.insert_cached_wav("ambience/drone.wav", &tone);
    mixer.start_sound(
        None,
        &SoundStart::by_name("ambience/drone.wav", 1, 1, Vec3::ZERO, 1.0, 0.0, 100),
        false,
        None,
    );
    mixer.handle_sound_command(
        None,
        &SoundCommand::Fade {
            percent: 100,
            hold_time: 1,
            fade_out: 1,
            fade_in: 1,
        },
        None,
    );
    let mut half_sec = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 2) * 2];
    mixer.mix_frames(&mut half_sec); // t=0..0.5s
    let mut held = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 2) * 2];
    mixer.mix_frames(&mut held); // t=0.5..1.0s
    let mut muted = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 2) * 2];
    mixer.mix_frames(&mut muted); // t=1.0..1.5s inside hold
    assert!(rms(&muted) < rms(&half_sec) * 0.1, "soundfade hold must attenuate output");

    // Advance through remainder of hold (1.5..2.0s) and fade_in (2.0..3.0s) into full recovery (3.0..3.5s).
    let mut fade_in = vec![0.0f32; (MIX_SAMPLE_RATE as usize * 3 / 2) * 2];
    mixer.mix_frames(&mut fade_in);
    let mut recovered = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 2) * 2];
    mixer.mix_frames(&mut recovered);
    assert!(
        rms(&recovered) > rms(&half_sec) * 0.8,
        "soundfade must recover gain after hold + fade_in"
    );
}

#[test]
fn cdtrack_and_mp3_console_commands() {
    let mut cvars = CVarRegistry::new_with_defaults();
    let actions = cvars.execute_script("mp3 loop media/Half-Life03.mp3; cd play 5; stopsound");
    assert!(actions.iter().any(|a| matches!(a, EngineAction::Mp3Command(_))));
    assert!(actions.iter().any(|a| matches!(a, EngineAction::CdCommand(_))));
    assert!(actions.iter().any(|a| matches!(a, EngineAction::StopSound)));

    let mut mc = MusicController::new();
    mc.set_params(0.7, true, 1.0);
    mc.handle_svc_cdtrack(12, 12);
    let cmds = mc.step(0.016);
    assert!(matches!(
        cmds.as_slice(),
        [MusicCommand::Play { path, looping: true, .. }] if path == "media/Half-Life11.mp3"
    ));

    mc.handle_mp3_command(&["stop".to_string()]);
    let fade_cmds = mc.step(0.5);
    assert!(matches!(fade_cmds.as_slice(), [MusicCommand::SetVolume(v)] if *v < 0.5));
    let done_cmds = mc.step(0.6);
    assert!(matches!(done_cmds.as_slice(), [MusicCommand::Stop]));
}

#[test]
fn real_mp3_menu_music_decode_if_installed() {
    if let Ok(vfs) = Vfs::discover(None, VfsConfig::default())
        && let Ok(mp3_bytes) = vfs.read_bytes("media/Half-Life01.mp3")
    {
        let dec = rodio::Decoder::new(Cursor::new(mp3_bytes)).expect("decode Half-Life01.mp3");
        let sr = rodio::Source::sample_rate(&dec).get();
        let ch = rodio::Source::channels(&dec).get();
        assert!(sr >= 22_050 && (ch == 1 || ch == 2));
        let first_samples: Vec<f32> = dec.take(2048).collect();
        assert_eq!(first_samples.len(), 2048);
    }
}

#[test]
fn gameplay_sound_events_radio_c4_footsteps_and_roomtype_drive_mixer() {
    let script = r#"
        MRAD_LOCKNLOAD radio/locknload(v100)
        MRAD_TERWIN radio/terwin(v100)
        MRAD_BOMBPL radio/bombpl(v100)
        MRAD_BOMBDEF radio/bombdef(v100)
        MRAD_RESCUED radio/rescued(v100)
    "#;
    let sentences = SentenceTable::parse(script);
    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);

    let clip = encode_test_wav_pcm16(22_050, 1, &sine_i16(22_050, 440.0, 2_205, 0.6), None, None);
    for path in [
        "radio/locknload.wav",
        "radio/terwin.wav",
        "radio/bombpl.wav",
        "radio/bombdef.wav",
        "radio/rescued.wav",
        "weapons/c4_plant.wav",
        "weapons/c4_beep1.wav",
        "weapons/c4_disarmed.wav",
        "weapons/c4_explode1.wav",
        "weapons/he_bounce-1.wav",
        "hostage/hos1.wav",
        "player/pl_step1.wav",
    ] {
        mixer.insert_cached_wav(path, &clip);
    }

    for token in ["%!MRAD_LOCKNLOAD", "%!MRAD_BOMBPL", "%!MRAD_BOMBDEF", "%!MRAD_RESCUED", "%!MRAD_TERWIN"] {
        let words = sentences.resolve(token).unwrap_or_else(|| panic!("resolve {token}"));
        let s = SoundStart::by_name(token, 0, 2, Vec3::ZERO, 1.0, 0.0, 100);
        mixer.start_vox_words(None, &words, &s, false, None);
    }

    for (ent, path) in [
        (30u16, "weapons/c4_plant.wav"),
        (31, "weapons/c4_beep1.wav"),
        (32, "weapons/c4_disarmed.wav"),
        (33, "weapons/c4_explode1.wav"),
        (34, "weapons/he_bounce-1.wav"),
        (35, "hostage/hos1.wav"),
        (36, "player/pl_step1.wav"),
    ] {
        let s = SoundStart::by_name(path, ent, 1, Vec3::new(32.0, 0.0, 0.0), 1.0, 0.8, 100);
        mixer.start_sound(None, &s, false, None);
    }
    mixer.handle_sound_command(None, &SoundCommand::RoomType(12), None);
    assert_eq!(mixer.room_type(), 12);

    let mut out = vec![0.0f32; (MIX_SAMPLE_RATE as usize / 20) * 2];
    mixer.mix_frames(&mut out);
    assert!(rms(&out) > 0.05, "gameplay event sounds must render active mixer PCM");
}

#[test]
fn material_impacts_and_predicted_footsteps() {
    let materials = [
        (CHAR_TEX_METAL, "debris/metal"),
        (CHAR_TEX_WOOD, "debris/wood"),
        (CHAR_TEX_GLASS, "debris/glass"),
        (CHAR_TEX_FLESH, "debris/bustflesh"),
        (CHAR_TEX_CONCRETE, "weapons/ric"),
    ];
    for &(tex, prefix) in &materials {
        let (path, vol, pitch) = material_impact_sound(tex, 42);
        assert!(
            path.starts_with(prefix),
            "material {} resolved unexpected {path}",
            char::from(tex)
        );
        assert!(vol > 0.0 && pitch >= 80);
    }
    assert!(ricochet_sound(b'M', 1).unwrap().0.contains("ric_metal"));
    assert!(ricochet_sound(b'F', 1).is_none());

    let starts = build_impact_sounds(Vec3::new(10.0, 20.0, 30.0), b'M', 7);
    assert_eq!(starts.len(), 2);
    assert_eq!(reload_sound_path(0), "weapons/generic_reload.wav");
    assert_eq!(reload_sound_path(1), "weapons/generic_shot_reload.wav");

    // Verify all mapped impact & reload WAV paths actually resolve in retail HL1/CS1.6 VFS.
    if let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) {
        for &tex in b"CMWGFDTVY" {
            for seed in 0..8 {
                let (p, _, _) = material_impact_sound(tex, seed);
                assert!(vfs.exists(&format!("sound/{p}")), "missing retail asset sound/{p}");
            }
        }
        for kind in [0u8, 1u8] {
            let rp = reload_sound_path(kind);
            assert!(vfs.exists(&format!("sound/{rp}")), "missing retail asset sound/{rp}");
        }
    }
}

#[test]
fn voice_opus_native_loopback_jitter_and_plc() {
    let mut mgr = VoiceManager::new();
    mgr.handle_event(
        VoiceEvent::Init {
            codec: "voice_opus".to_string(),
            quality: 4,
        },
        0.0,
    );
    assert_eq!(mgr.codec(), VoiceCodecKind::NativeOpus);

    // 60 ms of 440 Hz mic speech at 24 kHz (= 3 frames of 480 samples).
    let mic: Vec<f32> = (0..1_440)
        .map(|i| 0.35 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / VOICE_SAMPLE_RATE as f32).sin())
        .collect();
    mgr.push_mic_samples(&mic, VOICE_SAMPLE_RATE, 1.0, true, false, 0.5, 5.0);
    let pkts = mgr.take_outgoing();
    assert_eq!(pkts.len(), 3);
    assert!(mgr.is_local_transmitting(1.1));

    // Deliver frame 0, drop frame 1, deliver frame 2 out-of-order after priming with 3-frame burst:
    // jitter buffer emits decoded frame 0 + synthesized PLC for missing frame 1 + decoded frame 2.
    let mut rx = VoiceManager::new();
    rx.handle_event(
        VoiceEvent::Init {
            codec: "voice_opus".to_string(),
            quality: 4,
        },
        0.0,
    );
    rx.handle_event(
        VoiceEvent::Data {
            player_slot: 2,
            data: pkts[0].clone(),
        },
        1.0,
    );
    rx.handle_event(
        VoiceEvent::Data {
            player_slot: 2,
            data: pkts[2].clone(),
        },
        1.02,
    );
    let decoded = rx.drain_decoded(true);
    assert_eq!(decoded.len(), 3, "frame 0 + PLC frame 1 + frame 2 expected");
    assert!(decoded.iter().all(|(slot, rate, _)| *slot == 2 && *rate == VOICE_SAMPLE_RATE));
    assert!(rx.active_speakers(1.05).contains(&2));

    // When `voice_enable = false`, drain yields empty playback.
    rx.handle_event(
        VoiceEvent::Data {
            player_slot: 3,
            data: pkts[0].clone(),
        },
        1.1,
    );
    assert!(rx.drain_decoded(false).is_empty());
}

#[test]
fn voice_goldsrc_compat_steam_opus_miles_and_speex() {
    // 1. SteamPipe OPUS_PLC header + CRC32 + multi-chunk decode + VoiceManager ingest.
    let fake_opus = vec![0xF8u8, 0x01, 0x02, 0x03, 0x04];
    let pkt = encode_steam_opus_plc(76561197960265728, 24_000, 7, &fake_opus);
    let (rate, chunks) = decode_steam_opus_plc(&pkt).expect("valid OPUS_PLC packet");
    assert_eq!(rate, 24_000);
    assert_eq!(chunks, vec![(7, fake_opus.as_slice())]);

    let mut corrupted = pkt.clone();
    corrupted[10] ^= 0xFF;
    assert!(decode_steam_opus_plc(&corrupted).is_none(), "bad CRC must fail");

    // 2. voice_miles 11025 Hz 4-bit IMA-ADPCM roundtrip + VoiceManager ingest (2 frames prime jitter buffer).
    let src_i16 = sine_i16(11_025, 440.0, 1024, 0.5);
    let adpcm = encode_ima_adpcm(&src_i16[..512]);
    let back = decode_ima_adpcm(&adpcm, Some(512)).expect("decode IMA-ADPCM");
    assert_eq!(back.len(), 512);

    let mut miles = VoiceManager::new();
    miles.handle_event(
        VoiceEvent::Init {
            codec: "voice_miles".to_string(),
            quality: 3,
        },
        0.0,
    );
    assert_eq!(miles.codec(), VoiceCodecKind::MilesAdpcm);
    let mic_f32: Vec<f32> = src_i16.iter().map(|&s| f32::from(s) / 32768.0).collect();
    miles.push_mic_samples(&mic_f32, 11_025, 0.5, true, true, 0.5, 5.0);
    let miles_pcm = miles.drain_decoded(true);
    assert!(
        miles_pcm.iter().any(|(slot, rate, s)| *slot == 255 && *rate == 11_025 && !s.is_empty()),
        "voice_miles loopback must produce 11025 Hz PCM"
    );

    // 3. voice_speex narrowband framing & PLC fallback.
    let framed = frame_speex_nb_packet(3, 42, &[0x11; 28]);
    let framed2 = frame_speex_nb_packet(3, 43, &[0x11; 28]);
    assert_eq!(parse_speex_nb_packet(&framed), Some((3, 42, [0x11; 28].as_slice())));
    let mut speex = VoiceManager::new();
    speex.handle_event(
        VoiceEvent::Init {
            codec: "voice_speex".to_string(),
            quality: 3,
        },
        0.0,
    );
    assert_eq!(speex.codec(), VoiceCodecKind::SpeexNb { quality: 3 });
    speex.handle_event(VoiceEvent::Data { player_slot: 4, data: framed }, 0.2);
    speex.handle_event(VoiceEvent::Data { player_slot: 4, data: framed2 }, 0.22);
    let speex_out = speex.drain_decoded(true);
    assert!(speex_out.iter().any(|(slot, rate, s)| *slot == 4 && *rate == 8_000 && s.len() == 160));
}

#[test]
fn classic_audio_defaults_and_resampling_and_sound_list() {
    let pcm = sine_i16(22_050, 440.0, 2_000, 0.7);
    let wav = encode_test_wav_pcm16(22_050, 1, &pcm, None, None);

    // Classic GoldSrc s_occfactor default 0.0 means occlusion factor starts at 0.0 (no attenuation).
    let mut mixer = GoldSrcMixer::new();
    mixer.set_volumes(1.0, 0.25);
    mixer.set_room_off(true);
    let listener = ListenerState {
        origin: Vec3::ZERO,
        right: Vec3::X,
        local_entity: Some(1),
        waterlevel: 0,
    };
    mixer.update_listener_and_entities(listener, &HashMap::new(), None);
    mixer.insert_cached_wav("weapons/ak47-1.wav", &wav);

    let start = SoundStart::by_name("weapons/ak47-1.wav", 1, 2, Vec3::new(60.0, 0.0, 0.0), 1.0, 0.0, 100);
    mixer.start_sound(None, &start, true, None);

    let list = mixer.format_sound_list();
    assert!(list.contains("weapons/ak47-1.wav"), "{list}");

    let mut out_occ0 = vec![0.0f32; 512 * 2];
    mixer.mix_frames(&mut out_occ0);
    assert!(rms(&out_occ0) > 0.1);

    // Classic default occlusion_factor = 0.0 produces identical gain whether occluded=true or false.
    let mut mixer2 = GoldSrcMixer::new();
    mixer2.set_volumes(1.0, 0.25);
    mixer2.set_room_off(true);
    mixer2.update_listener_and_entities(listener, &HashMap::new(), None);
    mixer2.insert_cached_wav("weapons/ak47-1.wav", &wav);
    mixer2.start_sound(None, &start, false, None);
    let mut out_unocc = vec![0.0f32; 512 * 2];
    mixer2.mix_frames(&mut out_unocc);
    assert!((rms(&out_occ0) - rms(&out_unocc)).abs() < 1e-4);

    // Non-lerping & low-sound (11025 Hz cap) path still produces clean audible audio.
    mixer.set_resampling(false, false);
    mixer.start_sound(None, &start, false, None);
    let mut out_low = vec![0.0f32; 512 * 2];
    mixer.mix_frames(&mut out_low);
    assert!(rms(&out_low) > 0.1);
}
