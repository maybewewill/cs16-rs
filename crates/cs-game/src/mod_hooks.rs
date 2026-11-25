//! ABI v2 ReAPI gameplay hook sink, custom weapon registry, 1-line Protocol 48 VFX emitter,
//! and CS 1.6 mod-host native gameplay helpers for [`CsGame`](crate::CsGame).
//!
//! Clean-room GPL-3.0-or-later implementation referencing ReAPI / ReGameDLL_CS hook semantics
//! (`reapi_gamedll_const.inc`, `regamedll/dlls/player.cpp`, `client.cpp`, `weapons.cpp`,
//! `multiplay_gamerules.cpp`, read 2026-11-26) and AMX Mod X (`cstrike.inc`, `fun.inc`,
//! `dhudmessage.inc`, `amxconst.inc`, read 2026-11-26).

use std::collections::BTreeMap;

use cs_engine::consts::{FL_GODMODE, MOVETYPE_NOCLIP, MOVETYPE_WALK};
use cs_engine::{EdictId, EngineApi, MoveFilter, MsgDest};
use cs_mod_api::{
    ArmorKind, BarTimeSpec, BombEventCtx, CanBuyItemCtx, CanHavePlayerItemCtx, CustomWeaponSpec,
    DhudMessageSpec, GrenadeThrowCtx, HookAction, HookPhase, HostageEventCtx, HudMessageSpec,
    MenuSelectCtx, PlayerJumpCtx, PlayerKilledCtx, PlayerPostThinkCtx, PlayerPreThinkCtx,
    PlayerSayCtx, PlayerSpawnCtx, PlayerTakeDamageCtx, PlayersShouldCollideCtx, RoundEndCtx,
    RoundStartCtx, ScreenFadeSpec, ScreenShakeSpec, ShowMotdSpec, TeamKind, VfxOp, WeaponEventCtx,
};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::messages::{drc, write_svc_director};
use cs_proto48::tempent::{self, BeamParams};
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use crate::GameState;
use crate::inventory::{self, GRENADE_SLOT, KNIFE_SLOT, PISTOL_SLOT, PRIMARY_WEAPON_SLOT};
use crate::messages;
use crate::money;
use crate::player::{self, ATTN_NORM, ArmorType, CHAN_VOICE, DEAD_NO, PITCH_NORM, Team};
use crate::teams;
use crate::weapons::{MAX_AMMO_SLOTS, WeaponId, ammo_index, item_info, weapon_info};

/// Stock sprites precached when mods are active so all 12 `vfx_*` primitives resolve cleanly
/// without disturbing vanilla CS 1.6 precache ordering when no mods are loaded.
pub const VFX_STOCK_SPRITES: [&str; 5] = [
    "sprites/laserbeam.spr",
    "sprites/shockwave.spr",
    "sprites/zerogxplode.spr",
    "sprites/flare1.spr",
    "sprites/blood.spr",
];

