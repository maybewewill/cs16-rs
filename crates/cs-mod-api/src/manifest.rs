//! `mod.toml` manifest parser, validator, and capability bitmask (`Permission` / `Permissions`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ABI_VERSION;
use crate::abi::ModSide;

pub const DEFAULT_FUEL_PER_FRAME: u64 = 500_000;
pub const MAX_FUEL_PER_FRAME: u64 = 10_000_000;
pub const DEFAULT_MEMORY_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MEMORY_CAP: usize = 32 * 1024 * 1024;

/// Fine-grained sandbox permissions declared in `mod.toml`. Default is empty (no capabilities).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum Permission {
    Entities = 1 << 0,
    Messages = 1 << 1,
    Cvars = 1 << 2,
    Commands = 1 << 3,
    Menus = 1 << 4,
    Timers = 1 << 5,
    Traces = 1 << 6,
    Sound = 1 << 7,
    Storage = 1 << 8,
    Hud = 1 << 9,
    Input = 1 << 10,
    NetCustom = 1 << 11,
}

impl Permission {
    pub const ALL_VARIANTS: [Self; 12] = [
        Self::Entities,
        Self::Messages,
        Self::Cvars,
        Self::Commands,
        Self::Menus,
        Self::Timers,
        Self::Traces,
        Self::Sound,
        Self::Storage,
        Self::Hud,
        Self::Input,
        Self::NetCustom,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "entities" => Some(Self::Entities),
            "messages" => Some(Self::Messages),
            "cvars" => Some(Self::Cvars),
            "commands" => Some(Self::Commands),
            "menus" => Some(Self::Menus),
            "timers" => Some(Self::Timers),
            "traces" => Some(Self::Traces),
            "sound" => Some(Self::Sound),
            "storage" => Some(Self::Storage),
            "hud" => Some(Self::Hud),
            "input" => Some(Self::Input),
            "net_custom" | "netcustom" => Some(Self::NetCustom),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entities => "entities",
            Self::Messages => "messages",
            Self::Cvars => "cvars",
            Self::Commands => "commands",
            Self::Menus => "menus",
            Self::Timers => "timers",
            Self::Traces => "traces",
            Self::Sound => "sound",
            Self::Storage => "storage",
            Self::Hud => "hud",
            Self::Input => "input",
            Self::NetCustom => "net_custom",
        }
    }
}

/// Bitmask of declared [`Permission`]s.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions(u32);

impl Permissions {
    pub const NONE: Self = Self(0);

    pub fn from_bits_truncate(bits: u32) -> Self {
        let valid_mask: u32 = Permission::ALL_VARIANTS
            .iter()
            .fold(0u32, |acc, &p| acc | (p as u32));
        Self(bits & valid_mask)
    }

    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn contains(self, perm: Permission) -> bool {
        (self.0 & (perm as u32)) != 0
    }

    pub fn insert(&mut self, perm: Permission) {
        self.0 |= perm as u32;
    }

