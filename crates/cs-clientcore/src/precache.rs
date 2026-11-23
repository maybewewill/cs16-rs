//! The precache tables the server's `svc_resourcelist` fills: models, sounds, event scripts,
//! generic files and decals, each indexed by the number the server uses in later messages
//! (`modelindex` in entity states, the sound index of `svc_sound`, the event index of
//! `svc_event`).
//!
//! Xash3D FWGS fills the same tables when it loads the resources (`CL_PrecacheResources`, later
//! than the list arrives); the tables here are filled when the list arrives and hold the names,
//! the loading is the shell's business (read 2026-11-23, `engine/client/cl_parse.c`).

use std::collections::BTreeMap;

use cs_proto48::messages::ResourceEntry;

use crate::resources::res_type;

/// One precached resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrecacheEntry {
    pub name: String,
    pub download_size: u32,
    pub flags: u8,
}

/// Index to resource, per kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrecacheTables {
    pub models: BTreeMap<u16, PrecacheEntry>,
    /// Sound names as sent, without the `sound/` directory (a leading `!` or `#` marks a
    /// sentence, `*` an inline entry).
    pub sounds: BTreeMap<u16, PrecacheEntry>,
    /// Event scripts, e.g. `events/ak47.sc`.
    pub events: BTreeMap<u16, PrecacheEntry>,
    pub generic: BTreeMap<u16, PrecacheEntry>,
    pub decals: BTreeMap<u16, PrecacheEntry>,
}

impl PrecacheTables {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Adds the entries of one `svc_resourcelist`. A later entry with the same index of the
    /// same kind replaces the earlier one. Skins and the world type have no table.
    pub fn add(&mut self, entries: &[ResourceEntry]) {
        for e in entries {
            let table = match e.res_type {
                res_type::MODEL => &mut self.models,
                res_type::SOUND => &mut self.sounds,
                res_type::EVENTSCRIPT => &mut self.events,
                res_type::GENERIC => &mut self.generic,
                res_type::DECAL => &mut self.decals,
                _ => continue,
            };
            table.insert(
                e.index,
                PrecacheEntry {
                    name: e.filename.clone(),
                    download_size: e.download_size,
                    flags: e.flags,
                },
            );
        }
    }

    pub fn model(&self, index: u16) -> Option<&str> {
        self.models.get(&index).map(|e| e.name.as_str())
    }

    pub fn sound(&self, index: u16) -> Option<&str> {
        self.sounds.get(&index).map(|e| e.name.as_str())
    }

    pub fn event(&self, index: u16) -> Option<&str> {
        self.events.get(&index).map(|e| e.name.as_str())
    }

    pub fn generic(&self, index: u16) -> Option<&str> {
        self.generic.get(&index).map(|e| e.name.as_str())
    }

    /// The map: model 1 (`CL_RegisterResources`: `cl.worldmodel = CL_ModelHandle( 1 )`).
    pub fn world_model(&self) -> Option<&str> {
        self.model(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(res_type: u8, name: &str, index: u16) -> ResourceEntry {
        ResourceEntry {
            res_type,
            filename: name.to_string(),
            index,
            download_size: 10,
            flags: 0,
            md5: None,
        }
    }

    #[test]
    fn entries_land_in_the_table_of_their_type() {
        let mut t = PrecacheTables::default();
        t.add(&[
            entry(res_type::MODEL, "maps/de_dust2.bsp", 1),
            entry(res_type::MODEL, "models/w_ak47.mdl", 5),
            entry(res_type::SOUND, "weapons/ak47-1.wav", 3),
            entry(res_type::EVENTSCRIPT, "events/ak47.sc", 1),
            entry(res_type::GENERIC, "sprites/hud.txt", 2),
            entry(res_type::DECAL, "{shot1", 7),
            entry(res_type::SKIN, "skin", 9),
        ]);
        assert_eq!(t.world_model(), Some("maps/de_dust2.bsp"));
        assert_eq!(t.model(5), Some("models/w_ak47.mdl"));
        assert_eq!(t.sound(3), Some("weapons/ak47-1.wav"));
        assert_eq!(t.event(1), Some("events/ak47.sc"));
        assert_eq!(t.generic(2), Some("sprites/hud.txt"));
        assert_eq!(t.decals[&7].name, "{shot1");
        assert_eq!(t.models.len(), 2);
        t.clear();
        assert_eq!(t, PrecacheTables::default());
    }
}
