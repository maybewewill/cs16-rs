//! The map's world and the static entities every map has: `worldspawn` (`CWorld`), the spawn
//! points, the brush entities with no logic yet, and the choice of a spawn point.

use cs_assets::bsp::SpawnTeam;
use cs_engine::consts::{
    FL_CLIENT, FL_WORLDBRUSH, MOVETYPE_NONE, MOVETYPE_PUSH, SOLID_BSP, SOLID_NOT,
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
/// `CONTENTS_LADDER`: the `skin` of a ladder entity.
const CONTENTS_LADDER: i16 = -16;
/// `EF_NODRAW`.
const EF_NODRAW: u32 = 128;
/// The classname of the CT spawn points.
pub const CT_SPAWN_CLASS: &str = "info_player_start";
/// The classname of the terrorist spawn points.
pub const T_SPAWN_CLASS: &str = "info_player_deathmatch";

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

/// The `PRECACHE_SOUND` calls of `ClientPrecache` (ReGameDLL `client.cpp`, read 2026-11-22),
/// plus the flashlight sound (`SOUND_FLASHLIGHT_ON` and `_OFF` are the same file).
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
    "common/bodysplat.wav",
    "player/pl_pain2.wav",
    "player/pl_pain4.wav",
    "player/pl_pain5.wav",
    "player/pl_pain6.wav",
    "player/pl_pain7.wav",
    "items/flashlight1.wav",
];

/// The `PRECACHE_SOUND` calls of `CWorld::Precache` (`world.cpp`, read 2026-11-22) that do not
/// belong to the weapons or the bots.
const WORLD_SOUNDS: [&str; 14] = [
    "common/null.wav",
    "items/suitchargeok1.wav",
    "items/gunpickup2.wav",
    "common/bodydrop3.wav",
    "common/bodydrop4.wav",
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
fn atof(text: &str) -> f32 {
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

fn precache_sound(eng: &mut EngineApi<'_>, name: &str) {
    if let Err(err) = eng.precache_sound(name) {
        tracing::error!(sound = %name, ?err, "sound not precached");
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

    /// `CWorld::Precache` without the weapons (`WeaponsPrecache`), the bots (`BotPrecache`),
    /// the sentences, the texture types and the sound entity: the server settings, the common
    /// resources, the player's (`ClientPrecache`) and the light styles.
    /// ReGameDLL `world.cpp`, `client.cpp`, read 2026-11-22.
    fn precache(eng: &mut EngineApi<'_>, id: EdictId) {
        eng.set_cvar("sv_gravity", "800");
        eng.set_cvar("sv_maxspeed", "900");
        eng.set_cvar("sv_stepsize", "18");
        eng.set_cvar("room_type", "0");

        for name in CLIENT_SOUNDS {
            precache_sound(eng, name);
        }
        for name in PLAYER_MODELS {
            precache_model(eng, name);
        }
        for name in WORLD_SOUNDS {
            precache_sound(eng, name);
        }
        // `sv_language` 1 is German, which has its own gibs.
        if eng.cvar_f32("sv_language") == 1.0 {
            precache_model(eng, "models/germangibs.mdl");
        } else {
            precache_model(eng, "models/hgibs.mdl");
            precache_model(eng, "models/agibs.mdl");
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

/// `func_ladder` (`CLadder`, ReGameDLL `triggers.cpp`, read 2026-11-22): a non-solid brush
/// whose `skin` is `CONTENTS_LADDER`, invisible unless `showtriggers` is set. Movement finds
/// ladders by their bounds (see [`ladder_bounds`]).
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

/// World-space bounds of every `func_ladder` of the map, in entity order: all the ladder test of
/// the movement reads.
pub fn ladder_bounds(eng: &EngineApi<'_>) -> Vec<(Vec3, Vec3)> {
    let mut bounds = Vec::new();
    let mut after = None;
    while let Some(id) = eng.find_by_classname(after, "func_ladder") {
        after = Some(id);
        if let Some(v) = eng.ent(id)
            && v.maxs != v.mins
        {
            bounds.push((v.origin + v.mins, v.origin + v.maxs));
        }
    }
    bounds
}

/// How a [`StaticBrush`] behaves.
#[derive(Debug, Clone, Copy)]
pub(crate) enum BrushKind {
    /// Solid and pushing (`MOVETYPE_PUSH`, `SOLID_BSP`), but it never moves: there is no door,
    /// button or train logic yet.
    Solid,
    /// `func_illusionary`: visible, never solid.
    Illusionary,
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

    /// An entry without a `*N` brush model is refused: there is nothing to collide with.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let (class, kind) = (self.class, self.kind);
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
            BrushKind::Illusionary => {
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
/// info_player_start on level"). The VIP spawn (`info_vip_start`) is not ported. (ReGameDLL
/// `player.cpp`, read 2026-11-22.)
pub fn select_spawn_point(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    player: EdictId,
    team: SpawnTeam,
) -> Option<EdictId> {
    select_spawn_point_with(g, eng, player, team, true)
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
    }

    #[test]
    fn brush_models_are_numbered_from_one() {
        assert_eq!(brush_model_number("*12"), Some(12));
        assert_eq!(brush_model_number("*0"), None);
        assert_eq!(brush_model_number("maps/x.bsp"), None);
        assert_eq!(brush_model_number("*x"), None);
    }
}
