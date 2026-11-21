//! Spawning the entities of a map's entity lump when the map is activated: the engine allocates
//! an edict per entry, fills the variables it understands from the entry's keys and hands the
//! entry to [`GameDll::spawn_entity`], which accepts or refuses it.
//!
//! HLDS passes the keys to the game in file order. [`ServerMap::entities`] holds parsed
//! `Entity` values whose keys sit in a `HashMap`, so the file order is gone; the keys are passed
//! in a deterministic order instead (see [`MAX_ENTITY_KEYS`]). This is a known deviation caused
//! by the parsed type.

use cs_assets::bsp::Entity;
use glam::Vec3;

use crate::collision::MAX_WORLD_COORD;
use crate::edict::EntVars;
use crate::game::GameDll;
use crate::server::Server;

/// Most keys one lump entry hands to the game, `classname` included. Further keys are dropped.
pub const MAX_ENTITY_KEYS: usize = 256;
/// Longest key kept, in bytes. A pair with a longer key is dropped, not truncated.
pub const MAX_KEY_BYTES: usize = 64;
/// Longest value kept, in bytes. A pair with a longer value is dropped, not truncated.
pub const MAX_VALUE_BYTES: usize = 1024;

/// A lump entry reduced to what the engine applies and the game receives.
struct PreparedEntry {
    classname: String,
    /// `classname` first, then the other kept pairs sorted by key.
    keys: Vec<(String, String)>,
}

/// The pairs of `entity` that are handed on: pairs whose key exceeds [`MAX_KEY_BYTES`] or whose
/// value exceeds [`MAX_VALUE_BYTES`] are dropped, `classname` is always kept (first), and the
/// rest, sorted by key bytes, fill the list up to [`MAX_ENTITY_KEYS`]. `None` when the entry has
/// no usable `classname` (missing, empty or over the limits): such an entry is skipped.
fn prepare_entry(entity: &Entity) -> Option<PreparedEntry> {
    let fits =
        |key: &str, value: &str| key.len() <= MAX_KEY_BYTES && value.len() <= MAX_VALUE_BYTES;
    let classname = entity.properties.get("classname")?;
    if classname.is_empty() || !fits("classname", classname) {
        return None;
    }
    let mut rest: Vec<(&String, &String)> = entity
        .properties
        .iter()
        .filter(|(key, value)| key.as_str() != "classname" && fits(key, value))
        .collect();
    rest.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let keys = std::iter::once(("classname".to_string(), classname.clone()))
        .chain(
            rest.into_iter()
                .take(MAX_ENTITY_KEYS - 1)
                .map(|(k, v)| (k.clone(), v.clone())),
        )
        .collect();
    Some(PreparedEntry {
        classname: classname.clone(),
        keys,
    })
}

/// The first three whitespace-separated numbers of `value`; `None` with fewer than three or
/// with any non-finite one. Extra tokens are ignored.
fn parse_vec3(value: &str) -> Option<Vec3> {
    let mut parts = value.split_whitespace();
    let mut next = || {
        let n: f32 = parts.next()?.parse().ok()?;
        n.is_finite().then_some(n)
    };
    Some(Vec3::new(next()?, next()?, next()?))
}

/// One finite number; `None` for anything else.
fn parse_number(value: &str) -> Option<f32> {
    value.trim().parse::<f32>().ok().filter(|n| n.is_finite())
}

/// One finite number clamped to `0..=255` and truncated toward zero.
fn parse_byte(value: &str) -> Option<u8> {
    // `as` truncates toward zero; the clamp makes the cast exact.
    parse_number(value).map(|n| n.clamp(0.0, 255.0) as u8)
}

