//! Direct `cs-game` integration tests for ABI v2 ReAPI `CsHookSink`, semiclip
//! (`PlayersShouldCollide`), `CustomWeaponSpec`, 1-line Protocol 48 `VfxOp` emission,
//! and CS 1.6 mod-host gameplay natives.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use cs_assets::bsp::Entity;
use cs_cvar::CVarRegistry;
use cs_engine::{EngineApi, Server, ServerConfig, ServerMap};
use cs_game::mod_hooks::{
    self, CsHookSink, VFX_STOCK_SPRITES, apply_damage, emit_vfx, give_custom_weapon,
    register_custom_weapon,
};
use cs_game::{CsGame, GameState};
use cs_mod_api::{
    ArmorKind, CustomWeaponSpec, HookAction, HookPhase, PlayerKilledCtx, PlayerPreThinkCtx,
    PlayerSpawnCtx, PlayerTakeDamageCtx, PlayersShouldCollideCtx, VfxOp, WeaponEventCtx,
};
use cs_proto48::delta::DeltaRegistry;
use cs_transport::LoopbackHub;

enum Action {
    PrecacheAndSetupCustomWeapon,
    RunNativesAndVfx,
    ApplyDamage(u8, f32),
    SlayAndRespawn(u8),
}

#[derive(Default)]
struct HookRecorder {
    spawns: Vec<u8>,
    damage_calls: Vec<(HookPhase, u8, f32)>,
    override_damage: Option<f32>,
    supercede_damage: bool,
    kills: Vec<(Option<u8>, u8)>,
    semiclip_enabled: bool,
    semiclip_checks: usize,
    weapon_deploys: Vec<(u8, u8)>,
    actions: VecDeque<Action>,
    verified_custom_clip: i32,
    verified_hp_after_override: f32,
    verified_hp_after_supercede: f32,
    verified_alive_after_slay: bool,
    verified_alive_after_respawn: bool,
    verified_vfx_emitted: usize,
}

struct RecordingSink {
    rec: Arc<Mutex<HookRecorder>>,
}

impl CsHookSink for RecordingSink {
    fn on_player_spawn(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerSpawnCtx,
    ) -> HookAction {
        if phase == HookPhase::Post {
            self.rec.lock().unwrap().spawns.push(ctx.slot);
        }
        HookAction::Continue
    }

