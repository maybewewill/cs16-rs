//! Package loader for directory mods (`mods/<id>/mod.toml`) and `.csmod` ZIP archives,
//! with dependency resolution and topological load ordering.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use cs_mod_api::{ArchiveError, CsModArchive, ManifestError, ModManifest, sha256_hex};
use thiserror::Error;

use crate::compile_wat_to_wasm;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("I/O error reading `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("missing mod.toml in `{0}`")]
    MissingManifest(String),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error("invalid WASM/WAT entrypoint `{0}`: {1}")]
    EntryCompile(String, String),
    #[error("mod `{mod_id}` requires missing dependency `{dep}`")]
    MissingDependency { mod_id: String, dep: String },
    #[error("dependency cycle involving mod `{0}`")]
    DependencyCycle(String),
}

/// Self-contained mod package ready to load into [`ServerModHost`](crate::ServerModHost) or
/// [`ClientModHost`](crate::ClientModHost).
#[derive(Debug, Clone)]
pub struct LoadedPackage {
    pub manifest: ModManifest,
    pub source_path: Option<PathBuf>,
    pub server_wasm: Option<Vec<u8>>,
    pub client_wasm: Option<Vec<u8>>,
    /// Bundled client deliverable (`.csmod` zip containing `mod.toml`, `client.wasm`, and `assets/*`)
    /// plus its SHA-256 hex digest for auto-download verification.
    pub client_bundle: Option<(Vec<u8>, String)>,
    pub assets: BTreeMap<String, Vec<u8>>,
}

impl LoadedPackage {
    /// Builds `client_bundle` + SHA-256 digest when client-side WASM or assets exist.
    pub fn build_client_bundle(&mut self) -> Result<(), PackageError> {
        if self.client_wasm.is_none() && self.assets.is_empty() {
            self.client_bundle = None;
            return Ok(());
        }
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        let mut client_manifest = self.manifest.clone();
        client_manifest.entry_server = None;
        if self.client_wasm.is_some() {
            client_manifest.entry_client = Some("client.wasm".to_string());
        } else {
            client_manifest.entry_client = None;
        }
        let toml_text = client_manifest.to_toml();
        entries.push(("mod.toml".to_string(), toml_text.into_bytes()));
        if let Some(ref wasm) = self.client_wasm {
            entries.push(("client.wasm".to_string(), wasm.clone()));
        }
        for (rel, data) in &self.assets {
            entries.push((format!("assets/{rel}"), data.clone()));
        }
        let refs: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        let zip = CsModArchive::write_zip(refs)?;
        let sha = sha256_hex(&zip);
        self.client_bundle = Some((zip, sha));
        Ok(())
    }
}

