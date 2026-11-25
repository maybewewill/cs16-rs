//! Client console variables: the ones the connection reads (`cl_cmdrate`, `cl_updaterate`,
//! `rate`, `ex_interp`, `cl_lw`, `cl_lc`, `name`, `model`, ...) and the ones that make up the
//! userinfo string.
//!
//! The flag set and the "privileged / filterable" rules follow Xash3D FWGS `engine/common/cvar.c`
//! (`Cvar_ShouldSetCvar`, `Cvar_CommandWithPrivilegeCheck`, read 2026-11-23) and the cvar
//! definitions of `engine/client/cl_main.c` (read 2026-11-23). Defaults that differ from Xash are
//! named at the definition.

use std::collections::BTreeMap;

/// Part of the userinfo string (a change is sent to the server with `setinfo`).
pub const CVAR_USERINFO: u32 = 1 << 0;
/// Saved in the user's config.
pub const CVAR_ARCHIVE: u32 = 1 << 1;
/// Settable only by the local user, never from a stufftext (`FCVAR_PRIVILEGED`).
pub const CVAR_PRIVILEGED: u32 = 1 << 2;
/// Refused from a stufftext while `cl_filterstuffcmd` is on (`FCVAR_FILTERABLE`).
pub const CVAR_FILTERABLE: u32 = 1 << 3;
/// Never reported to the server in a `svc_sendcvarvalue` answer as its real value.
pub const CVAR_PROTECTED: u32 = 1 << 4;

/// Name prefixes a stufftext may not touch while `cl_filterstuffcmd` is on
/// (`Cvar_ShouldSetCvar`, `Cmd_ShouldAllowCommand`).
pub const FILTERED_PREFIXES: [&str; 8] = [
    "cl_", "gl_", "m_", "r_", "hud_", "joy_", "con_", "scr_",
];

/// One console variable.
#[derive(Debug, Clone, PartialEq)]
pub struct Cvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub flags: u32,
}

impl Cvar {
    /// The value as a float; `0.0` when it does not parse (`Q_atof`).
    pub fn float(&self) -> f32 {
        parse_float(&self.value)
    }
}

/// `Q_atof`-like: the leading number of the text, `0.0` otherwise.
pub fn parse_float(text: &str) -> f32 {
    let t = text.trim();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) || c == '.' {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    t[..end].parse().unwrap_or(0.0)
}

/// The client's console variables, looked up without regard to case (`Q_stricmp`).
#[derive(Debug, Clone)]
pub struct ClientCvars {
    vars: BTreeMap<String, Cvar>,
}

impl Default for ClientCvars {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientCvars {
    /// The cvars the connection needs, with their defaults.
    ///
    /// Deviations from Xash's defaults, kept so a client built on this core sends the userinfo
    /// `cs-probe` has always sent (`default_cs16_userinfo`): `cl_updaterate` 60 (Xash 20),
    /// `rate` 30000 (Xash 25000), `cl_cmdrate` 100 (Xash 30; 100 is the engine's upper bound and
    /// the probe's fixed rate), `topcolor`/`bottomcolor` 30/6, `model` `gordon`, `cl_dlmax` 512.
    pub fn new() -> Self {
        let mut cvars = Self {
            vars: BTreeMap::new(),
        };
        let userinfo = CVAR_USERINFO | CVAR_ARCHIVE;
        let table: &[(&str, &str, u32)] = &[
            ("cl_updaterate", "60", userinfo),
            ("cl_cmdrate", "100", CVAR_ARCHIVE),
            ("rate", "30000", userinfo | CVAR_FILTERABLE),
            ("ex_interp", "0.1", CVAR_ARCHIVE | CVAR_FILTERABLE),
            ("cl_lw", "1", userinfo),
            // Lag compensation switch; default `"1"` in Xash3D FWGS `engine/client/cl_main.c:3731`.
            ("cl_lc", "1", userinfo),
            ("cl_dlmax", "512", userinfo),
            ("name", "player", userinfo | CVAR_FILTERABLE),
            ("model", "gordon", userinfo | CVAR_FILTERABLE),
            ("topcolor", "30", userinfo | CVAR_FILTERABLE),
            ("bottomcolor", "6", userinfo | CVAR_FILTERABLE),
            ("cl_nointerp", "0", 0),
            // Task 5 (prediction.rs): `cl_nopred`, `cl_nosmooth`, `cl_smoothtime` (cl_pmove.c).
            ("cl_nopred", "0", 0),
            ("cl_nosmooth", "0", CVAR_ARCHIVE),
            ("cl_smoothtime", "0.1", CVAR_ARCHIVE),
            ("cl_solid_players", "1", 0),
            ("cl_allowdownload", "1", CVAR_ARCHIVE),
            ("cl_allowupload", "1", CVAR_ARCHIVE),
            ("cl_download_ingame", "1", CVAR_ARCHIVE),
            // Default `"1"` in Xash3D FWGS `engine/common/cvar.c:51` (`cl_filterstuffcmd`).
            ("cl_filterstuffcmd", "1", CVAR_ARCHIVE | CVAR_PRIVILEGED),
            ("cl_trace_stufftext", "0", 0),
            ("cl_trace_consistency", "0", 0),
        ];
        for &(name, value, flags) in table {
            cvars.register(name, value, flags);
        }
        cvars
    }

