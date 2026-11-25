//! "Black-Box" Time-Travel Plugin Crash Recorder ([`ModFaultDump`], [`FaultDumpStore`]) and
//! Deterministic Offline Replay Engine ([`replay_fault_dump`], [`ReplayReport`]).
//!
//! Whenever any sandboxed plugin (`.wasm`, `.py`, or `.amxx`) traps, panics, exceeds fuel, or
//! triggers a runtime error, [`ServerModHost`](crate::ServerModHost) isolates the fault so the
//! server keeps running at 100 Hz and records a bounded JSON black-box dump
//! (`<= 256 KiB`, ring-buffered to max `32` dumps in `logs/mod_faults/<timestamp>_<mod_id>_<hook>.json`
//! and/or [`ConfigStorage`]).
//!
//! Any captured dump can be replayed offline in milliseconds without launching the game or bots
//! against either the buggy binary (to reproduce the exact fault) or a fixed binary (to verify
//! `PASS` and inspect every emitted [`EngineOp`] and storage mutation step-by-step).
//!
//! Zero `unsafe` (`#![forbid(unsafe_code)]`) and fully compatible with `wasm32-unknown-unknown`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::{self, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use cs_amxx::amxxfile::ParsedAmx;
use cs_amxx::vm::{AmxError, ExecEntry, NativeDispatch, PawnVm};
use cs_mod_api::services::SharedPlatformServices;
use cs_mod_api::{AdminFlags, HookAction, HookId, HookPhase, Permissions, sha256_hex};
use cs_platform::ConfigStorage;

use crate::compile_wat_to_wasm;
use crate::natives::{ActiveEvent, ClientSnap, EngineOp, EngineSnapshot, EntitySnap};
#[cfg(not(target_arch = "wasm32"))]
use crate::package::{load_package_csmod, load_package_dir};
use crate::runtime::{
    ModCallOutcome, ModExecContext, ModInstance, ModRuntime, RuntimeError, WasmiRuntime,
};

/// Current JSON schema version for [`ModFaultDump`].
pub const FAULT_DUMP_SCHEMA_VERSION: u32 = 1;
/// Hard serialized byte cap per black-box crash dump (`256 KiB`).
pub const MAX_FAULT_DUMP_BYTES: usize = 256 * 1024;
/// Maximum crash dumps retained in the [`FaultDumpStore`] ring buffer (`32`).
pub const MAX_STORED_FAULTS: usize = 32;
/// Default deterministic WASI PRNG seed matching [`WasmiRuntime`].
pub const DEFAULT_RNG_SEED: u64 = 0x2545_F491_4F6C_DD1D;
/// [`ConfigStorage`] key storing the newline-delimited index of retained dump IDs.
pub const FAULT_STORAGE_INDEX_KEY: &str = "mod_faults__index";
/// [`ConfigStorage`] key prefix for individual serialized dump payloads.
pub const FAULT_STORAGE_KEY_PREFIX: &str = "mod_faults__";

/// Maximum accepted JSON payload size when parsing external dump files (`1 MiB`).
const MAX_PARSE_JSON_BYTES: usize = MAX_FAULT_DUMP_BYTES * 4;
/// Maximum JSON recursion depth accepted by the embedded RFC 8259 parser.
const MAX_JSON_DEPTH: usize = 32;

/// Returns the conventional disk directory for black-box crash dumps (`<base>/logs/mod_faults`).
pub fn default_fault_dump_dir(base_dir: &Path) -> PathBuf {
    base_dir.join("logs").join("mod_faults")
}

/// Plugin runtime / authoring language recorded in a [`ModFaultDump`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginLanguage {
    Wasm,
    Python,
    Amxx,
    Unknown,
}

impl PluginLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wasm => "wasm",
            Self::Python => "python",
            Self::Amxx => "amxx",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "wasm" | "rust" | "cpp" | "c++" | "c" | "go" | "wat" => Self::Wasm,
            "python" | "py" => Self::Python,
            "amxx" | "amx" | "pawn" => Self::Amxx,
            _ => Self::Unknown,
        }
    }

    /// Infers plugin language from a manifest entrypoint path and presence of WASM bytes.
    pub fn infer_from_entry(entry: Option<&str>, has_wasm: bool) -> Self {
        if let Some(rel) = entry {
            let lower = rel.to_ascii_lowercase();
            if lower.ends_with(".py") {
                return Self::Python;
            }
            if lower.ends_with(".amxx") || lower.ends_with(".amx") {
                return Self::Amxx;
            }
            if lower.ends_with(".wasm") || lower.ends_with(".wat") {
                return Self::Wasm;
            }
        }
        if has_wasm {
            Self::Wasm
        } else {
            Self::Unknown
        }
    }
}

impl fmt::Display for PluginLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Exact plugin entrypoint trigger that faulted and will be re-executed during replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvocationTrigger {
    Init,
    Hook {
        hook: HookId,
        phase: HookPhase,
    },
    Timer {
        cb_id: i32,
        arg: i32,
    },
    ChatCmd {
        slot: u8,
        team_only: bool,
        message: String,
        phase: HookPhase,
    },
    ConsoleCmd {
        slot: Option<u8>,
        command: String,
        args: Vec<String>,
        phase: HookPhase,
    },
    MenuSelect {
        cb_id: i32,
        slot: u8,
        menu_id: u32,
        item_index: i32,
        info: String,
    },
    Rpc {
        cb_id: i32,
        fn_name: String,
    },
    RpcEvent {
        cb_id: i32,
        event_name: String,
    },
    Webhook {
        cb_id: i32,
        status_code: i32,
    },
    AmxxPublic {
        function: String,
        args: Vec<i32>,
    },
}

impl InvocationTrigger {
    /// Filesystem-safe short slug used in `logs/mod_faults/<timestamp>_<mod_id>_<slug>.json`.
    pub fn slug(&self) -> String {
        match self {
            Self::Init => "init".to_string(),
            Self::Hook { hook, phase } => {
                format!("{}_{}", hook.as_name(), phase_str(*phase))
            }
            Self::Timer { cb_id, .. } => format!("timer_{cb_id}"),
            Self::ChatCmd { .. } => "chat_cmd".to_string(),
            Self::ConsoleCmd { command, .. } => {
                format!("console_cmd_{}", sanitize_id_segment(command, 24))
            }
            Self::MenuSelect { menu_id, .. } => format!("menu_select_{menu_id}"),
            Self::Rpc { fn_name, .. } => format!("rpc_{}", sanitize_id_segment(fn_name, 24)),
            Self::RpcEvent { event_name, .. } => {
                format!("event_{}", sanitize_id_segment(event_name, 24))
            }
            Self::Webhook { cb_id, .. } => format!("webhook_{cb_id}"),
            Self::AmxxPublic { function, .. } => {
                format!("amxx_{}", sanitize_id_segment(function, 28))
            }
        }
    }

    /// Compact human-readable label for summaries and `/api/mods/faults`.
    pub fn label(&self) -> String {
        match self {
            Self::Init => "init".to_string(),
            Self::Hook { hook, phase } => format!("hook:{}:{}", hook.as_name(), phase_str(*phase)),
            Self::Timer { cb_id, arg } => format!("timer(cb={cb_id}, arg={arg})"),
            Self::ChatCmd { slot, team_only, .. } => {
                format!("chat_cmd(slot={slot}, team_only={team_only})")
            }
            Self::ConsoleCmd { slot, command, .. } => match slot {
                Some(s) => format!("console_cmd(slot={s}, cmd={command})"),
                None => format!("console_cmd(server, cmd={command})"),
            },
            Self::MenuSelect {
                cb_id,
                slot,
                menu_id,
                item_index,
                ..
            } => {
                format!(
                    "menu_select(cb={cb_id}, slot={slot}, menu={menu_id}, item={item_index})"
                )
            }
            Self::Rpc { cb_id, fn_name } => format!("rpc(cb={cb_id}, fn={fn_name})"),
            Self::RpcEvent { cb_id, event_name } => {
                format!("rpc_event(cb={cb_id}, event={event_name})")
            }
            Self::Webhook { cb_id, status_code } => {
                format!("webhook(cb={cb_id}, status={status_code})")
            }
            Self::AmxxPublic { function, .. } => format!("amxx_public({function})"),
        }
    }
}

impl fmt::Display for InvocationTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// Complete deterministic black-box snapshot captured when a plugin call faults.
#[derive(Debug, Clone)]
pub struct ModFaultDump {
    pub schema_version: u32,
    pub dump_id: String,
    pub timestamp_unix_ms: u64,
    pub mod_id: String,
    pub version: String,
    pub language: PluginLanguage,
    pub source_path: Option<String>,
    pub binary_sha256: String,
    pub permissions_mask: u32,
    pub http_hosts: Vec<String>,
    pub fuel_budget: u64,
    pub max_memory_bytes: usize,
    pub trigger: InvocationTrigger,
    pub event: ActiveEvent,
    pub snapshot: EngineSnapshot,
    pub storage_cache: BTreeMap<String, String>,
    pub rng_seed: u64,
    pub fuel_consumed_before_trap: u64,
    pub error: String,
    pub stack_trace: Vec<String>,
    pub truncated: bool,
}

impl ModFaultDump {
    /// Constructs a canonical filesystem-safe dump ID (`<timestamp>_<mod_id>_<trigger>`).
    pub fn make_dump_id(
        timestamp_unix_ms: u64,
        mod_id: &str,
        trigger: &InvocationTrigger,
    ) -> String {
        let safe_mod = sanitize_id_segment(mod_id, 40);
        let safe_trig = sanitize_id_segment(&trigger.slug(), 48);
        format!("{timestamp_unix_ms}_{safe_mod}_{safe_trig}")
    }

    /// Reconstructs the declared [`Permissions`] bitmask for replay instantiation.
    pub fn permissions(&self) -> Permissions {
        Permissions::from_bits_truncate(self.permissions_mask)
    }

    /// Safe filename `<dump_id>.json`.
    pub fn file_name(&self) -> String {
        format!("{}.json", self.dump_id)
    }

    /// Produces a [`FaultSummary`] row for listings and `/api/mods/faults`.
    pub fn summary(&self, byte_len: usize) -> FaultSummary {
        FaultSummary {
            id: self.dump_id.clone(),
            file_name: self.file_name(),
            timestamp_unix_ms: self.timestamp_unix_ms,
            mod_id: self.mod_id.clone(),
            version: self.version.clone(),
            language: self.language,
            trigger: self.trigger.label(),
            error: self.error.clone(),
            fuel_consumed_before_trap: self.fuel_consumed_before_trap,
            byte_len,
            truncated: self.truncated,
        }
    }

    /// Serializes this dump as formatted (2-space indented) RFC 8259 JSON.
    pub fn to_json(&self) -> String {
        self.to_json_value().to_string_pretty()
    }

    /// Serializes this dump as compact single-line RFC 8259 JSON.
    pub fn to_json_compact(&self) -> String {
        self.to_json_value().to_string_compact()
    }

