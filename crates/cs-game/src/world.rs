//! The map's world and the static entities every map has: `worldspawn` (`CWorld`), the spawn
//! points, the brush entities with no logic yet, and the choice of a spawn point.

use cs_assets::bsp::SpawnTeam;
use cs_engine::consts::{
    EF_NODRAW, FL_CLIENT, FL_WORLDBRUSH, MOVETYPE_NOCLIP, MOVETYPE_NONE, MOVETYPE_PUSH, SOLID_BSP,
    SOLID_NOT,
};
use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use crate::GameState;
use crate::entity::GameEntity;
use crate::player::{dmg, player_slot, take_damage};

/// `MAX_PLAYER_USE_RADIUS` (ReGameDLL `player.h`, read 2026-11-22): how close to a spawn point
/// another player makes it unusable.
pub const MAX_PLAYER_USE_RADIUS: f32 = 64.0;

/// `kRenderTransTexture`.
const RENDER_TRANS_TEXTURE: u8 = 2;
/// `CONTENTS_LADDER`: the `skin` of a ladder entity (the same value as `cs_engine::CONTENTS_LADDER`,
/// which is an `i32`; `skin` is an `i16`).
const CONTENTS_LADDER: i16 = -16;
/// The classname of the CT spawn points.
pub const CT_SPAWN_CLASS: &str = "info_player_start";
/// The classname of the terrorist spawn points.
pub const T_SPAWN_CLASS: &str = "info_player_deathmatch";
/// The classname of the VIP spawn points.
pub const VIP_SPAWN_CLASS: &str = "info_vip_start";

/// `sPlayerModelFiles` without the two Condition Zero models (ReGameDLL `client.cpp`,
/// function `ClientPrecache`, read 2026-11-22).
const PLAYER_MODELS: [&str; 10] = [
    "models/player.mdl",
    "models/player/leet/leet.mdl",
    "models/player/gign/gign.mdl",
    "models/player/vip/vip.mdl",
    "models/player/gsg9/gsg9.mdl",
    "models/player/guerilla/guerilla.mdl",
    "models/player/arctic/arctic.mdl",
    "models/player/sas/sas.mdl",
    "models/player/terror/terror.mdl",
    "models/player/urban/urban.mdl",
];

/// The `PRECACHE_MODEL` calls of `ClientPrecache` that follow the player models (ReGameDLL
/// `client.cpp` 4095-4140, read 2026-11-22): the third-person models of the weapons and their
/// shield variants, and a few world models. `pev->weaponmodel` is a model index looked up by
/// name (`MODEL_INDEX(STRING(pev->weaponmodel))` in `AddToFullPack`), so a weapon whose
/// `p_*.mdl` is not here has no model in the hands of the player on the other clients.
const CLIENT_WEAPON_MODELS: [&str; 44] = [
    "models/p_ak47.mdl",
    "models/p_aug.mdl",
    "models/p_awp.mdl",
    "models/p_c4.mdl",
    "models/w_c4.mdl",
    "models/p_deagle.mdl",
    "models/shield/p_shield_deagle.mdl",
    "models/p_flashbang.mdl",
    "models/shield/p_shield_flashbang.mdl",
    "models/p_hegrenade.mdl",
    "models/shield/p_shield_hegrenade.mdl",
    "models/p_glock18.mdl",
    "models/shield/p_shield_glock18.mdl",
    "models/p_p228.mdl",
    "models/shield/p_shield_p228.mdl",
    "models/p_smokegrenade.mdl",
    "models/shield/p_shield_smokegrenade.mdl",
    "models/p_usp.mdl",
    "models/shield/p_shield_usp.mdl",
    "models/p_fiveseven.mdl",
    "models/shield/p_shield_fiveseven.mdl",
    "models/p_knife.mdl",
    "models/shield/p_shield_knife.mdl",
    "models/w_flashbang.mdl",
    "models/w_hegrenade.mdl",
    "models/p_sg550.mdl",
    "models/p_g3sg1.mdl",
    "models/p_m249.mdl",
    "models/p_m3.mdl",
    "models/p_m4a1.mdl",
    "models/p_mac10.mdl",
    "models/p_mp5.mdl",
    "models/p_ump45.mdl",
    "models/p_p90.mdl",
    "models/p_scout.mdl",
    "models/p_sg552.mdl",
    "models/w_smokegrenade.mdl",
    "models/p_tmp.mdl",
    "models/p_elite.mdl",
    "models/p_xm1014.mdl",
    "models/p_galil.mdl",
    "models/p_famas.mdl",
    "models/p_shield.mdl",
    "models/w_shield.mdl",
];

/// The sprites `ClientPrecache` precaches after the shadow (client.cpp 4342-4358, vanilla).
const CLIENT_SMOKE_SPRITES: [&str; 14] = [
    "sprites/wall_puff1.spr",
    "sprites/wall_puff2.spr",
    "sprites/wall_puff3.spr",
    "sprites/wall_puff4.spr",
    "sprites/black_smoke1.spr",
    "sprites/black_smoke2.spr",
    "sprites/black_smoke3.spr",
    "sprites/black_smoke4.spr",
    "sprites/fast_wallpuff1.spr",
    "sprites/pistol_smoke1.spr",
    "sprites/pistol_smoke2.spr",
    "sprites/rifle_smoke1.spr",
    "sprites/rifle_smoke2.spr",
    "sprites/rifle_smoke3.spr",
];