/// Loads a mod package from an unpacked directory (`mod.toml`, `.wasm`/`.wat`, optional `assets/`).
pub fn load_package_dir(dir: &Path) -> Result<LoadedPackage, PackageError> {
    let manifest_path = dir.join("mod.toml");
    let toml_src = fs::read_to_string(&manifest_path).map_err(|source| PackageError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest = ModManifest::parse_toml(&toml_src)?;

    let server_wasm = match manifest.entry_server.as_deref() {
        Some(rel) => Some(read_or_compile_entry_disk(dir, rel)?),
        None => None,
    };
    let client_wasm = match manifest.entry_client.as_deref() {
        Some(rel) => Some(read_or_compile_entry_disk(dir, rel)?),
        None => None,
    };

    let mut assets = BTreeMap::new();
    for rel in &manifest.assets {
        let candidate_assets = dir.join("assets").join(rel);
        let candidate_direct = dir.join(rel);
        let path = if candidate_assets.is_file() {
            candidate_assets
        } else {
            candidate_direct
        };
        let bytes = fs::read(&path).map_err(|source| PackageError::Io { path, source })?;
        assets.insert(rel.clone(), bytes);
    }

    // Also include any extra files under `assets/` even if not enumerated explicitly.
    let assets_dir = dir.join("assets");
    if assets_dir.is_dir() {
        collect_disk_assets(&assets_dir, &assets_dir, &mut assets)?;
    }

    let mut pkg = LoadedPackage {
        manifest,
        source_path: Some(dir.to_path_buf()),
        server_wasm,
        client_wasm,
        client_bundle: None,
        assets,
    };
    pkg.build_client_bundle()?;
    Ok(pkg)
}

/// Loads a mod package directly from `.csmod` ZIP bytes.
pub fn load_package_csmod(
    zip_bytes: &[u8],
    source_path: Option<PathBuf>,
) -> Result<LoadedPackage, PackageError> {
    let entries = CsModArchive::read_zip(zip_bytes)?;
    let toml_bytes = entries
        .get("mod.toml")
        .ok_or_else(|| PackageError::MissingManifest("archive".to_string()))?;
    let toml_src = std::str::from_utf8(toml_bytes)
        .map_err(|e| PackageError::EntryCompile("mod.toml".to_string(), e.to_string()))?;
    let manifest = ModManifest::parse_toml(toml_src)?;

    let server_wasm = match manifest.entry_server.as_deref() {
        Some(rel) => Some(read_or_compile_entry_map(&entries, rel)?),
        None => None,
    };
    let client_wasm = match manifest.entry_client.as_deref() {
        Some(rel) => Some(read_or_compile_entry_map(&entries, rel)?),
        None => None,
    };

    let mut assets = BTreeMap::new();
    for (k, v) in &entries {
        if let Some(rel) = k.strip_prefix("assets/") {
            assets.insert(rel.to_string(), v.clone());
        } else if manifest.assets.contains(k) {
            assets.insert(k.clone(), v.clone());
        }
    }

    let mut pkg = LoadedPackage {
        manifest,
        source_path,
        server_wasm,
        client_wasm,
        client_bundle: None,
        assets,
    };
    pkg.build_client_bundle()?;
    Ok(pkg)
}

/// Scans `mods_dir` (including nested category folders such as `mods/std/` and `mods/gamemodes/`)
/// for unpacked subdirectories (`<id>/mod.toml`) and `*.csmod` archives, returning packages sorted
/// deterministically in dependency order.
pub fn discover_packages(mods_dir: &Path) -> Result<Vec<LoadedPackage>, PackageError> {
    if !mods_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut raw_pkgs = Vec::new();
    collect_packages_recursive(mods_dir, &mut raw_pkgs)?;
    topo_sort_packages(raw_pkgs)
}

fn collect_packages_recursive(
    dir: &Path,
    out: &mut Vec<LoadedPackage>,
) -> Result<(), PackageError> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|source| PackageError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();

    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || matches!(name, "target" | "assets" | "src" | "client") {
            continue;
        }
        if path.is_dir() {
            if path.join("mod.toml").is_file() {
                out.push(load_package_dir(&path)?);
            } else {
                collect_packages_recursive(&path, out)?;
            }
        } else if path.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("csmod")
        {
            let bytes = fs::read(&path).map_err(|source| PackageError::Io {
                path: path.clone(),
                source,
            })?;
            out.push(load_package_csmod(&bytes, Some(path))?);
        }
    }
    Ok(())
}

