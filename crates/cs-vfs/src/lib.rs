//! Virtual File System (VFS) mirroring GoldSrc search-path semantics.
//!
//! Layers are mounted from highest priority to lowest:
//! `cstrike_downloads` -> `cstrike_hd` (optional) -> `cstrike` -> `valve` -> `platform`.
//! All lookups are normalized to forward slashes and matched case-insensitively.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use thiserror::Error;
use tracing::{debug, info};

#[derive(Debug, Error)]
pub enum VfsError {
    #[error("could not locate Half-Life / Counter-Strike 1.6 directory; pass --gamedir <path> or set CS16_GAMEDIR")]
    GameDirNotFound,
    #[error("game directory does not exist or lacks cstrike/valve folders: {0}")]
    InvalidGameDir(PathBuf),
    #[error("asset not found in VFS: {0}")]
    NotFound(String),
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct VfsConfig {
    pub enable_hd_models: bool,
    pub enable_downloads: bool,
}

impl Default for VfsConfig {
    fn default() -> Self {
        Self {
            enable_hd_models: false,
            enable_downloads: true,
        }
    }
}

/// Lazy asset backend for non-disk environments (e.g. browser DirectoryHandle / IndexedDB / OPFS).
pub trait VfsBackend: Send + Sync + std::fmt::Debug {
    /// Returns normalized lowercase relative paths available from this backend.
    fn list_paths(&self) -> Vec<String>;
    fn contains(&self, normalized_path: &str) -> bool;
    fn read_bytes(&self, normalized_path: &str) -> Result<Vec<u8>, VfsError>;
}

#[derive(Debug, Default)]
struct CacheInner {
    entries: HashMap<String, Vec<u8>>,
    order: VecDeque<String>,
    total_bytes: usize,
}

/// Caching wrapper around a [`VfsBackend`] for lazy reads within a byte budget.
#[derive(Debug)]
pub struct CachingBackend<B> {
    inner: B,
    max_bytes: usize,
    cache: RwLock<CacheInner>,
}

impl<B: VfsBackend> CachingBackend<B> {
    pub fn new(inner: B, max_bytes: usize) -> Self {
        Self {
            inner,
            max_bytes,
            cache: RwLock::new(CacheInner::default()),
        }
    }

    pub fn inner(&self) -> &B {
        &self.inner
    }

    pub fn cache_len(&self) -> usize {
        self.cache.read().map_or(0, |c| c.entries.len())
    }

    pub fn cached_bytes(&self) -> usize {
        self.cache.read().map_or(0, |c| c.total_bytes)
    }
}

impl<B: VfsBackend> VfsBackend for CachingBackend<B> {
    fn list_paths(&self) -> Vec<String> {
        self.inner.list_paths()
    }

    fn contains(&self, normalized_path: &str) -> bool {
        if let Ok(guard) = self.cache.read()
            && guard.entries.contains_key(normalized_path)
        {
            return true;
        }
        self.inner.contains(normalized_path)
    }

