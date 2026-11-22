//! What the game precaches, as the client's resource list shows it. The orders are those of the
//! original (the golden capture of the original server, read 2026-11-22): the index a sound or a
//! model has on the wire is its place in the precache order.

mod common;

use common::{Bench, entity, stub_spawns};
use cs_proto48::messages::{ResourceEntry, SvcMessage};

fn resources(b: &Bench) -> Vec<ResourceEntry> {
    b.probe
        .messages()
        .iter()
        .find_map(|m| match m {
            SvcMessage::ResourceList { resources, .. } => Some(resources.clone()),
            _ => None,
        })
        .expect("the probe was sent the resource list")
}

fn index_of(list: &[ResourceEntry], res_type: u8, name: &str) -> u16 {
    list.iter()
        .find(|r| r.res_type == res_type && r.filename == name)
        .unwrap_or_else(|| panic!("{name} (type {res_type}) is not in the resource list"))
        .index
}

const SOUND: u8 = 0;
const MODEL: u8 = 2;
const GENERIC: u8 = 4;

/// The sounds, models and generic files the original sends and this game did not: the
/// kevlar pickup sound opens the sound table (`item_suit`), the HUD and Geiger sounds, the scope
/// textures, `BotPrecache` (vanilla calls it with no bot around), the speaker icon of the voice
/// manager and the C4's own resources.
#[test]
fn the_resources_the_original_server_precaches_are_there() {
    let b = Bench::stub(stub_spawns());
    let list = resources(&b);
    // `CItemSuit::Precache` is the first precache of the first item: sound 1.
    assert_eq!(index_of(&list, SOUND, "items/tr_kevlar.wav"), 1);
    // ... and `CItemBattery::Precache` gives the second sound.
    assert_eq!(index_of(&list, SOUND, "items/gunpickup2.wav"), 2);
    for name in [
        "common/wpn_hudoff.wav",
        "common/wpn_select.wav",
        "common/wpn_denyselect.wav",
        "player/geiger6.wav",
        "player/geiger1.wav",
        "buttons/bell1.wav",
        "buttons/lightswitch2.wav",
        "ambience/quail1.wav",
        "events/tutor_msg.wav",
        "events/task_complete.wav",
        "weapons/c4_click.wav",
    ] {
        index_of(&list, SOUND, name);
    }
    for name in ["sprites/voiceicon.spr", "sprites/smoke.spr", "models/v_c4.mdl"] {
        index_of(&list, MODEL, name);
    }
    for name in [
        "sprites/scope_arc.tga",
        "sprites/scope_arc_nw.tga",
        "sprites/scope_arc_ne.tga",
        "sprites/scope_arc_sw.tga",
    ] {
        index_of(&list, GENERIC, name);
    }
}

/// The order that matters for the indices clients use: the speaker icon comes before the weapons
/// (`InstallGameRules` runs `CVoiceGameMgr::Init`), the C4's view model sits between the
/// grenades' and its own world model, the grenades' world models are `ClientPrecache`'s (after
/// the player models), `sprites/smoke.spr` follows the bullet smoke sprites, and the sounds of
/// `BotPrecache` come right before `common/null.wav`.
#[test]
fn the_precache_order_is_the_originals() {
    let b = Bench::stub(stub_spawns());
    let list = resources(&b);
    let model = |n: &str| index_of(&list, MODEL, n);
    let sound = |n: &str| index_of(&list, SOUND, n);
    assert!(model("sprites/voiceicon.spr") < model("models/w_kevlar.mdl"));
    assert_eq!(model("models/v_c4.mdl") + 1, model("models/w_backpack.mdl"));
    assert_eq!(model("models/shield/v_shield_smokegrenade.mdl") + 1, model("models/v_c4.mdl"));
    assert!(model("models/player.mdl") < model("models/w_flashbang.mdl"));
    assert!(model("models/w_flashbang.mdl") > model("models/p_knife.mdl"));
    assert_eq!(model("sprites/rifle_smoke3.spr") + 1, model("sprites/smoke.spr"));
    assert_eq!(model("sprites/smoke.spr") + 1, model("models/hgibs.mdl"));
    assert_eq!(sound("buttons/bell1.wav") + 11, sound("common/null.wav"));
    // `WeaponsPrecache`'s sounds, in the source's order: the explosion debris, the grenades, the
    // bullets, the dropped weapon.
    assert_eq!(sound("weapons/debris1.wav") + 6, sound("weapons/bullet_hit1.wav"));
    assert_eq!(sound("weapons/bullet_hit2.wav") + 1, sound("items/weapondrop1.wav"));
    // The armoury's models are its entities' to precache, not `WeaponsPrecache`'s: this map has
    // none, so the grenades' `w_` models come once, from `ClientPrecache`.
    assert!(model("models/player.mdl") < model("models/w_hegrenade.mdl"));
}

/// `CBreakable::Precache` (func_break.cpp): a metal breakable with a gib model of its own
/// precaches the metal's bust and hurt sounds and the map's model, not the material's.
#[test]
fn a_metal_breakable_precaches_its_sounds_and_the_models_of_the_map() {
    let b = Bench::stub(vec![
        entity(&[("classname", "worldspawn")]),
        entity(&[
            ("classname", "func_breakable"),
            ("model", "*1"),
            ("material", "2"),
            ("gibmodel", "models/mil_crategibs.mdl"),
        ]),
    ]);
    let list = resources(&b);
    let bust = index_of(&list, SOUND, "debris/bustmetal1.wav");
    assert_eq!(index_of(&list, SOUND, "debris/bustmetal2.wav"), bust + 1);
    assert_eq!(index_of(&list, SOUND, "debris/metal1.wav"), bust + 2);
    assert_eq!(index_of(&list, SOUND, "debris/metal3.wav"), bust + 4);
    index_of(&list, MODEL, "models/mil_crategibs.mdl");
    assert!(
        !list.iter().any(|r| r.filename == "models/metalplategibs.mdl"),
        "the map's `gibmodel` replaces the material's"
    );
}