/// Topologically orders packages so dependencies always initialize before dependents.
pub fn topo_sort_packages(pkgs: Vec<LoadedPackage>) -> Result<Vec<LoadedPackage>, PackageError> {
    let mut by_id: BTreeMap<String, LoadedPackage> = BTreeMap::new();
    for pkg in pkgs {
        by_id.insert(pkg.manifest.id.clone(), pkg);
    }

    for (id, pkg) in &by_id {
        for dep in &pkg.manifest.dependencies {
            if !by_id.contains_key(dep) {
                return Err(PackageError::MissingDependency {
                    mod_id: id.clone(),
                    dep: dep.clone(),
                });
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Visiting,
        Visited,
    }
    let mut states: BTreeMap<String, State> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let ids: Vec<String> = by_id.keys().cloned().collect();

    fn visit(
        id: &str,
        by_id: &BTreeMap<String, LoadedPackage>,
        states: &mut BTreeMap<String, State>,
        order: &mut Vec<String>,
    ) -> Result<(), PackageError> {
        if let Some(st) = states.get(id) {
            return match st {
                State::Visited => Ok(()),
                State::Visiting => Err(PackageError::DependencyCycle(id.to_string())),
            };
        }
        states.insert(id.to_string(), State::Visiting);
        if let Some(pkg) = by_id.get(id) {
            for dep in &pkg.manifest.dependencies {
                visit(dep, by_id, states, order)?;
            }
        }
        states.insert(id.to_string(), State::Visited);
        order.push(id.to_string());
        Ok(())
    }

    for id in &ids {
        visit(id, &by_id, &mut states, &mut order)?;
    }

    let mut seen = BTreeSet::new();
    let mut sorted = Vec::with_capacity(order.len());
    for id in order {
        if seen.insert(id.clone())
            && let Some(pkg) = by_id.remove(&id)
        {
            sorted.push(pkg);
        }
    }
    Ok(sorted)
}

fn read_or_compile_entry_disk(dir: &Path, rel: &str) -> Result<Vec<u8>, PackageError> {
    let exact = dir.join(rel);
    if exact.is_file() {
        let bytes = fs::read(&exact).map_err(|source| PackageError::Io {
            path: exact,
            source,
        })?;
        return maybe_compile_wat(rel, bytes);
    }
    if let Some(stem) = rel.strip_suffix(".wasm") {
        let wat_rel = format!("{stem}.wat");
        let wat_path = dir.join(&wat_rel);
        if wat_path.is_file() {
            let text = fs::read_to_string(&wat_path).map_err(|source| PackageError::Io {
                path: wat_path,
                source,
            })?;
            return compile_wat_to_wasm(&text)
                .map_err(|e| PackageError::EntryCompile(wat_rel, e));
        }
    }
    Err(PackageError::Io {
        path: exact,
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "entry module missing"),
    })
}

fn read_or_compile_entry_map(
    entries: &BTreeMap<String, Vec<u8>>,
    rel: &str,
) -> Result<Vec<u8>, PackageError> {
    if let Some(bytes) = entries.get(rel) {
        return maybe_compile_wat(rel, bytes.clone());
    }
    if let Some(stem) = rel.strip_suffix(".wasm") {
        let wat_rel = format!("{stem}.wat");
        if let Some(bytes) = entries.get(&wat_rel) {
            let text = std::str::from_utf8(bytes)
                .map_err(|e| PackageError::EntryCompile(wat_rel.clone(), e.to_string()))?;
            return compile_wat_to_wasm(text)
                .map_err(|e| PackageError::EntryCompile(wat_rel, e));
        }
    }
    if let Some(stem) = rel.strip_suffix(".wat") {
        let wasm_rel = format!("{stem}.wasm");
        if let Some(bytes) = entries.get(&wasm_rel) {
            return Ok(bytes.clone());
        }
    }
    Err(PackageError::EntryCompile(
        rel.to_string(),
        "entry not present in archive".to_string(),
    ))
}

fn maybe_compile_wat(rel: &str, bytes: Vec<u8>) -> Result<Vec<u8>, PackageError> {
    if rel.to_ascii_lowercase().ends_with(".py") {
        return Ok(bytes);
    }
    if rel.ends_with(".wat") || (!bytes.starts_with(b"\0asm") && bytes.trim_ascii_start().starts_with(b"(")) {
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| PackageError::EntryCompile(rel.to_string(), e.to_string()))?;
        compile_wat_to_wasm(text).map_err(|e| PackageError::EntryCompile(rel.to_string(), e))
    } else {
        Ok(bytes)
    }
}

fn collect_disk_assets(
    base: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), PackageError> {
    let entries = fs::read_dir(dir).map_err(|source| PackageError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_disk_assets(base, &path, out)?;
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(base)
        {
            let norm = rel.to_string_lossy().replace('\\', "/");
            if let std::collections::btree_map::Entry::Vacant(e) = out.entry(norm) {
                let bytes = fs::read(&path).map_err(|source| PackageError::Io {
                    path: path.clone(),
                    source,
                })?;
                e.insert(bytes);
            }
        }
    }
    Ok(())
}
