//! Precache tables: the model, sound and generic file names the network
//! protocol refers to by index.

use std::collections::HashMap;

use cs_proto48::messages::ResourceEntry;

pub const MAX_MODELS: usize = 512;
pub const MAX_SOUNDS: usize = 512;
pub const MAX_GENERIC: usize = 512;
/// `MAX_EVENTS` (`rehlds/common/qlimits.h`, read 2026-11-22): event indices are `1..256`.
pub const MAX_EVENTS: usize = 256;

/// Resource types of `svc_resourcelist`.
pub mod res_type {
    pub const SOUND: u8 = 0;
    pub const SKIN: u8 = 1;
    pub const MODEL: u8 = 2;
    pub const DECAL: u8 = 3;
    pub const GENERIC: u8 = 4;
    pub const EVENTSCRIPT: u8 = 5;
    pub const WORLD: u8 = 6;
}

#[derive(Debug, PartialEq)]
pub enum PrecacheError {
    /// The named table (`"model"`, `"sound"` or `"generic"`) has no free slot.
    TableFull(&'static str),
    /// A new name was precached after [`PrecacheTables::lock`].
    TooLate,
}

pub struct PrecacheTables {
    models: Table,
    sounds: Table,
    generic: Table,
    events: Table,
    locked: bool,
}

impl PrecacheTables {
    /// Models: `[0]` = "", `[1]` = `map_path`, `[2..]` = `"*1"..="*{n-1}"`
    /// for `submodel_count = n`. Sounds and generic: `[0]` = "".
    /// Submodels that do not fit [`MAX_MODELS`] are left out.
    pub fn new(map_path: &str, submodel_count: usize) -> Self {
        let mut models = Table::new("model", MAX_MODELS);
        let names = std::iter::once(map_path.to_string())
            .chain((1..submodel_count).map(|i| format!("*{i}")));
        for name in names {
            if models.insert(&name, false).is_err() {
                break;
            }
        }
        Self {
            models,
            sounds: Table::new("sound", MAX_SOUNDS),
            generic: Table::new("generic", MAX_GENERIC),
            events: Table::new("event", MAX_EVENTS),
            locked: false,
        }
    }

    pub fn precache_model(&mut self, name: &str) -> Result<u16, PrecacheError> {
        self.models.insert(name, self.locked)
    }

    pub fn precache_sound(&mut self, name: &str) -> Result<u16, PrecacheError> {
        self.sounds.insert(name, self.locked)
    }

    pub fn precache_generic(&mut self, name: &str) -> Result<u16, PrecacheError> {
        self.generic.insert(name, self.locked)
    }

    /// `EV_Precache_internal` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read 2026-11-22): the
    /// 1-based index of the event script `name` (`events/ak47.sc`, case-insensitive), a new one
    /// taking the next free index (at most 255). Not ported: it loads the script file and
    /// stops the server when it is missing; the file is the game directory's business here.
    pub fn precache_event(&mut self, name: &str) -> Result<u16, PrecacheError> {
        self.events.insert(name, self.locked)
    }

    /// Whether `index` names a precached event (`g_psv.event_precache[index].pszScript`).
    pub fn event_exists(&self, index: u16) -> bool {
        index >= 1 && usize::from(index) < self.events.names.len()
    }

    pub fn event_index(&self, name: &str) -> Option<u16> {
        self.events.index(name)
    }

    pub fn model_index(&self, name: &str) -> Option<u16> {
        self.models.index(name)
    }

    pub fn sound_index(&self, name: &str) -> Option<u16> {
        self.sounds.index(name)
    }

    /// Called after `server_activate`: from now on a new name is
    /// [`PrecacheError::TooLate`]; names already present still resolve.
    pub fn lock(&mut self) {
        self.locked = true;
    }

