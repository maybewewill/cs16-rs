//! Discovery of `mods/*.py` and `mods/<dir>/main.py` plus native mtime-polling [`PyHotReloader`].

use std::path::{Path, PathBuf};

use cs_mod_api::ModManifest;

use crate::error::PyError;
use crate::runtime::synthesize_manifest_from_py;

/// One discovered Python plugin file ready to be loaded into `ServerModHost`.
#[derive(Debug, Clone)]
pub struct PyDiscoveredMod {
    pub mod_id: String,
    pub script_path: PathBuf,
    pub source: String,
    pub manifest: ModManifest,
}

/// Discovers Python plugins in `mods_dir`:
/// - Single-file scripts: `mods/<name>.py`
/// - Package directories: `mods/<dir>/main.py` (using `<dir>/mod.toml` if present, else
///   synthesizing a manifest from `@mod.info(...)`).
///
/// Results are sorted deterministically by path.
pub fn discover_python_mods(mods_dir: &Path) -> Result<Vec<PyDiscoveredMod>, String> {
    if !mods_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    collect_python_mods_recursive(mods_dir, &mut found)?;
    Ok(found)
}

fn collect_python_mods_recursive(
    dir: &Path,
    found: &mut Vec<PyDiscoveredMod>,
) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to scan `{}`: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || matches!(name, "target" | "assets" | "src" | "client") {
            continue;
        }
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("py"))
        {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("py_plugin");
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("plugin.py");
            let source = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read `{}`: {e}", path.display()))?;
            let manifest = synthesize_manifest_from_py(stem, file_name, &source)
                .map_err(|e| format!("failed to validate `{}`: {e}", path.display()))?;
            found.push(PyDiscoveredMod {
                mod_id: manifest.id.clone(),
                script_path: path,
                source,
                manifest,
            });
        } else if path.is_dir() {
            let main_py = path.join("main.py");
            if main_py.is_file() {
                let dir_name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("py_plugin");
                let source = std::fs::read_to_string(&main_py)
                    .map_err(|e| format!("failed to read `{}`: {e}", main_py.display()))?;
                let toml_path = path.join("mod.toml");
                let manifest = if toml_path.is_file() {
                    let raw = std::fs::read_to_string(&toml_path)
                        .map_err(|e| format!("failed to read `{}`: {e}", toml_path.display()))?;
                    ModManifest::parse_toml(&raw)
                        .map_err(|e| format!("invalid `{}`: {e}", toml_path.display()))?
                } else {
                    synthesize_manifest_from_py(dir_name, "main.py", &source)
                        .map_err(|e| format!("failed to validate `{}`: {e}", main_py.display()))?
                };
                found.push(PyDiscoveredMod {
                    mod_id: manifest.id.clone(),
                    script_path: main_py,
                    source,
                    manifest,
                });
            } else if !path.join("mod.toml").is_file() {
                collect_python_mods_recursive(&path, found)?;
            }
        }
    }
    Ok(())
}

/// Loads one `.py` file from disk, synthesizing a manifest from `@mod.info(...)` (or adjacent
/// `mod.toml` when inside a directory).
pub fn load_python_mod_file(script_path: &Path) -> Result<PyDiscoveredMod, PyError> {
    let file_name = script_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("plugin.py");
    let stem = script_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("py_plugin");
    let fallback_id = if stem == "main" {
        script_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("py_plugin")
    } else {
        stem
    };
    let source = std::fs::read_to_string(script_path).map_err(|e| {
        PyError::runtime(
            "OSError",
            format!("failed to read `{}`: {e}", script_path.display()),
            Vec::new(),
        )
    })?;
    let manifest = synthesize_manifest_from_py(fallback_id, file_name, &source)?;
    Ok(PyDiscoveredMod {
        mod_id: manifest.id.clone(),
        script_path: script_path.to_path_buf(),
        source,
        manifest,
    })
}

/// Native mtime-polling hot-reload watcher (`sv_mod_autoreload 1`, default `1.0 s` interval).
#[cfg(not(target_arch = "wasm32"))]
pub mod watcher {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime};

    #[derive(Debug, Clone)]
    struct WatchedEntry {
        path: PathBuf,
        last_mtime: Option<SystemTime>,
        last_len: u64,
    }

    /// Lightweight `mtime` + file-length watcher that polls every `poll_interval` (default `1.0 s`)
    /// so saving any `mods/*.py` file hot-reloads that plugin while preserving its `mod.storage`.
    #[derive(Debug)]
    pub struct PyHotReloader {
        poll_interval: Duration,
        last_poll: Option<Instant>,
        entries: BTreeMap<String, WatchedEntry>,
    }

    impl Default for PyHotReloader {
        fn default() -> Self {
            Self::new(Duration::from_secs(1))
        }
    }

    impl PyHotReloader {
        pub fn new(poll_interval: Duration) -> Self {
            Self {
                poll_interval,
                last_poll: None,
                entries: BTreeMap::new(),
            }
        }

        pub fn watch(&mut self, mod_id: impl Into<String>, path: impl Into<PathBuf>) {
            let path = path.into();
            let (mtime, len) = file_stamp(&path);
            self.entries.insert(
                mod_id.into(),
                WatchedEntry {
                    path,
                    last_mtime: mtime,
                    last_len: len,
                },
            );
        }

        pub fn unwatch(&mut self, mod_id: &str) {
            self.entries.remove(mod_id);
        }

        pub fn watched_count(&self) -> usize {
            self.entries.len()
        }

        pub fn watched_path(&self, mod_id: &str) -> Option<&Path> {
            self.entries.get(mod_id).map(|e| e.path.as_path())
        }

        /// Polls all watched `.py` files if `poll_interval` has elapsed (or `force` is `true`),
        /// returning `(mod_id, script_path)` for every file whose `mtime` or size changed.
        pub fn poll_changes(&mut self, force: bool) -> Vec<(String, PathBuf)> {
            let now = Instant::now();
            if !force
                && let Some(prev) = self.last_poll
                && now.duration_since(prev) < self.poll_interval
            {
                return Vec::new();
            }
            self.last_poll = Some(now);

            let mut changed = Vec::new();
            for (mod_id, entry) in &mut self.entries {
                let (mtime, len) = file_stamp(&entry.path);
                if mtime != entry.last_mtime || len != entry.last_len {
                    entry.last_mtime = mtime;
                    entry.last_len = len;
                    changed.push((mod_id.clone(), entry.path.clone()));
                }
            }
            changed
        }
    }

    fn file_stamp(path: &Path) -> (Option<SystemTime>, u64) {
        match std::fs::metadata(path) {
            Ok(meta) => (meta.modified().ok(), meta.len()),
            Err(_) => (None, 0),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use watcher::PyHotReloader;