/// The `PRECACHE_SOUND` calls of `ClientPrecache` that come before the models (ReGameDLL
/// `client.cpp`, read 2026-11-22).
const CLIENT_SOUNDS: [&str; 110] = [
    "weapons/dryfire_pistol.wav",
    "weapons/dryfire_rifle.wav",
    "player/pl_shot1.wav",
    "player/pl_die1.wav",
    "player/headshot1.wav",
    "player/headshot2.wav",
    "player/headshot3.wav",
    "player/bhit_flesh-1.wav",
    "player/bhit_flesh-2.wav",
    "player/bhit_flesh-3.wav",
    "player/bhit_kevlar-1.wav",
    "player/bhit_helmet-1.wav",
    "player/die1.wav",
    "player/die2.wav",
    "player/die3.wav",
    "player/death6.wav",
    "radio/locknload.wav",
    "radio/letsgo.wav",
    "radio/moveout.wav",
    "radio/com_go.wav",
    "radio/rescued.wav",
    "radio/rounddraw.wav",
    "items/kevlar.wav",
    "items/ammopickup2.wav",
    "items/nvg_on.wav",
    "items/nvg_off.wav",
    "items/equip_nvg.wav",
    "weapons/c4_beep1.wav",
    "weapons/c4_beep2.wav",
    "weapons/c4_beep3.wav",
    "weapons/c4_beep4.wav",
    "weapons/c4_beep5.wav",
    "weapons/c4_explode1.wav",
    "weapons/c4_plant.wav",
    "weapons/c4_disarm.wav",
    "weapons/c4_disarmed.wav",
    "weapons/explode3.wav",
    "weapons/explode4.wav",
    "weapons/explode5.wav",
    "player/sprayer.wav",
    "player/pl_fallpain2.wav",
    "player/pl_fallpain3.wav",
    "player/pl_snow1.wav",
    "player/pl_snow2.wav",
    "player/pl_snow3.wav",
    "player/pl_snow4.wav",
    "player/pl_snow5.wav",
    "player/pl_snow6.wav",
    "player/pl_step1.wav",
    "player/pl_step2.wav",
    "player/pl_step3.wav",
    "player/pl_step4.wav",
    "common/npc_step1.wav",
    "common/npc_step2.wav",
    "common/npc_step3.wav",
    "common/npc_step4.wav",
    "player/pl_metal1.wav",
    "player/pl_metal2.wav",
    "player/pl_metal3.wav",
    "player/pl_metal4.wav",
    "player/pl_dirt1.wav",
    "player/pl_dirt2.wav",
    "player/pl_dirt3.wav",
    "player/pl_dirt4.wav",
    "player/pl_duct1.wav",
    "player/pl_duct2.wav",
    "player/pl_duct3.wav",
    "player/pl_duct4.wav",
    "player/pl_grate1.wav",
    "player/pl_grate2.wav",
    "player/pl_grate3.wav",
    "player/pl_grate4.wav",
    "player/pl_slosh1.wav",
    "player/pl_slosh2.wav",
    "player/pl_slosh3.wav",
    "player/pl_slosh4.wav",
    "player/pl_tile1.wav",
    "player/pl_tile2.wav",
    "player/pl_tile3.wav",
    "player/pl_tile4.wav",
    "player/pl_tile5.wav",
    "player/pl_swim1.wav",
    "player/pl_swim2.wav",
    "player/pl_swim3.wav",
    "player/pl_swim4.wav",
    "player/pl_ladder1.wav",
    "player/pl_ladder2.wav",
    "player/pl_ladder3.wav",
    "player/pl_ladder4.wav",
    "player/pl_wade1.wav",
    "player/pl_wade2.wav",
    "player/pl_wade3.wav",
    "player/pl_wade4.wav",
    "debris/wood1.wav",
    "debris/wood2.wav",
    "debris/wood3.wav",
    "plats/train_use1.wav",
    "plats/vehicle_ignition.wav",
    "buttons/spark5.wav",
    "buttons/spark6.wav",
    "debris/glass1.wav",
    "debris/glass2.wav",
    "debris/glass3.wav",
    // `SOUND_FLASHLIGHT_ON` and `SOUND_FLASHLIGHT_OFF` (the same file), between the glass and
    // the body splat.
    "items/flashlight1.wav",
    "common/bodysplat.wav",
    "player/pl_pain2.wav",
    "player/pl_pain4.wav",
    "player/pl_pain5.wav",
    "player/pl_pain6.wav",
    "player/pl_pain7.wav",
];

/// The `PRECACHE_SOUND` calls of `ClientPrecache` that follow the models and the bounds it
/// forces (client.cpp 4328-4338): the weapon-select sounds of the HUD and the Geiger counter.
const CLIENT_HUD_SOUNDS: [&str; 11] = [
    "common/wpn_hudoff.wav",
    "common/wpn_hudon.wav",
    "common/wpn_moveselect.wav",
    "common/wpn_select.wav",
    "common/wpn_denyselect.wav",
    "player/geiger6.wav",
    "player/geiger5.wav",
    "player/geiger4.wav",
    "player/geiger3.wav",
    "player/geiger2.wav",
    "player/geiger1.wav",
];

/// The `PRECACHE_GENERIC` calls of `ClientPrecache` (client.cpp 4361-4364): the textures of the
/// scope overlay, which the engine also asks the client to check (`force_exactfile`).
const CLIENT_GENERIC: [&str; 4] = [
    "sprites/scope_arc.tga",
    "sprites/scope_arc_nw.tga",
    "sprites/scope_arc_ne.tga",
    "sprites/scope_arc_sw.tga",
];

