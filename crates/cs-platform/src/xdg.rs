//! Cross-platform XDG, macOS Application Support, Windows, and Steam Deck path & Steam
//! library discovery for `cs16-rs` without `unsafe` code.
//!
//! Covers:
//! - XDG Base Directory (`XDG_CONFIG_HOME`, `XDG_DATA_HOME`), Flatpak Steam, and SteamOS /
//!   Steam Deck detection (`SteamDeck`, `gamescope`, `/etc/steamos-release`, `/etc/os-release`)
//! - macOS `~/Library/Application Support/Steam` and app directories
//! - Windows `%APPDATA%` / `%LOCALAPPDATA%` and Steam library paths
//! - `libraryfolders.vdf` parser for Half-Life / CS 1.6 installations (including Deck microSD)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Operating system category for path resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Linux,
    MacOs,
    Windows,
    Wasm,
    Other,
}

impl PlatformKind {
    /// Returns the compile-time target platform.
    pub const fn current() -> Self {
        if cfg!(target_arch = "wasm32") {
            Self::Wasm
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// Snapshot of environment variables and host OS markers used for deterministic path lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlatformEnv {
    pub platform: Option<PlatformKind>,
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_current_desktop: Option<String>,
    pub appdata: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub cs16_gamedir: Option<PathBuf>,
    pub steam_deck_env: Option<String>,
    pub steam_compat_client_install_path: Option<PathBuf>,
    pub flatpak_id: Option<String>,
    pub steamos_marker: bool,
}

impl PlatformEnv {
    /// Captures relevant environment variables and SteamOS release markers from the host.
    pub fn from_host() -> Self {
        let platform = PlatformKind::current();
        #[cfg(target_arch = "wasm32")]
        {
            Self {
                platform: Some(platform),
                ..Self::default()
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let env_path = |key: &str| {
                std::env::var_os(key)
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            };
            let env_str = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());

            let home = env_path("HOME").or_else(|| env_path("USERPROFILE"));
            let steam_deck_env = env_str("SteamDeck").or_else(|| env_str("STEAM_DECK"));
            let steamos_marker = detect_steamos_files();

            Self {
                platform: Some(platform),
                home,
                xdg_config_home: env_path("XDG_CONFIG_HOME"),
                xdg_data_home: env_path("XDG_DATA_HOME"),
                xdg_current_desktop: env_str("XDG_CURRENT_DESKTOP"),
                appdata: env_path("APPDATA"),
                local_appdata: env_path("LOCALAPPDATA"),
                cs16_gamedir: env_path("CS16_GAMEDIR"),
                steam_deck_env,
                steam_compat_client_install_path: env_path("STEAM_COMPAT_CLIENT_INSTALL_PATH"),
                flatpak_id: env_str("FLATPAK_ID"),
                steamos_marker,
            }
        }
    }

    fn effective_platform(&self) -> PlatformKind {
        self.platform.unwrap_or_else(PlatformKind::current)
    }
}

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
fn detect_steamos_files() -> bool {
    if Path::new("/etc/steamos-release").exists() {
        return true;
    }
    if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
        return parse_os_release_is_steamos(&os_release);
    }
    false
}

#[cfg(all(not(target_os = "linux"), not(target_arch = "wasm32")))]
fn detect_steamos_files() -> bool {
    false
}

/// Checks `/etc/os-release` contents for SteamOS / Steam Deck identifiers.
pub fn parse_os_release_is_steamos(contents: &str) -> bool {
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if let Some((k, v)) = line.split_once('=') {
            let val = v.trim().trim_matches('"');
            if k == "ID" && val.eq_ignore_ascii_case("steamos") {
                return true;
            }
            if k == "VARIANT_ID" && val.eq_ignore_ascii_case("steamdeck") {
                return true;
            }
        }
    }
    false
}

fn valid_app_name(app_name: &str) -> Option<&str> {
    let trimmed = app_name.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains(['/', '\\', '\0', ':'])
        || trimmed.contains("..")
    {
        return None;
    }
    Some(trimmed)
}