    /// Serializes this dump into a valid UTF-8 JSON document strictly `<= MAX_FAULT_DUMP_BYTES`
    /// (`256 KiB`), progressively trimming low-priority bulk fields (excess entities, large binary
    /// payloads, huge storage entries) if required while marking `truncated = true`.
    pub fn to_bounded_json(&self) -> (Self, String) {
        let pretty = self.to_json();
        if pretty.len() <= MAX_FAULT_DUMP_BYTES {
            return (self.clone(), pretty);
        }
        let compact = self.to_json_compact();
        if compact.len() <= MAX_FAULT_DUMP_BYTES {
            return (self.clone(), compact);
        }

        let mut trimmed = self.clone();
        trimmed.truncated = true;

        // Stage 1: cap binary payload and oversized event string args.
        if trimmed.event.payload.len() > 8192 {
            trimmed.event.payload.truncate(8192);
        }
        for s in &mut trimmed.event.str_args {
            truncate_utf8(s, 2048);
        }
        let json = trimmed.to_json_compact();
        if json.len() <= MAX_FAULT_DUMP_BYTES {
            return (trimmed, json);
        }

        // Stage 2: prioritize relevant entities (worldspawn 0, player slots 1..=max_clients,
        // and any entity indices referenced in `event.i32_args`).
        let mut keep_ids = BTreeSet::new();
        keep_ids.insert(0u16);
        for slot in 0..trimmed.snapshot.max_clients {
            keep_ids.insert(u16::from(slot) + 1);
        }
        for &arg in &trimmed.event.i32_args {
            if let Ok(idx) = u16::try_from(arg) {
                keep_ids.insert(idx);
            }
        }
        for cap in [64usize, 16, 0] {
            if trimmed.snapshot.entities.len() > cap {
                let mut next_ents = BTreeMap::new();
                for idx in &keep_ids {
                    if next_ents.len() >= cap {
                        break;
                    }
                    if let Some(ent) = trimmed.snapshot.entities.get(idx) {
                        next_ents.insert(*idx, ent.clone());
                    }
                }
                for (idx, ent) in &trimmed.snapshot.entities {
                    if next_ents.len() >= cap {
                        break;
                    }
                    next_ents.entry(*idx).or_insert_with(|| ent.clone());
                }
                trimmed.snapshot.entities = next_ents;
                let json = trimmed.to_json_compact();
                if json.len() <= MAX_FAULT_DUMP_BYTES {
                    return (trimmed, json);
                }
            }
        }

        // Stage 3: cap oversized storage values, then progressively shrink entry count.
        for val in trimmed.storage_cache.values_mut() {
            truncate_utf8(val, 512);
        }
        let json = trimmed.to_json_compact();
        if json.len() <= MAX_FAULT_DUMP_BYTES {
            return (trimmed, json);
        }

        while !trimmed.storage_cache.is_empty() {
            let target = trimmed.storage_cache.len() / 2;
            let keys: Vec<String> = trimmed
                .storage_cache
                .keys()
                .skip(target)
                .cloned()
                .collect();
            for k in keys {
                trimmed.storage_cache.remove(&k);
            }
            let json = trimmed.to_json_compact();
            if json.len() <= MAX_FAULT_DUMP_BYTES {
                return (trimmed, json);
            }
        }

        // Stage 4: cap remaining bulk collections for extreme adversarial inputs.
        trimmed.event.payload.truncate(512);
        for s in &mut trimmed.event.str_args {
            truncate_utf8(s, 512);
        }
        for c in &mut trimmed.snapshot.clients {
            c.weapon_clips.clear();
            c.weapon_bpammo.clear();
        }
        truncate_utf8(&mut trimmed.error, 4096);
        trimmed.stack_trace.truncate(32);
        for frame in &mut trimmed.stack_trace {
            truncate_utf8(frame, 512);
        }
        let json = trimmed.to_json_compact();
        if json.len() <= MAX_FAULT_DUMP_BYTES {
            return (trimmed, json);
        }

        trimmed.snapshot.cvars.clear();
        trimmed.snapshot.clients.truncate(32);
        trimmed.http_hosts.truncate(16);
        let json = trimmed.to_json_compact();
        (trimmed, json)
    }

    /// Parses a [`ModFaultDump`] from an RFC 8259 JSON string.
    pub fn from_json(raw: &str) -> Result<Self, String> {
        if raw.len() > MAX_PARSE_JSON_BYTES {
            return Err(format!(
                "fault dump JSON exceeds maximum parse limit ({MAX_PARSE_JSON_BYTES} bytes)"
            ));
        }
        let val = JsonParser::parse(raw)?;
        Self::from_json_value(&val)
    }

    fn to_json_value(&self) -> JsonValue {
        let mut root = BTreeMap::new();
        root.insert("schema_version".into(), JsonValue::uint(u64::from(self.schema_version)));
        root.insert("dump_id".into(), JsonValue::Str(self.dump_id.clone()));
        root.insert("timestamp_unix_ms".into(), JsonValue::uint(self.timestamp_unix_ms));
        root.insert("mod_id".into(), JsonValue::Str(self.mod_id.clone()));
        root.insert("version".into(), JsonValue::Str(self.version.clone()));
        root.insert("language".into(), JsonValue::Str(self.language.as_str().into()));
        root.insert(
            "source_path".into(),
            match &self.source_path {
                Some(p) => JsonValue::Str(p.clone()),
                None => JsonValue::Null,
            },
        );
        root.insert("binary_sha256".into(), JsonValue::Str(self.binary_sha256.clone()));
        root.insert("permissions_mask".into(), JsonValue::uint(u64::from(self.permissions_mask)));
        root.insert(
            "http_hosts".into(),
            JsonValue::Array(
                self.http_hosts
                    .iter()
                    .map(|h| JsonValue::Str(h.clone()))
                    .collect(),
            ),
        );
        root.insert("fuel_budget".into(), JsonValue::uint(self.fuel_budget));
        root.insert("max_memory_bytes".into(), JsonValue::uint(self.max_memory_bytes as u64));
        root.insert("trigger".into(), trigger_to_json(&self.trigger));
        root.insert("event".into(), event_to_json(&self.event));
        root.insert("snapshot".into(), snapshot_to_json(&self.snapshot));
        let mut storage_obj = BTreeMap::new();
        for (k, v) in &self.storage_cache {
            storage_obj.insert(k.clone(), JsonValue::Str(v.clone()));
        }
        root.insert("storage_cache".into(), JsonValue::Object(storage_obj));
        root.insert("rng_seed".into(), JsonValue::Str(format!("0x{:016x}", self.rng_seed)));
        root.insert(
            "fuel_consumed_before_trap".into(),
            JsonValue::uint(self.fuel_consumed_before_trap),
        );
        root.insert("error".into(), JsonValue::Str(self.error.clone()));
        root.insert(
            "stack_trace".into(),
            JsonValue::Array(
                self.stack_trace
                    .iter()
                    .map(|s| JsonValue::Str(s.clone()))
                    .collect(),
            ),
        );
        root.insert("truncated".into(), JsonValue::Bool(self.truncated));
        JsonValue::Object(root)
    }

    fn from_json_value(val: &JsonValue) -> Result<Self, String> {
        let obj = val.as_object().ok_or("fault dump root must be a JSON object")?;
        let schema_version = get_u32(obj, "schema_version").unwrap_or(FAULT_DUMP_SCHEMA_VERSION);
        let mod_id = get_req_str(obj, "mod_id")?;
        let version = get_str(obj, "version").unwrap_or_default();
        let language = get_str(obj, "language")
            .map(|s| PluginLanguage::parse(&s))
            .unwrap_or(PluginLanguage::Unknown);
        let source_path = get_opt_str(obj, "source_path");
        let binary_sha256 = get_str(obj, "binary_sha256").unwrap_or_default();
        let permissions_mask = get_u32(obj, "permissions_mask").unwrap_or(0);
        let http_hosts = get_str_vec(obj, "http_hosts");
        let fuel_budget = get_u64(obj, "fuel_budget").unwrap_or(cs_mod_api::DEFAULT_FUEL_PER_FRAME);
        let max_memory_bytes = get_u64(obj, "max_memory_bytes")
            .map(|n| n as usize)
            .unwrap_or(cs_mod_api::DEFAULT_MEMORY_BYTES);
        let trigger = trigger_from_json(
            obj.get("trigger")
                .ok_or("missing `trigger` in fault dump")?,
        )?;
        let timestamp_unix_ms = get_u64(obj, "timestamp_unix_ms").unwrap_or(0);
        let dump_id = get_str(obj, "dump_id")
            .filter(|id| is_safe_dump_id(id))
            .unwrap_or_else(|| Self::make_dump_id(timestamp_unix_ms, &mod_id, &trigger));
        let event = obj
            .get("event")
            .map(event_from_json)
            .transpose()?
            .unwrap_or_default();
        let snapshot = obj
            .get("snapshot")
            .map(snapshot_from_json)
            .transpose()?
            .unwrap_or_default();
        let storage_cache = get_str_map(obj, "storage_cache");
        let rng_seed = get_u64_or_hex(obj, "rng_seed").unwrap_or(DEFAULT_RNG_SEED);
        let fuel_consumed_before_trap = get_u64(obj, "fuel_consumed_before_trap").unwrap_or(0);
        let error = get_str(obj, "error").unwrap_or_default();
        let stack_trace = get_str_vec(obj, "stack_trace");
        let truncated = get_bool(obj, "truncated").unwrap_or(false);

        Ok(Self {
            schema_version,
            dump_id,
            timestamp_unix_ms,
            mod_id,
            version,
            language,
            source_path,
            binary_sha256,
            permissions_mask,
            http_hosts,
            fuel_budget,
            max_memory_bytes,
            trigger,
            event,
            snapshot,
            storage_cache,
            rng_seed,
            fuel_consumed_before_trap,
            error,
            stack_trace,
            truncated,
        })
    }
}

/// Lightweight summary row returned by [`FaultDumpStore::list_faults`] and `GET /api/mods/faults`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultSummary {
    pub id: String,
    pub file_name: String,
    pub timestamp_unix_ms: u64,
    pub mod_id: String,
    pub version: String,
    pub language: PluginLanguage,
    pub trigger: String,
    pub error: String,
    pub fuel_consumed_before_trap: u64,
    pub byte_len: usize,
    pub truncated: bool,
}

impl FaultSummary {
    pub fn to_json(&self) -> String {
        self.to_json_value().to_string_compact()
    }

    fn to_json_value(&self) -> JsonValue {
        let mut obj = BTreeMap::new();
        obj.insert("id".into(), JsonValue::Str(self.id.clone()));
        obj.insert("file_name".into(), JsonValue::Str(self.file_name.clone()));
        obj.insert("timestamp_unix_ms".into(), JsonValue::uint(self.timestamp_unix_ms));
        obj.insert("mod_id".into(), JsonValue::Str(self.mod_id.clone()));
        obj.insert("version".into(), JsonValue::Str(self.version.clone()));
        obj.insert("language".into(), JsonValue::Str(self.language.as_str().into()));
        obj.insert("trigger".into(), JsonValue::Str(self.trigger.clone()));
        obj.insert("error".into(), JsonValue::Str(self.error.clone()));
        obj.insert(
            "fuel_consumed_before_trap".into(),
            JsonValue::uint(self.fuel_consumed_before_trap),
        );
        obj.insert("byte_len".into(), JsonValue::uint(self.byte_len as u64));
        obj.insert("truncated".into(), JsonValue::Bool(self.truncated));
        JsonValue::Object(obj)
    }
}

struct StoredFault {
    summary: FaultSummary,
    dump: ModFaultDump,
    json: String,
}

/// Bounded in-memory + [`ConfigStorage`] + optional disk-directory ring buffer (default max `32`
/// dumps) backing `logs/mod_faults/<timestamp>_<mod_id>_<hook>.json` and `/api/mods/faults`.
pub struct FaultDumpStore {
    max_dumps: usize,
    entries: VecDeque<StoredFault>,
    disk_dir: Option<PathBuf>,
}

impl Default for FaultDumpStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FaultDumpStore {
    pub fn new() -> Self {
        Self::with_capacity(MAX_STORED_FAULTS)
    }

    pub fn with_capacity(max_dumps: usize) -> Self {
        Self {
            max_dumps: max_dumps.max(1),
            entries: VecDeque::new(),
            disk_dir: None,
        }
    }