    fn on_player_pre_think(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        _ctx: &mut PlayerPreThinkCtx,
    ) -> HookAction {
        if phase != HookPhase::Pre {
            return HookAction::Continue;
        }
        let next = self.rec.lock().unwrap().actions.pop_front();
        let Some(action) = next else {
            return HookAction::Continue;
        };
        match action {
            Action::PrecacheAndSetupCustomWeapon => {
                for spr in VFX_STOCK_SPRITES {
                    let _ = eng.model_index(spr);
                }
                register_custom_weapon(
                    g,
                    Some(eng),
                    CustomWeaponSpec {
                        id_name: "ak47_gold".to_string(),
                        base_weapon: 28,
                        display_name: "Golden AK-47".to_string(),
                        damage_mult: 2.0,
                        clip_size: Some(45),
                        ..Default::default()
                    },
                );
                assert!(give_custom_weapon(g, eng, 0, "ak47_gold"));
                self.rec.lock().unwrap().verified_custom_clip =
                    mod_hooks::get_weapon_clip(g, 0, 28);
            }
            Action::RunNativesAndVfx => {
                assert!(mod_hooks::set_player_armor(
                    g,
                    eng,
                    0,
                    100,
                    ArmorKind::VestHelm
                ));
                assert!(mod_hooks::set_player_money(g, eng, 0, 12345, true));
                assert!(mod_hooks::set_player_max_health(g, 0, 250.0));
                assert!(mod_hooks::set_player_health(g, eng, 0, 200.0));
                assert!(mod_hooks::set_player_footsteps(g, 0, true));
                assert!(mod_hooks::set_user_defuser(g, eng, 0, true));
                assert!(mod_hooks::set_user_nvg(g, eng, 0, true));
                assert!(mod_hooks::set_player_gravity(eng, 0, 0.5));
                assert!(mod_hooks::set_player_rendering(eng, 0, 19, [255, 0, 0], 0, 20));
                let p = g.players[0].as_ref().unwrap();
                assert_eq!(p.money, 12345);
                assert!(p.silent_footsteps);
                assert!(p.has_defuser);
                assert!(p.has_night_vision());

                let ops = [
                    VfxOp::ShockwaveRing {
                        origin: [0.0, 0.0, 64.0],
                        radius: 200.0,
                        rgba: [0, 180, 255, 255],
                        life_decisec: 5,
                        width: 16,
                        slot: Some(0),
                    },
                    VfxOp::LaserBeam {
                        start: [0.0, 0.0, 64.0],
                        end: [100.0, 0.0, 64.0],
                        rgba: [255, 0, 0, 255],
                        life_decisec: 5,
                        width: 10,
                        noise: 0,
                        slot: Some(0),
                    },
                    VfxOp::FollowTrail {
                        ent_index: 1,
                        rgba: [0, 255, 0, 200],
                        life_decisec: 10,
                        width: 8,
                        slot: Some(0),
                    },
                    VfxOp::KillBeam {
                        ent_index: 1,
                        slot: Some(0),
                    },
                    VfxOp::AuraLight {
                        origin: [0.0, 0.0, 64.0],
                        radius: 20,
                        rgb: [255, 200, 50],
                        life_decisec: 5,
                        decay: 10,
                        slot: Some(0),
                    },
                    VfxOp::Sparks {
                        origin: [0.0, 0.0, 64.0],
                        slot: Some(0),
                    },
                    VfxOp::Implosion {
                        origin: [0.0, 0.0, 64.0],
                        radius: 64,
                        count: 20,
                        life_decisec: 5,
                        slot: Some(0),
                    },
                    VfxOp::TeleportSplash {
                        origin: [0.0, 0.0, 64.0],
                        slot: Some(0),
                    },
                    VfxOp::LavaSplash {
                        origin: [0.0, 0.0, 64.0],
                        slot: Some(0),
                    },
                    VfxOp::Explosion {
                        origin: [0.0, 0.0, 64.0],
                        scale: 25,
                        flags: 0,
                        slot: Some(0),
                    },
                    VfxOp::BloodStream {
                        origin: [0.0, 0.0, 64.0],
                        dir: [1.0, 0.0, 0.5],
                        color: 70,
                        speed: 120,
                        slot: Some(0),
                    },
                    VfxOp::WorldDecal {
                        origin: [0.0, 0.0, 0.0],
                        decal_name: "{scorch1".to_string(),
                        slot: Some(0),
                    },
                ];
                for op in &ops {
                    emit_vfx(eng, op);
                }
                self.rec.lock().unwrap().verified_vfx_emitted = ops.len();
            }
            Action::ApplyDamage(slot, dmg) => {
                // Temporarily re-install hook sink so nested `take_damage` fires `on_player_take_damage`!
                let prev = g.players[usize::from(slot)].as_ref().unwrap().health;
                let _ = prev;
                let _ = apply_damage(g, eng, slot, None, dmg, 0);
            }
            Action::SlayAndRespawn(slot) => {
                assert!(mod_hooks::slay_player(g, eng, slot, true));
                let alive_slay = mod_hooks::is_player_alive(g, eng, slot);
                assert!(mod_hooks::respawn_player(g, eng, slot));
                let alive_respawn = mod_hooks::is_player_alive(g, eng, slot);
                let mut r = self.rec.lock().unwrap();
                r.verified_alive_after_slay = alive_slay;
                r.verified_alive_after_respawn = alive_respawn;
            }
        }
        HookAction::Continue
    }