/// Writes the keys the engine understands into `vars`; a value that does not parse leaves its
/// field at the default.
fn apply_keys(vars: &mut EntVars, keys: &[(String, String)]) {
    let get = |name: &str| {
        keys.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    if let Some(v) = get("classname") {
        vars.classname = v.to_string();
    }
    if let Some(origin) = get("origin")
        .and_then(parse_vec3)
        .filter(|o| o.abs().max_element() <= MAX_WORLD_COORD)
    {
        vars.origin = origin;
    }
    if let Some(angles) = get("angles").and_then(parse_vec3) {
        vars.angles = angles;
    } else if let Some(angle) = get("angle").and_then(parse_number) {
        vars.angles = if angle == -1.0 {
            Vec3::new(-90.0, 0.0, 0.0)
        } else if angle == -2.0 {
            Vec3::new(90.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, angle, 0.0)
        };
    }
    if let Some(v) = get("targetname") {
        vars.targetname = v.to_string();
    }
    if let Some(v) = get("target") {
        vars.target = v.to_string();
    }
    if let Some(flags) = get("spawnflags").and_then(|v| v.trim().parse::<i64>().ok()) {
        // The low 32 bits, so `-1` sets every flag.
        vars.spawnflags = flags as u32;
    }
    if let Some(v) = get("rendermode").and_then(parse_byte) {
        vars.rendermode = v;
    }
    if let Some(v) = get("renderamt").and_then(parse_byte) {
        vars.renderamt = v;
    }
    if let Some(v) = get("renderfx").and_then(parse_byte) {
        vars.renderfx = v;
    }
    if let Some(color) = get("rendercolor").and_then(parse_vec3) {
        vars.rendercolor = color.to_array().map(|c| c.clamp(0.0, 255.0) as u8);
    }
}

impl<G: GameDll> Server<G> {
    /// Spawns the entities of the current map's lump, in lump order, at the current server
    /// time. The first `worldspawn` entry uses the world edict; every other entry takes a new
    /// edict, which is freed again when the game refuses the entry. An entry without a usable
    /// `classname` is skipped. When the pool has no free edict the remaining entries are
    /// skipped and one error is logged with their number.
    pub(crate) fn spawn_map_entities(&mut self) {
        let total = self.map.entities.len();
        let mut world_taken = false;
        for index in 0..total {
            let Some(entry) = self.map.entities.get(index).and_then(prepare_entry) else {
                continue;
            };
            let is_world = !world_taken && entry.classname == "worldspawn";
            let id = if is_world {
                world_taken = true;
                self.edicts.world()
            } else if let Some(id) = self.edicts.alloc(self.time) {
                id
            } else {
                tracing::error!(
                    map = %self.map.name,
                    skipped = total - index,
                    "no free edict for the map's entities; the remaining entries are skipped"
                );
                break;
            };
            if let Some(edict) = self.edicts.get_mut(id) {
                apply_keys(&mut edict.vars, &entry.keys);
            }
            let model = entry
                .keys
                .iter()
                .find(|(k, _)| k == "model")
                .map(|(_, v)| v.as_str());
            let accepted = self.with_game(|game, eng| {
                if let Some(model) = model {
                    // A model that is not precached yet stays unset; the game sees the key.
                    eng.set_model(id, model);
                }
                let accepted = game.spawn_entity(eng, id, &entry.classname, &entry.keys);
                if accepted {
                    // The game may not have placed the entity itself; HLDS links it in
                    // `SET_ORIGIN` and `SET_MODEL`, which a game that spawns it as is skips.
                    eng.link_entity(id, false);
                }
                accepted
            });
            if !accepted && !is_world {
                // A game that already removed the entity itself leaves a stale id, which the
                // pool ignores.
                if self.edicts.get(id).is_some() {
                    self.world.forget(id.index);
                }
                self.edicts.free(id, self.time);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn entity(pairs: &[(&str, &str)]) -> Entity {
        Entity {
            properties: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
        }
    }

    fn vars_from(pairs: &[(&str, &str)]) -> EntVars {
        let mut vars = EntVars::default();
        let keys: Vec<_> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        apply_keys(&mut vars, &keys);
        vars
    }

    #[test]
    fn vectors_need_three_finite_numbers() {
        assert_eq!(parse_vec3(" 1 2.5\t-3 "), Some(Vec3::new(1.0, 2.5, -3.0)));
        assert_eq!(parse_vec3("1 2 3 4"), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(parse_vec3("1 2"), None);
        assert_eq!(parse_vec3("1 2 x"), None);
        assert_eq!(parse_vec3("1 NaN 3"), None);
        assert_eq!(parse_vec3("1 2 inf"), None);
        assert_eq!(parse_vec3("1e999 2 3"), None);
        assert_eq!(parse_vec3(""), None);
    }

    #[test]
    fn origin_beyond_the_world_is_refused() {
        let beyond = MAX_WORLD_COORD * 2.0;
        let v = vars_from(&[("origin", &format!("{beyond} 0 0"))]);
        assert_eq!(v.origin, Vec3::ZERO);
        let edge = MAX_WORLD_COORD;
        let v = vars_from(&[("origin", &format!("{edge} -{edge} 0"))]);
        assert_eq!(v.origin, Vec3::new(edge, -edge, 0.0));
        let v = vars_from(&[("origin", &format!("{} 0 0", edge + 1024.0))]);
        assert_eq!(v.origin, Vec3::ZERO);
        // The limit applies to origin only.
        let v = vars_from(&[("angles", &format!("{beyond} 0 0"))]);
        assert_eq!(v.angles, Vec3::new(beyond, 0.0, 0.0));
    }

    #[test]
    fn angle_is_used_only_without_a_valid_angles() {
        assert_eq!(
            vars_from(&[("angle", "45")]).angles,
            Vec3::new(0.0, 45.0, 0.0)
        );
        assert_eq!(
            vars_from(&[("angle", "-1")]).angles,
            Vec3::new(-90.0, 0.0, 0.0)
        );
        assert_eq!(
            vars_from(&[("angle", "-2")]).angles,
            Vec3::new(90.0, 0.0, 0.0)
        );
        assert_eq!(
            vars_from(&[("angle", "45"), ("angles", "1 2 3")]).angles,
            Vec3::new(1.0, 2.0, 3.0)
        );
        // A broken `angles` falls back to `angle`.
        assert_eq!(
            vars_from(&[("angle", "45"), ("angles", "1 2")]).angles,
            Vec3::new(0.0, 45.0, 0.0)
        );
        assert_eq!(vars_from(&[("angle", "NaN")]).angles, Vec3::ZERO);
        assert_eq!(vars_from(&[("angle", "1 2")]).angles, Vec3::ZERO);
    }

    #[test]
    fn spawnflags_are_a_strict_integer_of_32_bits() {
        assert_eq!(vars_from(&[("spawnflags", " 5 ")]).spawnflags, 5);
        assert_eq!(vars_from(&[("spawnflags", "-1")]).spawnflags, u32::MAX);
        assert_eq!(vars_from(&[("spawnflags", "4294967296")]).spawnflags, 0);
        // Truncation to the low 32 bits, not a rejection.
        assert_eq!(vars_from(&[("spawnflags", "4294967297")]).spawnflags, 1);
        assert_eq!(vars_from(&[("spawnflags", "5.0")]).spawnflags, 0);
        assert_eq!(vars_from(&[("spawnflags", "abc")]).spawnflags, 0);
        assert_eq!(
            vars_from(&[("spawnflags", "99999999999999999999")]).spawnflags,
            0
        );
    }

    #[test]
    fn render_values_are_clamped_and_truncated() {
        let v = vars_from(&[
            ("rendermode", "300"),
            ("renderamt", "127.9"),
            ("renderfx", "-4"),
            ("rendercolor", "255.5 -1 12.9"),
        ]);
        assert_eq!((v.rendermode, v.renderamt, v.renderfx), (255, 127, 0));
        assert_eq!(v.rendercolor, [255, 0, 12]);
        let v = vars_from(&[
            ("renderamt", "NaN"),
            ("renderfx", "1e999"),
            ("rendercolor", "1 2"),
        ]);
        assert_eq!((v.renderamt, v.renderfx, v.rendercolor), (0, 0, [0, 0, 0]));
        let v = vars_from(&[("rendercolor", "1 2 NaN")]);
        assert_eq!(v.rendercolor, [0, 0, 0]);
    }

    #[test]
    fn entries_without_a_usable_classname_are_skipped() {
        assert!(prepare_entry(&entity(&[("origin", "1 2 3")])).is_none());
        assert!(prepare_entry(&entity(&[("classname", "")])).is_none());
        let long = "c".repeat(MAX_VALUE_BYTES + 1);
        assert!(prepare_entry(&entity(&[("classname", &long)])).is_none());
        let ok = "c".repeat(MAX_VALUE_BYTES);
        assert!(prepare_entry(&entity(&[("classname", &ok)])).is_some());
    }

    #[test]
    fn keys_are_bounded_and_ordered() {
        let mut pairs: Vec<(String, String)> = (0..400)
            .map(|i| (format!("k{i:04}"), "v".to_string()))
            .collect();
        pairs.push(("classname".to_string(), "x".to_string()));
        pairs.push(("k".repeat(MAX_KEY_BYTES + 1), "v".to_string()));
        pairs.push(("big".to_string(), "v".repeat(MAX_VALUE_BYTES + 1)));
        pairs.push(("a_exact".to_string(), "v".repeat(MAX_VALUE_BYTES)));
        let entry = prepare_entry(&Entity {
            properties: pairs.into_iter().collect(),
        })
        .unwrap();
        assert_eq!(entry.keys.len(), MAX_ENTITY_KEYS);
        assert_eq!(entry.keys[0].0, "classname");
        assert_eq!(entry.keys[1].0, "a_exact");
        assert_eq!(entry.keys[2].0, "k0000");
        assert!(entry.keys.iter().all(|(k, _)| k != "big"));
        assert!(entry.keys.iter().all(|(k, _)| k.len() <= MAX_KEY_BYTES));
        // A key of exactly the limit is kept (it sorts after the `k` keys, so check separately).
        let exact = prepare_entry(&entity(&[
            ("classname", "x"),
            (&"m".repeat(MAX_KEY_BYTES), "v"),
            (&"n".repeat(MAX_KEY_BYTES + 1), "v"),
        ]))
        .unwrap();
        assert_eq!(exact.keys.len(), 2);
        assert_eq!(exact.keys[1].0.len(), MAX_KEY_BYTES);
        assert!(entry.keys.windows(2).skip(1).all(|w| w[0].0 < w[1].0));
    }
}