    pub fn capacity(&self) -> usize {
        self.max_dumps
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Configures an optional native filesystem directory (e.g. `logs/mod_faults`) where dumps
    /// are mirrored and ring-evicted alongside [`ConfigStorage`].
    pub fn set_disk_dir(&mut self, dir: Option<PathBuf>) {
        self.disk_dir = dir;
    }

    pub fn disk_dir(&self) -> Option<&Path> {
        self.disk_dir.as_deref()
    }

    /// Records a [`ModFaultDump`], bounding it to `<= MAX_FAULT_DUMP_BYTES`, enforcing the ring
    /// capacity (`<= max_dumps`), and persisting to `storage` and/or `disk_dir`.
    pub fn record(
        &mut self,
        mut dump: ModFaultDump,
        mut storage: Option<&mut dyn ConfigStorage>,
    ) -> FaultSummary {
        let base_id = if is_safe_dump_id(&dump.dump_id) {
            dump.dump_id.clone()
        } else {
            ModFaultDump::make_dump_id(dump.timestamp_unix_ms, &dump.mod_id, &dump.trigger)
        };
        dump.dump_id = self.allocate_unique_id(&base_id);

        let (bounded_dump, json) = dump.to_bounded_json();
        let summary = bounded_dump.summary(json.len());
        let id = summary.id.clone();
        let file_name = summary.file_name.clone();

        while self.entries.len() >= self.max_dumps {
            if let Some(evicted) = self.entries.pop_front() {
                if let Some(ref mut st) = storage {
                    let _ = st.remove(&storage_key_for_id(&evicted.summary.id));
                }
                self.remove_disk_file(&evicted.summary.file_name);
            }
        }

        if let Some(ref mut st) = storage {
            let _ = st.save(&storage_key_for_id(&id), &json);
        }
        self.write_disk_file(&file_name, &json);

        self.entries.push_back(StoredFault {
            summary: summary.clone(),
            dump: bounded_dump,
            json,
        });

        if let Some(st) = storage {
            self.persist_storage_index(st);
        }

        summary
    }

    /// Lists retained fault summaries ordered newest-first.
    pub fn list_faults(&self) -> Vec<FaultSummary> {
        self.entries
            .iter()
            .rev()
            .map(|e| e.summary.clone())
            .collect()
    }

    /// Serializes [`list_faults`](Self::list_faults) as a JSON array for `GET /api/mods/faults`.
    pub fn list_faults_json(&self) -> String {
        let arr = self
            .entries
            .iter()
            .rev()
            .map(|e| e.summary.to_json_value())
            .collect();
        JsonValue::Array(arr).to_string_compact()
    }

    /// Looks up the exact bounded JSON string for `id` (accepting either `<id>` or `<id>.json`).
    pub fn get_fault_json(&self, id: &str) -> Option<&str> {
        let normalized = normalize_lookup_id(id)?;
        self.entries
            .iter()
            .rev()
            .find(|e| e.summary.id == normalized)
            .map(|e| e.json.as_str())
    }

    /// Looks up the structured [`ModFaultDump`] for `id` (accepting either `<id>` or `<id>.json`).
    pub fn get_fault(&self, id: &str) -> Option<&ModFaultDump> {
        let normalized = normalize_lookup_id(id)?;
        self.entries
            .iter()
            .rev()
            .find(|e| e.summary.id == normalized)
            .map(|e| &e.dump)
    }

    /// Returns the most recently recorded [`ModFaultDump`], if any.
    pub fn latest(&self) -> Option<&ModFaultDump> {
        self.entries.back().map(|e| &e.dump)
    }

    /// Clears all retained dumps from memory, optional [`ConfigStorage`], and optional `disk_dir`.
    pub fn clear(&mut self, mut storage: Option<&mut dyn ConfigStorage>) {
        let old: Vec<StoredFault> = self.entries.drain(..).collect();
        for entry in old {
            if let Some(ref mut st) = storage {
                let _ = st.remove(&storage_key_for_id(&entry.summary.id));
            }
            self.remove_disk_file(&entry.summary.file_name);
        }
        if let Some(st) = storage {
            let _ = st.remove(FAULT_STORAGE_INDEX_KEY);
        }
    }

    /// Hydrates retained crash dumps from [`ConfigStorage`] (ignoring corrupted entries).
    pub fn load_from_storage(&mut self, storage: &dyn ConfigStorage) -> usize {
        let Some(index) = storage.load(FAULT_STORAGE_INDEX_KEY) else {
            return 0;
        };
        let mut loaded = 0usize;
        for raw_id in index.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !is_safe_dump_id(raw_id) {
                continue;
            }
            if self.entries.iter().any(|e| e.summary.id == raw_id) {
                continue;
            }
            let Some(json) = storage.load(&storage_key_for_id(raw_id)) else {
                continue;
            };
            if json.len() > MAX_FAULT_DUMP_BYTES {
                continue;
            }
            let Ok(dump) = ModFaultDump::from_json(&json) else {
                continue;
            };
            while self.entries.len() >= self.max_dumps {
                self.entries.pop_front();
            }
            let summary = dump.summary(json.len());
            self.entries.push_back(StoredFault {
                summary,
                dump,
                json,
            });
            loaded += 1;
        }
        loaded
    }

    /// Hydrates retained crash dumps from a disk directory (e.g. `logs/mod_faults`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_from_dir(&mut self, dir: &Path) -> Result<usize, String> {
        if !dir.is_dir() {
            return Ok(0);
        }
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("failed to read fault dump dir `{}`: {e}", dir.display()))?;
        let mut paths = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                paths.push(path);
            }
        }
        paths.sort();

        let mut loaded = 0usize;
        for path in paths {
            let Ok(json) = std::fs::read_to_string(&path) else {
                continue;
            };
            if json.len() > MAX_FAULT_DUMP_BYTES {
                continue;
            }
            let Ok(dump) = ModFaultDump::from_json(&json) else {
                continue;
            };
            if self.entries.iter().any(|e| e.summary.id == dump.dump_id) {
                continue;
            }
            while self.entries.len() >= self.max_dumps {
                self.entries.pop_front();
            }
            let summary = dump.summary(json.len());
            self.entries.push_back(StoredFault {
                summary,
                dump,
                json,
            });
            loaded += 1;
        }
        Ok(loaded)
    }

    fn allocate_unique_id(&self, base: &str) -> String {
        if !self.entries.iter().any(|e| e.summary.id == base) {
            return base.to_string();
        }
        for n in 2u32..=10_000 {
            let candidate = format!("{base}_{n}");
            if !self.entries.iter().any(|e| e.summary.id == candidate) {
                return candidate;
            }
        }
        format!("{base}_overflow")
    }

    fn persist_storage_index(&self, storage: &mut dyn ConfigStorage) {
        let index = self
            .entries
            .iter()
            .map(|e| e.summary.id.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = storage.save(FAULT_STORAGE_INDEX_KEY, &index);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_disk_file(&self, file_name: &str, json: &str) {
        let Some(ref dir) = self.disk_dir else {
            return;
        };
        if !is_safe_file_name(file_name) {
            return;
        }
        if std::fs::create_dir_all(dir).is_ok() {
            let _ = std::fs::write(dir.join(file_name), json);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn write_disk_file(&self, _file_name: &str, _json: &str) {}

    #[cfg(not(target_arch = "wasm32"))]
    fn remove_disk_file(&self, file_name: &str) {
        let Some(ref dir) = self.disk_dir else {
            return;
        };
        if !is_safe_file_name(file_name) {
            return;
        }
        let _ = std::fs::remove_file(dir.join(file_name));
    }

    #[cfg(target_arch = "wasm32")]
    fn remove_disk_file(&self, _file_name: &str) {}
}

// ===========================================================================
// Deterministic Offline Replay Engine
// ===========================================================================

/// Execution outcome of replaying a single captured [`ModFaultDump`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayOutcome {
    Pass(HookAction),
    Fault(String),
}

impl ReplayOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass(_))
    }

    pub fn is_fault(&self) -> bool {
        matches!(self, Self::Fault(_))
    }

    pub fn fault_reason(&self) -> Option<&str> {
        match self {
            Self::Fault(msg) => Some(msg.as_str()),
            Self::Pass(_) => None,
        }
    }
}

/// One key-value storage mutation emitted during a replayed plugin invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageMutation {
    Set { key: String, value: String },
    Remove { key: String },
}

impl fmt::Display for StorageMutation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set { key, value } => write!(f, "SET {key} = {value:?}"),
            Self::Remove { key } => write!(f, "REMOVE {key}"),
        }
    }
}

/// Detailed diagnostic output returned by [`replay_fault_dump`].
#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub dump_id: String,
    pub mod_id: String,
    pub language: PluginLanguage,
    pub trigger: InvocationTrigger,
    pub outcome: ReplayOutcome,
    pub fuel_budget: u64,
    pub fuel_used: u64,
    pub elapsed_us: u64,
    pub emitted_ops: Vec<EngineOp>,
    pub storage_mutations: Vec<StorageMutation>,
    pub override_i32: Option<i32>,
    pub override_i32_2: Option<i32>,
    pub override_f32: Option<f32>,
    pub override_str: Option<String>,
    pub rpc_return_str: Option<String>,
    pub wasi_logs: Vec<String>,
    pub stack_trace: Vec<String>,
}