/// The sounds `BotPrecache` precaches (ReGameDLL `game_shared/bot/bot_util.cpp`, vanilla branch,
/// read 2026-11-22; the `REGAMEDLL_FIXES` branch skips them when no bot can run): the vanilla
/// server precaches them for every game, bots or not, after its model `sprites/smoke.spr`.
const BOT_SOUNDS: [&str; 11] = [
    "buttons/bell1.wav",
    "buttons/blip1.wav",
    "buttons/blip2.wav",
    "buttons/button11.wav",
    "buttons/latchunlocked2.wav",
    "buttons/lightswitch2.wav",
    "ambience/quail1.wav",
    "events/tutor_msg.wav",
    "events/enemy_died.wav",
    "events/friend_died.wav",
    "events/task_complete.wav",
];

/// The sounds `CWorld::Precache` precaches before the gibs (`world.cpp`, read 2026-11-22).
const WORLD_SOUNDS_BEFORE_GIBS: [&str; 5] = [
    "common/null.wav",
    "items/suitchargeok1.wav",
    "items/gunpickup2.wav",
    "common/bodydrop3.wav",
    "common/bodydrop4.wav",
];

/// The sounds `CWorld::Precache` precaches after the gibs: the ricochets.
const WORLD_SOUNDS: [&str; 9] = [
    "weapons/ric1.wav",
    "weapons/ric2.wav",
    "weapons/ric3.wav",
    "weapons/ric4.wav",
    "weapons/ric5.wav",
    "weapons/ric_metal-1.wav",
    "weapons/ric_metal-2.wav",
    "weapons/ric_conc-1.wav",
    "weapons/ric_conc-2.wav",
];

/// The light styles `CWorld::Precache` sets: `(index, pattern)`.
const LIGHT_STYLES: [(u8, &str); 14] = [
    (0, "m"),
    (1, "mmnmmommommnonmmonqnmmo"),
    (2, "abcdefghijklmnopqrstuvwxyzyxwvutsrqponmlkjihgfedcba"),
    (3, "mmmmmaaaaammmmmaaaaaabcdefgabcdefg"),
    (4, "mamamamamama"),
    (5, "jklmnopqrstuvwxyzyxwvutsrqponmlkj"),
    (6, "nmonqnmomnmomomno"),
    (7, "mmmaaaabcdefgmmmmaaaammmaamm"),
    (8, "mmmaaammmaaammmabcdefaaaammmmabcdefmmmaaaa"),
    (9, "aaaaaaaazzzzzzzz"),
    (10, "mmamammmmammamamaaamammma"),
    (11, "abcdefghijklmnopqrrqponmlkjihgfedcba"),
    (12, "mmnnmmnnnmmnn"),
    (63, "a"),
];

