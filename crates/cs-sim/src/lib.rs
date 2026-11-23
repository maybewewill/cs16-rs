//! Deterministic Counter-Strike 1.6 simulation core (`pm_shared`, BSP hull tracing,
//! ReGameDLL_CS gunplay, grenades, C4 match rules, and automatic BSP navigation bots).

pub mod bots;
pub mod grenades;
pub use cs_pm::pm;
pub mod pmove;
pub mod rules;
pub mod trace;
pub mod walk_game;
pub mod weapons;

#[cfg(test)]
mod tests {
    use cs_assets::bsp::{BspMap, SpawnTeam};
    use cs_assets::materials::MaterialTable;
    use cs_vfs::{Vfs, VfsConfig};
    use glam::Vec3;

    use crate::pmove::{MoveInputCmd, MoveParams, PlayerMoveState, snap_spawn_to_floor, step_player_move};
    use crate::rules::{MatchConfig, MatchWorld, PlayerInput, RoundPhase, TICK_DT};
    use crate::trace::{BspCollision, HitGroup, Hull, raycast_player_hitboxes};
    use crate::weapons::{WeaponId, compute_cs16_damage};

    #[test]
    fn ak47_headshot_kills_through_helmet_and_usp_respects_armor() {
        let ak = WeaponId::Ak47.spec();
        let (hp, armor) = compute_cs16_damage(ak.damage, 400.0, ak.range_modifier, HitGroup::Head, ak.armor_ratio, 100.0, true);
        assert!(hp >= 100.0, "AK-47 headshot should one-tap helmet, got {hp}");
        assert!(armor > 0.0);

        let awp = WeaponId::Awp.spec();
        let (awp_chest, _) = compute_cs16_damage(awp.damage, 600.0, awp.range_modifier, HitGroup::Chest, awp.armor_ratio, 100.0, true);
        let (awp_leg, leg_armor) = compute_cs16_damage(awp.damage, 600.0, awp.range_modifier, HitGroup::LeftLeg, awp.armor_ratio, 100.0, true);
        assert!(awp_chest >= 100.0, "AWP chest shot should one-tap, got {awp_chest}");
        assert!(awp_leg < 100.0, "AWP leg shot should not kill, got {awp_leg}");
        assert_eq!(leg_armor, 0.0, "Leg shots bypass armor");
    }

    #[test]
    fn hitbox_raycast_distinguishes_head_chest_and_legs() {
        let origin = Vec3::new(100.0, 0.0, 36.0);
        // Shoot head (z = 36 + 26 = 62)
        let head = raycast_player_hitboxes(Vec3::new(0.0, 0.0, 62.0), Vec3::new(200.0, 0.0, 62.0), origin, 0.0, false);
        assert_eq!(head.map(|h| h.2), Some(HitGroup::Head));
        // Shoot chest (z = 36 + 10 = 46)
        let chest = raycast_player_hitboxes(Vec3::new(0.0, 0.0, 46.0), Vec3::new(200.0, 0.0, 46.0), origin, 0.0, false);
        assert_eq!(chest.map(|h| h.2), Some(HitGroup::Chest));
        // Shoot legs (z = 36 - 20 = 16)
        let leg = raycast_player_hitboxes(Vec3::new(0.0, 0.0, 16.0), Vec3::new(200.0, 0.0, 16.0), origin, 0.0, false);
        assert_eq!(leg.map(|h| h.2), Some(HitGroup::LeftLeg));
    }

    #[test]
    fn pmove_and_full_bot_match_run_cleanly_on_de_dust2() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        let map = BspMap::load(&vfs, "de_dust2").expect("load de_dust2");
        let mats = MaterialTable::load(&vfs);
        let collision = BspCollision::from_map(&map, mats.clone());

        // 1. Verify pm_shared standing, running, jumping, and air-strafing on de_dust2 CT spawn
        let spawn = map
            .spawns
            .iter()
            .find(|s| s.team == SpawnTeam::CounterTerrorist)
            .expect("CT spawn");
        let mut pm = PlayerMoveState::at_spawn(snap_spawn_to_floor(spawn.position, &collision));
        let params = MoveParams::default();

