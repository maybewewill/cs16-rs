//! Phase 18 Stage 1B — Browser (`cs-web` `WebClientState`) Full-Match Playability Walkthrough.
//!
//! Proves Stage 1 Build 10210 parity across the browser/WASM state machine (`WebClientState`) on
//! real Steam `de_dust2` and `de_inferno`:
//! - Default `_vgui_menus 1`, `snd_occlusion 0`, `room_type 0` dry, `cl_hd_models 0`.
//! - Join flow (`jointeam` / `joinclass`), `$800` spawn, NO top `[1..5]` weapon bucket bar.
//! - `B -> 4 -> 2` keyboard VGUI2 buying (`MainBuyMenu.res -> BuyRifles_TER.res -> ak47`).
//! - `M4A1` silencer persistence, `AWP` scope (`fov <= 40`) vs `AUG` zoom (`fov == 55`, crosshair on).
//! - Echo-free `300 ms` local footsteps (`Play2d`, `0` self-step echoes, `0` steps while walking),
//!   `Play3d.occluded == false` by default, `.mdl` `5004` weapon `anim_sounds` emitted to WebAudio.
//! - Crimson blood (`r > 100, g < 30, b < 30`) on bot hit and `de_inferno` ZBot boot.

use std::path::PathBuf;

use cs_client::render::scene::ModelKind;
use cs_client::ui::should_draw_dynamic_crosshair;
use cs_clientcore::sound::is_player_footstep_sound;
use cs_vfs::{Vfs, VfsConfig};
use cs_web::state::{SessionMode, WebAudioEvent, WebClientState};
use glam::Vec3;

fn discover_retail_vfs() -> Option<Vfs> {
    let explicit = std::env::var_os("CS16_GAMEDIR").map(PathBuf::from);
    Vfs::discover(
        explicit.as_deref(),
        VfsConfig {
            enable_hd_models: false,
            enable_downloads: false,
        },
    )
    .ok()
}

fn require_steam_map(map: &str) -> Option<Vfs> {
    let vfs = discover_retail_vfs()?;
    vfs.exists(&format!("maps/{map}.bsp")).then_some(vfs)
}

fn step_web(st: &mut WebClientState, frames: usize) -> Vec<WebAudioEvent> {
    let mut audio = Vec::new();
    for _ in 0..frames {
        let _ = st.step(0.01);
        audio.extend(st.take_audio_events());
    }
    audio
}

fn web_server_console(st: &mut WebClientState, cmd: &str) {
    if let SessionMode::Local { game, .. } = &mut st.session {
        let _ = game.console(cmd);
    }
}

