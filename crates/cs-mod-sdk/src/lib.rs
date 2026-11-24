//! Safe high-level Rust SDK for authoring `cs16-rs` server and client WASM mods (`#![forbid(unsafe_code)]`).

pub mod template;

pub use cs_mod_api::*;
pub use cs_mod_macros::{cs_mod_hook, cs_mod_hud, cs_mod_init};

use std::collections::BTreeMap;

/// Safe host capability seam implemented by mock harnesses (unit tests) or a WASM import shim.
pub trait HostBridge {
    fn time_ms(&self) -> i64;
    fn print_all(&mut self, text: &str);
    fn center_print(&mut self, slot: Option<u8>, text: &str);
    fn say_text(&mut self, slot: Option<u8>, text: &str);
    fn show_menu(&mut self, slot: u8, keys_mask: u16, time_sec: i8, text: &str);
    fn set_health(&mut self, ent_index: u16, health: f32);
    fn storage_get(&self, key: &str) -> Option<String>;
    fn storage_set(&mut self, key: &str, value: &str);
    fn send_custom_usermsg(&mut self, slot: Option<u8>, name: &str, data: &[u8]);
}

/// Deterministic in-memory [`HostBridge`] for unit testing mod logic without a server.
#[derive(Debug, Default)]
pub struct MockHostBridge {
    pub now_ms: i64,
    pub prints: Vec<String>,
    pub center_prints: Vec<(Option<u8>, String)>,
    pub say_texts: Vec<(Option<u8>, String)>,
    pub menus: Vec<(u8, u16, i8, String)>,
    pub health_sets: BTreeMap<u16, f32>,
    pub storage: BTreeMap<String, String>,
    pub usermsgs: Vec<(Option<u8>, String, Vec<u8>)>,
}

impl HostBridge for MockHostBridge {
    fn time_ms(&self) -> i64 {
        self.now_ms
    }

    fn print_all(&mut self, text: &str) {
        self.prints.push(text.to_string());
    }

    fn center_print(&mut self, slot: Option<u8>, text: &str) {
        self.center_prints.push((slot, text.to_string()));
    }

    fn say_text(&mut self, slot: Option<u8>, text: &str) {
        self.say_texts.push((slot, text.to_string()));
    }

    fn show_menu(&mut self, slot: u8, keys_mask: u16, time_sec: i8, text: &str) {
        self.menus.push((slot, keys_mask, time_sec, text.to_string()));
    }

    fn set_health(&mut self, ent_index: u16, health: f32) {
        self.health_sets.insert(ent_index, health);
    }

    fn storage_get(&self, key: &str) -> Option<String> {
        self.storage.get(key).cloned()
    }

    fn storage_set(&mut self, key: &str, value: &str) {
        self.storage.insert(key.to_string(), value.to_string());
    }

    fn send_custom_usermsg(&mut self, slot: Option<u8>, name: &str, data: &[u8]) {
        self.usermsgs.push((slot, name.to_string(), data.to_vec()));
    }
}

/// Fluent builder for classic GoldSrc `ShowMenu` screens.
#[derive(Debug, Default, Clone)]
pub struct MenuBuilder {
    title: String,
    items: Vec<(u8, String)>,
    time_sec: i8,
}

impl MenuBuilder {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
            time_sec: 15,
        }
    }

    pub fn display_time(mut self, seconds: i8) -> Self {
        self.time_sec = seconds;
        self
    }

    pub fn item(mut self, key_1_to_9: u8, label: impl Into<String>) -> Self {
        if (1..=9).contains(&key_1_to_9) {
            self.items.push((key_1_to_9, label.into()));
        }
        self
    }

    pub fn build(&self) -> (u16, i8, String) {
        let mut mask = 0u16;
        let mut text = format!("\\y{}\\w\n", self.title);
        for (k, label) in &self.items {
            mask |= 1u16 << (k - 1);
            text.push_str(&format!("{k}. {label}\n"));
        }
        mask |= 1u16 << 9;
        text.push_str("\n0. Exit\n");
        (mask, self.time_sec, text)
    }

    pub fn send_to(&self, bridge: &mut impl HostBridge, slot: u8) {
        let (mask, time_sec, text) = self.build();
        bridge.show_menu(slot, mask, time_sec, &text);
    }
}

/// Immediate-mode HUD builder collecting [`HudDrawCmd`]s for client mods.
#[derive(Debug, Default, Clone)]
pub struct HudCanvas {
    cmds: Vec<HudDrawCmd>,
}

impl HudCanvas {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn panel(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4], title: impl Into<String>) {
        self.cmds.push(HudDrawCmd::Panel {
            x,
            y,
            w,
            h,
            rgba,
            title: title.into(),
        });
    }

    pub fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, fraction: f32, rgba: [u8; 4]) {
        self.cmds.push(HudDrawCmd::ProgressBar {
            x,
            y,
            w,
            h,
            fraction: fraction.clamp(0.0, 1.0),
            rgba,
        });
    }

    pub fn text(&mut self, x: f32, y: f32, size: f32, rgba: [u8; 4], text: impl Into<String>) {
        self.cmds.push(HudDrawCmd::Text {
            x,
            y,
            size,
            rgba,
            text: text.into(),
        });
    }

    pub fn finish(self) -> Vec<HudDrawCmd> {
        self.cmds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cs_mod_init]
    fn on_init() -> i32 {
        0
    }

    #[cs_mod_hook(start_frame, pre)]
    fn on_frame() -> HookAction {
        HookAction::Continue
    }

    #[cs_mod_hud]
    fn on_hud(canvas: &mut HudCanvas) {
        canvas.panel(0.02, 0.2, 0.2, 0.1, [30, 40, 30, 200], "SDK");
    }

    #[test]
    fn macros_and_sdk_builders_compile_and_scaffold_valid_manifest() {
        assert_eq!(CS_MOD_INIT_SYMBOL, "on_init");
        assert_eq!(CS_MOD_HOOK_ON_FRAME, ("start_frame", "pre"));
        assert_eq!(CS_MOD_HUD_SYMBOL, "on_hud");
        assert_eq!(on_init(), 0);
        assert_eq!(on_frame(), HookAction::Continue);

        let mut bridge = MockHostBridge::default();
        MenuBuilder::new("Admin")
            .item(1, "Kick")
            .item(2, "Slay")
            .send_to(&mut bridge, 0);
        assert_eq!(bridge.menus.len(), 1);
        assert_eq!(bridge.menus[0].1 & 0b11, 0b11);

        let mut canvas = HudCanvas::new();
        on_hud(&mut canvas);
        let cmds = canvas.finish();
        assert_eq!(cmds.len(), 1);

        let files = template::scaffold_mod_project("my_mod", "My Mod", ModSide::Both);
        let parsed = ModManifest::parse_toml(&files["mod.toml"]).unwrap();
        assert_eq!(parsed.id, "my_mod");
        assert_eq!(parsed.side, ModSide::Both);
        assert!(files.contains_key("src/lib.rs"));
    }
}