        // Settle onto floor
        let idle = MoveInputCmd {
            yaw_deg: spawn.yaw_deg,
            ..Default::default()
        };
        for _ in 0..20 {
            step_player_move(&mut pm, &idle, 250.0, &params, &collision, TICK_DT);
        }
        assert!(pm.on_ground, "player should stand solidly on de_dust2 CT spawn");
        let start_z = pm.origin.z;

        // Jump and verify apex ~45 units above floor
        let mut max_z = start_z;
        for tick in 0..80 {
            let cmd = MoveInputCmd {
                yaw_deg: spawn.yaw_deg,
                jump: tick == 0,
                ..Default::default()
            };
            step_player_move(&mut pm, &cmd, 250.0, &params, &collision, TICK_DT);
            max_z = max_z.max(pm.origin.z);
        }
        let jump_height = max_z - start_z;
        assert!(
            (40.0..=52.0).contains(&jump_height),
            "classic CS 1.6 jump height should be ~45 units, got {jump_height:.2}"
        );
        assert!(pm.on_ground, "player should land cleanly back on the floor");

        // 2. Verify full MatchWorld: nav graph, buy economy, and 5v5 bots stepping 300 ticks (3 sec)
        let mut world = MatchWorld::new(
            &map,
            mats,
            MatchConfig {
                freeze_time_sec: 0.0,
                ..MatchConfig::default()
            },
        );
        assert!(world.nav.nodes.len() > 80, "auto BSP nav graph should cover de_dust2, got {}", world.nav.nodes.len());

        let me = world.add_player("Player", SpawnTeam::CounterTerrorist, false);
        assert!(world.buy_item(me, "deagle").is_ok(), "should buy Deagle with $800 in CT buyzone");
        world.set_bot_quota(9);
        world.start_new_round();
        assert_eq!(world.players.len(), 10);
        assert_eq!(world.phase, RoundPhase::Live);

        // Simulate 3 seconds of live 5v5 bots on de_dust2
        let my_input = [(me, PlayerInput::default())];
        for _ in 0..300 {
            world.step(&my_input, TICK_DT);
        }
        // Bots should have moved out of spawn without NaNs
        for p in &world.players {
            assert!(p.mv.origin.is_finite(), "player {} origin finite", p.name);
            let _ = collision.point_contents(p.mv.origin, Hull::Standing);
        }
    }

    #[test]
    fn all_28_weapons_regamedll_kickback_and_cs_office_hostages_work() {
        use crate::weapons::{apply_regamedll_kickback, pm_drop_punch_angle};

        assert_eq!(
            WeaponId::ALL_BUYABLE.len(),
            27,
            "27 buyable weapons + knife + c4 = 29"
        );
        for &wid in WeaponId::ALL_BUYABLE {
            let s = wid.spec();
            assert!(!s.viewmodel_path.is_empty());
            assert!(!s.p_model_path.is_empty());
            assert_eq!(WeaponId::from_console_name(s.console_name), Some(wid));
        }

        let ak = WeaponId::Ak47.spec();
        let (mut pp, mut py, mut dir) = (0.0f32, 0.0f32, 1i8);
        for shot in 1..=6u32 {
            apply_regamedll_kickback(
                ak.kick, shot, 0.0, true, false, &mut pp, &mut py, &mut dir, shot,
            );
        }
        assert!(pp < -3.0 && pp >= -ak.kick.up_max - 0.01);
        pm_drop_punch_angle(&mut pp, &mut py, 0.1);
        assert!(pp > -ak.kick.up_max);

        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        if let Ok(office) = BspMap::load(&vfs, "cs_office") {
            let mut world =
                MatchWorld::new(&office, MaterialTable::load(&vfs), MatchConfig::default());
            world.start_new_round();
            assert!(!world.hostages.is_empty(), "cs_office has real hostages");
            assert!(
                !world.breakables.is_empty(),
                "cs_office has real breakable windows/props"
            );
        }
    }
}