    fn read_bytes(&self, normalized_path: &str) -> Result<Vec<u8>, VfsError> {
        if let Ok(guard) = self.cache.read()
            && let Some(bytes) = guard.entries.get(normalized_path)
        {
            return Ok(bytes.clone());
        }
        let bytes = self.inner.read_bytes(normalized_path)?;
        if bytes.len() <= self.max_bytes
            && let Ok(mut guard) = self.cache.write()
            && !guard.entries.contains_key(normalized_path)
        {
            while guard.total_bytes + bytes.len() > self.max_bytes
                && let Some(evict) = guard.order.pop_front()
            {
                if let Some(old) = guard.entries.remove(&evict) {
                    guard.total_bytes = guard.total_bytes.saturating_sub(old.len());
                }
            }
            guard.total_bytes += bytes.len();
            guard.order.push_back(normalized_path.to_string());
            guard
                .entries
                .insert(normalized_path.to_string(), bytes.clone());
        }
        Ok(bytes)
    }
}

/// Where the bytes of an indexed file come from.
#[derive(Debug, Clone)]
enum Source {
    /// Winning real filesystem path.
    Disk(PathBuf),
    /// File contents held in memory.
    Memory(Vec<u8>),
    /// Deferred read via custom backend.
    Backend(Arc<dyn VfsBackend>),
}

/// Case-insensitive multi-layer VFS over an installed Half-Life / CS 1.6 folder,
/// a backend ([`Vfs::from_backend`]), or in-memory files ([`Vfs::from_memory`],
/// [`Vfs::from_layered_memory`]).
#[derive(Debug, Clone)]
pub struct Vfs {
    root: PathBuf,
    config: VfsConfig,
    /// Mounted subdirectories from highest to lowest priority.
    layers: Vec<PathBuf>,
    /// Normalized lowercase relative path -> file source.
    index: HashMap<String, Source>,
    /// Mod asset overlay layers above base GoldSrc directories (highest priority last).
    mod_overlays: Vec<(String, HashMap<String, Source>)>,
    /// Optional backend for lazy lookups outside static `index` enumeration.
    backend: Option<Arc<dyn VfsBackend>>,
    /// True when files were supplied in memory or via backend; never re-walks disk.
    in_memory: bool,
}

impl Vfs {
    /// Mount a specific Half-Life root directory (containing `valve/` and `cstrike/`).
    pub fn open(root: impl AsRef<Path>, config: VfsConfig) -> Result<Self, VfsError> {
        let root = root.as_ref().to_path_buf();
        let has_cstrike = root.join("cstrike").is_dir();
        let has_valve = root.join("valve").is_dir();
        if !root.is_dir() || (!has_cstrike && !has_valve) {
            return Err(VfsError::InvalidGameDir(root));
        }

        let mut vfs = Self {
            root,
            config,
            layers: Vec::new(),
            index: HashMap::new(),
            mod_overlays: Vec::new(),
            backend: None,
            in_memory: false,
        };
        vfs.rebuild_index();
        Ok(vfs)
    }