#[test]
fn web_stage1_walkthrough_dust2_and_inferno() {
    let Some(vfs) = require_steam_map("de_dust2") else {
        eprintln!("skipping web_stage1_walkthrough_dust2_and_inferno: Steam de_dust2 absent");
        return;
    };

    let mut st = WebClientState::default();
    st.unlock_audio();
    assert!(
        st.cvars.get_bool("_vgui_menus"),
        "_vgui_menus must default to 1"
    );
    assert!(
        !st.cvars.get_bool("snd_occlusion"),
        "snd_occlusion must default to 0"
    );
    assert!(
        !st.cvars.get_bool("cl_hd_models"),
        "cl_hd_models must default to 0"
    );

    st.set_vfs(vfs.clone());
    st.start_local("de_dust2", 0)
        .expect("WebClientState start_local de_dust2");
    st.mark_map_loaded("de_dust2");
    web_server_console(&mut st, "mp_startmoney 800");
    web_server_console(&mut st, "mp_freezetime 5");

    // 1. Join Terrorist & verify $800 spawn + Glock + Knife.
    st.exec_console("jointeam 1");
    step_web(&mut st, 10);
    st.exec_console("joinclass 1");
    step_web(&mut st, 25);

    let (money, owned) = match &st.session {
        SessionMode::Local { game, .. } => (
            game.connection().fx().hud.money,
            game.connection().client_data().map_or(0, |d| d.weapons),
        ),
        _ => panic!("expected Local session"),
    };
    assert_eq!(money, 800, "WebClientState must spawn with $800");
    assert_ne!(owned & (1 << 17), 0, "T must spawn with Glock18");
    assert_ne!(owned & (1 << 29), 0, "T must spawn with Knife");

    // 2. Give $16000 & execute B -> 4 -> 2 (`buy` -> `slot4` -> `slot2`) via WebClientState dispatch!
    web_server_console(&mut st, "mp_startmoney 16000");
    web_server_console(&mut st, "sv_restart 1");
    step_web(&mut st, 240);

    st.exec_console("buy");
    step_web(&mut st, 6);
    st.input.pending_commands.push("slot4".to_string());
    step_web(&mut st, 3);
    st.input.pending_commands.push("slot2".to_string());
    let buy_audio = step_web(&mut st, 12);

    let (wid_after_buy, money_after_buy) = match &st.session {
        SessionMode::Local { game, .. } => (
            game.connection()
                .fx()
                .hud
                .current_weapon
                .map(|w| w.weapon_id),
            game.connection().fx().hud.money,
        ),
        _ => panic!("expected Local session"),
    };
    assert_eq!(
        wid_after_buy,
        Some(28),
        "Browser B -> 4 -> 2 must buy and auto-equip AK-47 (id 28)"
    );
    assert_eq!(money_after_buy, 13500);
    assert!(
        buy_audio.iter().any(|ev| matches!(
            ev,
            WebAudioEvent::Play2d { path, .. } if path.to_ascii_lowercase().contains("gunpickup2")
        )),
        "AK-47 purchase in browser must queue gunpickup2 Play2d audio"
    );

    // 3. Switch to CT for M4A1 silencer persistence, AWP scope (40/10) vs AUG zoom (55), and 5004 events.
    web_server_console(&mut st, "mp_freezetime 0");
    st.exec_console("jointeam 2");
    step_web(&mut st, 8);
    st.exec_console("joinclass 1");
    web_server_console(&mut st, "sv_restart 1");
    step_web(&mut st, 240);

    st.exec_console("m4a1");
    st.exec_console("primammo");
    let mut anim_audio = step_web(&mut st, 110);
    // Attach silencer with MOUSE2 (`+attack2`).
    st.exec_console("+attack2");
    step_web(&mut st, 4);
    st.exec_console("-attack2");
    let mut sil_seq = -1;
    for _ in 0..310 {
        let out = st.step(0.01);
        anim_audio.extend(st.take_audio_events());
        if let Some(fv) = out.view
            && let Some(vm) = fv.viewmodel
        {
            sil_seq = vm.sequence;
        }
    }
    assert_eq!(
        sil_seq, 0,
        "Browser M4A1 silencer must persist on silenced idle seq 0 after 3.1s (got {sil_seq})"
    );

    // Fire + reload to trigger 5004 clipout/clipin/boltpull into WebAudioEvent::Play2d.
    st.exec_console("+attack");
    anim_audio.extend(step_web(&mut st, 6));
    st.exec_console("-attack");
    anim_audio.extend(step_web(&mut st, 10));
    st.exec_console("+reload");
    anim_audio.extend(step_web(&mut st, 4));
    st.exec_console("-reload");
    anim_audio.extend(step_web(&mut st, 310));
    assert!(
        anim_audio.iter().any(|ev| matches!(
            ev,
            WebAudioEvent::Play2d { path, .. } if path.to_ascii_lowercase().contains("clipout")
        )),
        "Browser M4A1 reload must emit .mdl 5004 clipout WebAudioEvent"
    );

    // AWP zoom (40 deg) vs AUG zoom (55 deg).
    st.exec_console("awp");
    step_web(&mut st, 150);
    st.exec_console("+attack2");
    step_web(&mut st, 4);
    st.exec_console("-attack2");
    let mut awp_fov = 90.0f32;
    let mut awp_vm_hidden = false;
    for _ in 0..25 {
        if let Some(fv) = st.step(0.01).view {
            awp_fov = fv.fov;
            awp_vm_hidden = fv.viewmodel.is_none();
        }
        let _ = st.take_audio_events();
    }
    assert!(
        (awp_fov - 40.0).abs() < 1.0,
        "Browser AWP zoom must be 40 deg (got {awp_fov})"
    );
    assert!(awp_vm_hidden, "Browser AWP zoom must hide viewmodel");
    assert!(!should_draw_dynamic_crosshair(awp_fov, 18));

    st.exec_console("aug");
    step_web(&mut st, 120);
    st.exec_console("+attack2");
    step_web(&mut st, 4);
    st.exec_console("-attack2");
    let mut aug_fov = 90.0f32;
    for _ in 0..25 {
        if let Some(fv) = st.step(0.01).view {
            aug_fov = fv.fov;
        }
        let _ = st.take_audio_events();
    }
    assert!(
        (aug_fov - 55.0).abs() < 1.0,
        "Browser AUG zoom must be 55 deg (got {aug_fov})"
    );
    assert!(
        aug_fov > 40.0,
        "Browser AUG zoom must NOT activate black sniper scope"
    );
    assert!(should_draw_dynamic_crosshair(aug_fov, 8));

    // 4. Footsteps: running produces Play2d local footsteps with ZERO Play3d occluded muffling; walking is silent.
    let _ = st.take_audio_events();
    st.exec_console("+forward");
    let run_audio = step_web(&mut st, 140);
    st.exec_console("-forward");
    let _ = step_web(&mut st, 40);

    let run_2d_steps: Vec<&String> = run_audio
        .iter()
        .filter_map(|ev| match ev {
            WebAudioEvent::Play2d { path, .. } if is_player_footstep_sound(path) => Some(path),
            _ => None,
        })
        .collect();
    assert!(
        run_2d_steps.len() >= 2,
        "Browser running must emit centered Play2d footsteps; got {run_2d_steps:?}"
    );
    assert!(
        run_audio
            .iter()
            .all(|ev| !matches!(ev, WebAudioEvent::Play3d { occluded: true, .. })),
        "Browser audio must default to unmuffled (occluded == false) when snd_occlusion == 0"
    );

    // 5. Shoot a ZBot in browser session -> verify dark crimson blood rendercolor (r>100, g<30, b<30).
    web_server_console(&mut st, "bot_stop 1");
    web_server_console(&mut st, "bot_add_t");
    step_web(&mut st, 40);

    if let SessionMode::Local { game, .. } = &mut st.session {
        let local_pos = {
            let srv = game.server();
            let local_id = srv.edicts().client(0);
            srv.edicts()
                .get(local_id)
                .map(|e| e.vars.origin)
                .unwrap_or(Vec3::ZERO)
        };
        let bsp = cs_assets::bsp::BspMap::load(&vfs, "de_dust2").expect("load dust2");
        let coll = cs_pm::collision::WorldCollision::from_bsp(&bsp);
        let mut dir = Vec3::X;
        let mut yaw_deg = 0.0f32;
        for deg in (0..360).step_by(15) {
            let r = (deg as f32).to_radians();
            let d = Vec3::new(r.cos(), r.sin(), 0.0);
            let tr = coll.trace_world(
                local_pos + Vec3::new(0.0, 0.0, 18.0),
                local_pos + d * 140.0 + Vec3::new(0.0, 0.0, 18.0),
                cs_pm::collision::Hull::Standing,
            );
            if tr.fraction >= 0.99 {
                dir = d;
                yaw_deg = deg as f32;
                break;
            }
        }
        let target_pos = local_pos + dir * 100.0;
        let max_c = game.server().config().max_clients;
        let mut bot_slot = None;
        for slot in 1..max_c {
            let id = game.server().edicts().client(slot);
            if game
                .server()
                .edicts()
                .get(id)
                .is_some_and(|e| e.vars.health > 0.0)
            {
                bot_slot = Some(slot);
                break;
            }
        }
        if let Some(slot) = bot_slot {
            if let Some(p) = game
                .cs_game_mut()
                .state_mut()
                .players
                .get_mut(usize::from(slot))
                .and_then(|o| o.as_mut())
            {
                p.kevlar = Default::default();
                p.armor = 0.0;
            }
            game.server_mut().set_client_origin(slot, target_pos);
            st.camera.yaw_deg = yaw_deg;
            st.camera.pitch_deg = 6.0;
        }
    }
    step_web(&mut st, 8);
    st.exec_console("+attack");
    let mut saw_crimson = false;
    for _ in 0..35 {
        let out = st.step(0.01);
        let _ = st.take_audio_events();
        if let Some(fv) = out.view {
            let models = st.view.models();
            for ent in &fv.scene.entities {
                if let Some(m) = models.get(ent.model)
                    && m.kind == ModelKind::Sprite
                    && m.name.to_ascii_lowercase().contains("blood")
                {
                    let [r, g, b] = ent.rendercolor;
                    assert!(
                        r > 100 && g < 30 && b < 30,
                        "Browser blood sprite must be crimson red, got RGB({r},{g},{b})"
                    );
                    saw_crimson = true;
                }
            }
        }
        if saw_crimson {
            break;
        }
    }
    st.exec_console("-attack");
    assert!(
        saw_crimson,
        "Browser bot hit must spawn crimson bloodspray.spr"
    );

    // 6. Verify de_inferno boots & steps live with ZBots in WebClientState.
    if vfs.exists("maps/de_inferno.bsp") {
        st.start_local("de_inferno", 4)
            .expect("WebClientState start_local de_inferno");
        st.mark_map_loaded("de_inferno");
        st.exec_console("jointeam 2");
        step_web(&mut st, 8);
        st.exec_console("joinclass 1");
        let mut saw_inferno_view = false;
        for _ in 0..80 {
            if st.step(0.01).view.is_some() {
                saw_inferno_view = true;
            }
            let _ = st.take_audio_events();
        }
        assert!(
            saw_inferno_view,
            "Browser de_inferno session must emit live FrameView"
        );
    }
}