/// C `atof`: the number a string starts with, 0 when it starts with none.
pub(crate) fn atof(text: &str) -> f32 {
    let text = text.trim_start();
    let mut end = 0;
    for (i, c) in text.char_indices() {
        let ok = c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'));
        if !ok {
            break;
        }
        end = i + c.len_utf8();
    }
    // The longest prefix that parses (`1.2.3` is 1.2).
    (1..=end)
        .rev()
        .find_map(|n| text[..n].parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

fn precache_model(eng: &mut EngineApi<'_>, name: &str) {
    if let Err(err) = eng.precache_model(name) {
        tracing::error!(model = %name, ?err, "model not precached");
    }
}

fn precache_generic(eng: &mut EngineApi<'_>, name: &str) {
    if let Err(err) = eng.precache_generic(name) {
        tracing::error!(generic = %name, ?err, "generic file not precached");
    }
}

fn precache_sound(eng: &mut EngineApi<'_>, name: &str) {
    if let Err(err) = eng.precache_sound(name) {
        tracing::error!(sound = %name, ?err, "sound not precached");
    }
}

/// The end of `CWorld::Spawn` (world.cpp 215-272, vanilla branch, read 2026-11-22): the briefing of
/// the map, `maps/<mapname>.txt`, or else `maps/default.txt`, is loaded and, when it holds
/// anything, precached as a generic file (the client reads it for the briefing window).
/// `briefings` are the briefing files that exist.
pub(crate) fn precache_briefing(eng: &mut EngineApi<'_>, briefings: &std::collections::BTreeSet<String>) {
    let map_file = format!("maps/{}.txt", eng.map_name());
    // The names are lower case, the way `Vfs` keys its files; the file system is not case
    // sensitive for the original.
    if briefings.contains(&map_file.to_ascii_lowercase()) {
        precache_generic(eng, &map_file);
    } else if briefings.contains("maps/default.txt") {
        precache_generic(eng, "maps/default.txt");
    }
}

/// `worldspawn` (`CWorld`).
pub(crate) struct World;

impl World {
    /// `CWorld::KeyValue`, the keys that concern a server.
    fn key_values(eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) {
        for (key, value) in keys {
            match key.as_str() {
                "skyname" => eng.set_cvar("sv_skyname", value),
                "WaveHeight" => {
                    let scale = atof(value) * 0.125;
                    if let Some(v) = eng.ent_mut(id) {
                        v.scale = scale;
                    }
                    eng.set_cvar("sv_wateramp", &scale.to_string());
                }
                "MaxRange" => {
                    if let Some(v) = eng.ent_mut(id) {
                        v.speed = atof(value);
                    }
                }
                // `chaptertitle` makes an `env_message` that shows the title on a single-player
                // map; `sounds` is a CD track; the other keys are single-player or client-side
                // (`startdark` is not applied on a dedicated server).
                _ => {}
            }
        }
    }

    /// `CWorld::Precache` without the bots (`BotPrecache`), the sentences and the texture types:
    /// the server settings, the sound entity, the weapons (`WeaponsPrecache`), the player's
    /// (`ClientPrecache`), the common resources and the light styles.
    /// ReGameDLL `world.cpp`, `client.cpp`, read 2026-11-22.
    fn precache(eng: &mut EngineApi<'_>, id: EdictId) {
        eng.set_cvar("sv_gravity", "800");
        eng.set_cvar("sv_maxspeed", "900");
        eng.set_cvar("sv_stepsize", "18");
        eng.set_cvar("room_type", "0");

        // `InstallGameRules` runs `CVoiceGameMgr::Init` (game_shared/voice_gamemgr.cpp), whose first
        // act is to precache the speaker icon; its two user messages (`VoiceMask`, `ReqState`)
        // wait for the voice manager.
        precache_model(eng, "sprites/voiceicon.spr");
        // The sound entity (`GetClassPtr<CCSSoundEnt>` then `Spawn`, world.cpp 299-308, which makes
        // it `SOLID_NOT`), made right after the game rules and before the precaches: an edict
        // with no model that the original keeps, so the map entities that follow have the same
        // numbers. Its think (the sound list of the monsters) is not ported.
        if let Some(sound_ent) = eng.create_entity("soundent")
            && let Some(v) = eng.ent_mut(sound_ent)
        {
            v.solid = SOLID_NOT;
        }
        // `CWorld::Precache` (world.cpp 277-365): `WeaponsPrecache()` comes before
        // `ClientPrecache()`, which comes before `BotPrecache()` and the world's own sounds and
        // gibs, so the weapons' models have the low indices and the third-person ones follow the
        // player models.
        crate::inventory::weapons_precache(eng);
        for name in CLIENT_SOUNDS {
            precache_sound(eng, name);
        }
        for name in PLAYER_MODELS {
            precache_model(eng, name);
        }
        for name in CLIENT_WEAPON_MODELS {
            precache_model(eng, name);
        }
        for name in CLIENT_HUD_SOUNDS {
            precache_sound(eng, name);
        }
        // `g_iShadowSprite`, which `InitHUD` sends in `ShadowIdx`.
        precache_model(eng, "sprites/shadow_circle.spr");
        // The end of `ClientPrecache` (client.cpp 4340-4364): the smoke sprites of the bullets
        // (`REGAMEDLL_FIXES`' `gas_puff_01.spr` is not vanilla), the scope textures and the
        // reset of the decals.
        for name in CLIENT_SMOKE_SPRITES {
            precache_model(eng, name);
        }
        for name in CLIENT_GENERIC {
            precache_generic(eng, name);
        }
        eng.precache_event("events/decal_reset.sc");
        // `BotPrecache`, which the original calls whether or not bots are around.
        precache_model(eng, "sprites/smoke.spr");
        for name in BOT_SOUNDS {
            precache_sound(eng, name);
        }
        for name in WORLD_SOUNDS_BEFORE_GIBS {
            precache_sound(eng, name);
        }
        // `sv_language` 1 is German, which has its own gibs.
        if eng.cvar_f32("sv_language") == 1.0 {
            precache_model(eng, "models/germangibs.mdl");
        } else {
            precache_model(eng, "models/hgibs.mdl");
            precache_model(eng, "models/agibs.mdl");
        }
        for name in WORLD_SOUNDS {
            precache_sound(eng, name);
        }
        for (index, pattern) in LIGHT_STYLES {
            eng.light_style(index, pattern);
        }

        let max_range = eng.ent(id).map_or(0.0, |v| v.speed);
        let zmax = if max_range > 0.0 { max_range } else { 4096.0 };
        eng.set_cvar("sv_zmax", &zmax.to_string());
    }
}

impl GameEntity for World {
    fn classname(&self) -> &'static str {
        "worldspawn"
    }

    /// `CWorld::Spawn`: the engine already made the world a `SOLID_BSP`, `MOVETYPE_PUSH` brush
    /// entity with the map model (`SV_SpawnServer`), so this reads the keys and precaches.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        Self::key_values(eng, id, keys);
        Self::precache(eng, id);
        true
    }
}

/// `info_player_start` (counter-terrorist) and `info_player_deathmatch` (terrorist): a point
/// players spawn at. It has no model and nothing to do; `SelectSpawnPoint` finds it by class.
pub(crate) struct InfoPlayer(pub(crate) &'static str);

impl GameEntity for InfoPlayer {
    fn classname(&self) -> &'static str {
        self.0
    }
}

/// `info_target` (`CPointEntity`, ReGameDLL `cbase.cpp`/`world.cpp`, read 2026-11-22): a point
/// with a name and nothing else to do. It is the default target of a `trigger_camera`, which the
/// intro camera looks at ([`crate::intro`]); the entity is kept so the target can be found by
/// its `targetname`.
pub(crate) struct InfoTarget;

impl GameEntity for InfoTarget {
    fn classname(&self) -> &'static str {
        "info_target"
    }
}

/// `trigger_camera` (`CTriggerCamera`, ReGameDLL `triggers.cpp`, read 2026-11-22), as far as the
/// join screen needs it: the entity exists, with its origin, `target` and `targetname` (the
/// engine copies them from the map), and the keyvalues of `CTriggerCamera::KeyValue` are kept.
/// `Spawn` makes it invisible and not solid. Not ported (gap): `Use` (the camera that takes a
/// player over and follows its `target`/`moveto` path), `FollowTarget`, `Move` and their think;
/// nothing triggers a camera in a competitive map, so only the intro uses it.
#[derive(Debug, Default)]
pub(crate) struct TriggerCamera {
    /// `m_flWait` (`wait`): how long the camera holds the view.
    pub(crate) wait: f32,
    /// `m_sPath` (`moveto`): the `path_corner` the camera moves to.
    pub(crate) moveto: String,
    /// `m_acceleration` (`acceleration`), 500 when the map gives none.
    pub(crate) acceleration: f32,
    /// `m_deceleration` (`deceleration`), 500 when the map gives none.
    pub(crate) deceleration: f32,
    /// `m_initialSpeed`: the `speed` the map gave.
    pub(crate) initial_speed: f32,
}