impl ReplayReport {
    /// Serializes this replay report into compact JSON (convenient for CLI `--json` or `/admin`).
    pub fn to_json(&self) -> String {
        let mut obj = BTreeMap::new();
        obj.insert("dump_id".into(), JsonValue::Str(self.dump_id.clone()));
        obj.insert("mod_id".into(), JsonValue::Str(self.mod_id.clone()));
        obj.insert("language".into(), JsonValue::Str(self.language.as_str().into()));
        obj.insert("trigger".into(), trigger_to_json(&self.trigger));
        match &self.outcome {
            ReplayOutcome::Pass(action) => {
                obj.insert("status".into(), JsonValue::Str("pass".into()));
                obj.insert("hook_action".into(), JsonValue::Str(format!("{action:?}")));
            }
            ReplayOutcome::Fault(err) => {
                obj.insert("status".into(), JsonValue::Str("fault".into()));
                obj.insert("error".into(), JsonValue::Str(err.clone()));
            }
        }
        obj.insert("fuel_budget".into(), JsonValue::uint(self.fuel_budget));
        obj.insert("fuel_used".into(), JsonValue::uint(self.fuel_used));
        obj.insert("elapsed_us".into(), JsonValue::uint(self.elapsed_us));
        obj.insert(
            "emitted_ops".into(),
            JsonValue::Array(
                self.emitted_ops
                    .iter()
                    .map(|op| JsonValue::Str(format!("{op:?}")))
                    .collect(),
            ),
        );
        obj.insert(
            "storage_mutations".into(),
            JsonValue::Array(
                self.storage_mutations
                    .iter()
                    .map(|m| JsonValue::Str(m.to_string()))
                    .collect(),
            ),
        );
        if let Some(v) = self.override_i32 {
            obj.insert("override_i32".into(), JsonValue::int(i64::from(v)));
        }
        if let Some(v) = self.override_i32_2 {
            obj.insert("override_i32_2".into(), JsonValue::int(i64::from(v)));
        }
        if let Some(v) = self.override_f32 {
            obj.insert("override_f32".into(), JsonValue::f32_val(v));
        }
        if let Some(ref v) = self.override_str {
            obj.insert("override_str".into(), JsonValue::Str(v.clone()));
        }
        if let Some(ref v) = self.rpc_return_str {
            obj.insert("rpc_return_str".into(), JsonValue::Str(v.clone()));
        }
        obj.insert(
            "wasi_logs".into(),
            JsonValue::Array(
                self.wasi_logs
                    .iter()
                    .map(|l| JsonValue::Str(l.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "stack_trace".into(),
            JsonValue::Array(
                self.stack_trace
                    .iter()
                    .map(|l| JsonValue::Str(l.clone()))
                    .collect(),
            ),
        );
        JsonValue::Object(obj).to_string_pretty()
    }
}

impl fmt::Display for ReplayReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Mod Fault Replay Report ===")?;
        writeln!(f, "Dump ID   : {}", self.dump_id)?;
        writeln!(f, "Plugin    : {} ({})", self.mod_id, self.language)?;
        writeln!(f, "Trigger   : {}", self.trigger)?;
        match &self.outcome {
            ReplayOutcome::Pass(action) => {
                writeln!(f, "Outcome   : PASS: HookAction::{action:?}")?;
            }
            ReplayOutcome::Fault(err) => {
                writeln!(f, "Outcome   : FAULT: {err}")?;
            }
        }
        writeln!(
            f,
            "Metrics   : fuel={} / {} | elapsed={} us",
            self.fuel_used, self.fuel_budget, self.elapsed_us
        )?;

        if self.override_i32.is_some()
            || self.override_i32_2.is_some()
            || self.override_f32.is_some()
            || self.override_str.is_some()
        {
            write!(f, "Overrides :")?;
            if let Some(v) = self.override_i32 {
                write!(f, " i32={v}")?;
            }
            if let Some(v) = self.override_i32_2 {
                write!(f, " i32_2={v}")?;
            }
            if let Some(v) = self.override_f32 {
                write!(f, " f32={v}")?;
            }
            if let Some(ref s) = self.override_str {
                write!(f, " str={s:?}")?;
            }
            writeln!(f)?;
        }

        if !self.stack_trace.is_empty() {
            writeln!(f, "Stack Trace:")?;
            for frame in &self.stack_trace {
                writeln!(f, "  {frame}")?;
            }
        }

        writeln!(f, "Emitted EngineOps ({}):", self.emitted_ops.len())?;
        for (idx, op) in self.emitted_ops.iter().enumerate() {
            writeln!(f, "  [{}] {op:?}", idx + 1)?;
        }

        if !self.storage_mutations.is_empty() {
            writeln!(f, "Storage Mutations ({}):", self.storage_mutations.len())?;
            for (idx, m) in self.storage_mutations.iter().enumerate() {
                writeln!(f, "  [{}] {m}", idx + 1)?;
            }
        }

        if !self.wasi_logs.is_empty() {
            writeln!(f, "Logs ({}):", self.wasi_logs.len())?;
            for line in &self.wasi_logs {
                writeln!(f, "  {line}")?;
            }
        }
        Ok(())
    }
}

/// Target plugin binary, live [`ModInstance`], or filesystem path to replay a [`ModFaultDump`]
/// against.
pub enum ReplayTarget<'a> {
    Instance(&'a mut dyn ModInstance),
    WasmBytes(&'a [u8]),
    AmxxBytes(&'a [u8]),
    #[cfg(not(target_arch = "wasm32"))]
    Path(&'a Path),
}

/// Hydrates a fresh [`SharedPlatformServices`] instance with the exact player language and admin
/// flag state captured inside `snapshot`.
pub fn hydrate_services_from_snapshot(snapshot: &EngineSnapshot) -> SharedPlatformServices {
    let services = SharedPlatformServices::new();
    {
        let mut svc = services.lock();
        if let Some(server_lang) = snapshot.cvars.get("amx_language")
            && !server_lang.trim().is_empty()
        {
            svc.lang.set_server_lang(server_lang);
        }
        for client in &snapshot.clients {
            svc.admins
                .set_slot_flags(client.slot, AdminFlags::from_bits(client.admin_flags));
            if !client.lang.trim().is_empty() {
                let ident = (!client.steamid.is_empty()).then_some(client.steamid.as_str());
                let _ = svc.lang.set_player_lang(client.slot, ident, &client.lang);
            }
        }
    }
    services
}

/// Replays a captured [`ModFaultDump`] deterministically against `target`.
pub fn replay_fault_dump(dump: &ModFaultDump, target: ReplayTarget<'_>) -> ReplayReport {
    match target {
        ReplayTarget::Instance(inst) => replay_fault_dump_with_instance(dump, inst),
        ReplayTarget::WasmBytes(bytes) => replay_fault_dump_wasm(dump, bytes),
        ReplayTarget::AmxxBytes(bytes) => replay_fault_dump_amxx(dump, bytes),
        #[cfg(not(target_arch = "wasm32"))]
        ReplayTarget::Path(path) => replay_fault_dump_path(dump, path),
    }
}

/// Replays a captured [`ModFaultDump`] directly on an initialized [`ModInstance`] (works across
/// WASM `WasmiInstance`, Python `PyModInstance`, and custom test runtimes, on both native and
/// `wasm32-unknown-unknown`).
pub fn replay_fault_dump_with_instance(
    dump: &ModFaultDump,
    instance: &mut dyn ModInstance,
) -> ReplayReport {
    let mut event = dump.event.clone();
    enrich_event_from_trigger(&dump.trigger, &mut event);

    let ctx = ModExecContext {
        snapshot: &dump.snapshot,
        event: &event,
        storage: &dump.storage_cache,
        fuel_budget: dump.fuel_budget,
    };

    let timer = ReplayTimer::start();
    let res: Result<ModCallOutcome, RuntimeError> = catch_mod_call(|| match &dump.trigger {
        InvocationTrigger::Init => instance.call_init(&ctx),
        InvocationTrigger::Hook { hook, phase } => instance.call_hook(*hook, *phase, &ctx),
        InvocationTrigger::Timer { cb_id, arg } => instance.call_timer(*cb_id, *arg, &ctx),
        InvocationTrigger::ChatCmd { phase, .. } => {
            instance.call_hook(HookId::PlayerSay, *phase, &ctx)
        }
        InvocationTrigger::ConsoleCmd { slot, phase, .. } => {
            let hook = if slot.is_some() {
                HookId::ClientCommand
            } else {
                HookId::ServerCommand
            };
            instance.call_hook(hook, *phase, &ctx)
        }
        InvocationTrigger::MenuSelect {
            cb_id,
            slot,
            menu_id,
            item_index,
            ..
        } => instance.call_menu(*cb_id, *slot, *menu_id, *item_index, &ctx),
        InvocationTrigger::Rpc { cb_id, .. } => instance.call_rpc(*cb_id, &ctx).map(|(_, o)| o),
        InvocationTrigger::RpcEvent { cb_id, .. } => instance.call_event(*cb_id, &ctx),
        InvocationTrigger::Webhook { cb_id, status_code } => {
            instance.call_webhook(*cb_id, *status_code, &ctx)
        }
        InvocationTrigger::AmxxPublic { function, .. } => Err(RuntimeError::Fault(format!(
            "trigger amxx_public({function}) requires an AMXX replay target"
        ))),
    });
    let elapsed_us = timer.elapsed_us();

    build_report_from_result(dump, res, elapsed_us)
}

/// Compiles/instantiates raw `.wasm` (or `.wat` text) bytes on a fresh deterministic
/// [`WasmiRuntime`], hydrates platform services from `dump.snapshot`, runs `cs_mod_init` (unless
/// the fault trigger itself was `Init`), and replays the exact failing invocation.
pub fn replay_fault_dump_wasm(dump: &ModFaultDump, wasm_or_wat: &[u8]) -> ReplayReport {
    let timer = ReplayTimer::start();
    let compiled = match maybe_compile_wat_bytes(wasm_or_wat) {
        Ok(b) => b,
        Err(err) => return fault_report(dump, err, 0, timer.elapsed_us()),
    };

    let runtime = WasmiRuntime::new();
    let max_mem = dump.max_memory_bytes.max(65_536);
    let mut inst = match runtime.instantiate(&dump.mod_id, &compiled, dump.permissions(), max_mem) {
        Ok(i) => i,
        Err(err) => return fault_report(dump, err.to_string(), 0, timer.elapsed_us()),
    };

    let services = hydrate_services_from_snapshot(&dump.snapshot);
    inst.configure_v2(services, dump.http_hosts.clone(), None);

    if dump.trigger != InvocationTrigger::Init {
        let init_snap = EngineSnapshot::default();
        let init_ev = ActiveEvent::default();
        let init_ctx = ModExecContext {
            snapshot: &init_snap,
            event: &init_ev,
            storage: &dump.storage_cache,
            fuel_budget: dump.fuel_budget,
        };
        if let Err(err) = catch_mod_call(|| inst.call_init(&init_ctx)) {
            let err_str = format!("replay init failed before target trigger: {err}");
            let fuel = infer_fuel_consumed(&err_str, dump.fuel_budget);
            return fault_report(dump, err_str, fuel, timer.elapsed_us());
        }
    }

    replay_fault_dump_with_instance(dump, inst.as_mut())
}

/// Offline deterministic replayer for `.amxx` / `.amx` Pawn 3.2 plugin binaries against a
/// [`ModFaultDump`].
pub fn replay_fault_dump_amxx(dump: &ModFaultDump, amxx_bytes: &[u8]) -> ReplayReport {
    let timer = ReplayTimer::start();
    let parsed = match ParsedAmx::parse_amxx(amxx_bytes) {
        Ok(p) => p,
        Err(err) => return fault_report(dump, err.to_string(), 0, timer.elapsed_us()),
    };
    let mut vm = match PawnVm::from_parsed(&parsed) {
        Ok(v) => v,
        Err(err) => return fault_report(dump, err.to_string(), 0, timer.elapsed_us()),
    };
    if dump.fuel_budget > 0 {
        vm.set_max_steps(dump.fuel_budget);
    }

    let (func_name, args): (String, Vec<i32>) = match &dump.trigger {
        InvocationTrigger::AmxxPublic { function, args } => (function.clone(), args.clone()),
        InvocationTrigger::Init => ("plugin_init".to_string(), Vec::new()),
        InvocationTrigger::Hook { hook, .. } => {
            (hook.as_name().to_string(), dump.event.i32_args.to_vec())
        }
        InvocationTrigger::Timer { cb_id, arg } => (format!("task_{cb_id}"), vec![*arg]),
        InvocationTrigger::ChatCmd { slot, .. } => {
            ("client_command".to_string(), vec![i32::from(*slot) + 1])
        }
        InvocationTrigger::ConsoleCmd { slot, command, .. } => {
            let mut a = Vec::new();
            if let Some(s) = slot {
                a.push(i32::from(*s) + 1);
            }
            (command.clone(), a)
        }
        InvocationTrigger::MenuSelect {
            slot,
            menu_id,
            item_index,
            ..
        } => (
            "menu_handler".to_string(),
            vec![i32::from(*slot) + 1, *menu_id as i32, *item_index],
        ),
        InvocationTrigger::Rpc { fn_name, .. } => (fn_name.clone(), dump.event.i32_args.to_vec()),
        InvocationTrigger::RpcEvent { event_name, .. } => {
            (event_name.clone(), dump.event.i32_args.to_vec())
        }
        InvocationTrigger::Webhook { cb_id, status_code } => {
            (format!("webhook_{cb_id}"), vec![*status_code])
        }
    };

    let Some(pub_idx) = vm.find_public(&func_name) else {
        return fault_report(
            dump,
            format!("AMXX public function `{func_name}` not found"),
            0,
            timer.elapsed_us(),
        );
    };

    let mut dispatch = OfflineAmxxNativeDispatch {
        snapshot: &dump.snapshot,
        ops: Vec::new(),
    };

    let exec_res = catch_unwind(AssertUnwindSafe(|| {
        vm.exec(ExecEntry::Public(pub_idx), &args, &mut dispatch)
    }));
    let elapsed_us = timer.elapsed_us();
    let steps = vm.last_steps();

    match exec_res {
        Ok(Ok(rc)) => ReplayReport {
            dump_id: dump.dump_id.clone(),
            mod_id: dump.mod_id.clone(),
            language: PluginLanguage::Amxx,
            trigger: dump.trigger.clone(),
            outcome: ReplayOutcome::Pass(HookAction::from_i32(rc)),
            fuel_budget: dump.fuel_budget,
            fuel_used: steps,
            elapsed_us,
            emitted_ops: dispatch.ops,
            storage_mutations: Vec::new(),
            override_i32: None,
            override_i32_2: None,
            override_f32: None,
            override_str: None,
            rpc_return_str: None,
            wasi_logs: Vec::new(),
            stack_trace: Vec::new(),
        },
        Ok(Err(err)) => {
            let reason = err.to_string();
            let mut trace = vec![format!("amxx::{func_name}() -> {reason}")];
            trace.push(format!("steps_executed={steps} budget={}", dump.fuel_budget));
            ReplayReport {
                dump_id: dump.dump_id.clone(),
                mod_id: dump.mod_id.clone(),
                language: PluginLanguage::Amxx,
                trigger: dump.trigger.clone(),
                outcome: ReplayOutcome::Fault(reason),
                fuel_budget: dump.fuel_budget,
                fuel_used: steps,
                elapsed_us,
                emitted_ops: dispatch.ops,
                storage_mutations: Vec::new(),
                override_i32: None,
                override_i32_2: None,
                override_f32: None,
                override_str: None,
                rpc_return_str: None,
                wasi_logs: Vec::new(),
                stack_trace: trace,
            }
        }
        Err(payload) => fault_report(
            dump,
            format!("AMXX VM panicked: {}", panic_payload_to_string(&*payload)),
            steps,
            elapsed_us,
        ),
    }
}

/// Loads `<dump_path>` from disk and replays it against `mod_override` (or `dump.source_path`
/// when `mod_override` is `None`). Designed for `cargo xtask mod replay <dump.json> [--mod-path]`.
#[cfg(not(target_arch = "wasm32"))]
pub fn replay_fault_dump_from_path(
    dump_path: &Path,
    mod_override: Option<&Path>,
) -> Result<ReplayReport, String> {
    let raw = std::fs::read_to_string(dump_path)
        .map_err(|e| format!("failed to read fault dump `{}`: {e}", dump_path.display()))?;
    let dump = ModFaultDump::from_json(&raw)?;
    let target_path: PathBuf = match mod_override {
        Some(p) => p.to_path_buf(),
        None => dump
            .source_path
            .as_ref()
            .map(PathBuf::from)
            .ok_or_else(|| {
                format!(
                    "dump `{}` has no recorded source_path; pass --mod-path <path>",
                    dump.dump_id
                )
            })?,
    };
    Ok(replay_fault_dump_path(&dump, &target_path))
}

#[cfg(not(target_arch = "wasm32"))]
fn replay_fault_dump_path(dump: &ModFaultDump, path: &Path) -> ReplayReport {
    let timer = ReplayTimer::start();
    if path.is_dir() {
        let pkg = match load_package_dir(path) {
            Ok(p) => p,
            Err(err) => {
                return fault_report(
                    dump,
                    format!("failed to load mod dir `{}`: {err}", path.display()),
                    0,
                    timer.elapsed_us(),
                );
            }
        };
        let Some(wasm) = pkg.server_wasm else {
            return fault_report(
                dump,
                format!("mod dir `{}` has no server WASM entrypoint", path.display()),
                0,
                timer.elapsed_us(),
            );
        };
        return replay_fault_dump_wasm(dump, &wasm);
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => {
            return fault_report(
                dump,
                format!("failed to read plugin `{}`: {err}", path.display()),
                0,
                timer.elapsed_us(),
            );
        }
    };

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match ext.as_str() {
        "csmod" => {
            let pkg = match load_package_csmod(&bytes, Some(path.to_path_buf())) {
                Ok(p) => p,
                Err(err) => {
                    return fault_report(
                        dump,
                        format!("failed to unpack `{}`: {err}", path.display()),
                        0,
                        timer.elapsed_us(),
                    );
                }
            };
            let Some(wasm) = pkg.server_wasm else {
                return fault_report(
                    dump,
                    format!("archive `{}` has no server WASM entrypoint", path.display()),
                    0,
                    timer.elapsed_us(),
                );
            };
            replay_fault_dump_wasm(dump, &wasm)
        }
        "amxx" | "amx" => replay_fault_dump_amxx(dump, &bytes),
        "wasm" | "wat" => replay_fault_dump_wasm(dump, &bytes),
        _ => {
            if dump.language == PluginLanguage::Amxx {
                replay_fault_dump_amxx(dump, &bytes)
            } else {
                replay_fault_dump_wasm(dump, &bytes)
            }
        }
    }
}

/// Minimal offline [`NativeDispatch`] for `.amxx` black-box replay without booting `EngineApi`.
struct OfflineAmxxNativeDispatch<'a> {
    snapshot: &'a EngineSnapshot,
    ops: Vec<EngineOp>,
}

impl NativeDispatch for OfflineAmxxNativeDispatch<'_> {
    fn call_native(
        &mut self,
        vm: &mut PawnVm,
        _native_idx: usize,
        native_name: &str,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr).unwrap_or(0);
        match native_name {
            "server_print" => {
                if argc >= 1
                    && let Ok(text) = vm.param_string(params_addr, 1)
                {
                    self.ops.push(EngineOp::PrintAll { text });
                }
                Ok(0)
            }
            "client_print" => {
                if argc >= 3 {
                    let id = vm.param(params_addr, 1).unwrap_or(0);
                    let text = vm.param_string(params_addr, 3).unwrap_or_default();
                    if id <= 0 {
                        self.ops.push(EngineOp::PrintAll { text });
                    } else if let Ok(slot) = u8::try_from(id - 1) {
                        self.ops.push(EngineOp::PrintClient { slot, text });
                    }
                }
                Ok(0)
            }
            "get_user_health" => {
                let id = vm.param(params_addr, 1).unwrap_or(0);
                let hp = u8::try_from(id - 1)
                    .ok()
                    .and_then(|s| self.snapshot.client(s))
                    .map_or(0, |c| c.health.round() as i32);
                Ok(hp)
            }
            "set_user_health" => {
                if argc >= 2 {
                    let id = vm.param(params_addr, 1).unwrap_or(0);
                    let hp = vm.param(params_addr, 2).unwrap_or(0);
                    if let Ok(slot) = u8::try_from(id - 1) {
                        self.ops.push(EngineOp::SetPlayerHealth {
                            slot,
                            health: hp as f32,
                        });
                    }
                }
                Ok(1)
            }
            "cs_get_user_money" => {
                let id = vm.param(params_addr, 1).unwrap_or(0);
                let money = u8::try_from(id - 1)
                    .ok()
                    .and_then(|s| self.snapshot.client(s))
                    .map_or(0, |c| c.money);
                Ok(money)
            }
            "cs_set_user_money" => {
                if argc >= 2 {
                    let id = vm.param(params_addr, 1).unwrap_or(0);
                    let amount = vm.param(params_addr, 2).unwrap_or(0);
                    let flash = argc < 3 || vm.param(params_addr, 3).unwrap_or(1) != 0;
                    if let Ok(slot) = u8::try_from(id - 1) {
                        self.ops.push(EngineOp::SetPlayerMoney {
                            slot,
                            amount,
                            flash,
                        });
                    }
                }
                Ok(1)
            }
            "get_maxplayers" => Ok(i32::from(self.snapshot.max_clients)),
            "get_gametime" => Ok((self.snapshot.time_secs as f32).to_bits() as i32),
            _ => Ok(0),
        }
    }
}

/// Executes a plugin invocation inside [`catch_unwind`] so even a panicking custom/script runtime
/// is isolated as [`RuntimeError::Fault`] and never brings down the game loop.
pub fn catch_mod_call<T>(f: impl FnOnce() -> Result<T, RuntimeError>) -> Result<T, RuntimeError> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(res) => res,
        Err(payload) => Err(RuntimeError::Fault(format!(
            "plugin panicked: {}",
            panic_payload_to_string(&*payload)
        ))),
    }
}

