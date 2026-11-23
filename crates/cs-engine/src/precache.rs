//! Precache tables: the model, sound and generic file names the network
//! protocol refers to by index.

use std::collections::HashMap;
use std::sync::Arc;

use cs_assets::mdl::StudioPoseData;
use cs_proto48::messages::ResourceEntry;

/// Reads the bytes of a game file (`models/player/gign/gign.mdl`) for the engine: the owner of
/// the server gives it the virtual file system, which the engine does not depend on.
pub type ModelLoader = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

pub const MAX_MODELS: usize = 512;
pub const MAX_SOUNDS: usize = 512;
pub const MAX_GENERIC: usize = 512;
/// `MAX_EVENTS` (`rehlds/common/qlimits.h`, read 2026-11-22): event indices are `1..256`.
pub const MAX_EVENTS: usize = 256;

/// Most decals the server knows by name: `MAX_BASE_DECALS`, `1 << MAX_DECAL_INDEX_BITS` with 9
/// bits (ReHLDS `rehlds/engine/decals.h`, read 2026-11-22). `Decal_Init` stops the server with
/// an error past it; here the list is cut instead.
pub const MAX_DECALS: usize = 512;
/// Characters of a decal name the server keeps (`decalname_t::name` is 16 with the NUL).
const MAX_DECAL_NAME: usize = 15;

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
    /// `model_precache_flags`, by model index: [`RES_FATALIFMISSING`] for the world, the
    /// submodels and every model the game did not precache with a leading `!`.
    model_flags: Vec<u8>,
    sounds: Table,
    generic: Table,
    events: Table,
    locked: bool,
    /// The decal names of `decals.wad` in the order of `sv_decalnames`; see
    /// [`decal_names_from_wads`]. Empty until the server sets them; it keeps them across maps.
    decals: Vec<String>,
    /// Where a precached `.mdl` is read from; without it no studio model is known and every
    /// entity is traced as its box.
    loader: Option<ModelLoader>,
    /// The studio models read at precache, by model index (`Mod_Extradata` of the original).
    studio: HashMap<u16, Arc<StudioPoseData>>,
    /// The parsed studio models by lowercase name, kept across map changes
    /// ([`studio_cache`](Self::studio_cache)): the game precaches the same player and weapon
    /// models on every map, and parsing them again costs a file read and a parse each.
    studio_by_name: HashMap<String, Arc<StudioPoseData>>,
}

/// Most parsed studio models [`PrecacheTables`] keeps by name: the size of the model table,
/// so the cache holds no more than one map's worth of models plus what the last maps left.
const MAX_STUDIO_CACHE: usize = MAX_MODELS;

impl PrecacheTables {
    /// Models: `[0]` = "", `[1]` = `map_path`, `[2..]` = `"*1"..="*{n-1}"`
    /// for `submodel_count = n`. Sounds and generic: `[0]` = "".
    /// Submodels that do not fit [`MAX_MODELS`] are left out.
    pub fn new(map_path: &str, submodel_count: usize) -> Self {
        let mut models = Table::new("model", MAX_MODELS);
        let names = std::iter::once(map_path.to_string())
            .chain((1..submodel_count).map(|i| format!("*{i}")));
        // `SV_SpawnServer`: the world and the submodels are `RES_FATALIFMISSING` (ReHLDS
        // `rehlds/engine/sv_main.cpp`, read 2026-11-22).
        let mut model_flags = vec![0];
        for name in names {
            if models.insert(&name, false).is_err() {
                break;
            }
            model_flags.push(RES_FATALIFMISSING);
        }
        Self {
            models,
            model_flags,
            sounds: Table::new("sound", MAX_SOUNDS),
            generic: Table::new("generic", MAX_GENERIC),
            events: Table::new("event", MAX_EVENTS),
            locked: false,
            decals: Vec::new(),
            loader: None,
            studio: HashMap::new(),
            studio_by_name: HashMap::new(),
        }
    }

    /// Sets the decal list `pfnDecalIndex` answers from (`sv_decalnames`, filled once by
    /// `Decal_Init`), cut to [`MAX_DECALS`].
    pub fn set_decals(&mut self, mut names: Vec<String>) {
        names.truncate(MAX_DECALS);
        // `Q_strncpy(sv_decalnames[i].name, ..., sizeof(name) - 1)`: 15 characters (lump names
        // are one byte per character).
        for name in &mut names {
            if let Some((end, _)) = name.char_indices().nth(MAX_DECAL_NAME) {
                name.truncate(end);
            }
        }
        self.decals = names;
    }