impl GameEntity for TriggerCamera {
    fn classname(&self) -> &'static str {
        "trigger_camera"
    }

    /// `CTriggerCamera::KeyValue` (the four keys it handles; the rest are `CBaseDelay`'s and the
    /// engine's), then `CTriggerCamera::Spawn`.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        for (key, value) in keys {
            match key.as_str() {
                "wait" => self.wait = atof(value),
                "moveto" => self.moveto.clone_from(value),
                "acceleration" => self.acceleration = atof(value),
                "deceleration" => self.deceleration = atof(value),
                _ => {}
            }
        }
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.movetype = MOVETYPE_NOCLIP;
        v.solid = SOLID_NOT;
        v.renderamt = 0;
        v.rendermode = RENDER_TRANS_TEXTURE;
        self.initial_speed = v.speed;
        if self.acceleration == 0.0 {
            self.acceleration = 500.0;
        }
        if self.deceleration == 0.0 {
            self.deceleration = 500.0;
        }
        true
    }
}

/// `func_ladder` (`CLadder`, ReGameDLL `triggers.cpp`, read 2026-11-22): a non-solid brush
/// whose `skin` is `CONTENTS_LADDER`, invisible unless `showtriggers` is set. The movement finds
/// it as a moveent of its physents (`SOLID_NOT` with this skin).
pub(crate) struct FuncLadder;

impl GameEntity for FuncLadder {
    fn classname(&self) -> &'static str {
        "func_ladder"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let showtriggers = eng.cvar_f32("showtriggers") != 0.0;
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        if brush_model_number(&v.model).is_none() {
            return false;
        }
        // `CLadder::Precache`, then `CLadder::Spawn`.
        v.solid = SOLID_NOT;
        v.skin = CONTENTS_LADDER;
        if !showtriggers {
            v.rendermode = RENDER_TRANS_TEXTURE;
            v.renderamt = 0;
        }
        v.effects &= !EF_NODRAW;
        v.movetype = MOVETYPE_PUSH;
        true
    }
}

/// How a [`StaticBrush`] behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrushKind {
    /// Solid and pushing (`MOVETYPE_PUSH`, `SOLID_BSP`), but it never moves: there is no door,
    /// button or train logic yet.
    Solid,
    /// `func_illusionary`: visible, never solid.
    Illusionary,
    /// `func_door` and `func_water` (`CBaseDoor::Spawn`, doors.cpp 192-215, read 2026-11-22): a
    /// door with a nonzero `skin` has special contents (water, slime...) and is never solid;
    /// otherwise `SF_DOOR_PASSABLE` makes it non-solid. Only the spawn part: the movement is
    /// phase 7.
    Door,
    /// `func_door_rotating` (`CRotDoor::Spawn`): solid unless `SF_DOOR_PASSABLE`; no `skin`.
    RotatingDoor,
}

/// `SF_DOOR_PASSABLE`.
const SF_DOOR_PASSABLE: u32 = 1 << 3;

/// `SF_DOOR_ACTUALLY_WATER` (`BIT(31)`, ReGameDLL `doors.h`, read 2026-11-22).
const SF_DOOR_ACTUALLY_WATER: u32 = 1 << 31;

/// `SetMovedir` (ReGameDLL `subs.cpp`, read 2026-11-22): `movedir` is the direction `angles`
/// point in, with the editor's special cases (0 -1 0) = up and (0 -2 0) = down; then `angles`
/// is zeroed, so the brush is never rotated by the key.
fn set_movedir(v: &mut cs_engine::EntVars) {
    v.movedir = if v.angles == Vec3::new(0.0, -1.0, 0.0) {
        Vec3::Z
    } else if v.angles == Vec3::new(0.0, -2.0, 0.0) {
        -Vec3::Z
    } else {
        cs_sim::pm::math::angle_vectors(v.angles).0
    };
    v.angles = Vec3::ZERO;
}

/// The brush model number `N` of a model named `*N`; `None` for the world (`*0`) and any other
/// name.
pub(crate) fn brush_model_number(model: &str) -> Option<usize> {
    model
        .strip_prefix('*')?
        .parse::<usize>()
        .ok()
        .filter(|&n| n > 0)
}

/// A brush entity that only has to be there for players and bullets to meet: `func_wall`
/// (`CFuncWall::Spawn`), `func_illusionary` (`CFuncIllusionary::Spawn`, ReGameDLL `bmodels.cpp`,
/// read 2026-11-22) and the doors, buttons, platforms, trains, breakables and pushables, which
/// stand still as solid walls until their own tasks.
pub(crate) struct StaticBrush {
    class: &'static str,
    kind: BrushKind,
}

impl StaticBrush {
    pub(crate) fn new(class: &'static str, kind: BrushKind) -> Self {
        Self { class, kind }
    }
}