/// Formats a structured stack trace for a faulted plugin invocation.
pub fn format_stack_trace(
    mod_id: &str,
    language: PluginLanguage,
    trigger: &InvocationTrigger,
    error: &str,
    fuel_consumed: u64,
    fuel_budget: u64,
) -> Vec<String> {
    let mut lines = Vec::new();
    let entry = match trigger {
        InvocationTrigger::Init => "cs_mod_init()".to_string(),
        InvocationTrigger::Hook { hook, phase } => {
            format!("cs_mod_hook(hook={}, phase={})", hook.as_name(), phase_str(*phase))
        }
        InvocationTrigger::Timer { cb_id, arg } => format!("cs_mod_timer(cb_id={cb_id}, arg={arg})"),
        InvocationTrigger::ChatCmd { slot, .. } => format!("chat_cmd(slot={slot})"),
        InvocationTrigger::ConsoleCmd { command, .. } => format!("console_cmd({command})"),
        InvocationTrigger::MenuSelect {
            cb_id,
            slot,
            menu_id,
            item_index,
            ..
        } => {
            format!("cs_mod_menu(cb={cb_id}, slot={slot}, menu={menu_id}, item={item_index})")
        }
        InvocationTrigger::Rpc { cb_id, fn_name } => {
            format!("cs_mod_rpc(cb={cb_id}, fn={fn_name})")
        }
        InvocationTrigger::RpcEvent { cb_id, event_name } => {
            format!("cs_mod_event(cb={cb_id}, event={event_name})")
        }
        InvocationTrigger::Webhook { cb_id, status_code } => {
            format!("cs_mod_webhook(cb={cb_id}, status={status_code})")
        }
        InvocationTrigger::AmxxPublic { function, .. } => format!("amxx::{function}()"),
    };
    lines.push(format!("#0 {language}::{mod_id}::{entry}"));
    for (idx, part) in error.lines().take(16).enumerate() {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            lines.push(format!("#{} {trimmed}", idx + 1));
        }
    }
    lines.push(format!(
        "fuel_consumed_before_trap={fuel_consumed} / budget={fuel_budget}"
    ));
    lines
}

/// Estimates `fuel_consumed_before_trap` when an engine trap string indicates fuel exhaustion.
pub fn infer_fuel_consumed(error: &str, fuel_budget: u64) -> u64 {
    let lower = error.to_ascii_lowercase();
    if lower.contains("out of fuel")
        || lower.contains("all fuel consumed")
        || lower.contains("outoffuel")
        || lower.contains("fuel exhausted")
    {
        fuel_budget
    } else {
        0
    }
}

/// Computes SHA-256 hex digest of a plugin's binary/script payload (or empty string if absent).
pub fn plugin_sha256_hex(bytes: Option<&[u8]>) -> String {
    bytes.map(sha256_hex).unwrap_or_default()
}

/// Returns current wall-clock UTC Unix milliseconds on native targets, or deterministic
/// engine-time milliseconds on `wasm32-unknown-unknown`.
#[cfg(not(target_arch = "wasm32"))]
pub fn current_timestamp_unix_ms(fallback_engine_time_secs: f64) -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_else(|_| (fallback_engine_time_secs.max(0.0) * 1000.0).round() as u64)
}

/// Returns deterministic engine-time milliseconds on `wasm32-unknown-unknown` (avoids unsupported
/// `SystemTime::now()` panics in browsers).
#[cfg(target_arch = "wasm32")]
pub fn current_timestamp_unix_ms(fallback_engine_time_secs: f64) -> u64 {
    (fallback_engine_time_secs.max(0.0) * 1000.0).round() as u64
}

/// Validates that `id` is a safe crash dump identifier (`[a-zA-Z0-9_-]`, `1..=160` bytes).
pub fn is_safe_dump_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 160
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// ===========================================================================
// Internal Helpers
// ===========================================================================

fn build_report_from_result(
    dump: &ModFaultDump,
    res: Result<ModCallOutcome, RuntimeError>,
    elapsed_us: u64,
) -> ReplayReport {
    match res {
        Ok(outcome) => {
            let storage_mutations = outcome
                .storage_writes
                .into_iter()
                .map(|(key, val)| match val {
                    Some(value) => StorageMutation::Set { key, value },
                    None => StorageMutation::Remove { key },
                })
                .collect();
            ReplayReport {
                dump_id: dump.dump_id.clone(),
                mod_id: dump.mod_id.clone(),
                language: dump.language,
                trigger: dump.trigger.clone(),
                outcome: ReplayOutcome::Pass(outcome.action),
                fuel_budget: dump.fuel_budget,
                fuel_used: outcome.fuel_used,
                elapsed_us,
                emitted_ops: outcome.ops,
                storage_mutations,
                override_i32: outcome.override_i32,
                override_i32_2: outcome.override_i32_2,
                override_f32: outcome.override_f32,
                override_str: outcome.override_str,
                rpc_return_str: outcome.rpc_return_str,
                wasi_logs: outcome.wasi_logs,
                stack_trace: Vec::new(),
            }
        }
        Err(err) => {
            let err_str = err.to_string();
            let fuel = infer_fuel_consumed(&err_str, dump.fuel_budget);
            fault_report(dump, err_str, fuel, elapsed_us)
        }
    }
}

fn fault_report(
    dump: &ModFaultDump,
    error: String,
    fuel_used: u64,
    elapsed_us: u64,
) -> ReplayReport {
    let stack_trace = format_stack_trace(
        &dump.mod_id,
        dump.language,
        &dump.trigger,
        &error,
        fuel_used,
        dump.fuel_budget,
    );
    ReplayReport {
        dump_id: dump.dump_id.clone(),
        mod_id: dump.mod_id.clone(),
        language: dump.language,
        trigger: dump.trigger.clone(),
        outcome: ReplayOutcome::Fault(error),
        fuel_budget: dump.fuel_budget,
        fuel_used,
        elapsed_us,
        emitted_ops: Vec::new(),
        storage_mutations: Vec::new(),
        override_i32: None,
        override_i32_2: None,
        override_f32: None,
        override_str: None,
        rpc_return_str: None,
        wasi_logs: Vec::new(),
        stack_trace,
    }
}

fn enrich_event_from_trigger(trigger: &InvocationTrigger, event: &mut ActiveEvent) {
    match trigger {
        InvocationTrigger::ChatCmd {
            slot,
            team_only,
            message,
            ..
        } if event.str_args[0].is_empty() && !message.is_empty() => {
            event.i32_args[0] = i32::from(*slot);
            event.i32_args[1] = i32::from(*team_only);
            event.str_args[0] = message.clone();
        }
        InvocationTrigger::ConsoleCmd {
            slot,
            command,
            args,
            ..
        } if event.str_args[0].is_empty() && !command.is_empty() => {
            if let Some(s) = slot {
                event.i32_args[0] = i32::from(*s);
                event.i32_args[1] = (args.len() + 1) as i32;
            }
            event.str_args[0] = command.clone();
            if let Some(first) = args.first() {
                event.str_args[1] = first.clone();
            }
            if args.is_empty() {
                event.str_args[2] = command.clone();
            } else {
                event.str_args[2] = format!("{command} {}", args.join(" "));
            }
        }
        InvocationTrigger::MenuSelect {
            cb_id,
            slot,
            menu_id,
            item_index,
            info,
        } if event.i32_args == [0; 6] && event.str_args[0].is_empty() => {
            event.i32_args = [
                i32::from(*slot),
                *menu_id as i32,
                *item_index,
                *cb_id,
                0,
                0,
            ];
            event.str_args[0] = info.clone();
        }
        _ => {}
    }
}