    /// Sounds, then models, then generic files, then the event scripts (type
    /// [`res_type::EVENTSCRIPT`], flag `RES_FATALIFMISSING`, in `SV_CreateResourceList` order
    /// last, after the decals), each in index order without the empty entry 0. Submodel names (`*N`) get size 0; every other name
    /// gets `size_of(name)`. Order is a working assumption about HLDS.
    pub fn resource_list(&self, size_of: &dyn Fn(&str) -> u32) -> Vec<ResourceEntry> {
        [
            (res_type::SOUND, &self.sounds),
            (res_type::MODEL, &self.models),
            (res_type::GENERIC, &self.generic),
        ]
        .into_iter()
        .flat_map(|(res_type, table)| {
            table
                .names
                .iter()
                .enumerate()
                .skip(1)
                .map(move |(index, name)| ResourceEntry {
                    res_type,
                    filename: name.clone(),
                    // Lossless: a table never holds more than 512 names.
                    index: index as u16,
                    download_size: if name.starts_with('*') {
                        0
                    } else {
                        size_of(name)
                    },
                    flags: 0,
                    md5: None,
                })
        })
        .chain(self.events.names.iter().enumerate().skip(1).map(|(index, name)| ResourceEntry {
            res_type: res_type::EVENTSCRIPT,
            filename: name.clone(),
            // Lossless: the table never holds more than 256 names.
            index: index as u16,
            download_size: size_of(name),
            flags: RES_FATALIFMISSING,
            md5: None,
        }))
        .collect()
    }
}

/// `RES_FATALIFMISSING` (`rehlds/common/resource.h`): the client stops when it cannot load the
/// resource. Assumption: the value (1 << 0) is from memory.
const RES_FATALIFMISSING: u8 = 1;

/// One precache table. Index 0 is the empty name.
struct Table {
    kind: &'static str,
    capacity: usize,
    /// First spelling seen, with `\` turned into `/`.
    names: Vec<String>,
    /// Lowercased, `/`-separated name to index.
    lookup: HashMap<String, u16>,
}

impl Table {
    fn new(kind: &'static str, capacity: usize) -> Self {
        Self {
            kind,
            capacity,
            names: vec![String::new()],
            lookup: HashMap::from([(String::new(), 0)]),
        }
    }

    fn index(&self, name: &str) -> Option<u16> {
        self.lookup.get(&key(name)).copied()
    }