impl GameEntity for StaticBrush {
    fn classname(&self) -> &'static str {
        self.class
    }

    /// Every brush class but `func_wall` (a plain `CBaseEntity`) is a `CBaseToggle`.
    fn is_delay(&self) -> bool {
        self.class != "func_wall"
    }

    /// An entry without a `*N` brush model is refused: there is nothing to collide with.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let (class, kind) = (self.class, self.kind);
        if class == "func_breakable" {
            // `CBreakable::Spawn` precaches before anything else.
            breakable_precache(eng, keys);
        }
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        if brush_model_number(&v.model).is_none() {
            return false;
        }
        v.velocity = Vec3::ZERO;
        match kind {
            BrushKind::Solid => {
                v.movetype = MOVETYPE_PUSH;
                v.solid = SOLID_BSP;
                if class == "func_wall" {
                    // "If it can't move/go away, it's really part of the world".
                    v.angles = Vec3::ZERO;
                    v.flags |= FL_WORLDBRUSH;
                }
            }
            BrushKind::Door | BrushKind::RotatingDoor => {
                // `skin` is a key of the entity; the engine copies only `spawnflags`.
                if kind == BrushKind::Door
                    && let Some((_, value)) = keys.iter().find(|(k, _)| k == "skin")
                {
                    v.skin = crate::menus::atoi(value).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                }
                let special = kind == BrushKind::Door && v.skin != 0;
                if kind == BrushKind::Door {
                    // `CBaseDoor::Spawn` (rg_doors.cpp, read 2026-11-22): `SetMovedir(pev)`
                    // before anything else; the rotating door keeps its angles.
                    set_movedir(v);
                    if special {
                        // "water is silent for now": marks a `func_door` as a `func_water`.
                        v.spawnflags |= SF_DOOR_ACTUALLY_WATER;
                    }
                }
                v.solid = if special || v.spawnflags & SF_DOOR_PASSABLE != 0 {
                    SOLID_NOT
                } else {
                    SOLID_BSP
                };
                v.movetype = MOVETYPE_PUSH;
            }
            BrushKind::Illusionary => {
                // `CFuncIllusionary::KeyValue`: `skin` is the contents of the brush (the maps
                // give -1, `CONTENTS_EMPTY`), which the entity state carries.
                if let Some((_, value)) = keys.iter().rev().find(|(k, _)| k == "skin") {
                    v.skin =
                        crate::menus::atoi(value).clamp(i32::from(i16::MIN), i32::from(i16::MAX))
                            as i16;
                }
                v.angles = Vec3::ZERO;
                v.movetype = MOVETYPE_NONE;
                v.solid = SOLID_NOT;
            }
        }
        // The engine linked the entity when it set the model, which was before it became
        // solid: link it again for the traces to see it.
        eng.link_entity(id, false);
        true
    }
}

/// `Materials` of `func_break.h` (ReGameDLL, read 2026-11-22): `matGlass` is the default of a
/// `func_breakable` whose map gives no `material`.
const MAT_GLASS: i32 = 0;
const MAT_WOOD: i32 = 1;
const MAT_METAL: i32 = 2;
const MAT_FLESH: i32 = 3;
const MAT_CINDER_BLOCK: i32 = 4;
const MAT_CEILING_TILE: i32 = 5;
const MAT_COMPUTER: i32 = 6;
const MAT_UNBREAKABLE_GLASS: i32 = 7;
const MAT_ROCKS: i32 = 8;
/// `matLastMaterial`.
const MAT_LAST: i32 = 10;

/// `CBreakable::KeyValue` (the keys `material` and `gibmodel`) and `CBreakable::Precache`
/// (`func_break.cpp`, read 2026-11-22): the gibs and sounds of the material, the sounds
/// `MaterialSoundPrecache` adds, and the model of the gibs the map names (`gibmodel`, which
/// replaces the material's). A `material` out of range is wood. Not ported: the object a
/// breakable spawns (`spawnobject`), which is a Half-Life item the game does not have.
fn breakable_precache(eng: &mut EngineApi<'_>, keys: &[(String, String)]) {
    let mut material = MAT_GLASS;
    let mut gib_model: Option<&str> = None;
    for (key, value) in keys {
        match key.as_str() {
            "material" => {
                let m = crate::menus::atoi(value);
                material = if (0..MAT_LAST).contains(&m) { m } else { MAT_WOOD };
            }
            "gibmodel" => gib_model = Some(value),
            _ => {}
        }
    }
    let (default_gib, busts): (Option<&str>, &[&str]) = match material {
        MAT_WOOD => (
            Some("models/woodgibs.mdl"),
            &["debris/bustcrate1.wav", "debris/bustcrate2.wav"],
        ),
        MAT_FLESH => (
            Some("models/fleshgibs.mdl"),
            &["debris/bustflesh1.wav", "debris/bustflesh2.wav"],
        ),
        MAT_COMPUTER => (
            Some("models/computergibs.mdl"),
            // The sparks come first, then the metal bursts.
            &[
                "buttons/spark5.wav",
                "buttons/spark6.wav",
                "debris/bustmetal1.wav",
                "debris/bustmetal2.wav",
            ],
        ),
        MAT_GLASS | MAT_UNBREAKABLE_GLASS => (
            Some("models/glassgibs.mdl"),
            &["debris/bustglass1.wav", "debris/bustglass2.wav"],
        ),
        MAT_METAL => (
            Some("models/metalplategibs.mdl"),
            &["debris/bustmetal1.wav", "debris/bustmetal2.wav"],
        ),
        MAT_CINDER_BLOCK => (
            Some("models/cindergibs.mdl"),
            &["debris/bustconcrete1.wav", "debris/bustconcrete2.wav"],
        ),
        MAT_ROCKS => (
            Some("models/rockgibs.mdl"),
            &["debris/bustconcrete1.wav", "debris/bustconcrete2.wav"],
        ),
        MAT_CEILING_TILE => (Some("models/ceilinggibs.mdl"), &["debris/bustceiling.wav"]),
        _ => (None, &[]),
    };
    for sound in busts {
        precache_sound(eng, sound);
    }
    // `MaterialSoundPrecache`: the sounds the material makes when hurt.
    let hurt: &[&str] = match material {
        MAT_WOOD => &["debris/wood1.wav", "debris/wood2.wav", "debris/wood3.wav"],
        MAT_FLESH => &[
            "debris/flesh1.wav",
            "debris/flesh2.wav",
            "debris/flesh3.wav",
            "debris/flesh5.wav",
            "debris/flesh6.wav",
            "debris/flesh7.wav",
        ],
        MAT_GLASS | MAT_COMPUTER | MAT_UNBREAKABLE_GLASS => {
            &["debris/glass1.wav", "debris/glass2.wav", "debris/glass3.wav"]
        }
        MAT_METAL => &["debris/metal1.wav", "debris/metal2.wav", "debris/metal3.wav"],
        MAT_CINDER_BLOCK | MAT_ROCKS => {
            &["debris/concrete1.wav", "debris/concrete2.wav", "debris/concrete3.wav"]
        }
        _ => &[],
    };
    for sound in hurt {
        precache_sound(eng, sound);
    }
    // `if (m_iszGibModel) pGibName = STRING(m_iszGibModel)`: the map's model wins over the
    // material's. Assumption: the maps give no empty `gibmodel`.
    if let Some(name) = gib_model.or(default_gib) {
        precache_model(eng, name);
    }
}