    /// The decal list, to carry it over to the tables of the next map.
    pub fn decals(&self) -> &[String] {
        &self.decals
    }

    /// `PF_DecalIndex` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read 2026-11-22): the position of
    /// the first decal called `name` (`Q_stricmp`), or -1 for an unknown name.
    pub fn decal_index(&self, name: &str) -> i32 {
        self.decals
            .iter()
            .position(|d| d.eq_ignore_ascii_case(name))
            .map_or(-1, |i| i as i32)
    }

    /// `PF_precache_model_I` (ReHLDS `rehlds/engine/pr_cmds.cpp`, read 2026-11-22): a leading
    /// `!` marks the model optional and is not part of the name; any other model is
    /// `RES_FATALIFMISSING` in the resource list. The flag is set when the name is new only.
    pub fn precache_model(&mut self, name: &str) -> Result<u16, PrecacheError> {
        let (optional, name) = match name.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, name),
        };
        let index = self.models.insert(name, self.locked)?;
        if usize::from(index) >= self.model_flags.len() {
            self.model_flags.resize(usize::from(index) + 1, 0);
            if !optional {
                self.model_flags[usize::from(index)] |= RES_FATALIFMISSING;
            }
        }
        self.load_studio(index, name);
        Ok(index)
    }

    /// Sets where studio models are read from and reads the `.mdl` models precached so far.
    pub fn set_model_loader(&mut self, loader: Option<ModelLoader>) {
        self.loader = loader;
        // What the old loader read is not what the new one reads.
        self.studio_by_name.clear();
        let names: Vec<(u16, String)> = self
            .models
            .names
            .iter()
            .enumerate()
            .map(|(i, n)| (i as u16, n.clone()))
            .collect();
        for (index, name) in names {
            self.load_studio(index, &name);
        }
    }

    /// The parsed studio models by name, to hand on to the tables of the next map with
    /// [`restore_studio_cache`](Self::restore_studio_cache) (cheap: the models are shared).
    pub fn studio_cache(&self) -> HashMap<String, Arc<StudioPoseData>> {
        self.studio_by_name.clone()
    }

    /// Takes the cache of the previous map's tables (see [`studio_cache`](Self::studio_cache)),
    /// bounded to [`MAX_STUDIO_CACHE`] names, after the loader is set. The models precached so
    /// far that the cache knows are not read again.
    pub fn restore_studio_cache(&mut self, cache: HashMap<String, Arc<StudioPoseData>>) {
        self.studio_by_name = cache;
        while self.studio_by_name.len() > MAX_STUDIO_CACHE {
            let Some(name) = self.studio_by_name.keys().next().cloned() else {
                break;
            };
            self.studio_by_name.remove(&name);
        }
        let names: Vec<(u16, String)> = self
            .models
            .names
            .iter()
            .enumerate()
            .map(|(i, n)| (i as u16, n.clone()))
            .collect();
        for (index, name) in names {
            self.load_studio(index, &name);
        }
    }

    /// The loader, to hand on to the tables of the next map.
    pub fn model_loader(&self) -> Option<ModelLoader> {
        self.loader.clone()
    }

    /// The studio model with model index `index`, `None` for a brush model, a missing file or
    /// one the loader could not read or parse.
    pub fn studio_model(&self, index: u16) -> Option<&Arc<StudioPoseData>> {
        self.studio.get(&index)
    }

    /// `Mod_LoadStudioModel` for the pose data the traces use: reads and parses the file once
    /// per index. A file that cannot be read or parsed is reported and traced as a box.
    fn load_studio(&mut self, index: u16, name: &str) {
        let is_mdl = name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("mdl"));
        if index == 0 || !is_mdl || self.studio.contains_key(&index) {
            return;
        }
        let Some(loader) = &self.loader else {
            return;
        };
        if let Some(model) = self.studio_by_name.get(&key(name)) {
            self.studio.insert(index, Arc::clone(model));
            return;
        }
        let Some(bytes) = loader(name) else {
            tracing::warn!(model = name, "model file not found; traced as a box");
            return;
        };
        match StudioPoseData::parse(&bytes) {
            Ok(model) => {
                let model = Arc::new(model);
                if self.studio_by_name.len() >= MAX_STUDIO_CACHE {
                    // Full: drop the models this map does not use before taking a new one.
                    let used: std::collections::HashSet<String> =
                        self.models.names.iter().map(|n| key(n)).collect();
                    self.studio_by_name.retain(|name, _| used.contains(name));
                }
                if self.studio_by_name.len() < MAX_STUDIO_CACHE {
                    self.studio_by_name.insert(key(name), Arc::clone(&model));
                }
                self.studio.insert(index, model);
            }
            Err(error) => {
                tracing::warn!(model = name, %error, "model refused; traced as a box");
            }
        }
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

    /// `SV_CreateResourceList` (ReHLDS `rehlds/engine/sv_main.cpp:5787`, read 2026-11-22, the
    /// build without `REHLDS_FIXES`): the generic files (flag `RES_FATALIFMISSING`), then the
    /// sounds, the models, the decals and the event scripts, each table in index order without
    /// the empty entry 0.
    ///
    /// - A sound whose name starts with `!` is a sentence: only the first of them is listed,
    ///   as the name `!` with size 0 and flag `RES_FATALIFMISSING`, at its own index. The size
    ///   of any other sound is `size_of("sound/<name>")`.
    /// - A model has the flags of `model_precache_flags`; a submodel (`*N`) has size 0 and
    ///   every other name `size_of(name)`.
    /// - A decal is `SV_AddResource(t_decal, sv_decalnames[i].name, Draw_DecalSize(i), 0, i)`
    ///   from index 0, with size 0 here (no file transfer).
    /// - An event script has type [`res_type::EVENTSCRIPT`] and flag `RES_FATALIFMISSING`.
    pub fn resource_list(&self, size_of: &dyn Fn(&str) -> u32) -> Vec<ResourceEntry> {
        let entry = |res_type, name: &str, index: usize, download_size, flags| ResourceEntry {
            res_type,
            filename: name.to_string(),
            // Lossless: a table never holds more than 512 names.
            index: index as u16,
            download_size,
            flags,
            md5: None,
            reserved: None,
        };
        let mut list: Vec<ResourceEntry> = self
            .generic
            .names
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, name)| entry(res_type::GENERIC, name, i, size_of(name), RES_FATALIFMISSING))
            .collect();
        let mut sentence_listed = false;
        for (i, name) in self.sounds.names.iter().enumerate().skip(1) {
            if name.starts_with('!') {
                if !sentence_listed {
                    sentence_listed = true;
                    list.push(entry(res_type::SOUND, "!", i, 0, RES_FATALIFMISSING));
                }
            } else {
                list.push(entry(res_type::SOUND, name, i, size_of(&format!("sound/{name}")), 0));
            }
        }
        for (i, name) in self.models.names.iter().enumerate().skip(1) {
            let size = if name.starts_with('*') { 0 } else { size_of(name) };
            let flags = self.model_flags.get(i).copied().unwrap_or(0);
            list.push(entry(res_type::MODEL, name, i, size, flags));
        }
        list.extend(
            self.decals
                .iter()
                .enumerate()
                .map(|(i, name)| entry(res_type::DECAL, name, i, 0, 0)),
        );
        list.extend(self.events.names.iter().enumerate().skip(1).map(|(i, name)| {
            entry(res_type::EVENTSCRIPT, name, i, size_of(name), RES_FATALIFMISSING)
        }));
        list
    }
}