fn non_empty_abs(platform: PlatformKind, path: Option<&PathBuf>) -> Option<&Path> {
    path.map(PathBuf::as_path).filter(|p| {
        if p.as_os_str().is_empty() {
            return false;
        }
        match platform {
            PlatformKind::Linux | PlatformKind::MacOs => {
                p.is_absolute() || p.to_string_lossy().starts_with('/')
            }
            _ => p.is_absolute(),
        }
    })
}

fn join_parts(platform: PlatformKind, base: &Path, parts: &[&str]) -> PathBuf {
    if platform == PlatformKind::current() {
        let mut p = base.to_path_buf();
        for seg in parts {
            p.push(seg);
        }
        return p;
    }
    let sep = match platform {
        PlatformKind::Windows => '\\',
        _ => '/',
    };
    let mut s = base.to_string_lossy().into_owned();
    for seg in parts {
        if !s.is_empty() && !s.ends_with(['/', '\\']) {
            s.push(sep);
        }
        s.push_str(seg);
    }
    PathBuf::from(s)
}

/// Returns the user configuration directory for `app_name` on the current host.
pub fn config_dir(app_name: &str) -> Option<PathBuf> {
    config_dir_with(&PlatformEnv::from_host(), app_name)
}

/// Resolves the user configuration directory for `app_name` using an explicit [`PlatformEnv`].
pub fn config_dir_with(env: &PlatformEnv, app_name: &str) -> Option<PathBuf> {
    let app = valid_app_name(app_name)?;
    let platform = env.effective_platform();
    match platform {
        PlatformKind::Wasm => None,
        PlatformKind::MacOs => env
            .home
            .as_ref()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|h| join_parts(platform, h, &["Library", "Application Support", app])),
        PlatformKind::Windows => non_empty_abs(platform, env.appdata.as_ref())
            .map(|p| join_parts(platform, p, &[app]))
            .or_else(|| {
                env.home
                    .as_ref()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|h| join_parts(platform, h, &["AppData", "Roaming", app]))
            }),
        PlatformKind::Linux | PlatformKind::Other => {
            non_empty_abs(platform, env.xdg_config_home.as_ref())
                .map(|p| join_parts(platform, p, &[app]))
                .or_else(|| {
                    env.home
                        .as_ref()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map(|h| join_parts(platform, h, &[".config", app]))
                })
        }
    }
}

/// Returns the user data directory for `app_name` on the current host.
pub fn data_dir(app_name: &str) -> Option<PathBuf> {
    data_dir_with(&PlatformEnv::from_host(), app_name)
}

/// Returns the canonical cache filename `<map>_<bsp_crc32>.nav` for an auto-generated `.nav`
/// mesh, rejecting empty or traversal-unsafe map names.
pub fn nav_cache_filename(map_name: &str, bsp_crc32: u32) -> Option<String> {
    let clean_map = valid_app_name(map_name)?;
    Some(format!("{clean_map}_{bsp_crc32:08x}.nav"))
}

/// Returns `<xdg_data_dir>/nav_cache` for `app_name` on the current host (`None` on wasm32).
pub fn nav_cache_dir(app_name: &str) -> Option<PathBuf> {
    nav_cache_dir_with(&PlatformEnv::from_host(), app_name)
}

/// Deterministic [`nav_cache_dir`] using an explicit [`PlatformEnv`].
pub fn nav_cache_dir_with(env: &PlatformEnv, app_name: &str) -> Option<PathBuf> {
    let base = data_dir_with(env, app_name)?;
    Some(join_parts(env.effective_platform(), &base, &["nav_cache"]))
}

/// Returns `<xdg_data_dir>/nav_cache/<map>_<bsp_crc32>.nav` on the current host (`None` on
/// wasm32 or invalid names).
pub fn nav_cache_path(app_name: &str, map_name: &str, bsp_crc32: u32) -> Option<PathBuf> {
    nav_cache_path_with(&PlatformEnv::from_host(), app_name, map_name, bsp_crc32)
}