    fn insert(&mut self, name: &str, locked: bool) -> Result<u16, PrecacheError> {
        let key = key(name);
        if let Some(&index) = self.lookup.get(&key) {
            return Ok(index);
        }
        if locked {
            return Err(PrecacheError::TooLate);
        }
        if self.names.len() >= self.capacity {
            return Err(PrecacheError::TableFull(self.kind));
        }
        // Lossless: capacity is at most 512.
        let index = self.names.len() as u16;
        self.names.push(name.replace('\\', "/"));
        self.lookup.insert(key, index);
        Ok(index)
    }
}

fn key(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_and_submodels_occupy_first_model_slots() {
        let t = PrecacheTables::new("maps/de_dust2.bsp", 3);
        assert_eq!(t.model_index("maps/de_dust2.bsp"), Some(1));
        assert_eq!(t.model_index("*1"), Some(2));
        assert_eq!(t.model_index("*2"), Some(3));
        assert_eq!(t.model_index("*3"), None);
        assert_eq!(t.model_index(""), Some(0));
        assert_eq!(t.sound_index(""), Some(0));
    }

    #[test]
    fn precache_is_idempotent_and_case_insensitive() {
        let mut t = PrecacheTables::new("maps/de_dust2.bsp", 1);
        let a = t.precache_model("models/Player/GIGN/gign.mdl").unwrap();
        assert_eq!(a, 2);
        assert_eq!(t.precache_model("models/player/gign/gign.mdl"), Ok(a));
        assert_eq!(t.precache_model("MODELS\\PLAYER\\GIGN\\GIGN.MDL"), Ok(a));
        assert_eq!(t.model_index("models\\player\\gign\\gign.mdl"), Some(a));
        assert_eq!(t.precache_model(""), Ok(0));

        let s = t.precache_sound("weapons\\ak47-1.wav").unwrap();
        assert_eq!(s, 1);
        assert_eq!(t.precache_sound("Weapons/AK47-1.wav"), Ok(s));
        assert_eq!(t.sound_index("weapons/ak47-1.wav"), Some(s));
        assert_eq!(t.precache_sound(""), Ok(0));

        assert_eq!(t.precache_generic("sprites/hud.txt"), Ok(1));
        assert_eq!(t.precache_generic("SPRITES/HUD.TXT"), Ok(1));

        // The first spelling is kept, with forward slashes.
        let list = t.resource_list(&|_| 0);
        assert!(list.iter().any(|r| r.filename == "weapons/ak47-1.wav"));
        assert!(
            list.iter()
                .any(|r| r.filename == "models/Player/GIGN/gign.mdl")
        );
        assert_eq!(list.len(), 4);
    }

    #[test]
    fn table_full_and_locked_are_errors() {
        let mut t = PrecacheTables::new("maps/de_dust2.bsp", 1);
        // Slots 0 and 1 are taken; 2..=511 fill the table.
        for i in 2..MAX_MODELS {
            assert_eq!(t.precache_model(&format!("models/m{i}.mdl")), Ok(i as u16));
        }
        assert_eq!(
            t.precache_model("models/one_too_many.mdl"),
            Err(PrecacheError::TableFull("model"))
        );
        for i in 1..MAX_SOUNDS {
            t.precache_sound(&format!("s{i}.wav")).unwrap();
        }
        assert_eq!(
            t.precache_sound("extra.wav"),
            Err(PrecacheError::TableFull("sound"))
        );
        for i in 1..MAX_GENERIC {
            t.precache_generic(&format!("g{i}.txt")).unwrap();
        }
        assert_eq!(
            t.precache_generic("extra.txt"),
            Err(PrecacheError::TableFull("generic"))
        );

        let mut t = PrecacheTables::new("maps/de_dust2.bsp", 1);
        let gign = t.precache_model("models/gign.mdl").unwrap();
        let shot = t.precache_sound("shot.wav").unwrap();
        let hud = t.precache_generic("hud.txt").unwrap();
        t.lock();
        assert_eq!(
            t.precache_model("models/new.mdl"),
            Err(PrecacheError::TooLate)
        );
        assert_eq!(t.precache_sound("new.wav"), Err(PrecacheError::TooLate));
        assert_eq!(t.precache_generic("new.txt"), Err(PrecacheError::TooLate));
        assert_eq!(t.precache_model("MODELS/GIGN.MDL"), Ok(gign));
        assert_eq!(t.precache_sound("shot.wav"), Ok(shot));
        assert_eq!(t.precache_generic("hud.txt"), Ok(hud));
        assert_eq!(t.precache_model(""), Ok(0));
        assert_eq!(t.model_index("models/new.mdl"), None);
    }

    #[test]
    fn resource_list_orders_sounds_models_generic_and_zero_sizes_submodels() {
        let mut t = PrecacheTables::new("maps/de_dust2.bsp", 2);
        t.precache_model("models/gign.mdl").unwrap();
        t.precache_sound("a.wav").unwrap();
        t.precache_sound("b.wav").unwrap();
        t.precache_generic("hud.txt").unwrap();
        let list = t.resource_list(&|name| name.len() as u32 * 10);
        let entry = |res_type, filename: &str, index, download_size| ResourceEntry {
            res_type,
            filename: filename.to_string(),
            index,
            download_size,
            flags: 0,
            md5: None,
        };
        assert_eq!(
            list,
            vec![
                entry(res_type::SOUND, "a.wav", 1, 50),
                entry(res_type::SOUND, "b.wav", 2, 50),
                entry(res_type::MODEL, "maps/de_dust2.bsp", 1, 170),
                entry(res_type::MODEL, "*1", 2, 0),
                entry(res_type::MODEL, "models/gign.mdl", 3, 150),
                entry(res_type::GENERIC, "hud.txt", 1, 70),
            ]
        );
    }
}