/// The players other than `player` within `radius` of `spot`, alive or dead.
fn players_near(eng: &EngineApi<'_>, player: EdictId, spot: EdictId, radius: f32) -> Vec<EdictId> {
    let Some(origin) = eng.ent(spot).map(|v| v.origin) else {
        return Vec::new();
    };
    eng.entities_in_sphere(origin, radius)
        .into_iter()
        .filter(|&e| e != player && eng.ent(e).is_some_and(|v| v.flags & FL_CLIENT != 0))
        .collect()
}

/// Whether `spot` can be used by `player`: no other player, alive or dead, within `radius` of
/// it (`IsSpawnPointValid`, ReGameDLL `player.cpp`, read 2026-11-22, the vanilla branch: the
/// `REGAMEDLL_FIXES` build skips dead players, the target build does not; `IsTriggered` is
/// always true for a spawn point).
fn is_spawn_point_valid(eng: &EngineApi<'_>, player: EdictId, spot: EdictId, radius: f32) -> bool {
    players_near(eng, player, spot, radius).is_empty()
}

/// The telefrag of `SelectSpawnSpot`: every spawn point is taken, so each player standing at
/// `spot` (other than `player`) takes 200 generic damage from the world
/// (`TakeDamage(VARS(eoNullEntity), VARS(eoNullEntity), 200, DMG_GENERIC)`), which kills them.
/// An edict flagged as a client that is no player of the game is left alone.
fn kill_spawn_occupants(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    player: EdictId,
    spot: EdictId,
) {
    let victims = players_near(eng, player, spot, MAX_PLAYER_USE_RADIUS);
    if !victims.is_empty() {
        tracing::debug!(?victims, "every spawn point is taken: the occupants of the first one die");
    }
    for victim in victims {
        if let Some(slot) = player_slot(g, eng, victim) {
            take_damage(g, eng, slot as u8, None, None, 200.0, dmg::GENERIC);
        }
    }
}

/// `CBasePlayer::SelectSpawnSpot`: from the spawn point after `last` of class `classname`
/// (wrapping round to the first), the first one that is free and not at the origin. When every
/// point is taken the point the search started from is used, after its occupants were killed
/// ([`kill_spawn_occupants`]). `None` when the map has no point of that class. (ReGameDLL
/// `player.cpp`, read 2026-11-22.)
fn select_spawn_spot(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    player: EdictId,
    classname: &str,
    last: Option<EdictId>,
    kill_occupants: bool,
) -> Option<EdictId> {
    let mut spot = eng.find_by_classname(last, classname);
    if spot.is_none() {
        spot = eng.find_by_classname(None, classname);
    }
    let first = spot;
    // The search cannot take more steps than there are entities, plus the wrap.
    let mut budget = eng.entity_ids().len() + 2;
    loop {
        if let Some(candidate) = spot
            && is_spawn_point_valid(eng, player, candidate, MAX_PLAYER_USE_RADIUS)
            && eng.ent(candidate).is_some_and(|v| v.origin != Vec3::ZERO)
        {
            return Some(candidate);
        }
        spot = eng.find_by_classname(spot, classname);
        budget -= 1;
        if spot == first || budget == 0 {
            break;
        }
    }
    if let Some(first) = first
        && kill_occupants
    {
        kill_spawn_occupants(g, eng, player, first);
    }
    first
}

/// `CBasePlayer::EntSelectSpawnPoint` for a deathmatch game: the spawn point `team` spawns
/// at, counter-terrorists at `info_player_start` and terrorists at `info_player_deathmatch`,
/// each team cycling through its points. When the team's class has no point it falls back to
/// the first `info_player_deathmatch`. Either way the point becomes the team's last one
/// (`ReturnSpot`). `None` when the map has no spawn point at all ("PutClientInServer: no
/// info_player_start on level"). (ReGameDLL `player.cpp`, read 2026-11-22.)
pub fn select_spawn_point(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    player: EdictId,
    team: SpawnTeam,
) -> Option<EdictId> {
    select_spawn_point_with(g, eng, player, team, true)
}

/// `CBasePlayer::EntSelectSpawnPoint` VIP branch (`player.cpp` 5668-5679): selects the first
/// `info_vip_start`, falling back to CT spawn points (`info_player_start`) if none exists.
pub fn select_vip_spawn_point(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    player: EdictId,
) -> Option<EdictId> {
    if let Some(spot) = eng.find_by_classname(None, VIP_SPAWN_CLASS) {
        g.last_ct_spawn = Some(spot);
        return Some(spot);
    }
    select_spawn_point(g, eng, player, SpawnTeam::CounterTerrorist)
}