fn maybe_compile_wat_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if !bytes.starts_with(b"\0asm") && bytes.trim_ascii_start().starts_with(b"(") {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| format!("invalid UTF-8 in WAT replay input: {e}"))?;
        compile_wat_to_wasm(text)
    } else {
        Ok(bytes.to_vec())
    }
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct ReplayTimer {
    start: std::time::Instant,
}

#[cfg(not(target_arch = "wasm32"))]
impl ReplayTimer {
    fn start() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }

    fn elapsed_us(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

#[cfg(target_arch = "wasm32")]
struct ReplayTimer;

#[cfg(target_arch = "wasm32")]
impl ReplayTimer {
    fn start() -> Self {
        Self
    }

    fn elapsed_us(&self) -> u64 {
        0
    }
}

fn phase_str(phase: HookPhase) -> &'static str {
    match phase {
        HookPhase::Pre => "pre",
        HookPhase::Post => "post",
    }
}

fn parse_phase(raw: &str) -> Option<HookPhase> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "pre" | "0" => Some(HookPhase::Pre),
        "post" | "1" => Some(HookPhase::Post),
        _ => None,
    }
}

fn sanitize_id_segment(raw: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_len));
    let mut prev_underscore = false;
    for ch in raw.chars() {
        if out.len() >= max_len {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '-' {
            out.push(ch.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore && !out.is_empty() {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

fn normalize_lookup_id(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_suffix(".json").unwrap_or(trimmed);
    is_safe_dump_id(stripped).then_some(stripped)
}

#[cfg(not(target_arch = "wasm32"))]
fn is_safe_file_name(file_name: &str) -> bool {
    file_name
        .strip_suffix(".json")
        .is_some_and(is_safe_dump_id)
}

fn storage_key_for_id(id: &str) -> String {
    format!("{FAULT_STORAGE_KEY_PREFIX}{id}.json")
}

fn truncate_utf8(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0F)] as char);
    }
    out
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    let trimmed = hex.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if !trimmed.len().is_multiple_of(2) {
        return Err("odd-length hex string".to_string());
    }
    let mut out = Vec::with_capacity(trimmed.len() / 2);
    let raw = trimmed.as_bytes();
    let (pairs, _) = raw.as_chunks::<2>();
    for pair in pairs {
        let hi = hex_nibble(pair[0]).ok_or("invalid hex nibble")?;
        let lo = hex_nibble(pair[1]).ok_or("invalid hex nibble")?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

const fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ===========================================================================
// Domain <-> JSON Conversion
// ===========================================================================

fn trigger_to_json(trigger: &InvocationTrigger) -> JsonValue {
    let mut obj = BTreeMap::new();
    match trigger {
        InvocationTrigger::Init => {
            obj.insert("kind".into(), JsonValue::Str("init".into()));
        }
        InvocationTrigger::Hook { hook, phase } => {
            obj.insert("kind".into(), JsonValue::Str("hook".into()));
            obj.insert("hook".into(), JsonValue::Str(hook.as_name().into()));
            obj.insert("hook_id".into(), JsonValue::uint(u64::from(*hook as u32)));
            obj.insert("phase".into(), JsonValue::Str(phase_str(*phase).into()));
        }
        InvocationTrigger::Timer { cb_id, arg } => {
            obj.insert("kind".into(), JsonValue::Str("timer".into()));
            obj.insert("cb_id".into(), JsonValue::int(i64::from(*cb_id)));
            obj.insert("arg".into(), JsonValue::int(i64::from(*arg)));
        }
        InvocationTrigger::ChatCmd {
            slot,
            team_only,
            message,
            phase,
        } => {
            obj.insert("kind".into(), JsonValue::Str("chat_cmd".into()));
            obj.insert("slot".into(), JsonValue::uint(u64::from(*slot)));
            obj.insert("team_only".into(), JsonValue::Bool(*team_only));
            obj.insert("message".into(), JsonValue::Str(message.clone()));
            obj.insert("phase".into(), JsonValue::Str(phase_str(*phase).into()));
        }
        InvocationTrigger::ConsoleCmd {
            slot,
            command,
            args,
            phase,
        } => {
            obj.insert("kind".into(), JsonValue::Str("console_cmd".into()));
            obj.insert(
                "slot".into(),
                match slot {
                    Some(s) => JsonValue::uint(u64::from(*s)),
                    None => JsonValue::Null,
                },
            );
            obj.insert("command".into(), JsonValue::Str(command.clone()));
            obj.insert(
                "args".into(),
                JsonValue::Array(args.iter().map(|a| JsonValue::Str(a.clone())).collect()),
            );
            obj.insert("phase".into(), JsonValue::Str(phase_str(*phase).into()));
        }
        InvocationTrigger::MenuSelect {
            cb_id,
            slot,
            menu_id,
            item_index,
            info,
        } => {
            obj.insert("kind".into(), JsonValue::Str("menu_select".into()));
            obj.insert("cb_id".into(), JsonValue::int(i64::from(*cb_id)));
            obj.insert("slot".into(), JsonValue::uint(u64::from(*slot)));
            obj.insert("menu_id".into(), JsonValue::uint(u64::from(*menu_id)));
            obj.insert("item_index".into(), JsonValue::int(i64::from(*item_index)));
            obj.insert("info".into(), JsonValue::Str(info.clone()));
        }
        InvocationTrigger::Rpc { cb_id, fn_name } => {
            obj.insert("kind".into(), JsonValue::Str("rpc".into()));
            obj.insert("cb_id".into(), JsonValue::int(i64::from(*cb_id)));
            obj.insert("fn_name".into(), JsonValue::Str(fn_name.clone()));
        }
        InvocationTrigger::RpcEvent { cb_id, event_name } => {
            obj.insert("kind".into(), JsonValue::Str("rpc_event".into()));
            obj.insert("cb_id".into(), JsonValue::int(i64::from(*cb_id)));
            obj.insert("event_name".into(), JsonValue::Str(event_name.clone()));
        }
        InvocationTrigger::Webhook { cb_id, status_code } => {
            obj.insert("kind".into(), JsonValue::Str("webhook".into()));
            obj.insert("cb_id".into(), JsonValue::int(i64::from(*cb_id)));
            obj.insert("status_code".into(), JsonValue::int(i64::from(*status_code)));
        }
        InvocationTrigger::AmxxPublic { function, args } => {
            obj.insert("kind".into(), JsonValue::Str("amxx_public".into()));
            obj.insert("function".into(), JsonValue::Str(function.clone()));
            obj.insert(
                "args".into(),
                JsonValue::Array(args.iter().map(|&a| JsonValue::int(i64::from(a))).collect()),
            );
        }
    }
    JsonValue::Object(obj)
}

fn trigger_from_json(val: &JsonValue) -> Result<InvocationTrigger, String> {
    let obj = val.as_object().ok_or("`trigger` must be a JSON object")?;
    let kind = get_req_str(obj, "kind")?;
    match kind.as_str() {
        "init" => Ok(InvocationTrigger::Init),
        "hook" => {
            let hook = get_str(obj, "hook")
                .and_then(|n| HookId::from_name(&n))
                .or_else(|| get_u32(obj, "hook_id").and_then(HookId::from_u32))
                .ok_or("invalid or unknown hook in trigger")?;
            let phase = get_str(obj, "phase")
                .and_then(|p| parse_phase(&p))
                .unwrap_or(HookPhase::Pre);
            Ok(InvocationTrigger::Hook { hook, phase })
        }
        "timer" => Ok(InvocationTrigger::Timer {
            cb_id: get_i32(obj, "cb_id").unwrap_or(0),
            arg: get_i32(obj, "arg").unwrap_or(0),
        }),
        "chat_cmd" => Ok(InvocationTrigger::ChatCmd {
            slot: get_u8(obj, "slot").unwrap_or(0),
            team_only: get_bool(obj, "team_only").unwrap_or(false),
            message: get_str(obj, "message").unwrap_or_default(),
            phase: get_str(obj, "phase")
                .and_then(|p| parse_phase(&p))
                .unwrap_or(HookPhase::Pre),
        }),
        "console_cmd" => Ok(InvocationTrigger::ConsoleCmd {
            slot: obj.get("slot").and_then(|v| match v {
                JsonValue::Null => None,
                _ => v.as_u64().and_then(|n| u8::try_from(n).ok()),
            }),
            command: get_str(obj, "command").unwrap_or_default(),
            args: get_str_vec(obj, "args"),
            phase: get_str(obj, "phase")
                .and_then(|p| parse_phase(&p))
                .unwrap_or(HookPhase::Pre),
        }),
        "menu_select" => Ok(InvocationTrigger::MenuSelect {
            cb_id: get_i32(obj, "cb_id").unwrap_or(0),
            slot: get_u8(obj, "slot").unwrap_or(0),
            menu_id: get_u32(obj, "menu_id").unwrap_or(0),
            item_index: get_i32(obj, "item_index").unwrap_or(0),
            info: get_str(obj, "info").unwrap_or_default(),
        }),
        "rpc" => Ok(InvocationTrigger::Rpc {
            cb_id: get_i32(obj, "cb_id").unwrap_or(0),
            fn_name: get_str(obj, "fn_name").unwrap_or_default(),
        }),
        "rpc_event" => Ok(InvocationTrigger::RpcEvent {
            cb_id: get_i32(obj, "cb_id").unwrap_or(0),
            event_name: get_str(obj, "event_name").unwrap_or_default(),
        }),
        "webhook" => Ok(InvocationTrigger::Webhook {
            cb_id: get_i32(obj, "cb_id").unwrap_or(0),
            status_code: get_i32(obj, "status_code").unwrap_or(0),
        }),
        "amxx_public" => Ok(InvocationTrigger::AmxxPublic {
            function: get_req_str(obj, "function")?,
            args: get_i32_vec(obj, "args"),
        }),
        other => Err(format!("unknown invocation trigger kind `{other}`")),
    }
}

fn event_to_json(ev: &ActiveEvent) -> JsonValue {
    let mut obj = BTreeMap::new();
    obj.insert(
        "i32_args".into(),
        JsonValue::Array(
            ev.i32_args
                .iter()
                .map(|&n| JsonValue::int(i64::from(n)))
                .collect(),
        ),
    );
    obj.insert(
        "f32_args".into(),
        JsonValue::Array(ev.f32_args.iter().map(|&f| JsonValue::f32_val(f)).collect()),
    );
    obj.insert(
        "str_args".into(),
        JsonValue::Array(ev.str_args.iter().map(|s| JsonValue::Str(s.clone())).collect()),
    );
    obj.insert("payload_hex".into(), JsonValue::Str(encode_hex(&ev.payload)));
    JsonValue::Object(obj)
}

fn event_from_json(val: &JsonValue) -> Result<ActiveEvent, String> {
    let obj = val.as_object().ok_or("`event` must be a JSON object")?;
    let mut ev = ActiveEvent::default();
    if let Some(arr) = obj.get("i32_args").and_then(JsonValue::as_array) {
        for (dst, src) in ev.i32_args.iter_mut().zip(arr.iter()) {
            *dst = src.as_i64().map(|n| n as i32).unwrap_or(0);
        }
    }
    if let Some(arr) = obj.get("f32_args").and_then(JsonValue::as_array) {
        for (dst, src) in ev.f32_args.iter_mut().zip(arr.iter()) {
            *dst = src.as_f64().map(|f| f as f32).unwrap_or(0.0);
        }
    }
    if let Some(arr) = obj.get("str_args").and_then(JsonValue::as_array) {
        for (dst, src) in ev.str_args.iter_mut().zip(arr.iter()) {
            if let Some(s) = src.as_str() {
                *dst = s.to_string();
            }
        }
    }
    if let Some(hex) = get_str(obj, "payload_hex") {
        ev.payload = decode_hex(&hex)?;
    }
    Ok(ev)
}

fn snapshot_to_json(snap: &EngineSnapshot) -> JsonValue {
    let mut obj = BTreeMap::new();
    obj.insert("time_secs".into(), JsonValue::f64_val(snap.time_secs));
    obj.insert("frametime".into(), JsonValue::f32_val(snap.frametime));
    obj.insert("max_clients".into(), JsonValue::uint(u64::from(snap.max_clients)));
    obj.insert("map_name".into(), JsonValue::Str(snap.map_name.clone()));
    obj.insert(
        "entities".into(),
        JsonValue::Array(snap.entities.values().map(entity_to_json).collect()),
    );
    obj.insert(
        "clients".into(),
        JsonValue::Array(snap.clients.iter().map(client_to_json).collect()),
    );
    let mut cvars_obj = BTreeMap::new();
    for (k, v) in &snap.cvars {
        cvars_obj.insert(k.clone(), JsonValue::Str(v.clone()));
    }
    obj.insert("cvars".into(), JsonValue::Object(cvars_obj));
    obj.insert("client_health".into(), JsonValue::int(i64::from(snap.client_health)));
    obj.insert("client_armor".into(), JsonValue::int(i64::from(snap.client_armor)));
    obj.insert("client_money".into(), JsonValue::int(i64::from(snap.client_money)));
    JsonValue::Object(obj)
}

fn snapshot_from_json(val: &JsonValue) -> Result<EngineSnapshot, String> {
    let obj = val.as_object().ok_or("`snapshot` must be a JSON object")?;
    let mut entities = BTreeMap::new();
    if let Some(arr) = obj.get("entities").and_then(JsonValue::as_array) {
        for item in arr {
            let ent = entity_from_json(item)?;
            entities.insert(ent.index, ent);
        }
    }
    let mut clients = Vec::new();
    if let Some(arr) = obj.get("clients").and_then(JsonValue::as_array) {
        for item in arr {
            clients.push(client_from_json(item)?);
        }
    }
    Ok(EngineSnapshot {
        time_secs: get_f64(obj, "time_secs").unwrap_or(0.0),
        frametime: get_f32(obj, "frametime").unwrap_or(0.0),
        max_clients: get_u8(obj, "max_clients").unwrap_or(0),
        map_name: get_str(obj, "map_name").unwrap_or_default(),
        entities,
        clients,
        cvars: get_str_map(obj, "cvars"),
        collision: None,
        client_health: get_i32(obj, "client_health").unwrap_or(0),
        client_armor: get_i32(obj, "client_armor").unwrap_or(0),
        client_money: get_i32(obj, "client_money").unwrap_or(0),
    })
}

fn entity_to_json(ent: &EntitySnap) -> JsonValue {
    let mut obj = BTreeMap::new();
    obj.insert("index".into(), JsonValue::uint(u64::from(ent.index)));
    obj.insert("classname".into(), JsonValue::Str(ent.classname.clone()));
    obj.insert("origin".into(), vec3_to_json(ent.origin));
    obj.insert("velocity".into(), vec3_to_json(ent.velocity));
    obj.insert("health".into(), JsonValue::f32_val(ent.health));
    obj.insert("flags".into(), JsonValue::uint(u64::from(ent.flags)));
    JsonValue::Object(obj)
}

fn entity_from_json(val: &JsonValue) -> Result<EntitySnap, String> {
    let obj = val.as_object().ok_or("entity snapshot must be an object")?;
    Ok(EntitySnap {
        index: get_u16(obj, "index").unwrap_or(0),
        classname: get_str(obj, "classname").unwrap_or_default(),
        origin: get_vec3(obj, "origin"),
        velocity: get_vec3(obj, "velocity"),
        health: get_f32(obj, "health").unwrap_or(0.0),
        flags: get_u32(obj, "flags").unwrap_or(0),
    })
}

fn client_to_json(c: &ClientSnap) -> JsonValue {
    let mut obj = BTreeMap::new();
    obj.insert("slot".into(), JsonValue::uint(u64::from(c.slot)));
    obj.insert("active".into(), JsonValue::Bool(c.active));
    obj.insert("supports_mods".into(), JsonValue::Bool(c.supports_mods));
    obj.insert("name".into(), JsonValue::Str(c.name.clone()));
    obj.insert("health".into(), JsonValue::f32_val(c.health));
    obj.insert("max_health".into(), JsonValue::f32_val(c.max_health));
    obj.insert("armor".into(), JsonValue::int(i64::from(c.armor)));
    obj.insert("armor_kind".into(), JsonValue::uint(u64::from(c.armor_kind)));
    obj.insert("money".into(), JsonValue::int(i64::from(c.money)));
    obj.insert("frags".into(), JsonValue::int(i64::from(c.frags)));
    obj.insert("deaths".into(), JsonValue::int(i64::from(c.deaths)));
    obj.insert("team".into(), JsonValue::uint(u64::from(c.team)));
    obj.insert("alive".into(), JsonValue::Bool(c.alive));
    obj.insert("active_weapon".into(), JsonValue::uint(u64::from(c.active_weapon)));
    obj.insert("clip".into(), JsonValue::int(i64::from(c.clip)));
    obj.insert("bpammo".into(), JsonValue::int(i64::from(c.bpammo)));
    obj.insert("weapon_clips".into(), u8_i32_map_to_json(&c.weapon_clips));
    obj.insert("weapon_bpammo".into(), u8_i32_map_to_json(&c.weapon_bpammo));
    obj.insert("has_defuser".into(), JsonValue::Bool(c.has_defuser));
    obj.insert("has_nvg".into(), JsonValue::Bool(c.has_nvg));
    obj.insert("steamid".into(), JsonValue::Str(c.steamid.clone()));
    obj.insert("ip".into(), JsonValue::Str(c.ip.clone()));
    obj.insert("ping".into(), JsonValue::int(i64::from(c.ping)));
    obj.insert("loss".into(), JsonValue::int(i64::from(c.loss)));
    obj.insert("aim_origin".into(), vec3_to_json(c.aim_origin));
    obj.insert("admin_flags".into(), JsonValue::uint(u64::from(c.admin_flags)));
    obj.insert("lang".into(), JsonValue::Str(c.lang.clone()));
    JsonValue::Object(obj)
}

fn client_from_json(val: &JsonValue) -> Result<ClientSnap, String> {
    let obj = val.as_object().ok_or("client snapshot must be an object")?;
    Ok(ClientSnap {
        slot: get_u8(obj, "slot").unwrap_or(0),
        active: get_bool(obj, "active").unwrap_or(false),
        supports_mods: get_bool(obj, "supports_mods").unwrap_or(false),
        name: get_str(obj, "name").unwrap_or_default(),
        health: get_f32(obj, "health").unwrap_or(0.0),
        max_health: get_f32(obj, "max_health").unwrap_or(0.0),
        armor: get_i32(obj, "armor").unwrap_or(0),
        armor_kind: get_u8(obj, "armor_kind").unwrap_or(0),
        money: get_i32(obj, "money").unwrap_or(0),
        frags: get_i32(obj, "frags").unwrap_or(0),
        deaths: get_i32(obj, "deaths").unwrap_or(0),
        team: get_u8(obj, "team").unwrap_or(0),
        alive: get_bool(obj, "alive").unwrap_or(false),
        active_weapon: get_u8(obj, "active_weapon").unwrap_or(0),
        clip: get_i32(obj, "clip").unwrap_or(0),
        bpammo: get_i32(obj, "bpammo").unwrap_or(0),
        weapon_clips: get_u8_i32_map(obj, "weapon_clips"),
        weapon_bpammo: get_u8_i32_map(obj, "weapon_bpammo"),
        has_defuser: get_bool(obj, "has_defuser").unwrap_or(false),
        has_nvg: get_bool(obj, "has_nvg").unwrap_or(false),
        steamid: get_str(obj, "steamid").unwrap_or_default(),
        ip: get_str(obj, "ip").unwrap_or_default(),
        ping: get_i32(obj, "ping").unwrap_or(0),
        loss: get_i32(obj, "loss").unwrap_or(0),
        aim_origin: get_vec3(obj, "aim_origin"),
        admin_flags: get_u32(obj, "admin_flags").unwrap_or(0),
        lang: get_str(obj, "lang").unwrap_or_default(),
    })
}

fn vec3_to_json(v: [f32; 3]) -> JsonValue {
    JsonValue::Array(vec![
        JsonValue::f32_val(v[0]),
        JsonValue::f32_val(v[1]),
        JsonValue::f32_val(v[2]),
    ])
}

fn get_vec3(obj: &BTreeMap<String, JsonValue>, key: &str) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    if let Some(arr) = obj.get(key).and_then(JsonValue::as_array) {
        for (dst, src) in out.iter_mut().zip(arr.iter()) {
            *dst = src.as_f64().map(|f| f as f32).unwrap_or(0.0);
        }
    }
    out
}

fn u8_i32_map_to_json(map: &BTreeMap<u8, i32>) -> JsonValue {
    let mut obj = BTreeMap::new();
    for (&k, &v) in map {
        obj.insert(k.to_string(), JsonValue::int(i64::from(v)));
    }
    JsonValue::Object(obj)
}

fn get_u8_i32_map(obj: &BTreeMap<String, JsonValue>, key: &str) -> BTreeMap<u8, i32> {
    let mut out = BTreeMap::new();
    if let Some(map) = obj.get(key).and_then(JsonValue::as_object) {
        for (k, v) in map {
            if let (Ok(wid), Some(val)) = (k.parse::<u8>(), v.as_i64()) {
                out.insert(wid, val as i32);
            }
        }
    }
    out
}

fn get_str(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<String> {
    obj.get(key).and_then(JsonValue::as_str).map(str::to_string)
}

fn get_req_str(obj: &BTreeMap<String, JsonValue>, key: &str) -> Result<String, String> {
    get_str(obj, key).ok_or_else(|| format!("missing string field `{key}`"))
}

fn get_opt_str(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(JsonValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn get_bool(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<bool> {
    match obj.get(key) {
        Some(JsonValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn get_u8(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<u8> {
    obj.get(key)?.as_u64().and_then(|n| u8::try_from(n).ok())
}

fn get_u16(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<u16> {
    obj.get(key)?.as_u64().and_then(|n| u16::try_from(n).ok())
}

fn get_u32(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<u32> {
    obj.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok())
}

fn get_u64(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<u64> {
    obj.get(key)?.as_u64()
}

fn get_u64_or_hex(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<u64> {
    let v = obj.get(key)?;
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        {
            return u64::from_str_radix(hex, 16).ok();
        }
        return trimmed.parse::<u64>().ok();
    }
    None
}

fn get_i32(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<i32> {
    obj.get(key)?.as_i64().and_then(|n| i32::try_from(n).ok())
}

fn get_f32(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<f32> {
    let f = obj.get(key)?.as_f64()? as f32;
    f.is_finite().then_some(f)
}

fn get_f64(obj: &BTreeMap<String, JsonValue>, key: &str) -> Option<f64> {
    let f = obj.get(key)?.as_f64()?;
    f.is_finite().then_some(f)
}

fn get_str_vec(obj: &BTreeMap<String, JsonValue>, key: &str) -> Vec<String> {
    obj.get(key)
        .and_then(JsonValue::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(JsonValue::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn get_i32_vec(obj: &BTreeMap<String, JsonValue>, key: &str) -> Vec<i32> {
    obj.get(key)
        .and_then(JsonValue::as_array)
        .map(|arr| arr.iter().filter_map(|v| v.as_i64().map(|n| n as i32)).collect())
        .unwrap_or_default()
}

fn get_str_map(obj: &BTreeMap<String, JsonValue>, key: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(map) = obj.get(key).and_then(JsonValue::as_object) {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                out.insert(k.clone(), s.to_string());
            }
        }
    }
    out
}

// ===========================================================================
// Zero-Dependency RFC 8259 JSON Serializer & Recursive-Descent Parser
// ===========================================================================

#[derive(Debug, Clone)]
enum JsonValue {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    fn int(n: i64) -> Self {
        Self::Number(n.to_string())
    }

    fn uint(n: u64) -> Self {
        Self::Number(n.to_string())
    }

    fn f32_val(v: f32) -> Self {
        if !v.is_finite() {
            return Self::Number("0.0".to_string());
        }
        let mut s = v.to_string();
        if !s.contains('.') && !s.contains('e') && !s.contains('E') {
            s.push_str(".0");
        }
        Self::Number(s)
    }

    fn f64_val(v: f64) -> Self {
        if !v.is_finite() {
            return Self::Number("0.0".to_string());
        }
        let mut s = v.to_string();
        if !s.contains('.') && !s.contains('e') && !s.contains('E') {
            s.push_str(".0");
        }
        Self::Number(s)
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Number(n) => n.parse::<i64>().ok(),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(n) => n.parse::<u64>().ok(),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => n.parse::<f64>().ok(),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(m) => Some(m),
            _ => None,
        }
    }

    fn to_string_compact(&self) -> String {
        let mut out = String::with_capacity(256);
        self.write_json(&mut out, None, 0);
        out
    }

    fn to_string_pretty(&self) -> String {
        let mut out = String::with_capacity(512);
        self.write_json(&mut out, Some(2), 0);
        out
    }

    fn write_json(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Number(n) => out.push_str(n),
            Self::Str(s) => write_json_str(out, s),
            Self::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if let Some(step) = indent {
                        out.push('\n');
                        push_spaces(out, (depth + 1) * step);
                    }
                    item.write_json(out, indent, depth + 1);
                }
                if let Some(step) = indent {
                    out.push('\n');
                    push_spaces(out, depth * step);
                }
                out.push(']');
            }
            Self::Object(map) => {
                if map.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    if let Some(step) = indent {
                        out.push('\n');
                        push_spaces(out, (depth + 1) * step);
                    }
                    write_json_str(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write_json(out, indent, depth + 1);
                }
                if let Some(step) = indent {
                    out.push('\n');
                    push_spaces(out, depth * step);
                }
                out.push('}');
            }
        }
    }
}

fn push_spaces(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push(' ');
    }
}

fn write_json_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn parse(raw: &'a str) -> Result<JsonValue, String> {
        let mut p = Self {
            src: raw.as_bytes(),
            pos: 0,
        };
        p.skip_ws();
        let val = p.parse_value(0)?;
        p.skip_ws();
        if p.pos != p.src.len() {
            return Err(format!("trailing characters at byte {}", p.pos));
        }
        Ok(val)
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, String> {
        if depth > MAX_JSON_DEPTH {
            return Err("JSON nesting depth exceeded".to_string());
        }
        self.skip_ws();
        let Some(b) = self.peek() else {
            return Err("unexpected end of JSON input".to_string());
        };
        match b {
            b'{' => self.parse_object(depth + 1),
            b'[' => self.parse_array(depth + 1),
            b'"' => self.parse_string().map(JsonValue::Str),
            b't' => self.expect_lit(b"true", JsonValue::Bool(true)),
            b'f' => self.expect_lit(b"false", JsonValue::Bool(false)),
            b'n' => self.expect_lit(b"null", JsonValue::Null),
            b'-' | b'0'..=b'9' => self.parse_number(),
            other => Err(format!(
                "unexpected byte 0x{other:02x} at offset {}",
                self.pos
            )),
        }
    }

    fn expect_lit(&mut self, lit: &[u8], val: JsonValue) -> Result<JsonValue, String> {
        if self.src.get(self.pos..self.pos + lit.len()) == Some(lit) {
            self.pos += lit.len();
            Ok(val)
        } else {
            Err(format!("invalid JSON literal at offset {}", self.pos))
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.bump(); // '{'
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(format!("expected object key string at offset {}", self.pos));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.bump() != Some(b':') {
                return Err(format!("expected ':' after key at offset {}", self.pos));
            }
            let val = self.parse_value(depth)?;
            map.insert(key, val);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => return Ok(JsonValue::Object(map)),
                _ => return Err(format!("expected ',' or '}}' at offset {}", self.pos)),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.bump(); // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(JsonValue::Array(items));
        }
        loop {
            let val = self.parse_value(depth)?;
            items.push(val);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => return Ok(JsonValue::Array(items)),
                _ => return Err(format!("expected ',' or ']' at offset {}", self.pos)),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.bump() != Some(b'"') {
            return Err(format!("expected '\"' at offset {}", self.pos));
        }
        let mut out = String::new();
        while let Some(b) = self.peek() {
            if b == b'"' {
                self.bump();
                return Ok(out);
            }
            if b == b'\\' {
                self.bump();
                let esc = self.bump().ok_or("unterminated JSON string escape")?;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{08}'),
                    b'f' => out.push('\u{0C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code) {
                            if self.src.get(self.pos..self.pos + 2) == Some(b"\\u") {
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if (0xDC00..=0xDFFF).contains(&low) {
                                    let combined = 0x10000
                                        + ((u32::from(code) - 0xD800) << 10)
                                        + (u32::from(low) - 0xDC00);
                                    let ch = char::from_u32(combined)
                                        .ok_or("invalid UTF-16 surrogate pair")?;
                                    out.push(ch);
                                } else {
                                    return Err("invalid low UTF-16 surrogate".to_string());
                                }
                            } else {
                                return Err("unpaired high UTF-16 surrogate".to_string());
                            }
                        } else {
                            let ch = char::from_u32(u32::from(code))
                                .ok_or("invalid unicode escape")?;
                            out.push(ch);
                        }
                    }
                    other => {
                        return Err(format!("invalid escape `\\{}`", other as char));
                    }
                }
            } else if b < 0x20 {
                return Err(format!("unescaped control byte 0x{b:02x} in string"));
            } else {
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c == b'"' || c == b'\\' || c < 0x20 {
                        break;
                    }
                    self.pos += 1;
                }
                let slice = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|e| format!("invalid UTF-8 in JSON string: {e}"))?;
                out.push_str(slice);
            }
        }
        Err("unterminated JSON string".to_string())
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let slice = self
            .src
            .get(self.pos..self.pos + 4)
            .ok_or("truncated \\uXXXX escape")?;
        self.pos += 4;
        let mut val = 0u16;
        for &b in slice {
            let n = hex_nibble(b).ok_or("invalid hex digit in \\uXXXX escape")?;
            val = (val << 4) | u16::from(n);
        }
        Ok(val)
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let mut saw_digit = false;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                saw_digit = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if !saw_digit {
            return Err(format!("invalid JSON number at offset {start}"));
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            let mut frac_digit = false;
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    frac_digit = true;
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if !frac_digit {
                return Err(format!("invalid JSON fraction at offset {start}"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            let mut exp_digit = false;
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    exp_digit = true;
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if !exp_digit {
                return Err(format!("invalid JSON exponent at offset {start}"));
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|e| format!("invalid number bytes: {e}"))?;
        Ok(JsonValue::Number(text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use cs_platform::MemoryStorage;

    use super::*;

    fn sample_dump(id_suffix: &str) -> ModFaultDump {
        let mut entities = BTreeMap::new();
        entities.insert(
            1,
            EntitySnap {
                index: 1,
                classname: "player".into(),
                origin: [10.5, -20.25, 64.0],
                velocity: [1.0, 2.0, 0.0],
                health: 85.0,
                flags: 512,
            },
        );
        let mut storage_cache = BTreeMap::new();
        storage_cache.insert("vip_points".into(), "420".into());
        let mut cvars = BTreeMap::new();
        cvars.insert("mp_friendlyfire".into(), "0".into());
        cvars.insert("amx_language".into(), "ru".into());

        let mut weapon_clips = BTreeMap::new();
        weapon_clips.insert(28, 30);

        let trigger = InvocationTrigger::Hook {
            hook: HookId::PlayerTakeDamage,
            phase: HookPhase::Pre,
        };
        ModFaultDump {
            schema_version: FAULT_DUMP_SCHEMA_VERSION,
            dump_id: ModFaultDump::make_dump_id(1_700_000_000_000, &format!("mod_{id_suffix}"), &trigger),
            timestamp_unix_ms: 1_700_000_000_000,
            mod_id: format!("mod_{id_suffix}"),
            version: "1.2.3".into(),
            language: PluginLanguage::Wasm,
            source_path: Some("mods/buggy".into()),
            binary_sha256: "a".repeat(64),
            permissions_mask: Permissions::NONE.bits(),
            http_hosts: vec!["discord.com".into()],
            fuel_budget: 250_000,
            max_memory_bytes: 1_048_576,
            trigger,
            event: ActiveEvent {
                i32_args: [0, 1, 2, 28, 4096, 0],
                f32_args: [75.5, 0.0, 0.0, 0.0],
                str_args: ["quote\"and\\slash\nПривет".into(), String::new(), String::new()],
                payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
            snapshot: EngineSnapshot {
                time_secs: 123.45,
                frametime: 0.01,
                max_clients: 32,
                map_name: "de_dust2".into(),
                entities,
                clients: vec![ClientSnap {
                    slot: 0,
                    active: true,
                    supports_mods: true,
                    name: "Hero".into(),
                    health: 85.0,
                    max_health: 100.0,
                    armor: 100,
                    armor_kind: 2,
                    money: 4200,
                    frags: 9,
                    deaths: 3,
                    team: 2,
                    alive: true,
                    active_weapon: 28,
                    clip: 30,
                    bpammo: 90,
                    weapon_clips,
                    weapon_bpammo: BTreeMap::new(),
                    has_defuser: true,
                    has_nvg: false,
                    steamid: "STEAM_0:1:12345".into(),
                    ip: "127.0.0.1".into(),
                    ping: 15,
                    loss: 0,
                    aim_origin: [1.0, 2.0, 3.0],
                    admin_flags: AdminFlags::parse_str("abct").bits(),
                    lang: "ru".into(),
                }],
                cvars,
                collision: None,
                client_health: 0,
                client_armor: 0,
                client_money: 0,
            },
            storage_cache,
            rng_seed: 0x9E37_79B9_7F4A_7C15,
            fuel_consumed_before_trap: 12_345,
            error: "WASM trap: integer divide by zero".into(),
            stack_trace: vec!["#0 wasm::mod_a::cs_mod_hook".into()],
            truncated: false,
        }
    }

    #[test]
    fn json_roundtrip_preserves_unicode_high_u64_seed_and_all_fields() {
        let dump = sample_dump("alpha");
        let json = dump.to_json();
        let parsed = ModFaultDump::from_json(&json).expect("valid JSON roundtrip");
        assert_eq!(parsed.dump_id, dump.dump_id);
        assert_eq!(parsed.mod_id, dump.mod_id);
        assert_eq!(parsed.language, PluginLanguage::Wasm);
        assert_eq!(parsed.trigger, dump.trigger);
        assert_eq!(parsed.event.i32_args, dump.event.i32_args);
        assert_eq!(parsed.event.f32_args, dump.event.f32_args);
        assert_eq!(parsed.event.str_args, dump.event.str_args);
        assert_eq!(parsed.event.payload, dump.event.payload);
        assert_eq!(parsed.snapshot.map_name, "de_dust2");
        assert_eq!(parsed.snapshot.clients.len(), 1);
        assert_eq!(parsed.snapshot.clients[0].weapon_clips.get(&28), Some(&30));
        assert_eq!(parsed.snapshot.clients[0].lang, "ru");
        assert_eq!(parsed.rng_seed, 0x9E37_79B9_7F4A_7C15);
        assert_eq!(parsed.fuel_consumed_before_trap, 12_345);
        assert_eq!(parsed.storage_cache.get("vip_points").map(String::as_str), Some("420"));
    }

    #[test]
    fn bounded_json_truncates_oversized_dump_to_256_kib() {
        let mut dump = sample_dump("giant");
        for i in 0..3000u16 {
            dump.snapshot.entities.insert(
                i,
                EntitySnap {
                    index: i,
                    classname: format!("func_breakable_{i}"),
                    origin: [i as f32, 0.0, 0.0],
                    velocity: [0.0; 3],
                    health: 100.0,
                    flags: 0,
                },
            );
        }
        for i in 0..500 {
            dump.storage_cache
                .insert(format!("blob_{i}"), "X".repeat(1024));
        }

        let (bounded, json) = dump.to_bounded_json();
        assert!(json.len() <= MAX_FAULT_DUMP_BYTES, "len={}", json.len());
        assert!(bounded.truncated);
        let parsed = ModFaultDump::from_json(&json).expect("truncated dump stays valid JSON");
        assert!(parsed.truncated);
        assert_eq!(parsed.mod_id, "mod_giant");
    }

    #[test]
    fn fault_dump_store_enforces_32_entry_ring_and_persists_storage() {
        let mut store = FaultDumpStore::new();
        let mut storage = MemoryStorage::default();
        for i in 0..40u64 {
            let mut d = sample_dump(&format!("m{i}"));
            d.timestamp_unix_ms = 1_700_000_000_000 + i;
            d.dump_id = ModFaultDump::make_dump_id(d.timestamp_unix_ms, &d.mod_id, &d.trigger);
            store.record(d, Some(&mut storage));
        }

        assert_eq!(store.len(), MAX_STORED_FAULTS);
        let list = store.list_faults();
        assert_eq!(list.len(), MAX_STORED_FAULTS);
        assert_eq!(list[0].mod_id, "mod_m39");
        assert_eq!(list[MAX_STORED_FAULTS - 1].mod_id, "mod_m8");
        assert!(store.get_fault_json(&list[0].id).is_some());
        assert!(store.get_fault_json(&list[0].file_name).is_some());
        assert!(store.get_fault_json("../etc/passwd").is_none());

        // Hydrate fresh store from persisted ConfigStorage.
        let mut restored = FaultDumpStore::new();
        assert_eq!(restored.load_from_storage(&storage), MAX_STORED_FAULTS);
        assert_eq!(restored.list_faults()[0].mod_id, "mod_m39");
    }
}