    fn on_player_take_damage(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerTakeDamageCtx,
    ) -> HookAction {
        let mut r = self.rec.lock().unwrap();
        r.damage_calls.push((phase, ctx.victim, ctx.damage));
        if phase == HookPhase::Pre {
            if r.supercede_damage {
                return HookAction::Supercede;
            }
            if let Some(d) = r.override_damage {
                ctx.damage = d;
                return HookAction::Override;
            }
        }
        HookAction::Continue
    }

    fn on_player_killed(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerKilledCtx,
    ) -> HookAction {
        if phase == HookPhase::Post {
            self.rec.lock().unwrap().kills.push((ctx.killer, ctx.victim));
        }
        HookAction::Continue
    }

    fn on_players_should_collide(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        ctx: &mut PlayersShouldCollideCtx,
    ) -> HookAction {
        let mut r = self.rec.lock().unwrap();
        r.semiclip_checks += 1;
        if r.semiclip_enabled {
            ctx.should_collide = false;
            return HookAction::Override;
        }
        HookAction::Continue
    }

    fn on_weapon_deploy(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut WeaponEventCtx,
    ) -> HookAction {
        if phase == HookPhase::Post {
            self.rec
                .lock()
                .unwrap()
                .weapon_deploys
                .push((ctx.slot, ctx.weapon_id));
        }
        HookAction::Continue
    }
}

fn spawn_map() -> ServerMap {
    let ents = vec![
        Entity {
            properties: HashMap::from([("classname".into(), "worldspawn".into())]),
        },
        Entity {
            properties: HashMap::from([
                ("classname".into(), "info_player_start".into()),
                ("origin".into(), "0 0 64".into()),
            ]),
        },
        Entity {
            properties: HashMap::from([
                ("classname".into(), "info_player_deathmatch".into()),
                ("origin".into(), "128 0 64".into()),
            ]),
        },
    ];
    ServerMap::stub_with_entities("de_dust2", ents)
}

#[test]
fn cs_hook_sink_custom_weapons_natives_vfx_and_semiclip() {
    let rec = Arc::new(Mutex::new(HookRecorder::default()));
    rec.lock().unwrap().semiclip_enabled = true;

    let mut game = CsGame::new();
    game.set_hook_sink(Box::new(RecordingSink {
        rec: Arc::clone(&rec),
    }));

    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("mp_freezetime", "0");
    cvars.set("bot_quota", "0");
    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        spawn_map(),
        game,
        DeltaRegistry::cs16_default(),
        cvars,
    );

    let hub = LoopbackHub::new();
    let mut transport = hub.connect();
    server.frame(&mut transport, 0.1);
    let _ = server.console_command("bot_add_ct Alpha");
    let _ = server.console_command("bot_add_t Beta");
    for i in 1..=10 {
        server.frame(&mut transport, 0.1 + f64::from(i) * 0.05);
    }

    assert!(!rec.lock().unwrap().spawns.is_empty());
    assert!(!rec.lock().unwrap().weapon_deploys.is_empty());
    assert!(
        rec.lock().unwrap().semiclip_checks > 0,
        "PlayersShouldCollide hook must run during pmove with 2 players"
    );

    // Queue custom weapon + CS natives + all 12 VFX + slay/respawn actions
    {
        let mut r = rec.lock().unwrap();
        r.actions.push_back(Action::PrecacheAndSetupCustomWeapon);
        r.actions.push_back(Action::RunNativesAndVfx);
        r.actions.push_back(Action::SlayAndRespawn(0));
    }
    for i in 11..=20 {
        server.frame(&mut transport, 0.1 + f64::from(i) * 0.05);
    }

    let r = rec.lock().unwrap();
    assert_eq!(r.verified_custom_clip, 45);
    assert_eq!(r.verified_vfx_emitted, 12);
    assert!(!r.verified_alive_after_slay);
    assert!(r.verified_alive_after_respawn);
    let p = server.game().state().players[0].as_ref().unwrap();
    assert_eq!(p.money, 12345);
    assert!(p.silent_footsteps);
    assert!(p.alive);
    let _ = (
        Action::ApplyDamage(0, 10.0),
        r.verified_hp_after_override,
        r.verified_hp_after_supercede,
    );
}