/// `RES_FATALIFMISSING` (`rehlds/common/resource.h`): the client stops when it cannot load the
/// resource. Assumption: the value (1 << 0) is from memory.
const RES_FATALIFMISSING: u8 = 1;

/// The lump names of a WAD3 file in directory order, lowercased and cut at 16 bytes as
/// `W_CleanupName` does (ReHLDS `rehlds/engine/wad.cpp`, read 2026-11-22); `None` for a file
/// that is not a WAD3 or whose directory does not fit in it.
pub fn wad_lump_names(wad: &[u8]) -> Option<Vec<String>> {
    if wad.get(..4)? != b"WAD3" {
        return None;
    }
    let word = |at: usize| -> Option<usize> {
        let b = wad.get(at..at + 4)?;
        usize::try_from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok()
    };
    // A wad with more lumps than the server keeps is cut, so a hostile directory cannot make
    // the merge slow; `Decal_Init` stops the server on such a wad.
    let count = word(4)?.min(MAX_DECALS);
    let table = word(8)?;
    let end = table.checked_add(count.checked_mul(32)?)?;
    if end > wad.len() {
        return None;
    }
    Some(
        (0..count)
            .map(|i| {
                let raw = &wad[table + i * 32 + 16..table + i * 32 + 32];
                let len = raw.iter().position(|&c| c == 0).unwrap_or(16);
                raw[..len]
                    .to_ascii_lowercase()
                    .iter()
                    .map(|&c| char::from(c))
                    .collect()
            })
            .collect(),
    )
}

