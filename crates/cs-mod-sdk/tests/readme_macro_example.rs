#![forbid(unsafe_code)]

//! Compile and runtime verification for the `#![forbid(unsafe_code)]` Rust WASM mod examples
//! documented in `README.md` and `docs/MODDING.md`.

use cs_mod_sdk::{HookAction, HookId, HookPhase, MockEntity, MockHostBridge, Player, VfxOp};

mod readme_example {
    use cs_mod_sdk::{
        HookAction, HookId, HookPhase, HostBridge, Player, cs_mod_hook, cs_mod_init, vfx,
    };

    cs_mod_sdk::cs16_wasm_host_bridge!();

    #[cs_mod_init]
    pub fn init_mod(host: &mut impl HostBridge) -> i32 {
        host.hook_register(HookId::PlayerSpawn, HookPhase::Post, 0);
        0
    }

    #[cs_mod_hook(PlayerSpawn, post)]
    pub fn on_player_spawn(host: &mut impl HostBridge) -> HookAction {
        let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
        if let Some(pos) = Player::new(slot).origin(host) {
            vfx::shockwave_ring(host, pos, 160.0, [255, 176, 0, 220], 6, 12, None);
        }
        HookAction::Continue
    }

    cs_mod_sdk::cs16_wasm_exports! {
        init: init_mod,
        hooks: [(HookId::PlayerSpawn, HookPhase::Post) => on_player_spawn],
    }
}

mod modding_guide_example {
    use cs_mod_sdk::{
        CustomWeaponBuilder, HookAction, HookId, HookPhase, HostBridge, Player, cs_mod_hook,
        cs_mod_init, vfx, weapon_id,
    };

    cs_mod_sdk::cs16_wasm_host_bridge!();

    #[cs_mod_init]
    pub fn init_mod(host: &mut impl HostBridge) -> i32 {
        CustomWeaponBuilder::new("golden_ak47", weapon_id::AK47)
            .display_name("Golden AK-47")
            .damage_mult(1.5)
            .clip_size(35)
            .knockback(1.8)
            .register(host);
        host.hook_register(HookId::PlayerSpawn, HookPhase::Post, 0);
        0
    }

    #[cs_mod_hook(PlayerSpawn, post)]
    pub fn on_player_spawn(host: &mut impl HostBridge) -> HookAction {
        let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
        let p = Player::new(slot);
        p.give_custom_weapon(host, "golden_ak47");
        if let Some(pos) = p.origin(host) {
            vfx::shockwave_ring(host, pos, 180.0, [255, 200, 0, 220], 8, 12, None);
        }
        HookAction::Continue
    }

    cs_mod_sdk::cs16_wasm_exports! {
        init: init_mod,
        hooks: [(HookId::PlayerSpawn, HookPhase::Post) => on_player_spawn],
    }
}

fn mock_host_with_spawned_player(slot: u8, origin: [f32; 3]) -> MockHostBridge {
    let mut host = MockHostBridge::default();
    host.add_player(slot, "Player");
    let edict = Player::new(slot).edict_index();
    host.entities.insert(
        edict,
        MockEntity {
            index: edict,
            classname: "player".to_string(),
            health: 100.0,
            origin,
            velocity: [0.0; 3],
            flags: 0,
        },
    );
    host.active_i32_args[0] = i32::from(slot);
    host
}

#[test]
fn readme_macro_example_compiles_forbid_unsafe_and_emits_shockwave() {
    assert_eq!(readme_example::CS_MOD_INIT_SYMBOL, "init_mod");
    assert_eq!(
        readme_example::CS_MOD_HOOK_ON_PLAYER_SPAWN,
        ("player_spawn", "post")
    );
    assert_eq!(
        readme_example::CS_MOD_HOOK_ON_PLAYER_SPAWN_SPEC,
        ("player_spawn", "post", 0)
    );

    let spawn_origin = [128.0, -64.0, 32.0];
    let mut host = mock_host_with_spawned_player(0, spawn_origin);
    assert_eq!(readme_example::init_mod(&mut host), 0);
    assert_eq!(host.hooks, vec![(HookId::PlayerSpawn, HookPhase::Post, 0)]);

    assert_eq!(
        readme_example::on_player_spawn(&mut host),
        HookAction::Continue
    );
    assert_eq!(
        host.vfx_ops,
        vec![VfxOp::ShockwaveRing {
            origin: spawn_origin,
            radius: 160.0,
            rgba: [255, 176, 0, 220],
            life_decisec: 6,
            width: 12,
            slot: None,
        }]
    );
}

#[test]
fn modding_guide_example_registers_custom_weapon_and_handles_spawn() {
    assert_eq!(modding_guide_example::CS_MOD_INIT_SYMBOL, "init_mod");
    assert_eq!(
        modding_guide_example::CS_MOD_HOOK_ON_PLAYER_SPAWN,
        ("player_spawn", "post")
    );
    assert_eq!(
        modding_guide_example::CS_MOD_HOOK_ON_PLAYER_SPAWN_SPEC,
        ("player_spawn", "post", 0)
    );

    let spawn_origin = [256.0, 512.0, 0.0];
    let mut host = mock_host_with_spawned_player(0, spawn_origin);
    assert_eq!(modding_guide_example::init_mod(&mut host), 0);
    assert_eq!(host.hooks, vec![(HookId::PlayerSpawn, HookPhase::Post, 0)]);
    assert_eq!(host.custom_weapons.len(), 1);
    assert_eq!(host.custom_weapons[0].id_name, "golden_ak47");
    assert_eq!(host.custom_weapons[0].clip_size, Some(35));

    assert_eq!(
        modding_guide_example::on_player_spawn(&mut host),
        HookAction::Continue
    );
    assert_eq!(
        host.given_custom_weapons,
        vec![(0, "golden_ak47".to_string())]
    );
    assert_eq!(
        host.vfx_ops,
        vec![VfxOp::ShockwaveRing {
            origin: spawn_origin,
            radius: 180.0,
            rgba: [255, 200, 0, 220],
            life_decisec: 8,
            width: 12,
            slot: None,
        }]
    );
}

#[test]
fn rust_scaffold_template_uses_safe_wasm_exports_without_raw_unsafe() {
    let files = cs_mod_sdk::template::scaffold_mod_project(
        "hello",
        "Hello Mod",
        cs_mod_sdk::ModSide::Server,
    );
    let lib_rs = &files["src/lib.rs"];
    assert!(lib_rs.contains("cs16_wasm_exports!"));
    assert!(
        !lib_rs.contains("#[unsafe("),
        "scaffolded src/lib.rs should not emit raw #[unsafe(...)] attributes"
    );
}
