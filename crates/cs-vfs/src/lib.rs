//! Virtual File System (VFS) mirroring GoldSrc search-path semantics.
//!
//! Layers are mounted from highest priority to lowest:
//! `cstrike_downloads` -> `cstrike_hd` (optional) -> `cstrike` -> `valve` -> `platform`.
//! All lookups are normalized to forward slashes and matched case-insensitively.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

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

/// Case-insensitive multi-layer VFS over an installed Half-Life / CS 1.6 folder.
#[derive(Debug, Clone)]
pub struct Vfs {
    root: PathBuf,
    config: VfsConfig,
    /// Mounted subdirectories from highest to lowest priority.
    layers: Vec<PathBuf>,
    /// Normalized lowercase relative path -> winning real filesystem path.
    index: HashMap<String, PathBuf>,
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
        };
        vfs.rebuild_index();
        Ok(vfs)
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

    pub fn rebuild_index(&mut self) {
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

    pub fn resolve(&self, rel_path: &str) -> Option<&Path> {
        let key = Self::normalize(rel_path);
        self.index.get(&key).map(PathBuf::as_path)
    }

    pub fn exists(&self, rel_path: &str) -> bool {
        self.resolve(rel_path).is_some()
    }

    pub fn read_bytes(&self, rel_path: &str) -> Result<Vec<u8>, VfsError> {
        let real_path = self
            .resolve(rel_path)
            .ok_or_else(|| VfsError::NotFound(rel_path.to_string()))?;
        fs::read(real_path).map_err(|source| VfsError::Io {
            path: real_path.to_path_buf(),
            source,
        })
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
        for key in self.index.keys() {
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

fn walk_layer(base: &Path, current: &Path, index: &mut HashMap<String, PathBuf>) {
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
            index.insert(key, path);
        }
    }
}

fn candidate_game_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. Parse Steam libraryfolders.vdf on Windows/Linux/macOS.
    let mut vdf_paths = Vec::new();
    #[cfg(target_os = "windows")]
    {
        vdf_paths.push(PathBuf::from(
            r"C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf",
        ));
        vdf_paths.push(PathBuf::from(
            r"C:\Program Files\Steam\steamapps\libraryfolders.vdf",
        ));
    }
    #[cfg(target_os = "linux")]
    if let Ok(home) = std::env::var("HOME") {
        let h = PathBuf::from(home);
        vdf_paths.push(h.join(".local/share/Steam/steamapps/libraryfolders.vdf"));
        vdf_paths.push(h.join(".steam/steam/steamapps/libraryfolders.vdf"));
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        vdf_paths.push(
            PathBuf::from(home)
                .join("Library/Application Support/Steam/steamapps/libraryfolders.vdf"),
        );
    }

    for vdf in vdf_paths {
        if let Ok(content) = fs::read_to_string(&vdf) {
            for lib in parse_steam_library_paths(&content) {
                dirs.push(lib.join("steamapps").join("common").join("Half-Life"));
            }
        }
    }

    // 2. Common fallback locations.
    #[cfg(target_os = "windows")]
    {
        for drive in ["C:", "D:", "E:", "F:"] {
            dirs.push(PathBuf::from(format!(
                r"{drive}\SteamLibrary\steamapps\common\Half-Life"
            )));
            dirs.push(PathBuf::from(format!(
                r"{drive}\Program Files (x86)\Steam\steamapps\common\Half-Life"
            )));
            dirs.push(PathBuf::from(format!(r"{drive}\Games\Counter-Strike 1.6")));
        }
    }

    dirs
}

/// Extract quoted `"path"` values from Steam's `libraryfolders.vdf`.
pub fn parse_steam_library_paths(vdf: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in vdf.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("\"path\"") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            let unescaped = inner.replace("\\\\", "\\");
            paths.push(PathBuf::from(unescaped));
        }
    }
    paths
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
}