    /// Adds a cvar, or resets the default and flags of one that exists.
    pub fn register(&mut self, name: &str, value: &str, flags: u32) {
        self.vars.insert(
            name.to_ascii_lowercase(),
            Cvar {
                name: name.to_string(),
                value: value.to_string(),
                default: value.to_string(),
                flags,
            },
        );
    }

    pub fn find(&self, name: &str) -> Option<&Cvar> {
        self.vars.get(&name.to_ascii_lowercase())
    }

    /// The value of `name`, or `""` when there is no such cvar (`Cvar_VariableString`).
    pub fn string(&self, name: &str) -> &str {
        self.find(name).map_or("", |c| c.value.as_str())
    }

    /// The value of `name` as a float, `0.0` when there is no such cvar.
    pub fn float(&self, name: &str) -> f32 {
        self.find(name).map_or(0.0, Cvar::float)
    }

    /// Sets `name` to `value` regardless of privilege (`Cvar_DirectSet`). Returns the
    /// previous value when it changed, `None` when the cvar does not exist or kept its value.
    pub fn set(&mut self, name: &str, value: &str) -> Option<String> {
        let var = self.vars.get_mut(&name.to_ascii_lowercase())?;
        if var.value == value {
            return None;
        }
        Some(std::mem::replace(&mut var.value, value.to_string()))
    }

    /// Every cvar that belongs in the userinfo string, as `(key, value)`.
    pub fn userinfo_pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .values()
            .filter(|c| c.flags & CVAR_USERINFO != 0)
            .map(|c| (c.name.as_str(), c.value.as_str()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Cvar> {
        self.vars.values()
    }

    /// Whether a stufftext may set `cvar`: `Cvar_ShouldSetCvar` for an unprivileged source
    /// (Xash `engine/common/cvar.c`, read 2026-11-23). The game-specific "filter quirks"
    /// exceptions are not ported.
    pub fn stufftext_may_set(&self, cvar: &Cvar) -> bool {
        if cvar.flags & CVAR_PRIVILEGED != 0 || cvar.name.starts_with('@') {
            return false;
        }
        if self.float("cl_filterstuffcmd") <= 0.0 {
            return true;
        }
        if cvar.flags & CVAR_FILTERABLE != 0 {
            return false;
        }
        !has_filtered_prefix(&cvar.name)
    }
}

/// Whether `name` starts with one of [`FILTERED_PREFIXES`], without regard to case.
pub fn has_filtered_prefix(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    FILTERED_PREFIXES.iter().any(|p| lower.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_ignores_case_and_set_reports_the_old_value() {
        let mut c = ClientCvars::new();
        assert_eq!(c.string("CL_CmdRate"), "100");
        assert_eq!(c.set("cl_cmdrate", "30").as_deref(), Some("100"));
        assert_eq!(c.set("cl_cmdrate", "30"), None);
        assert_eq!(c.set("no_such_cvar", "1"), None);
        assert_eq!(c.float("cl_cmdrate"), 30.0);
    }

    #[test]
    fn userinfo_cvars_are_the_probe_defaults() {
        let c = ClientCvars::new();
        let pairs: BTreeMap<_, _> = c.userinfo_pairs().collect();
        assert_eq!(pairs["cl_updaterate"], "60");
        assert_eq!(pairs["rate"], "30000");
        assert_eq!(pairs["cl_lw"], "1");
        assert_eq!(pairs["model"], "gordon");
        assert!(!pairs.contains_key("cl_cmdrate"));
    }

    #[test]
    fn stufftext_filter_follows_cvar_should_set_cvar() {
        let mut c = ClientCvars::new();
        let may = |c: &ClientCvars, n: &str| c.stufftext_may_set(c.find(n).unwrap());
        // `cl_` prefix, filterable flag, privileged flag.
        assert!(!may(&c, "cl_lw"));
        assert!(!may(&c, "name"));
        assert!(!may(&c, "cl_filterstuffcmd"));
        c.register("sv_thing", "0", 0);
        assert!(may(&c, "sv_thing"));
        // Filter off: everything but privileged ones.
        c.set("cl_filterstuffcmd", "0");
        assert!(may(&c, "name"));
        assert!(may(&c, "cl_lw"));
        assert!(!may(&c, "cl_filterstuffcmd"));
    }

    #[test]
    fn float_parsing_is_lenient() {
        assert_eq!(parse_float("0.1"), 0.1);
        assert_eq!(parse_float("  25 "), 25.0);
        assert_eq!(parse_float("-3.5x"), -3.5);
        assert_eq!(parse_float("abc"), 0.0);
        assert_eq!(parse_float(""), 0.0);
    }
}