/// Hook receiver attached to [`GameState`] by a mod host (`cs-mod-host` / `cs-amxx`).
///
/// Vanilla `CsGame` leaves `hook_sink == None`, incurring zero overhead.
pub trait CsHookSink: Send {
    fn on_player_spawn(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut PlayerSpawnCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_player_take_damage(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut PlayerTakeDamageCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_player_killed(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut PlayerKilledCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_player_pre_think(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut PlayerPreThinkCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_player_post_think(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut PlayerPostThinkCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_player_jump(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut PlayerJumpCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_players_should_collide(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _ctx: &mut PlayersShouldCollideCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_can_buy_item(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut CanBuyItemCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_can_have_player_item(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut CanHavePlayerItemCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_weapon_deploy(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut WeaponEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_weapon_primary_attack(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut WeaponEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_weapon_reload(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut WeaponEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_player_say(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut PlayerSayCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_menu_select(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut MenuSelectCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_freeze_time_end(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_round_start(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut RoundStartCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_round_end(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut RoundEndCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_bomb_planting(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut BombEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_bomb_planted(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut BombEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_bomb_defusing(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut BombEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_bomb_defused(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut BombEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_bomb_exploded(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut BombEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_hostage_touched(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut HostageEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_hostage_rescued(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut HostageEventCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_grenade_throw(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _ctx: &mut GrenadeThrowCtx,
    ) -> HookAction {
        HookAction::Continue
    }

    fn on_entity_touch(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _phase: HookPhase,
        _toucher: EdictId,
        _other: EdictId,
    ) -> HookAction {
        HookAction::Continue
    }
}

/// Safely invokes `f` against `g.hook_sink` without holding a borrow on `g`.
pub(crate) fn fire_hook<F>(g: &mut GameState, eng: &mut EngineApi<'_>, f: F) -> HookAction
where
    F: FnOnce(&mut dyn CsHookSink, &mut GameState, &mut EngineApi<'_>) -> HookAction,
{
    let Some(mut sink) = g.hook_sink.take() else {
        return HookAction::Continue;
    };
    let action = f(&mut *sink, g, eng);
    if g.hook_sink.is_none() {
        g.hook_sink = Some(sink);
    }
    action
}

// ---------------------------------------------------------------------------
// Custom Weapon Registry
// ---------------------------------------------------------------------------

/// Server-wide declarative custom weapon registry and per-player active overrides.
#[derive(Debug, Clone, Default)]
pub struct CustomWeaponRegistry {
    specs: BTreeMap<String, CustomWeaponSpec>,
    /// `(slot, base WeaponId)` -> registered `id_name`.
    player_weapons: BTreeMap<(u8, WeaponId), String>,
}

impl CustomWeaponRegistry {
    pub fn register(&mut self, spec: CustomWeaponSpec) {
        let key = spec.id_name.trim().to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        self.specs.insert(key, spec);
    }

    pub fn get(&self, id_name: &str) -> Option<&CustomWeaponSpec> {
        self.specs.get(&id_name.trim().to_ascii_lowercase())
    }

    pub fn all_specs(&self) -> impl Iterator<Item = &CustomWeaponSpec> {
        self.specs.values()
    }

    pub fn bind_player_weapon(&mut self, slot: u8, base: WeaponId, id_name: &str) {
        let key = id_name.trim().to_ascii_lowercase();
        if self.specs.contains_key(&key) {
            self.player_weapons.insert((slot, base), key);
        }
    }

    pub fn clear_player_weapon(&mut self, slot: u8, base: WeaponId) {
        self.player_weapons.remove(&(slot, base));
    }

    pub fn clear_player(&mut self, slot: u8) {
        self.player_weapons.retain(|&(s, _), _| s != slot);
    }

    pub fn clear_all_players(&mut self) {
        self.player_weapons.clear();
    }

    pub fn active_for(&self, slot: u8, base: WeaponId) -> Option<&CustomWeaponSpec> {
        let name = self.player_weapons.get(&(slot, base))?;
        self.specs.get(name)
    }

    pub fn active_name_for(&self, slot: u8, base: WeaponId) -> Option<&str> {
        self.player_weapons.get(&(slot, base)).map(String::as_str)
    }

    /// Precaches all models and fire sounds declared by registered custom weapons.
    pub fn precache_all(&self, eng: &mut EngineApi<'_>) {
        for spec in self.specs.values() {
            for model in [&spec.v_model, &spec.p_model, &spec.w_model]
                .into_iter()
                .flatten()
            {
                if !model.is_empty() {
                    let _ = eng.precache_model(model);
                }
            }
            if let Some(sound) = &spec.fire_sound
                && !sound.is_empty()
            {
                let _ = eng.precache_sound(sound);
            }
        }
    }
}

/// Registers a [`CustomWeaponSpec`] and attempts to precache its assets if the precache table is open.
pub fn register_custom_weapon(
    g: &mut GameState,
    eng: Option<&mut EngineApi<'_>>,
    spec: CustomWeaponSpec,
) {
    if let Some(eng) = eng {
        for model in [&spec.v_model, &spec.p_model, &spec.w_model]
            .into_iter()
            .flatten()
        {
            if !model.is_empty() {
                let _ = eng.precache_model(model);
            }
        }
        if let Some(sound) = &spec.fire_sound
            && !sound.is_empty()
        {
            let _ = eng.precache_sound(sound);
        }
    }
    g.custom_weapons.register(spec);
}

/// Grants a previously registered custom weapon (`id_name`) to `slot`, replacing its base slot
/// weapon if needed, applying custom clip size, and deploying its custom `v_`/`p_` models.
pub fn give_custom_weapon(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    id_name: &str,
) -> bool {
    let Some(spec) = g.custom_weapons.get(id_name).cloned() else {
        return false;
    };
    let Some(base_id) = WeaponId::from_i32(i32::from(spec.base_weapon)) else {
        return false;
    };
    let Some(info) = item_info(base_id) else {
        return false;
    };
    g.custom_weapons.bind_player_weapon(slot, base_id, &spec.id_name);
    let already = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| inventory::has_player_item(p, base_id));
    if !already {
        let _ = inventory::give_named_item(g, eng, slot, info.name);
    }
    if let Some(clip) = spec.clip_size
        && let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
        && let Some((s, i)) = p.find_item(base_id)
    {
        p.items[s][i].weapon.clip = clip.max(0);
    }
    if !crate::weapons::core::select_item(g, eng, slot, base_id) {
        // Already active: re-deploy so custom viewmodel/weaponmodel take effect immediately.
        crate::weapons::core::with_weapon(g, eng, slot, base_id, |b, w| {
            b.deploy(w);
        });
    }
    true
}

// ---------------------------------------------------------------------------
// 1-Line Protocol 48 VFX Emitter
// ---------------------------------------------------------------------------

/// Precaches the stock VFX sprites (`laserbeam`, `shockwave`, `zerogxplode`, `flare1`, `blood`).
pub fn precache_vfx_sprites(eng: &mut EngineApi<'_>) {
    for sprite in VFX_STOCK_SPRITES {
        let _ = eng.precache_model(sprite);
    }
}

fn vfx_dest_pvs(origin: Vec3, slot: Option<u8>) -> MsgDest {
    match slot {
        Some(s) => MsgDest::One(s),
        None => MsgDest::Pvs(origin),
    }
}

fn vfx_dest_broadcast(slot: Option<u8>) -> MsgDest {
    match slot {
        Some(s) => MsgDest::One(s),
        None => MsgDest::Broadcast,
    }
}

fn sprite_index_fallback(eng: &EngineApi<'_>, primary: &str, fallback: &str) -> u16 {
    eng.model_index(primary)
        .or_else(|| eng.model_index(fallback))
        .unwrap_or(0)
}

/// Translates a declarative [`VfxOp`] into its exact Protocol 48 `svc_temp_entity` wire packet.
pub fn emit_vfx(eng: &mut EngineApi<'_>, op: &VfxOp) {
    match op {
        VfxOp::ShockwaveRing {
            origin,
            radius,
            rgba,
            life_decisec,
            width,
            slot,
        } => {
            let center = Vec3::from_array(*origin);
            let axis = Vec3::new(center.x, center.y, center.z + radius.max(1.0));
            let sprite = sprite_index_fallback(eng, "sprites/shockwave.spr", "sprites/laserbeam.spr");
            let te = tempent::beam_cylinder(
                center,
                axis,
                &BeamParams {
                    sprite,
                    start_frame: 0,
                    frame_rate: 0,
                    life: (*life_decisec).max(1),
                    width: (*width).max(1),
                    noise: 0,
                    color: [rgba[0], rgba[1], rgba[2]],
                    brightness: rgba[3],
                    speed: 0,
                },
            );
            eng.send_temp_entity(vfx_dest_pvs(center, *slot), &te);
        }
        VfxOp::LaserBeam {
            start,
            end,
            rgba,
            life_decisec,
            width,
            noise,
            slot,
        } => {
            let s = Vec3::from_array(*start);
            let e = Vec3::from_array(*end);
            let sprite = sprite_index_fallback(eng, "sprites/laserbeam.spr", "sprites/flare1.spr");
            let te = tempent::beam_points(
                s,
                e,
                &BeamParams {
                    sprite,
                    start_frame: 0,
                    frame_rate: 0,
                    life: (*life_decisec).max(1),
                    width: (*width).max(1),
                    noise: *noise,
                    color: [rgba[0], rgba[1], rgba[2]],
                    brightness: rgba[3],
                    speed: 0,
                },
            );
            eng.send_temp_entity(vfx_dest_pvs(s, *slot), &te);
        }
        VfxOp::FollowTrail {
            ent_index,
            rgba,
            life_decisec,
            width,
            slot,
        } => {
            let sprite = sprite_index_fallback(eng, "sprites/laserbeam.spr", "sprites/flare1.spr");
            let te = tempent::beam_follow(
                *ent_index,
                sprite,
                (*life_decisec).max(1),
                (*width).max(1),
                [rgba[0], rgba[1], rgba[2]],
                rgba[3],
            );
            eng.send_temp_entity(vfx_dest_broadcast(*slot), &te);
        }
        VfxOp::KillBeam { ent_index, slot } => {
            let te = tempent::kill_beam(*ent_index);
            eng.send_temp_entity(vfx_dest_broadcast(*slot), &te);
        }
        VfxOp::AuraLight {
            origin,
            radius,
            rgb,
            life_decisec,
            decay,
            slot,
        } => {
            let pos = Vec3::from_array(*origin);
            let te = tempent::dlight(pos, *radius, *rgb, (*life_decisec).max(1), *decay);
            eng.send_temp_entity(vfx_dest_pvs(pos, *slot), &te);
        }
        VfxOp::Sparks { origin, slot } => {
            let pos = Vec3::from_array(*origin);
            let te = tempent::sparks(pos);
            eng.send_temp_entity(vfx_dest_pvs(pos, *slot), &te);
        }
        VfxOp::Implosion {
            origin,
            radius,
            count,
            life_decisec,
            slot,
        } => {
            let pos = Vec3::from_array(*origin);
            let te = tempent::implosion(pos, *radius, *count, (*life_decisec).max(1));
            eng.send_temp_entity(vfx_dest_pvs(pos, *slot), &te);
        }
        VfxOp::TeleportSplash { origin, slot } => {
            let pos = Vec3::from_array(*origin);
            let te = tempent::teleport(pos);
            eng.send_temp_entity(vfx_dest_pvs(pos, *slot), &te);
        }
        VfxOp::LavaSplash { origin, slot } => {
            let pos = Vec3::from_array(*origin);
            let te = tempent::lava_splash(pos);
            eng.send_temp_entity(vfx_dest_pvs(pos, *slot), &te);
        }
        VfxOp::Explosion {
            origin,
            scale,
            flags,
            slot,
        } => {
            let pos = Vec3::from_array(*origin);
            let sprite = sprite_index_fallback(eng, "sprites/zerogxplode.spr", "sprites/fexplo.spr");
            let te = tempent::explosion(pos, sprite, (*scale).max(1), 15, *flags);
            let dest = match slot {
                Some(s) => MsgDest::One(*s),
                None => MsgDest::Pas(pos),
            };
            eng.send_temp_entity(dest, &te);
        }
        VfxOp::BloodStream {
            origin,
            dir,
            color,
            speed,
            slot,
        } => {
            let pos = Vec3::from_array(*origin);
            let d = Vec3::from_array(*dir);
            let te = tempent::blood_stream(pos, d, *color, *speed);
            eng.send_temp_entity(vfx_dest_pvs(pos, *slot), &te);
        }
        VfxOp::WorldDecal {
            origin,
            decal_name,
            slot,
        } => {
            let pos = Vec3::from_array(*origin);
            let idx = eng.decal_index(decal_name);
            let idx = if idx >= 0 { idx } else { 0 };
            if let Some(te) = tempent::world_decal_for(pos, idx) {
                eng.send_temp_entity(vfx_dest_broadcast(*slot), &te);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Classic CS 1.6 HUD / DHUD / MOTD / Screen Effects Helpers
// ---------------------------------------------------------------------------

fn fixed_signed_16(val: f32, scale: f32) -> i16 {
    if val < 0.0 && (val - (-1.0)).abs() < 0.001 {
        return -1;
    }
    (val * scale).round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

fn fixed_unsigned_16(val: f32, scale: f32) -> u16 {
    (val * scale).round().clamp(0.0, f32::from(u16::MAX)) as u16
}

/// Sends a classic GoldSrc `TE_TEXTMESSAGE` HUD message (`channels 0..=4`).
pub fn send_hud_message(eng: &mut EngineApi<'_>, spec: &HudMessageSpec) {
    let channel = if spec.channel == 0 || spec.channel > 4 {
        1
    } else {
        spec.channel
    };
    let x = fixed_signed_16(spec.x, 8192.0);
    let y = fixed_signed_16(spec.y, 8192.0);
    let fadein = fixed_unsigned_16(spec.fadein, 256.0);
    let fadeout = fixed_unsigned_16(spec.fadeout, 256.0);
    let holdtime = fixed_unsigned_16(spec.hold, 256.0);
    let fxtime = (spec.effect == 2).then(|| fixed_unsigned_16(spec.fxtime, 256.0));
    let text = if spec.text.len() > 511 {
        let mut end = 511;
        while !spec.text.is_char_boundary(end) {
            end -= 1;
        }
        &spec.text[..end]
    } else {
        spec.text.as_str()
    };
    let te = tempent::text_message(
        channel,
        x,
        y,
        spec.effect,
        spec.rgba1,
        spec.rgba2,
        fadein,
        fadeout,
        holdtime,
        fxtime,
        text,
    );
    let dest = match spec.slot {
        Some(s) => MsgDest::One(s),
        None => MsgDest::All,
    };
    eng.send_temp_entity(dest, &te);
}

/// Sends a large Director HUD message (`svc_director` `DRC_CMD_MESSAGE`, AMXX `dhudmessage.inc`).
pub fn send_dhud_message(eng: &mut EngineApi<'_>, spec: &DhudMessageSpec) {
    let mut payload = MsgWriter::new();
    payload.write_u8(drc::CMD_MESSAGE);
    payload.write_u8(spec.effect);
    let packed_color: u32 =
        (u32::from(spec.rgb[0]) << 16) | (u32::from(spec.rgb[1]) << 8) | u32::from(spec.rgb[2]);
    payload.write_u32(packed_color);
    payload.write_f32(spec.x);
    payload.write_f32(spec.y);
    payload.write_f32(spec.fadein);
    payload.write_f32(spec.fadeout);
    payload.write_f32(spec.hold);
    payload.write_f32(spec.fxtime);
    // Director message payload max is 255 bytes; header is 30 bytes, leaving 224 bytes + NUL.
    let max_text = 224usize;
    let text = if spec.text.len() > max_text {
        let mut end = max_text;
        while !spec.text.is_char_boundary(end) {
            end -= 1;
        }
        &spec.text[..end]
    } else {
        spec.text.as_str()
    };
    payload.write_str(text);

    let mut wire = MsgWriter::new();
    write_svc_director(&mut wire, payload.as_slice());
    let dest = match spec.slot {
        Some(s) => MsgDest::One(s),
        None => MsgDest::All,
    };
    eng.send(dest, wire.as_slice());
}

/// Sends an authentic CS 1.6 VGUI2 `MOTD.res` black-and-orange HTML/text MOTD window (`ServerName` + chunked `MOTD`).
pub fn send_motd_window(eng: &mut EngineApi<'_>, spec: &ShowMotdSpec) {
    if !spec.title.is_empty() {
        eng.send_usermsg(MsgDest::One(spec.slot), &CsUserMsg::ServerName(spec.title.clone()));
    }
    let chunks = messages::motd_chunks(spec.body.as_bytes());
    if chunks.is_empty() {
        let _ = messages::send_raw(eng, MsgDest::One(spec.slot), "MOTD", &[1, 0]);
        return;
    }
    for (finished, chunk) in chunks {
        let mut payload = Vec::with_capacity(chunk.len() + 2);
        payload.push(u8::from(finished));
        payload.extend_from_slice(chunk);
        payload.push(0);
        let _ = messages::send_raw(eng, MsgDest::One(spec.slot), "MOTD", &payload);
    }
}

/// Sends a classic `ScreenFade` usermsg.
pub fn send_screen_fade(eng: &mut EngineApi<'_>, spec: &ScreenFadeSpec) {
    let dest = match spec.slot {
        Some(s) => MsgDest::One(s),
        None => MsgDest::All,
    };
    messages::screen_fade(eng, dest, spec.duration_sec, spec.hold_sec, spec.flags, spec.rgba);
}

/// Sends a classic `ScreenShake` usermsg.
pub fn send_screen_shake(eng: &mut EngineApi<'_>, spec: &ScreenShakeSpec) {
    let dest = match spec.slot {
        Some(s) => MsgDest::One(s),
        None => MsgDest::All,
    };
    messages::screen_shake(eng, dest, spec.amplitude, spec.duration_sec, spec.frequency);
}

/// Sends a classic `BarTime` progress bar (`seconds == 0` hides it).
pub fn send_bar_time(g: &mut GameState, eng: &mut EngineApi<'_>, spec: &BarTimeSpec) {
    player::set_progress_bar_time(g, eng, spec.slot, i32::from(spec.seconds));
}

// ---------------------------------------------------------------------------
// Comprehensive CS 1.6 Player / Fun / Engine / CStrike Natives
// ---------------------------------------------------------------------------

pub fn set_player_health(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, hp: f32) -> bool {
    if !hp.is_finite() {
        return false;
    }
    let clamped = hp.max(0.0);
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.health = clamped;
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.health = clamped;
    }
    messages::health(eng, slot, clamped.clamp(0.0, 255.0) as u8);
    messages::broadcast_health_info(g, eng, slot, clamped as i32);
    true
}

pub fn set_player_max_health(g: &mut GameState, slot: u8, max_hp: f32) -> bool {
    if !max_hp.is_finite() || max_hp <= 0.0 {
        return false;
    }
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.max_health = max_hp;
    true
}

pub fn set_player_armor(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    armor: i32,
    kind: ArmorKind,
) -> bool {
    let clamped = armor.clamp(0, 999);
    let kevlar = match kind {
        ArmorKind::None => ArmorType::None,
        ArmorKind::Kevlar => ArmorType::Vest,
        ArmorKind::VestHelm => ArmorType::VestHelm,
    };
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.armor = clamped as f32;
    p.kevlar = kevlar;
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::Battery(clamped.clamp(0, i32::from(i16::MAX)) as i16),
    );
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::ArmorType(u8::from(kevlar == ArmorType::VestHelm)),
    );
    true
}

pub fn set_player_money(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    amount: i32,
    flash: bool,
) -> bool {
    let clamped = amount.clamp(0, money::MAX_ACCOUNT);
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.money = clamped;
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::Money {
            amount: clamped,
            flash,
        },
    );
    messages::broadcast_account(g, eng, slot, clamped);
    true
}

pub fn add_player_money(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    delta: i32,
    flash: bool,
) -> bool {
    let Some(cur) = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| p.money)
    else {
        return false;
    };
    set_player_money(g, eng, slot, cur.saturating_add(delta), flash)
}

pub fn set_player_score(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    frags: i32,
    deaths: i32,
) -> bool {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.frags = frags;
    p.deaths = deaths;
    let id = eng.client_edict(slot);
    if let Some(p_ref) = g.players[usize::from(slot)].as_ref() {
        messages::score_info(eng, id.index, p_ref);
    }
    true
}

pub fn set_player_team(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    team: TeamKind,
    model_class: Option<u8>,
) -> bool {
    let new_team = match team {
        TeamKind::Unassigned => Team::Unassigned,
        TeamKind::Terrorist => Team::Terrorist,
        TeamKind::Ct => Team::Ct,
        TeamKind::Spectator => Team::Spectator,
    };
    let class = {
        let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
            return false;
        };
        p.team = new_team;
        if let Some(c) = model_class {
            p.class = c;
        }
        p.class
    };
    player::apply_model(eng, slot, new_team, class);
    teams::team_change_update(g, eng, slot);
    true
}

pub fn respawn_player(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    player::respawn_player(g, eng, slot)
}

/// Classic `user_slap` (`fun.inc`): subtracts `power` HP, adds random horizontal/upward impulse,
/// plays slap sound, and kicks `punchangle`. Kills the player if HP drops to `<= 0`.
pub fn slap_player(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    power: i32,
    random_dir: bool,
) -> bool {
    let id = eng.client_edict(slot);
    let alive = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.alive);
    if !alive {
        return false;
    }
    let rx = if random_dir {
        g.random_range(-180, 180) as f32
    } else {
        120.0
    };
    let ry = if random_dir {
        g.random_range(-180, 180) as f32
    } else {
        0.0
    };
    let px = g.random_range(-12, 12) as f32;
    let py = g.random_range(-12, 12) as f32;
    if let Some(v) = eng.ent_mut(id) {
        v.velocity.x += rx;
        v.velocity.y += ry;
        v.velocity.z = v.velocity.z.max(0.0) + 200.0;
        v.punchangle.x += px;
        v.punchangle.y += py;
    }
    eng.emit_sound(id, CHAN_VOICE, "player/pl_pain2.wav", 1.0, ATTN_NORM, PITCH_NORM);
    if power > 0 {
        let cur = g.players[usize::from(slot)].as_ref().map_or(0.0, |p| p.health);
        if cur - power as f32 <= 0.0 {
            let _ = player::kill(g, eng, slot);
        } else {
            let _ = set_player_health(g, eng, slot, cur - power as f32);
        }
    }
    true
}

/// Classic `user_kill` (`amxmodx.inc`): slays a living player cleanly, optionally preserving frags.
pub fn slay_player(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    preserve_frags: bool,
) -> bool {
    let prev_frags = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map(|p| p.frags);
    let ok = player::kill(g, eng, slot);
    if ok
        && preserve_frags
        && let Some(prev) = prev_frags
        && let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
    {
        p.frags = prev;
        let id = eng.client_edict(slot);
        if let Some(p_ref) = g.players[usize::from(slot)].as_ref() {
            messages::score_info(eng, id.index, p_ref);
        }
    }
    ok
}

pub fn set_player_godmode(eng: &mut EngineApi<'_>, slot: u8, enabled: bool) -> bool {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent_mut(id) else {
        return false;
    };
    if enabled {
        v.flags |= FL_GODMODE;
    } else {
        v.flags &= !FL_GODMODE;
    }
    true
}

pub fn set_player_noclip(eng: &mut EngineApi<'_>, slot: u8, enabled: bool) -> bool {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent_mut(id) else {
        return false;
    };
    v.movetype = if enabled { MOVETYPE_NOCLIP } else { MOVETYPE_WALK };
    true
}

pub fn set_player_maxspeed(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    maxspeed: f32,
) -> bool {
    if !maxspeed.is_finite() {
        return false;
    }
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    if maxspeed <= 0.0 {
        p.maxspeed_override = None;
        player::reset_max_speed(g, eng, slot);
    } else {
        p.maxspeed_override = Some(maxspeed);
        let id = eng.client_edict(slot);
        if let Some(v) = eng.ent_mut(id) {
            v.max_speed = maxspeed;
        }
    }
    true
}

pub fn set_player_gravity(eng: &mut EngineApi<'_>, slot: u8, gravity: f32) -> bool {
    if !gravity.is_finite() || gravity < 0.0 {
        return false;
    }
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent_mut(id) else {
        return false;
    };
    v.gravity = gravity;
    true
}

pub fn set_player_footsteps(g: &mut GameState, slot: u8, silent: bool) -> bool {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.silent_footsteps = silent;
    true
}

pub fn set_player_rendering(
    eng: &mut EngineApi<'_>,
    slot: u8,
    fx: u8,
    rgb: [u8; 3],
    mode: u8,
    amount: u8,
) -> bool {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent_mut(id) else {
        return false;
    };
    v.renderfx = fx;
    v.rendercolor = rgb;
    v.rendermode = mode;
    v.renderamt = amount;
    true
}

pub fn set_player_model(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    model_name: &str,
) -> bool {
    let trimmed = model_name.trim();
    if trimmed.is_empty() {
        return reset_player_model(g, eng, slot);
    }
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.custom_model = Some(trimmed.to_string());
    eng.set_client_key(slot, "model", trimmed);
    let full_path = if trimmed.contains('/') {
        trimmed.to_string()
    } else {
        format!("models/player/{trimmed}/{trimmed}.mdl")
    };
    let id = eng.client_edict(slot);
    let _ = eng.set_model(id, &full_path);
    true
}

pub fn reset_player_model(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    let (team, class) = {
        let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
            return false;
        };
        p.custom_model = None;
        (p.team, p.class)
    };
    player::apply_model(eng, slot, team, class);
    true
}

/// Traces a line from `slot`'s eyes along its view angles up to `8192` units (`get_user_origin(..., 3)`).
pub fn get_aim_origin(eng: &EngineApi<'_>, slot: u8) -> Option<[f32; 3]> {
    let id = eng.client_edict(slot);
    let v = eng.ent(id)?;
    let start = v.origin + v.view_ofs;
    let (fwd, _, _) = angle_vectors(v.v_angle);
    let end = start + fwd * 8192.0;
    let tr = eng.trace_line(start, end, MoveFilter::Normal, Some(id));
    Some(tr.trace.end_pos.to_array())
}

pub fn give_item(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, item: &str) -> bool {
    match item {
        "item_kevlar" => set_player_armor(g, eng, slot, 100, ArmorKind::Kevlar),
        "item_assaultsuit" => set_player_armor(g, eng, slot, 100, ArmorKind::VestHelm),
        "item_thighpack" => {
            player::give_defuser(g, eng, slot);
            true
        }
        "weapon_c4" => player::make_bomber(g, eng, slot),
        "weapon_shield" => {
            inventory::give_shield(g, eng, slot, true);
            true
        }
        other => inventory::give_named_item(g, eng, slot, other),
    }
}

pub fn strip_user_weapons(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    g.custom_weapons.clear_player(slot);
    inventory::remove_all_items(g, eng, slot, false);
}

pub fn get_weapon_clip(g: &GameState, slot: u8, weapon_id: u8) -> i32 {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return -1;
    };
    let wid = if weapon_id == 0 {
        let Some(active) = p.active_item else {
            return -1;
        };
        active
    } else {
        let Some(w) = WeaponId::from_i32(i32::from(weapon_id)) else {
            return -1;
        };
        w
    };
    p.find_item(wid)
        .map_or(-1, |(s, i)| p.items[s][i].weapon.clip)
}

pub fn set_weapon_clip(g: &mut GameState, slot: u8, weapon_id: u8, clip: i32) -> bool {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    let wid = if weapon_id == 0 {
        let Some(active) = p.active_item else {
            return false;
        };
        active
    } else {
        let Some(w) = WeaponId::from_i32(i32::from(weapon_id)) else {
            return false;
        };
        w
    };
    let Some((s, i)) = p.find_item(wid) else {
        return false;
    };
    p.items[s][i].weapon.clip = clip.max(0);
    true
}

fn bpammo_index_for_weapon(weapon_id: u8) -> Option<usize> {
    let wid = WeaponId::from_i32(i32::from(weapon_id))?;
    let ammo_name = item_info(wid)
        .and_then(|i| i.ammo1)
        .or_else(|| weapon_info(wid).map(|w| w.ammo_name1).filter(|s| !s.is_empty()))?;
    usize::try_from(ammo_index(ammo_name))
        .ok()
        .filter(|&idx| idx < MAX_AMMO_SLOTS)
}

pub fn get_user_bpammo(g: &GameState, slot: u8, weapon_id: u8) -> i32 {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return 0;
    };
    let idx = if let Some(i) = bpammo_index_for_weapon(weapon_id) {
        i
    } else if let Some(active) = p.active_item
        && let Some(i) = bpammo_index_for_weapon(active as i32 as u8)
    {
        i
    } else {
        return 0;
    };
    p.ammo[idx]
}

pub fn set_user_bpammo(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    weapon_id: u8,
    amount: i32,
) -> bool {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    let idx = if let Some(i) = bpammo_index_for_weapon(weapon_id) {
        i
    } else if let Some(active) = p.active_item
        && let Some(i) = bpammo_index_for_weapon(active as i32 as u8)
    {
        i
    } else {
        return false;
    };
    let clamped = amount.clamp(0, 255);
    p.ammo[idx] = clamped;
    crate::weapons::core::tabulate_ammo(p);
    messages::ammo_x(eng, slot, idx as u8, clamped as u8);
    true
}

pub fn set_user_defuser(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, has: bool) -> bool {
    if g.players.get(usize::from(slot)).is_none_or(Option::is_none) {
        return false;
    }
    if has {
        player::give_defuser(g, eng, slot);
    } else {
        player::remove_defuser(g, eng, slot);
    }
    true
}

pub fn set_user_nvg(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, has: bool) -> bool {
    let has_defuser = {
        let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
            return false;
        };
        p.has_night_vision = has;
        if !has {
            p.night_vision_on = false;
        }
        p.has_defuser
    };
    crate::buy::send_item_status(eng, slot, has, has_defuser);
    if !has {
        eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::NvgToggle(false));
    }
    true
}

/// Drops the head weapon of inventory slot `inv_slot` (`1` = primary, `2` = pistol, `3` = knife,
/// `4` = grenade, `5` = C4; `0` = active weapon).
pub fn drop_slot(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, inv_slot: u8) -> bool {
    let target_name: Option<&'static str> = match usize::from(inv_slot) {
        0 => None,
        PRIMARY_WEAPON_SLOT | PISTOL_SLOT | KNIFE_SLOT | GRENADE_SLOT | 5 => {
            let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
                return false;
            };
            let Some(first) = p.items.get(usize::from(inv_slot)).and_then(|l| l.first()) else {
                return false;
            };
            item_info(first.weapon.id).map(|info| info.name)
        }
        _ => return false,
    };
    inventory::drop_player_item(g, eng, slot, target_name).is_some()
}

/// Applies damage to `slot` through `player::take_damage` (running `PlayerTakeDamage` Pre/Post hooks).
pub fn apply_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    attacker: Option<u8>,
    damage: f32,
    bits: u32,
) -> bool {
    if !damage.is_finite() || damage < 0.0 {
        return false;
    }
    let att_edict = attacker.map(|a| eng.client_edict(a));
    player::take_damage(g, eng, slot, att_edict, att_edict, damage, bits)
}

/// Returns whether `slot` is alive (`deadflag == DEAD_NO` and `player.alive`).
pub fn is_player_alive(g: &GameState, eng: &EngineApi<'_>, slot: u8) -> bool {
    g.players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.alive)
        && eng
            .ent(eng.client_edict(slot))
            .is_some_and(|v| v.deadflag == DEAD_NO)
}

/// Returns `(weapon_id, clip, bpammo)` for every weapon currently held by `slot`.
pub fn snapshot_player_weapons(g: &GameState, slot: u8) -> Vec<(u8, i32, i32)> {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for slot_items in &p.items {
        for inst in slot_items {
            let wid = inst.weapon.id as i32 as u8;
            out.push((wid, inst.weapon.clip, get_user_bpammo(g, slot, wid)));
        }
    }
    out
}

/// Converts a round-end sentence (e.g. `#Target_Bombed`) into its ABI v2 numeric reason code.
pub(crate) fn round_end_reason_code(sentence: &str) -> u8 {
    match sentence {
        "#Target_Bombed" => 1,
        "#VIP_Escaped" => 2,
        "#VIP_Assassinated" => 3,
        "#Terrorists_Escaped" => 4,
        "#CTs_PreventEscape" => 5,
        "#Escaping_Terrorists_Neutralized" => 6,
        "#Bomb_Defused" => 7,
        "#CTs_Win" => 8,
        "#Terrorists_Win" => 9,
        "#Round_Draw" => 10,
        "#All_Hostages_Rescued" => 11,
        "#Target_Saved" => 12,
        "#Hostages_Not_Rescued" => 13,
        "#Terrorists_Not_Escaped" => 14,
        "#VIP_Not_Escaped" => 15,
        "#Game_Commencing" => 16,
        _ => 0,
    }
}