/// The decal names the server holds, from the lump names of each `decals.wad` the engine loads,
/// in load order (`DEFAULTGAME`, then `GAME`): `Decal_Init` and `Decal_MergeInDecals` (ReHLDS
/// `rehlds/engine/decals.cpp`, read 2026-11-22).
///
/// The first list is kept as it is. Each later list is merged in as `Decal_MergeInDecals`
/// does: the lumps of the list so far are put one by one at the head of a linked list
/// (`Decal_ReplaceOrAppendLump`, which only replaces a lump of the same name in place), then
/// the new lumps the same way, and the result is read from the head. Quirk kept: after a merge
/// the order is reversed, a lump already in the list keeps its place in that reversed order,
/// and a new lump goes in front of everything. With the same file loaded twice (as the stock
/// `valve` and `cstrike` directories do) the indices run from the last lump of the file to the
/// first.
pub fn decal_names_from_wads(wads: &[Vec<String>]) -> Vec<String> {
    let Some((first, later)) = wads.split_first() else {
        return Vec::new();
    };
    // Each list is cut to the table size before the merge, whose search is quadratic.
    let mut names: Vec<String> = first.iter().take(MAX_DECALS).cloned().collect();
    for wad in later {
        let wad = &wad[..wad.len().min(MAX_DECALS)];
        // The end of `list` is the head of the original linked list: pushing is its prepend.
        let mut list: Vec<String> = Vec::new();
        for lump in names.iter().chain(wad) {
            match list.iter_mut().find(|n| n.eq_ignore_ascii_case(lump)) {
                Some(slot) => *slot = lump.clone(),
                None => list.push(lump.clone()),
            }
        }
        list.reverse();
        names = list;
    }
    names.truncate(MAX_DECALS);
    names
}

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

    /// `SV_CreateResourceList` writes the generic files first, then the sounds, the models, the
    /// decals and the event scripts; submodels have size 0, a sound is sized by its `sound/`
    /// path, and the flags are the ones of the source.
    #[test]
    fn resource_list_orders_generic_sounds_models_decals_events() {
        let mut t = PrecacheTables::new("maps/de_dust2.bsp", 2);
        t.precache_model("models/gign.mdl").unwrap();
        t.precache_model("!models/optional.mdl").unwrap();
        t.precache_sound("a.wav").unwrap();
        t.precache_sound("b.wav").unwrap();
        t.precache_generic("hud.txt").unwrap();
        t.set_decals(vec!["{shot1".to_string()]);
        t.precache_event("events/ak47.sc").unwrap();
        let list = t.resource_list(&|name| name.len() as u32 * 10);
        let entry = |res_type, filename: &str, index, download_size, flags| ResourceEntry {
            res_type,
            filename: filename.to_string(),
            index,
            download_size,
            flags,
            md5: None,
            reserved: None,
        };
        assert_eq!(
            list,
            vec![
                entry(res_type::GENERIC, "hud.txt", 1, 70, 1),
                entry(res_type::SOUND, "a.wav", 1, 110, 0),
                entry(res_type::SOUND, "b.wav", 2, 110, 0),
                entry(res_type::MODEL, "maps/de_dust2.bsp", 1, 170, 1),
                entry(res_type::MODEL, "*1", 2, 0, 1),
                entry(res_type::MODEL, "models/gign.mdl", 3, 150, 1),
                entry(res_type::MODEL, "models/optional.mdl", 4, 190, 0),
                entry(res_type::DECAL, "{shot1", 0, 0, 0),
                entry(res_type::EVENTSCRIPT, "events/ak47.sc", 1, 140, 1),
            ]
        );
    }

    /// Only the first sentence sound is listed, as the name `!` (`SV_CreateResourceList`).
    #[test]
    fn resource_list_has_one_sentence_marker() {
        let mut t = PrecacheTables::new("maps/de_dust2.bsp", 1);
        t.precache_sound("a.wav").unwrap();
        t.precache_sound("!HG_1").unwrap();
        t.precache_sound("!HG_2").unwrap();
        t.precache_sound("c.wav").unwrap();
        let sounds: Vec<(&str, u16, u8)> = t
            .resource_list(&|_| 0)
            .iter()
            .filter(|r| r.res_type == res_type::SOUND)
            .map(|r| (if r.filename == "!" { "!" } else { "x" }, r.index, r.flags))
            .collect();
        assert_eq!(sounds, vec![("x", 1, 0), ("!", 2, 1), ("x", 4, 0)]);
    }
}