/// Deterministic [`nav_cache_path`] using an explicit [`PlatformEnv`].
pub fn nav_cache_path_with(
    env: &PlatformEnv,
    app_name: &str,
    map_name: &str,
    bsp_crc32: u32,
) -> Option<PathBuf> {
    let dir = nav_cache_dir_with(env, app_name)?;
    let filename = nav_cache_filename(map_name, bsp_crc32)?;
    Some(join_parts(env.effective_platform(), &dir, &[&filename]))
}

/// Resolves the user data directory for `app_name` using an explicit [`PlatformEnv`].
pub fn data_dir_with(env: &PlatformEnv, app_name: &str) -> Option<PathBuf> {
    let app = valid_app_name(app_name)?;
    let platform = env.effective_platform();
    match platform {
        PlatformKind::Wasm => None,
        PlatformKind::MacOs => env
            .home
            .as_ref()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|h| join_parts(platform, h, &["Library", "Application Support", app])),
        PlatformKind::Windows => non_empty_abs(platform, env.local_appdata.as_ref())
            .or_else(|| non_empty_abs(platform, env.appdata.as_ref()))
            .map(|p| join_parts(platform, p, &[app]))
            .or_else(|| {
                env.home
                    .as_ref()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|h| join_parts(platform, h, &["AppData", "Local", app]))
            }),
        PlatformKind::Linux | PlatformKind::Other => {
            non_empty_abs(platform, env.xdg_data_home.as_ref())
                .map(|p| join_parts(platform, p, &[app]))
                .or_else(|| {
                    env.home
                        .as_ref()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map(|h| join_parts(platform, h, &[".local", "share", app]))
                })
        }
    }
}

/// Returns `true` when running on a Steam Deck / SteamOS Gaming Mode environment.
pub fn is_steam_deck() -> bool {
    is_steam_deck_with(&PlatformEnv::from_host())
}

/// Evaluates Steam Deck / SteamOS indicators from an explicit [`PlatformEnv`].
pub fn is_steam_deck_with(env: &PlatformEnv) -> bool {
    if env.effective_platform() != PlatformKind::Linux {
        return false;
    }
    if env.steamos_marker {
        return true;
    }
    if let Some(val) = env.steam_deck_env.as_deref() {
        let trimmed = val.trim();
        if trimmed == "1" || trimmed.eq_ignore_ascii_case("true") {
            return true;
        }
    }
    if let Some(desktop) = env.xdg_current_desktop.as_deref()
        && desktop.to_ascii_lowercase().contains("gamescope")
    {
        return true;
    }
    false
}

/// Extracts quoted `"path"` entries from Steam's `libraryfolders.vdf`.
pub fn parse_library_folders_vdf(vdf: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in vdf.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("\"path\"") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            let unescaped = inner.replace("\\\\", "\\");
            if !unescaped.is_empty() && seen.insert(unescaped.clone()) {
                out.push(PathBuf::from(unescaped));
            }
        }
    }
    out
}