/// [`select_spawn_point`] for a player that only joins (`ClientPutInServer` looks for the
/// counter-terrorists' spot to stand at): the same choice, and the point still becomes the
/// team's last one, but when every point is taken nobody is killed (the vanilla join path
/// would telefrag; a spectating newcomer has no business doing that).
pub fn select_join_spawn_point(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    player: EdictId,
    team: SpawnTeam,
) -> Option<EdictId> {
    select_spawn_point_with(g, eng, player, team, false)
}

/// `CBasePlayer::EntSelectSpawnPoint` for a player on no team (a spectator, or one that has not
/// picked): neither the counter-terrorists' nor the terrorists' branch applies, so it is the
/// first `info_player_deathmatch`, which `ReturnSpot` records as the counter-terrorists' last
/// spawn point (the player is not a terrorist). (ReGameDLL `player.cpp`, read 2026-11-22.)
fn select_no_team_spawn_point(g: &mut GameState, eng: &EngineApi<'_>) -> Option<EdictId> {
    let spot = eng.find_by_classname(None, T_SPAWN_CLASS);
    match spot {
        Some(spot) => g.last_ct_spawn = Some(spot),
        None => tracing::error!("PutClientInServer: no info_player_start on level"),
    }
    spot
}

/// `CHalfLifeMultiplay::GetPlayerSpawnSpot` for the player of `slot` (ReGameDLL `gamerules.cpp`
/// and `multiplay_gamerules.cpp`, read 2026-11-22): picks the spawn point of the player's team
/// as it is now (`EntSelectSpawnPoint`, which kills the players standing on every point when
/// all are taken) and puts the player there: origin one unit above the point, the view angles,
/// velocity and punch zero, the angles the point's, `fixangle` 1. Returns the point's own origin
/// and angles (the world's, zero, when the map has no point). The `target` of the point is
/// not fired: there are no trigger entities yet.
pub(crate) fn get_player_spawn_spot(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
) -> (Vec3, Vec3) {
    let id = eng.client_edict(slot);
    let (team, is_vip) = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map_or((None, false), |p| (Some(p.team), p.is_vip));
    let spot = if is_vip {
        select_vip_spawn_point(g, eng, id)
    } else {
        match team.and_then(|t| t.spawn_team()) {
            Some(spawn_team) => select_spawn_point(g, eng, id, spawn_team),
            None => select_no_team_spawn_point(g, eng),
        }
    };
    let (origin, angles) = spot
        .and_then(|s| eng.ent(s))
        .map_or((Vec3::ZERO, Vec3::ZERO), |v| (v.origin, v.angles));
    let placed = origin + Vec3::new(0.0, 0.0, 1.0);
    if let Some(v) = eng.ent_mut(id) {
        v.origin = placed;
        v.v_angle = Vec3::ZERO;
        v.velocity = Vec3::ZERO;
        v.angles = angles;
        v.punchangle = Vec3::ZERO;
        v.fixangle = 1;
    }
    eng.set_origin(id, placed);
    (origin, angles)
}

fn select_spawn_point_with(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    player: EdictId,
    team: SpawnTeam,
    kill_occupants: bool,
) -> Option<EdictId> {
    let (class, last) = match team {
        SpawnTeam::CounterTerrorist => (CT_SPAWN_CLASS, g.last_ct_spawn),
        SpawnTeam::Terrorist => (T_SPAWN_CLASS, g.last_t_spawn),
    };
    let spot = select_spawn_spot(g, eng, player, class, last, kill_occupants)
        .or_else(|| eng.find_by_classname(None, T_SPAWN_CLASS));
    let last = match team {
        SpawnTeam::CounterTerrorist => &mut g.last_ct_spawn,
        SpawnTeam::Terrorist => &mut g.last_t_spawn,
    };
    match spot {
        Some(spot) => *last = Some(spot),
        None => tracing::error!("PutClientInServer: no info_player_start on level"),
    }
    spot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atof_reads_the_leading_number() {
        assert_eq!(atof("2.5"), 2.5);
        assert_eq!(atof("  -3"), -3.0);
        assert_eq!(atof("1.2.3"), 1.2);
        assert_eq!(atof("abc"), 0.0);
        assert_eq!(atof(""), 0.0);
        assert_eq!(atof("16 sky"), 16.0);
        assert_eq!(atof("1x"), 1.0);
    }

    #[test]
    fn brush_models_are_numbered_from_one() {
        assert_eq!(brush_model_number("*12"), Some(12));
        assert_eq!(brush_model_number("*0"), None);
        assert_eq!(brush_model_number("maps/x.bsp"), None);
        assert_eq!(brush_model_number("*x"), None);
    }

    /// Every sample the movement tables can play (`cs_sim::pm::all_sample_names`: the step
    /// families and the wade sounds) and the ones `WaterMove` plays (the gasp and the swim
    /// sounds) are in the precache list (`ClientPrecache`); a new family fails the test.
    #[test]
    fn every_sample_the_pm_tables_can_produce_is_precached() {
        let mut wanted = cs_sim::pm::all_sample_names();
        assert!(wanted.len() >= 40, "{wanted:?}");
        for n in 1..=4 {
            wanted.push(format!("player/pl_swim{n}.wav"));
        }
        let missing: Vec<_> = wanted
            .iter()
            .filter(|w| !CLIENT_SOUNDS.contains(&w.as_str()))
            .collect();
        assert!(missing.is_empty(), "not precached: {missing:?}");
    }
}