    pub fn names(self) -> Vec<&'static str> {
        Permission::ALL_VARIANTS
            .iter()
            .copied()
            .filter(|&p| self.contains(p))
            .map(Permission::as_str)
            .collect()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ManifestError {
    #[error("missing required field `{0}` in mod.toml")]
    MissingField(&'static str),
    #[error("invalid mod id `{0}` (expected [a-z0-9_-])")]
    InvalidId(String),
    #[error("unsupported mod api_version {0} (host supports {ABI_VERSION})")]
    UnsupportedApiVersion(u32),
    #[error("invalid side `{0}` (expected server, client, or both)")]
    InvalidSide(String),
    #[error("unknown permission `{0}`")]
    UnknownPermission(String),
    #[error("unsafe relative path `{0}`")]
    UnsafePath(String),
    #[error("malformed mod.toml line: {0}")]
    ParseLine(String),
}

/// Parsed and validated `mod.toml` package manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub side: ModSide,
    pub entry_server: Option<String>,
    pub entry_client: Option<String>,
    pub dependencies: Vec<String>,
    pub permissions: Permissions,
    pub assets: Vec<String>,
    pub custom_usermsgs: Vec<String>,
    pub fuel_per_frame: u64,
    pub max_memory_bytes: usize,
}

impl ModManifest {
    /// Parses and strictly validates a `mod.toml` UTF-8 document.
    pub fn parse_toml(src: &str) -> Result<Self, ManifestError> {
        let mut id = None;
        let mut name = None;
        let mut version = None;
        let mut api_version = ABI_VERSION;
        let mut side = ModSide::Server;
        let mut entry_server = None;
        let mut entry_client = None;
        let mut dependencies = Vec::new();
        let mut permissions = Permissions::NONE;
        let mut assets = Vec::new();
        let mut custom_usermsgs = Vec::new();
        let mut fuel_per_frame = DEFAULT_FUEL_PER_FRAME;
        let mut max_memory_bytes = DEFAULT_MEMORY_BYTES;

        let mut pending_key: Option<String> = None;
        let mut pending_buf = String::new();

        for raw_line in src.lines() {
            let line = strip_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(ref key) = pending_key {
                pending_buf.push(' ');
                pending_buf.push_str(line);
                if line.contains(']') {
                    let k = key.clone();
                    let val = std::mem::take(&mut pending_buf);
                    pending_key = None;
                    apply_kv(
                        &k,
                        &val,
                        &mut id,
                        &mut name,
                        &mut version,
                        &mut api_version,
                        &mut side,
                        &mut entry_server,
                        &mut entry_client,
                        &mut dependencies,
                        &mut permissions,
                        &mut assets,
                        &mut custom_usermsgs,
                        &mut fuel_per_frame,
                        &mut max_memory_bytes,
                    )?;
                }
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                continue;
            }
            let Some((k_raw, v_raw)) = line.split_once('=') else {
                return Err(ManifestError::ParseLine(line.to_string()));
            };
            let k = k_raw.trim().to_string();
            let v = v_raw.trim();
            if v.starts_with('[') && !v.contains(']') {
                pending_key = Some(k);
                pending_buf = v.to_string();
                continue;
            }
            apply_kv(
                &k,
                v,
                &mut id,
                &mut name,
                &mut version,
                &mut api_version,
                &mut side,
                &mut entry_server,
                &mut entry_client,
                &mut dependencies,
                &mut permissions,
                &mut assets,
                &mut custom_usermsgs,
                &mut fuel_per_frame,
                &mut max_memory_bytes,
            )?;
        }

        let id = id.ok_or(ManifestError::MissingField("id"))?;
        if !is_valid_id(&id) {
            return Err(ManifestError::InvalidId(id));
        }
        let version = version.ok_or(ManifestError::MissingField("version"))?;
        if version.is_empty() {
            return Err(ManifestError::MissingField("version"));
        }
        if api_version != ABI_VERSION {
            return Err(ManifestError::UnsupportedApiVersion(api_version));
        }

        for p in entry_server.iter().chain(entry_client.iter()).chain(assets.iter()) {
            if !is_safe_rel_path(p) {
                return Err(ManifestError::UnsafePath(p.clone()));
            }
        }
        for dep in &dependencies {
            if !is_valid_id(dep) {
                return Err(ManifestError::InvalidId(dep.clone()));
            }
        }

        let name = name.unwrap_or_else(|| id.clone());
        Ok(Self {
            id,
            name,
            version,
            api_version,
            side,
            entry_server,
            entry_client,
            dependencies,
            permissions,
            assets,
            custom_usermsgs,
            fuel_per_frame: fuel_per_frame.clamp(1_000, MAX_FUEL_PER_FRAME),
            max_memory_bytes: max_memory_bytes.clamp(65_536, MAX_MEMORY_CAP),
        })
    }