/// Returns ordered Steam root directory candidates (including Flatpak Steam and macOS).
pub fn steam_root_candidates_with(env: &PlatformEnv) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    let mut push_unique = |p: PathBuf| {
        if !p.as_os_str().is_empty() && seen.insert(p.clone()) {
            roots.push(p);
        }
    };

    if let Some(compat) = env.steam_compat_client_install_path.as_ref() {
        push_unique(compat.clone());
    }

    let platform = env.effective_platform();
    match platform {
        PlatformKind::Wasm => {}
        PlatformKind::Linux | PlatformKind::Other => {
            if let Some(xdg_data) = non_empty_abs(platform, env.xdg_data_home.as_ref()) {
                push_unique(join_parts(platform, xdg_data, &["Steam"]));
            }
            if let Some(home) = env.home.as_ref().filter(|p| !p.as_os_str().is_empty()) {
                push_unique(join_parts(platform, home, &[".local", "share", "Steam"]));
                push_unique(join_parts(platform, home, &[".steam", "steam"]));
                push_unique(join_parts(platform, home, &[".steam", "debian-installation"]));
                push_unique(join_parts(
                    platform,
                    home,
                    &[".var", "app", "com.valvesoftware.Steam", ".local", "share", "Steam"],
                ));
                push_unique(join_parts(
                    platform,
                    home,
                    &[".var", "app", "com.valvesoftware.Steam", ".steam", "steam"],
                ));
                if env.flatpak_id.is_some() {
                    push_unique(join_parts(platform, home, &[".local", "share", "Steam"]));
                }
            }
        }
        PlatformKind::MacOs => {
            if let Some(home) = env.home.as_ref().filter(|p| !p.as_os_str().is_empty()) {
                push_unique(join_parts(
                    platform,
                    home,
                    &["Library", "Application Support", "Steam"],
                ));
            }
        }
        PlatformKind::Windows => {
            push_unique(PathBuf::from(r"C:\Program Files (x86)\Steam"));
            push_unique(PathBuf::from(r"C:\Program Files\Steam"));
        }
    }

    roots
}

/// Returns candidate Half-Life / CS 1.6 directories (before checking filesystem existence).
pub fn candidate_steam_half_life_dirs_with(
    env: &PlatformEnv,
    read_vdf: &dyn Fn(&Path) -> Option<String>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    let mut push_unique = |p: PathBuf| {
        if !p.as_os_str().is_empty() && seen.insert(p.clone()) {
            dirs.push(p);
        }
    };

    if let Some(explicit) = env.cs16_gamedir.as_ref() {
        push_unique(explicit.clone());
    }

    let platform = env.effective_platform();
    for root in steam_root_candidates_with(env) {
        let vdf = join_parts(platform, &root, &["steamapps", "libraryfolders.vdf"]);
        if let Some(content) = read_vdf(&vdf) {
            for lib in parse_library_folders_vdf(&content) {
                push_unique(join_parts(platform, &lib, &["steamapps", "common", "Half-Life"]));
            }
        }
        push_unique(join_parts(
            platform,
            &root,
            &["steamapps", "common", "Half-Life"],
        ));
    }

    match env.effective_platform() {
        PlatformKind::Linux => {
            // Common Steam Deck microSD mount path when libraryfolders.vdf is absent.
            push_unique(PathBuf::from(
                "/run/media/mmcblk0p1/SteamLibrary/steamapps/common/Half-Life",
            ));
            push_unique(PathBuf::from(
                "/run/media/deck/SteamLibrary/steamapps/common/Half-Life",
            ));
        }
        PlatformKind::Windows => {
            for drive in ["C:", "D:", "E:", "F:"] {
                push_unique(PathBuf::from(format!(
                    r"{drive}\SteamLibrary\steamapps\common\Half-Life"
                )));
                push_unique(PathBuf::from(format!(
                    r"{drive}\Program Files (x86)\Steam\steamapps\common\Half-Life"
                )));
                push_unique(PathBuf::from(format!(r"{drive}\Games\Counter-Strike 1.6")));
            }
        }
        PlatformKind::MacOs | PlatformKind::Wasm | PlatformKind::Other => {}
    }

    dirs
}

/// Discovers existing Half-Life / CS 1.6 directories on the current host across Steam
/// libraries, Flatpak Steam, Steam Deck microSD mounts, and macOS Application Support.
pub fn discover_steam_half_life_dirs() -> Vec<PathBuf> {
    #[cfg(target_arch = "wasm32")]
    {
        Vec::new()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let env = PlatformEnv::from_host();
        discover_steam_half_life_dirs_with(
            &env,
            &|p| std::fs::read_to_string(p).ok(),
            &|p| p.is_dir(),
        )
    }
}