    /// Build a VFS over in-memory files. Paths are normalized with [`Vfs::normalize`];
    /// a later entry with the same normalized path replaces an earlier one.
    /// The instance has no disk root or layers, and [`Vfs::resolve`] returns `None` for its files.
    pub fn from_memory(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Self {
        let index = files
            .into_iter()
            .map(|(path, bytes)| (Self::normalize(&path), Source::Memory(bytes)))
            .collect();
        Self {
            root: PathBuf::new(),
            config: VfsConfig::default(),
            layers: Vec::new(),
            index,
            mod_overlays: Vec::new(),
            backend: None,
            in_memory: true,
        }
    }

    /// Build a multi-layer VFS from relative paths exported by browser folder pickers or drag-and-drop
    /// (`Half-Life/cstrike/...`, `valve/...`, `cstrike_hd/...`, `cstrike_downloads/...`, `mods/<id>/...`),
    /// enforcing GoldSrc layer priority regardless of iteration order.
    pub fn from_layered_memory(
        entries: impl IntoIterator<Item = (String, Vec<u8>)>,
        config: VfsConfig,
    ) -> Self {
        let mut best: HashMap<String, (u8, Vec<u8>)> = HashMap::new();
        for (raw_path, bytes) in entries {
            let Some((rank, rel)) = split_goldsrc_layer(&raw_path, &config) else {
                continue;
            };
            match best.get(&rel) {
                Some((cur_rank, _)) if *cur_rank > rank => {}
                _ => {
                    best.insert(rel, (rank, bytes));
                }
            }
        }
        let index = best
            .into_iter()
            .map(|(path, (_, bytes))| (path, Source::Memory(bytes)))
            .collect();
        Self {
            root: PathBuf::new(),
            config,
            layers: Vec::new(),
            index,
            mod_overlays: Vec::new(),
            backend: None,
            in_memory: true,
        }
    }

    /// Build a VFS backed by a custom lazy [`VfsBackend`] (for browser/OPFS/IndexedDB storage).
    pub fn from_backend(backend: Arc<dyn VfsBackend>) -> Self {
        let index = backend
            .list_paths()
            .into_iter()
            .map(|p| (Self::normalize(&p), Source::Backend(Arc::clone(&backend))))
            .collect();
        Self {
            root: PathBuf::new(),
            config: VfsConfig::default(),
            layers: Vec::new(),
            index,
            mod_overlays: Vec::new(),
            backend: Some(backend),
            in_memory: true,
        }
    }

    /// Auto-discover Half-Life / CS 1.6 directory from explicit arg, `CS16_GAMEDIR`,
    /// Steam's `libraryfolders.vdf`, or common install locations.
    pub fn discover(explicit: Option<&Path>, config: VfsConfig) -> Result<Self, VfsError> {
        if let Some(dir) = explicit {
            return Self::open(dir, config);
        }

        if let Ok(env_dir) = std::env::var("CS16_GAMEDIR") {
            let path = PathBuf::from(env_dir);
            if let Ok(vfs) = Self::open(&path, config.clone()) {
                return Ok(vfs);
            }
        }

        for candidate in candidate_game_dirs() {
            if let Ok(vfs) = Self::open(&candidate, config.clone()) {
                info!(path = %candidate.display(), "Discovered Half-Life / CS 1.6 assets");
                return Ok(vfs);
            }
        }

        Err(VfsError::GameDirNotFound)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &VfsConfig {
        &self.config
    }

    pub fn layers(&self) -> &[PathBuf] {
        &self.layers
    }

    pub fn file_count(&self) -> usize {
        self.index.len()
    }

    /// Toggle HD models layer (`cstrike_hd`) and re-index immediately.
    pub fn set_hd_models(&mut self, enabled: bool) {
        if self.config.enable_hd_models != enabled {
            self.config.enable_hd_models = enabled;
            self.rebuild_index();
        }
    }

    /// Re-scan the disk layers. Does nothing for an in-memory VFS.
    pub fn rebuild_index(&mut self) {
        if self.in_memory {
            return;
        }
        self.layers.clear();
        self.index.clear();

        let mut layer_names: Vec<&str> = Vec::new();
        if self.config.enable_downloads {
            layer_names.push("cstrike_downloads");
        }
        if self.config.enable_hd_models {
            layer_names.push("cstrike_hd");
        }
        layer_names.push("cstrike");
        layer_names.push("valve");
        layer_names.push("platform");

        for name in layer_names {
            let dir = self.root.join(name);
            if dir.is_dir() {
                self.layers.push(dir);
            }
        }

        // Index from lowest priority to highest so higher priority layers overwrite lower ones.
        for layer in self.layers.iter().rev() {
            walk_layer(layer, layer, &mut self.index);
        }

        // Layer any on-disk `<root>/mods/<id>` asset packs above base GoldSrc layers.
        let mods_root = self.root.join("mods");
        if mods_root.is_dir()
            && let Ok(entries) = fs::read_dir(&mods_root)
        {
            let mut mod_dirs: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            mod_dirs.sort();
            for dir in mod_dirs {
                let assets_sub = dir.join("assets");
                let scan_root = if assets_sub.is_dir() {
                    &assets_sub
                } else {
                    &dir
                };
                walk_layer(scan_root, scan_root, &mut self.index);
            }
        }

        debug!(
            root = %self.root.display(),
            layers = self.layers.len(),
            files = self.index.len(),
            "Indexed GoldSrc VFS"
        );
    }

    /// Normalize a GoldSrc asset path (forward slashes, stripped leading `./` or `/`, lowercase).
    pub fn normalize(rel_path: &str) -> String {
        let mut s = rel_path.trim().replace('\\', "/");
        while let Some(stripped) = s.strip_prefix("./") {
            s = stripped.to_string();
        }
        while let Some(stripped) = s.strip_prefix('/') {
            s = stripped.to_string();
        }
        s.to_ascii_lowercase()
    }

    /// Validates a mod identifier (`[a-z0-9_-]`) for VFS overlay mounting.
    pub fn is_valid_mod_id(mod_id: &str) -> bool {
        !mod_id.is_empty()
            && mod_id
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    }

    fn is_safe_overlay_path(rel_path: &str) -> bool {
        let norm = Self::normalize(rel_path);
        !norm.is_empty()
            && !norm.contains('\0')
            && !norm.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..")
    }

    fn lookup_source(&self, normalized_key: &str) -> Option<&Source> {
        for (_, overlay) in self.mod_overlays.iter().rev() {
            if let Some(src) = overlay.get(normalized_key) {
                return Some(src);
            }
        }
        self.index.get(normalized_key)
    }

    /// Mounts a disk directory as an asset overlay for `mod_id` above `cstrike` layers.
    pub fn mount_mod_dir(&mut self, mod_id: &str, dir: &Path) -> Result<usize, VfsError> {
        if !Self::is_valid_mod_id(mod_id) || !dir.is_dir() {
            return Err(VfsError::InvalidGameDir(dir.to_path_buf()));
        }
        let mut map = HashMap::new();
        walk_layer(dir, dir, &mut map);
        let count = map.len();
        self.unmount_mod_assets(mod_id);
        self.mod_overlays.push((mod_id.to_string(), map));
        Ok(count)
    }

    /// Mounts in-memory asset bytes for `mod_id` above `cstrike` layers (for downloaded mods).
    pub fn mount_mod_assets(
        &mut self,
        mod_id: &str,
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<usize, VfsError> {
        if !Self::is_valid_mod_id(mod_id) {
            return Err(VfsError::InvalidGameDir(PathBuf::from(mod_id)));
        }
        let mut map = HashMap::new();
        for (raw_path, bytes) in files {
            if !Self::is_safe_overlay_path(&raw_path) {
                return Err(VfsError::NotFound(raw_path));
            }
            let key = Self::normalize(&raw_path);
            map.insert(key, Source::Memory(bytes));
        }
        let count = map.len();
        self.unmount_mod_assets(mod_id);
        self.mod_overlays.push((mod_id.to_string(), map));
        Ok(count)
    }

    /// Removes any mounted asset overlay for `mod_id`.
    pub fn unmount_mod_assets(&mut self, mod_id: &str) -> bool {
        let before = self.mod_overlays.len();
        self.mod_overlays.retain(|(id, _)| id != mod_id);
        self.mod_overlays.len() != before
    }

    /// Real filesystem path of a disk-backed file; `None` if absent or held in memory/backend.
    pub fn resolve(&self, rel_path: &str) -> Option<&Path> {
        match self.lookup_source(&Self::normalize(rel_path))? {
            Source::Disk(path) => Some(path),
            Source::Memory(_) | Source::Backend(_) => None,
        }
    }

    pub fn exists(&self, rel_path: &str) -> bool {
        let key = Self::normalize(rel_path);
        self.lookup_source(&key).is_some()
            || self.backend.as_ref().is_some_and(|b| b.contains(&key))
    }

    pub fn read_bytes(&self, rel_path: &str) -> Result<Vec<u8>, VfsError> {
        let key = Self::normalize(rel_path);
        match self.lookup_source(&key) {
            None => {
                if let Some(backend) = &self.backend
                    && backend.contains(&key)
                {
                    return backend.read_bytes(&key);
                }
                Err(VfsError::NotFound(rel_path.to_string()))
            }
            Some(Source::Memory(bytes)) => Ok(bytes.clone()),
            Some(Source::Disk(real_path)) => fs::read(real_path).map_err(|source| VfsError::Io {
                path: real_path.clone(),
                source,
            }),
            Some(Source::Backend(backend)) => backend.read_bytes(&key),
        }
    }

    pub fn read_string_lossy(&self, rel_path: &str) -> Result<String, VfsError> {
        let bytes = self.read_bytes(rel_path)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// List unique file stems or normalized relative paths directly inside `dir` with extension `ext`
    /// (without leading dot, case-insensitive). Returns sorted filenames (e.g. `"de_dust2.bsp"`).
    pub fn list_dir(&self, dir: &str, ext: Option<&str>) -> Vec<String> {
        let prefix = Self::normalize(dir);
        let prefix_slash = if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        };
        let ext_lower = ext.map(|e| format!(".{}", e.trim_start_matches('.').to_ascii_lowercase()));

        let mut results = BTreeSet::new();
        for key in self
            .index
            .keys()
            .chain(self.mod_overlays.iter().flat_map(|(_, m)| m.keys()))
        {
            let Some(rest) = key.strip_prefix(&prefix_slash) else {
                continue;
            };
            if rest.contains('/') {
                continue;
            }
            if let Some(ref suffix) = ext_lower
                && !rest.ends_with(suffix)
            {
                continue;
            }
            results.insert(rest.to_string());
        }
        results.into_iter().collect()
    }

    /// Return sorted map names (without `.bsp` extension) from `maps/`.
    pub fn list_maps(&self) -> Vec<String> {
        self.list_dir("maps", Some("bsp"))
            .into_iter()
            .filter_map(|name| name.strip_suffix(".bsp").map(str::to_string))
            .collect()
    }
}

fn split_goldsrc_layer(raw_path: &str, config: &VfsConfig) -> Option<(u8, String)> {
    let norm = Vfs::normalize(raw_path);
    if norm.is_empty() {
        return None;
    }
    let parts: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    for (idx, &seg) in parts.iter().enumerate() {
        let rank = match seg {
            "platform" => Some((1u8, idx + 1)),
            "valve" => Some((2u8, idx + 1)),
            "cstrike" => Some((3u8, idx + 1)),
            "cstrike_hd" => config.enable_hd_models.then_some((4u8, idx + 1)),
            "cstrike_downloads" => config.enable_downloads.then_some((5u8, idx + 1)),
            "mods" if idx + 2 < parts.len() => Some((6u8, idx + 2)),
            _ => None,
        };
        if let Some((r, rest_start)) = rank {
            if rest_start >= parts.len() {
                return None;
            }
            return Some((r, parts[rest_start..].join("/")));
        }
    }
    Some((3, parts.join("/")))
}

fn walk_layer(base: &Path, current: &Path, index: &mut HashMap<String, Source>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            walk_layer(base, &path, index);
        } else if ft.is_file()
            && let Ok(rel) = path.strip_prefix(base)
        {
            let rel_str = rel.to_string_lossy();
            let key = Vfs::normalize(&rel_str);
            index.insert(key, Source::Disk(path));
        }
    }
}

fn candidate_game_dirs() -> Vec<PathBuf> {
    candidate_game_dirs_with(&cs_platform::xdg::PlatformEnv::from_host(), &|p| {
        fs::read_to_string(p).ok()
    })
}

/// Returns candidate Half-Life / CS 1.6 install directories using XDG, Flatpak Steam, SteamOS/Deck
/// microSD, macOS Application Support, and Windows Steam library discovery.
pub fn candidate_game_dirs_with(
    env: &cs_platform::xdg::PlatformEnv,
    read_vdf: &dyn Fn(&Path) -> Option<String>,
) -> Vec<PathBuf> {
    let mut dirs = cs_platform::xdg::candidate_steam_half_life_dirs_with(env, read_vdf);
    if let Some(data_dir) = cs_platform::xdg::data_dir_with(env, "cs16-rs") {
        let candidate = data_dir.join("Half-Life");
        if !dirs.contains(&candidate) {
            dirs.push(candidate);
        }
    }
    dirs
}

/// Returns the user configuration directory (`$XDG_CONFIG_HOME/cs16-rs` on Linux).
pub fn xdg_config_dir() -> Option<PathBuf> {
    cs_platform::xdg::config_dir("cs16-rs")
}

/// Returns the user data directory (`$XDG_DATA_HOME/cs16-rs` on Linux).
pub fn xdg_data_dir() -> Option<PathBuf> {
    cs_platform::xdg::data_dir("cs16-rs")
}

/// Resolves candidate paths for a user config file (such as `config.cfg`), checking the XDG/OS
/// config directory first and then the user data directory.
pub fn candidate_user_config_paths(filename: &str) -> Vec<PathBuf> {
    candidate_user_config_paths_with(&cs_platform::xdg::PlatformEnv::from_host(), filename)
}

/// Deterministic [`candidate_user_config_paths`] using an explicit [`cs_platform::xdg::PlatformEnv`].
pub fn candidate_user_config_paths_with(
    env: &cs_platform::xdg::PlatformEnv,
    filename: &str,
) -> Vec<PathBuf> {
    if filename.is_empty()
        || filename.contains(['/', '\\', '\0'])
        || filename.contains("..")
    {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Some(cfg) = cs_platform::xdg::config_dir_with(env, "cs16-rs") {
        paths.push(cfg.join(filename));
    }
    if let Some(data) = cs_platform::xdg::data_dir_with(env, "cs16-rs") {
        let p = data.join(filename);
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

/// Extract quoted `"path"` values from Steam's `libraryfolders.vdf`.
pub fn parse_steam_library_paths(vdf: &str) -> Vec<PathBuf> {
    cs_platform::xdg::parse_library_folders_vdf(vdf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_goldsrc_paths() {
        assert_eq!(Vfs::normalize(r"\Maps\DE_DUST2.BSP"), "maps/de_dust2.bsp");
        assert_eq!(
            Vfs::normalize("./resource/TrackerScheme.res"),
            "resource/trackerscheme.res"
        );
    }

    #[test]
    fn parses_steam_library_vdf() {
        let sample = r#"
        "libraryfolders"
        {
            "0"
            {
                "path"      "C:\\Program Files (x86)\\Steam"
            }
            "1"
            {
                "path"      "D:\\SteamLibrary"
            }
        }
        "#;
        let libs = parse_steam_library_paths(sample);
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0], PathBuf::from(r"C:\Program Files (x86)\Steam"));
        assert_eq!(libs[1], PathBuf::from(r"D:\SteamLibrary"));
    }

    #[test]
    fn memory_vfs_reads_lists_and_is_case_insensitive() {
        let vfs = Vfs::from_memory([("Maps/De_Dust2.bsp".to_string(), b"x".to_vec())]);
        assert!(vfs.exists("maps/de_dust2.bsp"));
        assert_eq!(vfs.read_bytes("MAPS/de_dust2.BSP").unwrap(), b"x");
        assert_eq!(vfs.list_maps(), vec!["de_dust2"]);
        assert_eq!(vfs.file_count(), 1);
        assert!(vfs.resolve("maps/de_dust2.bsp").is_none());
    }

    #[test]
    fn discovers_local_cs16_if_installed() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return; // Soft-skip in CI environments without local Half-Life assets
        };
        assert!(vfs.file_count() > 100);
        assert!(vfs.exists("resource/TrackerScheme.res"));
        assert!(vfs.exists("sprites/hud.txt"));
        assert!(vfs.exists("maps/de_dust2.bsp"));
        let maps = vfs.list_maps();
        assert!(maps.iter().any(|m| m == "de_dust2"));
    }

    #[derive(Debug)]
    struct CountingBackend {
        files: HashMap<String, Vec<u8>>,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl VfsBackend for CountingBackend {
        fn list_paths(&self) -> Vec<String> {
            self.files.keys().cloned().collect()
        }

        fn contains(&self, normalized_path: &str) -> bool {
            self.files.contains_key(normalized_path)
        }

        fn read_bytes(&self, normalized_path: &str) -> Result<Vec<u8>, VfsError> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.files
                .get(normalized_path)
                .cloned()
                .ok_or_else(|| VfsError::NotFound(normalized_path.to_string()))
        }
    }

    #[test]
    fn layered_memory_vfs_resolves_goldsrc_layer_priority() {
        // Deliberately insert higher-priority cstrike before valve to verify priority beats order.
        let entries = [
            (
                "Half-Life/cstrike/sprites/hud.txt".to_string(),
                b"cs-hud".to_vec(),
            ),
            ("Half-Life/valve/sprites/hud.txt".to_string(), b"valve-hud".to_vec()),
            ("Half-Life/valve/gfx/palette.lmp".to_string(), b"pal".to_vec()),
            (
                "Half-Life/cstrike_hd/models/v_ak47.mdl".to_string(),
                b"hd-ak".to_vec(),
            ),
            (
                "Half-Life/cstrike/models/v_ak47.mdl".to_string(),
                b"sd-ak".to_vec(),
            ),
        ];
        let vfs_sd = Vfs::from_layered_memory(entries.clone(), VfsConfig::default());
        assert_eq!(vfs_sd.read_bytes("sprites/hud.txt").unwrap(), b"cs-hud");
        assert_eq!(vfs_sd.read_bytes("gfx/palette.lmp").unwrap(), b"pal");
        assert_eq!(vfs_sd.read_bytes("models/v_ak47.mdl").unwrap(), b"sd-ak");

        let vfs_hd = Vfs::from_layered_memory(
            entries,
            VfsConfig {
                enable_hd_models: true,
                enable_downloads: true,
            },
        );
        assert_eq!(vfs_hd.read_bytes("models/v_ak47.mdl").unwrap(), b"hd-ak");
    }

    #[test]
    fn custom_backend_and_caching_backend_read_lazily_and_cache() {
        let mut files = HashMap::new();
        files.insert("maps/de_dust2.bsp".to_string(), vec![1u8; 8]);
        files.insert("sound/test.wav".to_string(), vec![2u8; 8]);
        let backend = CachingBackend::new(
            CountingBackend {
                files,
                reads: std::sync::atomic::AtomicUsize::new(0),
            },
            10,
        );
        let arc = Arc::new(backend);
        let vfs = Vfs::from_backend(arc.clone());
        assert_eq!(vfs.list_maps(), vec!["de_dust2"]);
        assert_eq!(vfs.read_bytes("MAPS/DE_DUST2.BSP").unwrap().len(), 8);
        assert_eq!(vfs.read_bytes("maps/de_dust2.bsp").unwrap().len(), 8);
        assert_eq!(
            arc.inner().reads.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        // Second 8-byte entry exceeds the 10-byte cache budget, evicting the first entry.
        assert_eq!(vfs.read_bytes("sound/test.wav").unwrap().len(), 8);
        assert_eq!(arc.cache_len(), 1);
        assert_eq!(arc.cached_bytes(), 8);
    }

    #[test]
    fn mod_asset_overlay_overrides_cstrike_and_unmounts_cleanly() {
        let mut vfs = Vfs::from_layered_memory(
            [
                ("Half-Life/cstrike/sprites/hud.txt".to_string(), b"base-hud".to_vec()),
            ],
            VfsConfig::default(),
        );
        assert_eq!(vfs.read_bytes("sprites/hud.txt").unwrap(), b"base-hud");

        let count = vfs
            .mount_mod_assets(
                "custom_hud",
                [
                    ("sprites/hud.txt".to_string(), b"mod-hud".to_vec()),
                    ("sprites/mod_badge.txt".to_string(), b"badge".to_vec()),
                ],
            )
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(vfs.read_bytes("sprites/hud.txt").unwrap(), b"mod-hud");
        assert!(vfs.exists("sprites/mod_badge.txt"));

        assert!(vfs.unmount_mod_assets("custom_hud"));
        assert_eq!(vfs.read_bytes("sprites/hud.txt").unwrap(), b"base-hud");
        assert!(!vfs.exists("sprites/mod_badge.txt"));
    }

    #[test]
    fn xdg_discovery_preserves_cs16_gamedir_priority_and_resolves_configs() {
        let env = cs_platform::xdg::PlatformEnv {
            platform: Some(cs_platform::xdg::PlatformKind::Linux),
            home: Some(PathBuf::from("/home/player")),
            xdg_config_home: Some(PathBuf::from("/etc/xdg-custom")),
            xdg_data_home: Some(PathBuf::from("/srv/xdg-data")),
            cs16_gamedir: Some(PathBuf::from("/override/Half-Life")),
            ..Default::default()
        };
        let dirs = candidate_game_dirs_with(&env, &|_| None);
        assert_eq!(dirs.first(), Some(&PathBuf::from("/override/Half-Life")));
        assert!(dirs.contains(&PathBuf::from("/srv/xdg-data/Steam/steamapps/common/Half-Life")));
        assert!(dirs.contains(&PathBuf::from("/srv/xdg-data/cs16-rs/Half-Life")));

        let cfg_paths = candidate_user_config_paths_with(&env, "config.cfg");
        assert_eq!(
            cfg_paths,
            vec![
                PathBuf::from("/etc/xdg-custom/cs16-rs/config.cfg"),
                PathBuf::from("/srv/xdg-data/cs16-rs/config.cfg"),
            ]
        );
        assert!(candidate_user_config_paths_with(&env, "../escape.cfg").is_empty());
    }
}