    pub fn to_toml(&self) -> String {
        let mut s = format!(
            "[mod]\nid = \"{}\"\nname = \"{}\"\nversion = \"{}\"\napi_version = {}\nside = \"{}\"\n",
            self.id,
            self.name.replace('"', "'"),
            self.version.replace('"', "'"),
            self.api_version,
            self.side.as_str(),
        );
        if let Some(ref e) = self.entry_server {
            s.push_str(&format!("entry_server = \"{e}\"\n"));
        }
        if let Some(ref e) = self.entry_client {
            s.push_str(&format!("entry_client = \"{e}\"\n"));
        }
        if !self.dependencies.is_empty() {
            s.push_str(&format!("dependencies = {}\n", fmt_str_array(&self.dependencies)));
        }
        let perm_names: Vec<String> = self
            .permissions
            .names()
            .into_iter()
            .map(str::to_string)
            .collect();
        s.push_str(&format!("permissions = {}\n", fmt_str_array(&perm_names)));
        if !self.assets.is_empty() {
            s.push_str(&format!("assets = {}\n", fmt_str_array(&self.assets)));
        }
        if !self.custom_usermsgs.is_empty() {
            s.push_str(&format!(
                "custom_usermsgs = {}\n",
                fmt_str_array(&self.custom_usermsgs)
            ));
        }
        s.push_str(&format!("fuel_per_frame = {}\n", self.fuel_per_frame));
        s.push_str(&format!("max_memory_bytes = {}\n", self.max_memory_bytes));
        s
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_kv(
    k: &str,
    v: &str,
    id: &mut Option<String>,
    name: &mut Option<String>,
    version: &mut Option<String>,
    api_version: &mut u32,
    side: &mut ModSide,
    entry_server: &mut Option<String>,
    entry_client: &mut Option<String>,
    dependencies: &mut Vec<String>,
    permissions: &mut Permissions,
    assets: &mut Vec<String>,
    custom_usermsgs: &mut Vec<String>,
    fuel_per_frame: &mut u64,
    max_memory_bytes: &mut usize,
) -> Result<(), ManifestError> {
    match k {
        "id" => *id = Some(parse_str(v)?),
        "name" => *name = Some(parse_str(v)?),
        "version" => *version = Some(parse_str(v)?),
        "api_version" => {
            *api_version = v
                .parse::<u32>()
                .map_err(|_| ManifestError::ParseLine(format!("{k} = {v}")))?;
        }
        "side" => {
            let s = parse_str(v)?;
            *side = ModSide::parse(&s).ok_or(ManifestError::InvalidSide(s))?;
        }
        "entry_server" => *entry_server = Some(parse_str(v)?),
        "entry_client" => *entry_client = Some(parse_str(v)?),
        "dependencies" => *dependencies = parse_str_array(v)?,
        "permissions" => {
            *permissions = Permissions::NONE;
            for item in parse_str_array(v)? {
                let perm = Permission::parse(&item).ok_or(ManifestError::UnknownPermission(item))?;
                permissions.insert(perm);
            }
        }
        "assets" => *assets = parse_str_array(v)?,
        "custom_usermsgs" => *custom_usermsgs = parse_str_array(v)?,
        "fuel_per_frame" => {
            *fuel_per_frame = v
                .replace('_', "")
                .parse::<u64>()
                .map_err(|_| ManifestError::ParseLine(format!("{k} = {v}")))?;
        }
        "max_memory_bytes" => {
            *max_memory_bytes = v
                .replace('_', "")
                .parse::<usize>()
                .map_err(|_| ManifestError::ParseLine(format!("{k} = {v}")))?;
        }
        _ => {}
    }
    Ok(())
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_str(v: &str) -> Result<String, ManifestError> {
    let t = v.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        Ok(t[1..t.len() - 1].to_string())
    } else {
        Err(ManifestError::ParseLine(v.to_string()))
    }
}

fn parse_str_array(v: &str) -> Result<Vec<String>, ManifestError> {
    let t = v.trim();
    let inner = t
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| ManifestError::ParseLine(v.to_string()))?
        .trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for item in inner.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(parse_str(trimmed)?);
    }
    Ok(out)
}

fn fmt_str_array(items: &[String]) -> String {
    let quoted: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", s.replace('"', "'")))
        .collect();
    format!("[{}]", quoted.join(", "))
}

pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

pub fn is_safe_rel_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with(['/', '\\']) || path.contains(['\\', '\0', ':']) {
        return false;
    }
    !path
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_valid_toml_and_rejects_bad_paths_or_abi() {
        let raw = r#"
            [mod]
            id = "gungame"
            name = "GunGame"
            version = "1.0.0"
            api_version = 1
            side = "both"
            entry_server = "server.wasm"
            entry_client = "client.wasm"
            dependencies = ["deathmatch"]
            permissions = ["entities", "messages", "storage", "hud"]
            assets = ["sprites/gg.txt"]
            custom_usermsgs = ["CsModGg"]
            fuel_per_frame = 250_000
            max_memory_bytes = 2_097_152
        "#;
        let m = ModManifest::parse_toml(raw).unwrap();
        assert_eq!(m.id, "gungame");
        assert_eq!(m.side, ModSide::Both);
        assert!(m.permissions.contains(Permission::Entities));
        assert!(m.permissions.contains(Permission::Hud));
        assert!(!m.permissions.contains(Permission::Traces));
        assert_eq!(m.fuel_per_frame, 250_000);
        assert_eq!(m.max_memory_bytes, 2_097_152);
        let round = ModManifest::parse_toml(&m.to_toml()).unwrap();
        assert_eq!(round, m);

        let bad_abi = "id = \"x\"\nversion = \"1.0\"\napi_version = 99\n";
        assert_eq!(
            ModManifest::parse_toml(bad_abi),
            Err(ManifestError::UnsupportedApiVersion(99))
        );

        let bad_path = "id = \"x\"\nversion = \"1.0\"\nassets = [\"../secret\"]\n";
        assert!(matches!(
            ModManifest::parse_toml(bad_path),
            Err(ManifestError::UnsafePath(_))
        ));
    }
}