/// Filters [`candidate_steam_half_life_dirs_with`] using explicit VDF reader and directory probe.
pub fn discover_steam_half_life_dirs_with(
    env: &PlatformEnv,
    read_vdf: &dyn Fn(&Path) -> Option<String>,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    candidate_steam_half_life_dirs_with(env, read_vdf)
        .into_iter()
        .filter(|p| is_dir(p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_xdg_and_platform_config_and_data_dirs() {
        let linux_default = PlatformEnv {
            platform: Some(PlatformKind::Linux),
            home: Some(PathBuf::from("/home/alice")),
            ..PlatformEnv::default()
        };
        assert_eq!(
            config_dir_with(&linux_default, "cs16-rs"),
            Some(PathBuf::from("/home/alice/.config/cs16-rs"))
        );
        assert_eq!(
            data_dir_with(&linux_default, "cs16-rs"),
            Some(PathBuf::from("/home/alice/.local/share/cs16-rs"))
        );

        let linux_xdg = PlatformEnv {
            platform: Some(PlatformKind::Linux),
            home: Some(PathBuf::from("/home/alice")),
            xdg_config_home: Some(PathBuf::from("/mnt/cfg")),
            xdg_data_home: Some(PathBuf::from("/mnt/data")),
            ..PlatformEnv::default()
        };
        assert_eq!(
            config_dir_with(&linux_xdg, "cs16-rs"),
            Some(PathBuf::from("/mnt/cfg/cs16-rs"))
        );
        assert_eq!(
            data_dir_with(&linux_xdg, "cs16-rs"),
            Some(PathBuf::from("/mnt/data/cs16-rs"))
        );

        let macos = PlatformEnv {
            platform: Some(PlatformKind::MacOs),
            home: Some(PathBuf::from("/Users/alice")),
            ..PlatformEnv::default()
        };
        assert_eq!(
            config_dir_with(&macos, "cs16-rs"),
            Some(PathBuf::from("/Users/alice/Library/Application Support/cs16-rs"))
        );
        assert_eq!(
            data_dir_with(&macos, "cs16-rs"),
            Some(PathBuf::from("/Users/alice/Library/Application Support/cs16-rs"))
        );

        let windows = PlatformEnv {
            platform: Some(PlatformKind::Windows),
            appdata: Some(PathBuf::from(r"C:\Users\Alice\AppData\Roaming")),
            local_appdata: Some(PathBuf::from(r"C:\Users\Alice\AppData\Local")),
            ..PlatformEnv::default()
        };
        assert_eq!(
            config_dir_with(&windows, "cs16-rs"),
            Some(PathBuf::from(r"C:\Users\Alice\AppData\Roaming\cs16-rs"))
        );
        assert_eq!(
            data_dir_with(&windows, "cs16-rs"),
            Some(PathBuf::from(r"C:\Users\Alice\AppData\Local\cs16-rs"))
        );

        let wasm = PlatformEnv {
            platform: Some(PlatformKind::Wasm),
            ..PlatformEnv::default()
        };
        assert_eq!(config_dir_with(&wasm, "cs16-rs"), None);
        assert_eq!(data_dir_with(&wasm, "cs16-rs"), None);
        assert_eq!(nav_cache_path_with(&wasm, "cs16-rs", "fy_pool_day", 0x1234abcd), None);
        assert_eq!(config_dir_with(&linux_default, "../escape"), None);

        assert_eq!(
            nav_cache_filename("fy_pool_day", 0x0a1b2c3d).as_deref(),
            Some("fy_pool_day_0a1b2c3d.nav")
        );
        assert_eq!(nav_cache_filename("../evil", 1), None);
        assert_eq!(
            nav_cache_path_with(&linux_xdg, "cs16-rs", "fy_pool_day", 0x0a1b2c3d),
            Some(PathBuf::from("/mnt/data/cs16-rs/nav_cache/fy_pool_day_0a1b2c3d.nav"))
        );
        assert_eq!(
            nav_cache_path_with(&windows, "cs16-rs", "awp_india", 0xdeadbeef),
            Some(PathBuf::from(
                r"C:\Users\Alice\AppData\Local\cs16-rs\nav_cache\awp_india_deadbeef.nav"
            ))
        );
    }

    #[test]
    fn detects_steam_deck_from_env_gamescope_and_steamos_release() {
        let plain_linux = PlatformEnv {
            platform: Some(PlatformKind::Linux),
            home: Some(PathBuf::from("/home/alice")),
            xdg_current_desktop: Some("GNOME".to_string()),
            ..PlatformEnv::default()
        };
        assert!(!is_steam_deck_with(&plain_linux));

        let env_flag = PlatformEnv {
            steam_deck_env: Some("1".to_string()),
            ..plain_linux.clone()
        };
        assert!(is_steam_deck_with(&env_flag));

        let gamescope = PlatformEnv {
            xdg_current_desktop: Some("gamescope".to_string()),
            ..plain_linux.clone()
        };
        assert!(is_steam_deck_with(&gamescope));

        let steamos_marker = PlatformEnv {
            steamos_marker: true,
            ..plain_linux
        };
        assert!(is_steam_deck_with(&steamos_marker));

        assert!(parse_os_release_is_steamos("NAME=\"SteamOS\"\nID=steamos\nVARIANT_ID=steamdeck\n"));
        assert!(!parse_os_release_is_steamos("NAME=\"Ubuntu\"\nID=ubuntu\n"));

        let mac = PlatformEnv {
            platform: Some(PlatformKind::MacOs),
            steam_deck_env: Some("1".to_string()),
            ..PlatformEnv::default()
        };
        assert!(!is_steam_deck_with(&mac));
    }

    #[test]
    fn discovers_linux_flatpak_deck_sd_and_macos_steam_half_life_dirs() {
        let deck_env = PlatformEnv {
            platform: Some(PlatformKind::Linux),
            home: Some(PathBuf::from("/home/deck")),
            cs16_gamedir: Some(PathBuf::from("/custom/Half-Life")),
            ..PlatformEnv::default()
        };
        let vdf = r#"
        "libraryfolders"
        {
            "0"
            {
                "path"  "/home/deck/.local/share/Steam"
            }
            "1"
            {
                "path"  "/run/media/mmcblk0p1/SteamLibrary"
            }
        }
        "#;
        let flatpak_vdf = r#"
        "libraryfolders"
        {
            "0"
            {
                "path"  "/home/deck/.var/app/com.valvesoftware.Steam/.local/share/Steam"
            }
        }
        "#;

        let read_vdf = |p: &Path| {
            let s = p.to_string_lossy();
            if s.contains("com.valvesoftware.Steam") && s.contains("libraryfolders.vdf") {
                Some(flatpak_vdf.to_string())
            } else if s.ends_with("/home/deck/.local/share/Steam/steamapps/libraryfolders.vdf")
                || s.ends_with(r"\home\deck\.local\share\Steam\steamapps\libraryfolders.vdf")
            {
                Some(vdf.to_string())
            } else {
                None
            }
        };

        let existing = [
            PathBuf::from("/custom/Half-Life"),
            PathBuf::from("/run/media/mmcblk0p1/SteamLibrary/steamapps/common/Half-Life"),
            PathBuf::from(
                "/home/deck/.var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/Half-Life",
            ),
        ];
        let found = discover_steam_half_life_dirs_with(&deck_env, &read_vdf, &|p| {
            existing.iter().any(|e| e == p)
        });
        assert_eq!(found, existing.to_vec());

        let mac_env = PlatformEnv {
            platform: Some(PlatformKind::MacOs),
            home: Some(PathBuf::from("/Users/player")),
            ..PlatformEnv::default()
        };
        let mac_candidates = candidate_steam_half_life_dirs_with(&mac_env, &|_| None);
        assert_eq!(
            mac_candidates,
            vec![PathBuf::from(
                "/Users/player/Library/Application Support/Steam/steamapps/common/Half-Life"
            )]
        );
    }
}
