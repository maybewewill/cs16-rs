//! Integration tests proving the live browser Python Mod Playground (`/playground/`) end-to-end
//! against `WebClientState` (`dm_rusthouse` Instant Demo):
//! 1. All 4 starter templates extracted verbatim from `web/playground/playground.js` compile, load,
//!    and hot-reload cleanly in `< 5 ms`.
//! 2. Hooks (`PlayerSpawn`, `PlayerTakeDamage`, `PlayerKilled`, `PlayerPreThink`, `PlayerSay`,
//!    `MenuSelect`) execute live during `WebClientState::step`, mutating state/HUD/VFX and
//!    populating `mod_resmon_json()` + `drain_mod_logs_json()`.
//! 3. Syntax errors return structured errors without crashing or dropping the running match.

use cs_web::state::{SessionMode, WebClientState};

const PLAYGROUND_JS: &str = include_str!("../../../web/playground/playground.js");

/// Extracts the exact Python source string for `template_key` from `STARTER_TEMPLATES` in
/// `web/playground/playground.js`, decoding JS template-literal backslash escapes identically to V8.
fn extract_js_template(key: &str) -> String {
    let marker = format!("  {key}: {{");
    let start = PLAYGROUND_JS
        .find(&marker)
        .unwrap_or_else(|| panic!("missing starter template `{key}` in playground.js"));
    let slice = &PLAYGROUND_JS[start..];
    let code_start = slice
        .find("code: `")
        .unwrap_or_else(|| panic!("missing `code: ` for `{key}` in playground.js"))
        + "code: `".len();
    let bytes = slice.as_bytes();
    let mut i = code_start;
    let mut out = String::new();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'`' {
            break;
        }
        if b == b'\\' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            match next {
                b'\\' => out.push('\\'),
                b'`' => out.push('`'),
                b'$' => out.push('$'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                other => {
                    out.push('\\');
                    out.push(other as char);
                }
            }
            i += 2;
        } else {
            // Preserve UTF-8 cleanly by slicing the char
            let ch = slice[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    assert!(!out.trim().is_empty(), "empty Python template for `{key}`");
    out
}

fn parse_load_ms(json: &str) -> f64 {
    let key = "\"load_ms\":";
    let idx = json
        .find(key)
        .unwrap_or_else(|| panic!("missing load_ms in {json}"));
    let rest = &json[idx + key.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    rest[..end]
        .parse::<f64>()
        .unwrap_or_else(|e| panic!("invalid load_ms in {json}: {e}"))
}

#[test]
fn playground_four_starter_templates_hot_reload_under_5ms_and_run_in_instant_demo() {
    let templates = [
        ("vampire_shockwave", "vampire_shockwave.py"),
        ("golden_ak_damager", "golden_ak_damager.py"),
        ("moon_gravity_parachute", "moon_gravity_parachute.py"),
        ("vip_menu", "vip_menu.py"),
    ];
    let extracted: Vec<(&str, &str, String)> = templates
        .iter()
        .map(|&(id, file)| (id, file, extract_js_template(id)))
        .collect();

    let mut st = WebClientState::default();

    // 1. Pre-queue `vip_menu` while Idle -> should validate cleanly with queued=true.
    let queued_json = st
        .reload_python_plugin("vip_menu.py", &extracted[3].2)
        .expect("queue vip_menu before Instant Demo");
    assert!(
        queued_json.contains("\"queued\":true"),
        "json={queued_json}"
    );
    assert!(
        queued_json.contains("\"id\":\"vip_menu\""),
        "json={queued_json}"
    );
    let idle_resmon = st.mod_resmon_json();
    assert!(
        idle_resmon.contains("\"status\":\"Queued\""),
        "resmon={idle_resmon}"
    );

    // 2. Start Instant Demo (`dm_rusthouse` 5v5 bots) -> queued `vip_menu` runs on tick 0!
    let (files, bytes) = st
        .start_instant_demo(9)
        .expect("start_instant_demo with queued Python plugin");
    assert!(files > 0 && bytes > 0);
    assert!(st.is_session_active());
    for _ in 0..8 {
        let _ = st.step(0.01);
    }

    // Trigger `/vip` via exec_command -> GoldSrc \yVIP Playground Menu opens!
    st.exec_command("say /vip");
    for _ in 0..8 {
        let _ = st.step(0.01);
    }
    let menu_text = match &st.session {
        SessionMode::Local { game, .. } => game
            .connection()
            .fx()
            .hud
            .menu
            .as_ref()
            .map(|m| m.text.clone())
            .unwrap_or_default(),
        _ => panic!("expected Local session"),
    };
    assert!(
        menu_text.contains("VIP Playground Menu"),
        "expected VIP menu on HUD after exec_command('say /vip'), got: {menu_text:?}"
    );

    // Pick menu item 1 (`+150 HP & 100 VestHelm`) via `menuselect 1`.
    st.exec_command("menuselect 1");
    for _ in 0..8 {
        let _ = st.step(0.01);
    }
    let (hp, armor) = match &st.session {
        SessionMode::Local { game, .. } => {
            let p = game.cs_game().state().players[0]
                .as_ref()
                .expect("player 0");
            (p.health, p.armor)
        }
        _ => panic!("expected Local session"),
    };
    assert!((hp - 150.0).abs() < 0.5, "expected 150 HP, got {hp}");
    assert!(
        (armor - 100.0).abs() < 0.5,
        "expected 100 armor, got {armor}"
    );

    // Test quick-action `give_money 16000`.
    st.exec_command("give_money 16000");
    let money = match &st.session {
        SessionMode::Local { game, .. } => {
            game.cs_game().state().players[0].as_ref().unwrap().money
        }
        _ => panic!("expected Local session"),
    };
    assert_eq!(money, 16000);

    // 3. Hot-reload ALL 4 starter templates live into running Instant Demo and verify `< 5 ms`!
    // Warm up once, then measure every template cleanly.
    let mut timings = Vec::new();
    for (id, file, code) in &extracted {
        let json = st
            .reload_python_plugin(file, code)
            .unwrap_or_else(|e| panic!("failed to reload starter template `{file}`: {e}"));
        assert!(
            json.contains("\"queued\":false"),
            "expected live reload: {json}"
        );
        assert!(
            json.contains(&format!("\"id\":\"{id}\"")),
            "expected id `{id}` in {json}"
        );
        let ms = parse_load_ms(&json);
        timings.push((*file, ms));
        assert!(
            ms < 5.0,
            "hot-reload for `{file}` took {ms:.3} ms (must be < 5.0 ms!)"
        );
    }
    eprintln!("Playground hot-reload latencies: {timings:?}");

    // 4. Exercise chat hooks on the active templates (respecting 0.66s GoldSrc say anti-flood) & verify ResMon + Logs.
    for cmd in ["say /vamp", "say /gold", "say /moon"] {
        for _ in 0..75 {
            let _ = st.step(0.01);
        }
        st.exec_command(cmd);
        for _ in 0..8 {
            let _ = st.step(0.01);
        }
    }

    let resmon = st.mod_resmon_json();
    for (id, _, _) in &extracted {
        assert!(
            resmon.contains(&format!("\"id\":\"{id}\"")),
            "missing `{id}` in ResMon JSON: {resmon}"
        );
    }
    assert!(resmon.contains("\"active\":true"), "resmon={resmon}");
    assert!(
        !resmon.contains("\"hook_calls\":0"),
        "every loaded starter template should have fired >= 1 hook: {resmon}"
    );

    let logs = st.drain_mod_logs_json();
    assert!(logs.starts_with('[') && logs.ends_with(']'), "logs={logs}");
    assert!(
        logs.contains("Hot-reloaded"),
        "expected reload notices in logs: {logs}"
    );

    // 5. Syntax error during live edit returns clean error without crashing or dropping match!
    let bad_err = st
        .reload_python_plugin("vampire_shockwave.py", "def broken_syntax(:\n    pass\n")
        .expect_err("syntax error must be rejected");
    assert!(
        bad_err.to_ascii_lowercase().contains("syntax") || bad_err.contains("line"),
        "expected structured syntax error, got: {bad_err}"
    );
    assert!(
        st.is_session_active(),
        "match must remain active after syntax error"
    );
    let err_logs = st.drain_mod_logs_json();
    assert!(
        err_logs.contains("\"level\":\"error\""),
        "err_logs={err_logs}"
    );

    // 6. Unload all 4 cleanly.
    for (id, _, _) in &extracted {
        assert!(st.unload_python_plugin(id), "failed to unload `{id}`");
    }

    // 7. Verify both `player.hud(...)` (TE_TEXTMESSAGE) and `player.dhud(...)` (SVC_DIRECTOR
    //    DRC_CMD_MESSAGE) propagate through ViewBuilder into `fx.hud.te_text_messages` for
    //    Native + Browser HUD rendering.
    let hud_probe_py = r#"from cs16 import mod, HookAction

@mod.info(id="hud_probe", version="1.0.0", permissions=["messages", "commands"])
def _meta():
    pass

@mod.chat_command("/hudprobe")
def cmd_probe(player, args):
    player.hud("HUD_LINE_OK", x=-1.0, y=0.35, channel=2, rgba=(255, 200, 50, 255), hold=3.0)
    player.dhud("DHUD_BANNER_OK", x=-1.0, y=0.22, rgb=(255, 80, 40), hold=3.0)
    return HookAction.SUPERCEDE
"#;
    st.reload_python_plugin("hud_probe.py", hud_probe_py)
        .expect("load hud_probe");
    for _ in 0..75 {
        let _ = st.step(0.01);
    }
    st.exec_command("say /hudprobe");
    for _ in 0..10 {
        let _ = st.step(0.01);
    }
    let te_texts: Vec<String> = match &st.session {
        SessionMode::Local { game, .. } => game
            .connection()
            .fx()
            .hud
            .te_text_messages
            .iter()
            .map(|m| m.text.clone())
            .collect(),
        _ => panic!("expected Local session"),
    };
    assert!(
        te_texts.iter().any(|t| t == "HUD_LINE_OK"),
        "expected TE_TEXTMESSAGE HUD_LINE_OK in hud.te_text_messages, got: {te_texts:?}"
    );
    assert!(
        te_texts.iter().any(|t| t == "DHUD_BANNER_OK"),
        "expected DRC_CMD_MESSAGE DHUD_BANNER_OK in hud.te_text_messages, got: {te_texts:?}"
    );
    assert!(st.unload_python_plugin("hud_probe"));
}
