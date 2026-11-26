//! Polyglot mod scaffolding and build CLI (`cargo xtask mod new|build` + `mod-build` alias).
//!
//! Supports Rust (`cs-mod-sdk` -> `wasm32-unknown-unknown`), C/C++20 (`sdk/cpp/cs16_mod.hpp` ->
//! `clang++` or verified prebuilt `.wasm`), Go (`sdk/go/cs16mod` -> `GOOS=wasip1 GOARCH=wasm`
//! or verified prebuilt `.wasm`), and zero-compile Python scripts (`main.py` + `cs16.pyi`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use cs_mod_api::manifest::is_valid_id;
use cs_mod_api::{ABI_VERSION, CsModArchive, ModManifest, ModSide, sha256_hex};

pub const DEFAULT_MOD_OUT: &str = "dist/mods";

const MOD_USAGE: &str = "\
usage: cargo xtask mod new <id> --lang rust|cpp|go|py [--dir <parent>] [--name <title>] [--side server|client|both]
       cargo xtask mod build [dir] [--out <dir>]
       cargo xtask mod replay <dump.json> [--mod-path <override>] [--json]
       cargo xtask mod-build [--out <dir>]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModLang {
    Rust,
    Cpp,
    Go,
    Python,
}

impl ModLang {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "cpp" | "c++" | "cxx" | "c" => Some(Self::Cpp),
            "go" | "golang" => Some(Self::Go),
            "py" | "python" => Some(Self::Python),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Cpp => "cpp",
            Self::Go => "go",
            Self::Python => "py",
        }
    }
}

/// Entrypoint for `cargo xtask mod <subcommand> ...`.
pub fn run_mod_cli(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    match args.next().as_deref() {
        Some("new") => run_mod_new(args),
        Some("build") => run_mod_build_args(args),
        Some("replay") => run_mod_replay_args(args),
        Some(other) => Err(format!("unknown `mod` subcommand `{other}`\n{MOD_USAGE}")),
        None => Err(MOD_USAGE.to_string()),
    }
}

/// Backwards-compatible entrypoint for `cargo xtask mod-build [--out <dir>]`.
pub fn run_mod_build_alias(args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    run_mod_build_args(args)
}

fn run_mod_new(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let mut id: Option<String> = None;
    let mut lang: Option<ModLang> = None;
    let mut parent_dir = PathBuf::from("mods");
    let mut display_name: Option<String> = None;
    let mut side = ModSide::Server;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lang" => {
                let raw = args.next().ok_or("--lang needs a value (rust|cpp|go|py)")?;
                lang = Some(ModLang::parse(&raw).ok_or_else(|| {
                    format!("unsupported --lang `{raw}` (expected rust, cpp, go, or py)")
                })?);
            }
            "--dir" => {
                parent_dir = PathBuf::from(args.next().ok_or("--dir needs a value")?);
            }
            "--name" => {
                display_name = Some(args.next().ok_or("--name needs a value")?);
            }
            "--side" => {
                let raw = args.next().ok_or("--side needs a value")?;
                side = ModSide::parse(&raw).ok_or_else(|| {
                    format!("invalid --side `{raw}` (expected server, client, or both)")
                })?;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown argument `{other}`\n{MOD_USAGE}"));
            }
            pos => {
                if id.is_some() {
                    return Err(format!("unexpected positional argument `{pos}`\n{MOD_USAGE}"));
                }
                id = Some(pos.to_string());
            }
        }
    }

    let id = id.ok_or_else(|| format!("missing <id>\n{MOD_USAGE}"))?;
    let lang = lang.ok_or_else(|| format!("missing --lang rust|cpp|go|py\n{MOD_USAGE}"))?;
    let title = display_name.unwrap_or_else(|| title_case_id(&id));
    let target = parent_dir.join(&id);
    let root = workspace_root();

    let created = scaffold_project(&target, &id, &title, lang, side, &root)?;
    println!(
        "Created {} mod `{}` at {} ({} files)",
        lang.as_str(),
        id,
        target.display(),
        created.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn run_mod_build_args(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let mut out_dir = PathBuf::from(DEFAULT_MOD_OUT);
    let mut source_dir = PathBuf::from("mods");
    let mut got_dir = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                out_dir = PathBuf::from(args.next().ok_or("--out needs a value")?);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown argument `{other}`\n{MOD_USAGE}"));
            }
            pos => {
                if got_dir {
                    return Err(format!("unexpected positional argument `{pos}`\n{MOD_USAGE}"));
                }
                source_dir = PathBuf::from(pos);
                got_dir = true;
            }
        }
    }

    let built = build_mods_in(&source_dir, &out_dir, &workspace_root())?;
    for item in &built {
        println!(
            "[{}] {} ({} bytes, sha256={})",
            item.mod_id,
            item.output_path.display(),
            item.byte_len,
            item.sha256
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn run_mod_replay_args(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let mut dump_path: Option<PathBuf> = None;
    let mut mod_override: Option<PathBuf> = None;
    let mut json_out = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mod-path" => {
                mod_override = Some(PathBuf::from(
                    args.next().ok_or("--mod-path needs a value")?,
                ));
            }
            "--json" => json_out = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown argument `{other}`\n{MOD_USAGE}"));
            }
            pos => {
                if dump_path.is_some() {
                    return Err(format!("unexpected positional argument `{pos}`\n{MOD_USAGE}"));
                }
                dump_path = Some(PathBuf::from(pos));
            }
        }
    }

    let dump_path = dump_path.ok_or_else(|| format!("mod replay requires <dump.json>\n{MOD_USAGE}"))?;
    let report = replay_dump_file(&dump_path, mod_override.as_deref())?;
    if json_out {
        println!("{}", report.to_json());
    } else {
        print!("{report}");
    }
    if report.outcome.is_pass() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

/// Replays a `ModFaultDump` JSON file deterministically across `.wasm`, `.csmod`, `.py`, and `.amxx`.
pub fn replay_dump_file(
    dump_path: &Path,
    mod_override: Option<&Path>,
) -> Result<cs_mod_host::crash_dump::ReplayReport, String> {
    use cs_mod_host::crash_dump::{
        InvocationTrigger, ModFaultDump, PluginLanguage, hydrate_services_from_snapshot,
        replay_fault_dump_from_path, replay_fault_dump_with_instance,
    };
    use cs_mod_host::natives::ActiveEvent;
    use cs_mod_host::runtime::{ModExecContext, ModInstance};

    let raw = fs::read_to_string(dump_path).map_err(|e| io_err(dump_path, &e))?;
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

    let py_file = if target_path.is_file()
        && target_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("py"))
    {
        Some(target_path.clone())
    } else if target_path.is_dir() && target_path.join("main.py").is_file() {
        Some(target_path.join("main.py"))
    } else {
        None
    };

    if py_file.is_some() || dump.language == PluginLanguage::Python {
        let script_path = py_file.unwrap_or(target_path);
        let source = fs::read_to_string(&script_path).map_err(|e| io_err(&script_path, &e))?;
        let file_name = script_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("plugin.py");
        let max_mem = dump.max_memory_bytes.max(65_536);
        let mut inst = cs_mod_py::PyModInstance::from_source(
            &dump.mod_id,
            file_name,
            &source,
            dump.permissions(),
            max_mem,
        )
        .map_err(|e| format!("failed to load Python plugin `{}`: {e}", script_path.display()))?;
        let services = hydrate_services_from_snapshot(&dump.snapshot);
        inst.configure_v2(services, dump.http_hosts.clone(), None);
        if dump.trigger != InvocationTrigger::Init {
            let init_ev = ActiveEvent::default();
            let init_ctx = ModExecContext {
                snapshot: &dump.snapshot,
                event: &init_ev,
                storage: &dump.storage_cache,
                fuel_budget: dump.fuel_budget.max(500_000),
            };
            let _ = inst.call_init(&init_ctx);
        }
        return Ok(replay_fault_dump_with_instance(&dump, &mut inst));
    }

    replay_fault_dump_from_path(dump_path, mod_override)
}

/// Summary of one packaged `.csmod` or validated standalone script produced by [`build_mods_in`].
#[derive(Debug, Clone)]
pub struct BuiltModArtifact {
    pub mod_id: String,
    pub output_path: PathBuf,
    pub byte_len: usize,
    pub sha256: String,
}

/// Scaffolds a ready-to-edit polyglot mod directory under `target_dir`.
pub fn scaffold_project(
    target_dir: &Path,
    id: &str,
    display_name: &str,
    lang: ModLang,
    side: ModSide,
    repo_root: &Path,
) -> Result<Vec<PathBuf>, String> {
    if !is_valid_id(id) {
        return Err(format!(
            "invalid mod id `{id}` (expected 1..=48 lowercase [a-z0-9_-])"
        ));
    }
    if target_dir.exists() {
        let is_non_empty = fs::read_dir(target_dir)
            .map(|mut rd| rd.next().is_some())
            .unwrap_or(true);
        if is_non_empty {
            return Err(format!(
                "target directory `{}` already exists and is not empty",
                target_dir.display()
            ));
        }
    }

    let files = generate_template_files(target_dir, id, display_name, lang, side, repo_root);
    let mut written = Vec::with_capacity(files.len());
    for (rel, content) in files {
        let dst = target_dir.join(&rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent, &e))?;
        }
        fs::write(&dst, content).map_err(|e| io_err(&dst, &e))?;
        written.push(dst);
    }
    Ok(written)
}

fn generate_template_files(
    target_dir: &Path,
    id: &str,
    name: &str,
    lang: ModLang,
    side: ModSide,
    repo_root: &Path,
) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();

    let lang_toml = format!(
        "[en]\n\
         WELCOME = \"\\x04[{name}]\\x01 Welcome, {{player}}! Type /hello for a shockwave.\"\n\
         HELLO_REPLY = \"\\x04[{name}]\\x01 Hello, {{player}}!\"\n\
         \n\
         [ru]\n\
         WELCOME = \"\\x04[{name}]\\x01 Добро пожаловать, {{player}}! Введите /hello.\"\n\
         HELLO_REPLY = \"\\x04[{name}]\\x01 Привет, {{player}}!\"\n"
    );
    files.insert("lang.toml".to_string(), lang_toml);

    let side_str = side.as_str();
    let max_mem = if lang == ModLang::Go {
        16_777_216
    } else {
        4_194_304
    };
    let entry_server_line = if side.has_server() {
        match lang {
            ModLang::Python => "entry_server = \"main.py\"\n",
            _ => "entry_server = \"server.wasm\"\n",
        }
    } else {
        ""
    };
    let entry_client_line = if side.has_client() {
        "entry_client = \"client.wasm\"\n"
    } else {
        ""
    };

    files.insert(
        "mod.toml".to_string(),
        format!(
            "[mod]\n\
             id = \"{id}\"\n\
             name = \"{name}\"\n\
             version = \"0.1.0\"\n\
             api_version = {ABI_VERSION}\n\
             side = \"{side_str}\"\n\
             {entry_server_line}{entry_client_line}\
             permissions = [\"entities\", \"messages\", \"menus\", \"vfx\", \"commands\", \"storage\"]\n\
             fuel_per_frame = 500000\n\
             max_memory_bytes = {max_mem}\n"
        ),
    );

    match lang {
        ModLang::Rust => {
            let sdk_path = repo_root
                .join("crates")
                .join("cs-mod-sdk")
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(
                "Cargo.toml".to_string(),
                format!(
                    "[workspace]\n\n\
                     [package]\n\
                     name = \"{id}\"\n\
                     version = \"0.1.0\"\n\
                     edition = \"2024\"\n\
                     license = \"GPL-3.0-or-later\"\n\
                     \n\
                     [lib]\n\
                     crate-type = [\"cdylib\", \"rlib\"]\n\
                     \n\
                     [dependencies]\n\
                     cs-mod-sdk = {{ path = \"{sdk_path}\" }}\n"
                ),
            );
            files.insert(
                "src/lib.rs".to_string(),
                r#"use cs_mod_sdk::{
    HookAction, HookId, HookPhase, HostBridge, Player, cs_mod_chat, cs_mod_hook, cs_mod_init, vfx,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);
    host.hook_register(HookId::PlayerTakeDamage, HookPhase::Pre, 0);
    host.hook_register(HookId::PlayerSay, HookPhase::Pre, 0);
    0
}

#[cs_mod_hook(PlayerTakeDamage, pre)]
pub fn on_take_damage(host: &mut impl HostBridge) -> HookAction {
    let damage = host.hook_arg_f32(0);
    if damage > 150.0 {
        host.hook_set_override_f32(150.0);
        return HookAction::Override;
    }
    HookAction::Continue
}

#[cs_mod_chat("/hello", "!hello")]
pub fn on_hello(host: &mut impl HostBridge, slot: u8) -> HookAction {
    let player = Player::new(slot);
    let name = player.name(host);
    player.say_tr(host, "HELLO_REPLY", &[("player", &name)]);
    if let Some(origin) = player.origin(host) {
        vfx::shockwave_ring(host, origin, 160.0, [0, 220, 255, 220], 8, 10, None);
    }
    HookAction::Handled
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::PlayerTakeDamage, HookPhase::Pre) => on_take_damage(host),
        (HookId::PlayerSay, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let msg = host.hook_arg_str(0).unwrap_or_default();
            if on_hello_matches_chat(&msg) {
                return on_hello(host, slot);
            }
            HookAction::Continue
        }
        _ => HookAction::Continue,
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_init() -> i32 {
    let mut host = WasmHostBridge;
    init_mod(&mut host)
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_hook(hook_id: i32, phase: i32) -> i32 {
    let (Some(hook), Some(ph)) = (HookId::from_i32(hook_id), HookPhase::from_i32(phase)) else {
        return HookAction::Continue as i32;
    };
    let mut host = WasmHostBridge;
    dispatch_hook(&mut host, hook, ph) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_mod_sdk::MockHostBridge;

    #[test]
    fn hello_command_and_damage_cap() {
        let mut host = MockHostBridge::default();
        host.add_player(0, "Player");
        host.set_player_lang(0, "ru");
        assert_eq!(init_mod(&mut host), 0);

        host.active_f32_args[0] = 220.0;
        assert_eq!(
            dispatch_hook(&mut host, HookId::PlayerTakeDamage, HookPhase::Pre),
            HookAction::Override
        );
        assert_eq!(host.override_f32, Some(150.0));

        host.active_i32_args[0] = 0;
        host.active_str_args[0] = "/hello".to_string();
        assert_eq!(
            dispatch_hook(&mut host, HookId::PlayerSay, HookPhase::Pre),
            HookAction::Handled
        );
        assert!(host.say_texts[0].1.contains("Привет, Player"));
    }
}
"#
                .to_string(),
            );
            files.insert(
                "README.md".to_string(),
                format!(
                    "# {name} (Rust WASM Mod)\n\n\
                     - Test natively: `cargo test`\n\
                     - Build & pack `.csmod`: `cargo xtask mod build {}`\n",
                    target_dir.display()
                ),
            );
        }
        ModLang::Cpp => {
            let sdk_cpp = repo_root
                .join("sdk")
                .join("cpp")
                .to_string_lossy()
                .replace('\\', "/");
            let dir_norm = target_dir.to_string_lossy().replace('\\', "/");
            files.insert(
                "main.cpp".to_string(),
                format!(
                    "#include \"cs16_mod.hpp\"\n\n\
                     CS16_PLUGIN_INFO(\"{id}\", \"{name}\", \"0.1.0\")\n\n\
                     CS16_ON_HOOK(cs16::HookId::PlayerTakeDamage, cs16::HookPhase::Pre, on_take_damage) {{\n\
                         auto ev = cs16::DamageEvent::current();\n\
                         if (ev.damage > 150.0f) {{\n\
                             ev.set_damage(150.0f);\n\
                             return cs16::HookAction::Override;\n\
                         }}\n\
                         return cs16::HookAction::Continue;\n\
                     }}\n\n\
                     CS16_ON_CHAT(\"/hello\", on_hello) {{\n\
                         (void)args;\n\
                         auto nm = player.name();\n\
                         cs16::Buf<128> kv;\n\
                         kv.append(\"player=\");\n\
                         kv.append(nm.view());\n\
                         player.say_tr(\"HELLO_REPLY\", kv.view());\n\
                         cs16::Vec3 pos;\n\
                         if (player.origin(pos)) {{\n\
                             cs16::vfx::shockwave_ring(pos, 160.0f, cs16::Color::Cyan(), 8, 10);\n\
                         }}\n\
                         return cs16::HookAction::Handled;\n\
                     }}\n\n\
                     CS16_EXPORT_PLUGIN()\n"
                ),
            );
            files.insert(
                "compile_commands.json".to_string(),
                format!(
                    "[\n\
                     \x20 {{\n\
                     \x20   \"directory\": \"{dir_norm}\",\n\
                     \x20   \"command\": \"clang++ --target=wasm32 -std=c++20 -O2 -nostdlib -fno-exceptions -I{sdk_cpp} -c main.cpp -o server.wasm\",\n\
                     \x20   \"file\": \"main.cpp\"\n\
                     \x20 }}\n\
                     ]\n"
                ),
            );
            files.insert(
                "README.md".to_string(),
                format!(
                    "# {name} (C++20 WASM Mod)\n\n\
                     Build with Clang (`--target=wasm32 -std=c++20 -O2 -nostdlib -fno-exceptions -Wl,--no-entry -Wl,--export-dynamic`) or run `cargo xtask mod build {}`.\n",
                    target_dir.display()
                ),
            );
        }
        ModLang::Go => {
            let sdk_go = repo_root
                .join("sdk")
                .join("go")
                .join("cs16mod")
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(
                "go.mod".to_string(),
                format!(
                    "module {id}\n\n\
                     go 1.24\n\n\
                     require github.com/cs16-rs/cs16-rs/sdk/go/cs16mod v0.0.0\n\n\
                     replace github.com/cs16-rs/cs16-rs/sdk/go/cs16mod => {sdk_go}\n"
                ),
            );
            files.insert(
                "main.go".to_string(),
                r#"//go:build wasm

package main

import (
	_ "embed"

	"github.com/cs16-rs/cs16-rs/sdk/go/cs16mod"
)

//go:embed lang.toml
var langTOML string

func init() {
	cs16mod.OnInit(func() int32 {
		cs16mod.LangLoadTOML(langTOML)
		return 0
	})

	cs16mod.OnPlayerTakeDamage(cs16mod.PhasePre, func(ev *cs16mod.DamageEvent) cs16mod.HookAction {
		if ev.Damage > 150.0 {
			ev.SetDamage(150.0)
			return cs16mod.Override
		}
		return cs16mod.Continue
	})

	cs16mod.OnChat("/hello", func(p cs16mod.Player, _ string) cs16mod.HookAction {
		p.SayTr("HELLO_REPLY", cs16mod.Arg("player", p.Name()))
		if pos, ok := p.Origin(); ok {
			cs16mod.VFX.ShockwaveRing(pos, 160, cs16mod.Color{R: 0, G: 220, B: 255, A: 220}, 8, 10, cs16mod.SlotAll)
		}
		return cs16mod.Handled
	})
}

func main() {}
"#
                .to_string(),
            );
            files.insert(
                "README.md".to_string(),
                format!(
                    "# {name} (Go WASM Mod)\n\n\
                     Build with `GOOS=wasip1 GOARCH=wasm go build -buildmode=c-shared -o server.wasm .` or run `cargo xtask mod build {}`.\n",
                    target_dir.display()
                ),
            );
        }
        ModLang::Python => {
            files.insert(
                "main.py".to_string(),
                format!(
                    r#"from cs16 import mod, vfx, HookAction, Color

@mod.info(id="{id}", name="{name}", version="0.1.0")
def plugin_init():
    pass

@mod.hook("player_take_damage", phase="pre")
def on_take_damage(event):
    if event.damage > 150.0:
        event.damage = 150.0
        return HookAction.OVERRIDE
    return HookAction.CONTINUE

@mod.chat_command("/hello", "!hello")
def on_hello(player, args):
    player.chat(player.tr("HELLO_REPLY", player=player.name))
    vfx.shockwave_ring(player.origin, 160.0, Color.CYAN, 8, 10)
    return HookAction.HANDLED
"#
                ),
            );
            let pyi_src = repo_root.join("sdk").join("python").join("cs16.pyi");
            let pyi_text = fs::read_to_string(&pyi_src)
                .unwrap_or_else(|_| FALLBACK_CS16_PYI.to_string());
            files.insert("cs16.pyi".to_string(), pyi_text);
            files.insert(
                "pyrightconfig.json".to_string(),
                "{\n  \"include\": [\".\"],\n  \"extraPaths\": [\".\"]\n}\n".to_string(),
            );
            files.insert(
                "README.md".to_string(),
                format!(
                    "# {name} (Zero-Compile Python Mod)\n\n\
                     Edit `main.py` directly (VS Code / PyCharm IntelliSense provided by `cs16.pyi`) and validate/pack with `cargo xtask mod build {}`.\n",
                    target_dir.display()
                ),
            );
        }
    }

    files
}

/// Builds every discovered mod in `source_dir` (single mod directory, single `.py` file,
/// or directory tree) and writes `.csmod` packages into `out_dir`.
pub fn build_mods_in(
    source_dir: &Path,
    out_dir: &Path,
    repo_root: &Path,
) -> Result<Vec<BuiltModArtifact>, String> {
    fs::create_dir_all(out_dir).map_err(|e| io_err(out_dir, &e))?;

    if source_dir.is_file() {
        if source_dir.extension().and_then(|e| e.to_str()) == Some("py") {
            let src = fs::read_to_string(source_dir).map_err(|e| io_err(source_dir, &e))?;
            validate_python_syntax(&src)
                .map_err(|e| format!("{}: {e}", source_dir.display()))?;
            let id = source_dir
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("script")
                .to_string();
            let sha = sha256_hex(src.as_bytes());
            return Ok(vec![BuiltModArtifact {
                mod_id: id,
                output_path: source_dir.to_path_buf(),
                byte_len: src.len(),
                sha256: sha,
            }]);
        }
        return Err(format!(
            "unsupported mod source file `{}` (expected a directory or .py script)",
            source_dir.display()
        ));
    }

    if !source_dir.is_dir() {
        return Err(format!(
            "mod directory `{}` does not exist",
            source_dir.display()
        ));
    }

    let mut mod_dirs = Vec::new();
    let mut standalone_py = Vec::new();
    collect_mod_targets(source_dir, &mut mod_dirs, &mut standalone_py)?;
    mod_dirs.sort();
    standalone_py.sort();

    let mut artifacts = Vec::new();
    for dir in mod_dirs {
        artifacts.push(build_and_pack_mod_dir(&dir, out_dir, repo_root)?);
    }
    for py_path in standalone_py {
        let src = fs::read_to_string(&py_path).map_err(|e| io_err(&py_path, &e))?;
        validate_python_syntax(&src).map_err(|e| format!("{}: {e}", py_path.display()))?;
        let id = py_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("script")
            .to_string();
        artifacts.push(BuiltModArtifact {
            mod_id: id,
            output_path: py_path,
            byte_len: src.len(),
            sha256: sha256_hex(src.as_bytes()),
        });
    }

    Ok(artifacts)
}

fn collect_mod_targets(
    dir: &Path,
    mod_dirs: &mut Vec<PathBuf>,
    standalone_py: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if dir.join("mod.toml").is_file() {
        mod_dirs.push(dir.to_path_buf());
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| io_err(dir, &e))?;
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            collect_mod_targets(&path, mod_dirs, standalone_py)?;
        } else if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("py") {
            standalone_py.push(path);
        }
    }
    Ok(())
}

fn build_and_pack_mod_dir(
    dir: &Path,
    out_dir: &Path,
    repo_root: &Path,
) -> Result<BuiltModArtifact, String> {
    let manifest_path = dir.join("mod.toml");
    let toml_src = fs::read_to_string(&manifest_path).map_err(|e| io_err(&manifest_path, &e))?;
    let mut manifest = ModManifest::parse_toml(&toml_src)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let is_python = manifest
        .entry_server
        .as_deref()
        .is_some_and(|e| e.ends_with(".py"))
        || (dir.join("main.py").is_file()
            && !dir.join("Cargo.toml").is_file()
            && !dir.join("go.mod").is_file()
            && !dir.join("main.cpp").is_file());

    if is_python {
        let entry_rel = manifest
            .entry_server
            .clone()
            .unwrap_or_else(|| "main.py".to_string());
        let py_path = dir.join(&entry_rel);
        let py_src = fs::read_to_string(&py_path).map_err(|e| io_err(&py_path, &e))?;
        validate_python_syntax(&py_src)
            .map_err(|e| format!("{}: {e}", py_path.display()))?;
        manifest.entry_server = Some(entry_rel.clone());
        entries.push(("mod.toml".to_string(), manifest.to_toml().into_bytes()));
        entries.push((entry_rel, py_src.into_bytes()));
    } else {
        if let Some(server_rel) = manifest.entry_server.clone() {
            let wasm_bytes = build_or_resolve_wasm(dir, &server_rel, repo_root)?;
            manifest.entry_server = Some("server.wasm".to_string());
            entries.push(("server.wasm".to_string(), wasm_bytes));
        }
        if let Some(client_rel) = manifest.entry_client.clone() {
            let wasm_bytes = build_or_resolve_wasm(dir, &client_rel, repo_root)?;
            manifest.entry_client = Some("client.wasm".to_string());
            entries.push(("client.wasm".to_string(), wasm_bytes));
        }
        entries.insert(0, ("mod.toml".to_string(), manifest.to_toml().into_bytes()));
    }

    let lang_path = dir.join("lang.toml");
    if lang_path.is_file() {
        let bytes = fs::read(&lang_path).map_err(|e| io_err(&lang_path, &e))?;
        entries.push(("lang.toml".to_string(), bytes));
    }

    let mut assets = BTreeMap::new();
    for rel in &manifest.assets {
        let cand_a = dir.join("assets").join(rel);
        let cand_b = dir.join(rel);
        let p = if cand_a.is_file() { cand_a } else { cand_b };
        let bytes = fs::read(&p).map_err(|e| io_err(&p, &e))?;
        assets.insert(rel.clone(), bytes);
    }
    let assets_dir = dir.join("assets");
    if assets_dir.is_dir() {
        collect_assets_recursive(&assets_dir, &assets_dir, &mut assets)?;
    }
    for (rel, data) in assets {
        entries.push((format!("assets/{rel}"), data));
    }

    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    let zip = CsModArchive::write_zip(refs)
        .map_err(|e| format!("package {}: {e}", manifest.id))?;
    let sha = sha256_hex(&zip);
    let dst = out_dir.join(format!("{}.csmod", manifest.id));
    fs::write(&dst, &zip).map_err(|e| io_err(&dst, &e))?;

    Ok(BuiltModArtifact {
        mod_id: manifest.id,
        output_path: dst,
        byte_len: zip.len(),
        sha256: sha,
    })
}

fn build_or_resolve_wasm(
    dir: &Path,
    entry_rel: &str,
    repo_root: &Path,
) -> Result<Vec<u8>, String> {
    let entry_path = dir.join(entry_rel);

    // 1. Rust crate (`Cargo.toml` or `client/Cargo.toml` for side="both" client HUD modules)
    if entry_rel == "client.wasm" && dir.join("client").join("Cargo.toml").is_file() {
        return build_rust_wasm(&dir.join("client"), &entry_path, repo_root);
    }
    if dir.join("Cargo.toml").is_file() && entry_rel != "client.wasm" {
        return build_rust_wasm(dir, &entry_path, repo_root);
    }

    // 2. Go module (`go.mod`)
    if dir.join("go.mod").is_file() && entry_rel != "client.wasm" {
        return build_go_wasm(dir, &entry_path);
    }

    // 3. C/C++ source (`main.cpp`, `src/main.cpp`, `main.c`)
    let cpp_candidate = [
        dir.join("main.cpp"),
        dir.join("src").join("main.cpp"),
        dir.join("main.c"),
    ]
    .into_iter()
    .find(|p| p.is_file());
    if let Some(src_path) = cpp_candidate
        && entry_rel != "client.wasm"
    {
        return build_cpp_wasm(dir, &src_path, &entry_path, repo_root);
    }

    // 4. Existing `.wasm` or `.wat` (legacy compatibility)
    if entry_path.is_file() {
        let bytes = fs::read(&entry_path).map_err(|e| io_err(&entry_path, &e))?;
        if entry_rel.ends_with(".wat") || bytes.trim_ascii_start().starts_with(b"(") {
            let text = std::str::from_utf8(&bytes)
                .map_err(|e| format!("{}: {e}", entry_path.display()))?;
            let wasm = cs_mod_host::compile_wat_to_wasm(text)
                .map_err(|e| format!("{}: {e}", entry_path.display()))?;
            verify_wasm_bytes(&entry_path, &wasm)?;
            return Ok(wasm);
        }
        verify_wasm_bytes(&entry_path, &bytes)?;
        return Ok(bytes);
    }
    if let Some(stem) = entry_rel.strip_suffix(".wasm") {
        let wat_path = dir.join(format!("{stem}.wat"));
        if wat_path.is_file() {
            let text = fs::read_to_string(&wat_path).map_err(|e| io_err(&wat_path, &e))?;
            let wasm = cs_mod_host::compile_wat_to_wasm(&text)
                .map_err(|e| format!("{}: {e}", wat_path.display()))?;
            verify_wasm_bytes(&wat_path, &wasm)?;
            return Ok(wasm);
        }
    }

    Err(format!(
        "missing entry module `{}` in `{}`",
        entry_rel,
        dir.display()
    ))
}

fn build_rust_wasm(dir: &Path, out_wasm: &Path, repo_root: &Path) -> Result<Vec<u8>, String> {
    let manifest_path = dir.join("Cargo.toml");
    let pkg_name = parse_cargo_package_name(&manifest_path)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("target").join("mods-wasm"));

    let status = Command::new(cargo)
        .arg("build")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .map_err(|e| format!("failed to invoke cargo for `{}`: {e}", dir.display()))?;
    if !status.success() {
        return Err(format!(
            "cargo build --target wasm32-unknown-unknown failed in `{}`",
            dir.display()
        ));
    }

    let lib_file = format!("{}.wasm", pkg_name.replace('-', "_"));
    let built_path = target_dir
        .join("wasm32-unknown-unknown")
        .join("debug")
        .join(lib_file);
    let bytes = fs::read(&built_path).map_err(|e| io_err(&built_path, &e))?;
    verify_wasm_bytes(&built_path, &bytes)?;
    let _ = fs::write(out_wasm, &bytes);
    Ok(bytes)
}

fn build_go_wasm(dir: &Path, out_wasm: &Path) -> Result<Vec<u8>, String> {
    let tmp_name = ".cs16_go_tmp.wasm";
    let tmp_path = dir.join(tmp_name);
    let _ = fs::remove_file(&tmp_path);

    let go_res = Command::new("go")
        .current_dir(dir)
        .env("GOOS", "wasip1")
        .env("GOARCH", "wasm")
        .args(["build", "-buildmode=c-shared", "-o", tmp_name, "."])
        .status();

    match go_res {
        Ok(st) if st.success() => {
            let bytes = fs::read(&tmp_path).map_err(|e| io_err(&tmp_path, &e));
            let _ = fs::remove_file(&tmp_path);
            let bytes = bytes?;
            verify_wasm_bytes(&tmp_path, &bytes)?;
            if fs::read(out_wasm).ok().as_deref() != Some(bytes.as_slice()) {
                let _ = fs::write(out_wasm, &bytes);
            }
            return Ok(bytes);
        }
        Ok(st) => {
            let _ = fs::remove_file(&tmp_path);
            if out_wasm.is_file()
                && let Ok(bytes) = fs::read(out_wasm)
                && verify_wasm_bytes(out_wasm, &bytes).is_ok()
            {
                eprintln!(
                    "warning: go build (GOOS=wasip1 GOARCH=wasm) exited with {st} in `{}`; using verified prebuilt `{}`",
                    dir.display(),
                    out_wasm.display()
                );
                return Ok(bytes);
            }
            return Err(format!(
                "go build (GOOS=wasip1 GOARCH=wasm) failed in `{}` with {st} and no valid prebuilt `{}` present",
                dir.display(),
                out_wasm.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = fs::remove_file(&tmp_path);
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("failed to execute `go`: {e}"));
        }
    }

    // Fallback: verified prebuilt `.wasm` when Go toolchain is not installed.
    if out_wasm.is_file() {
        let bytes = fs::read(out_wasm).map_err(|e| io_err(out_wasm, &e))?;
        verify_wasm_bytes(out_wasm, &bytes)?;
        return Ok(bytes);
    }

    Err(format!(
        "`go` toolchain not found and no prebuilt `{}` present in `{}`",
        out_wasm.display(),
        dir.display()
    ))
}

fn build_cpp_wasm(
    dir: &Path,
    src_path: &Path,
    out_wasm: &Path,
    repo_root: &Path,
) -> Result<Vec<u8>, String> {
    let is_c = src_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("c"));
    let compiler = if is_c { "clang" } else { "clang++" };
    let std_flag = if is_c { "-std=c99" } else { "-std=c++20" };

    let raw_include_dir = repo_root.join("sdk").join("cpp");
    let include_dir = if raw_include_dir.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(&raw_include_dir))
            .unwrap_or(raw_include_dir)
    } else {
        raw_include_dir
    };
    let src_rel = src_path.strip_prefix(dir).unwrap_or(src_path);
    let tmp_name = ".cs16_cpp_tmp.wasm";
    let tmp_path = dir.join(tmp_name);
    let _ = fs::remove_file(&tmp_path);

    let mut cmd = Command::new(compiler);
    cmd.current_dir(dir)
        .args([
            "--target=wasm32",
            std_flag,
            "-O2",
            "-nostdlib",
            "-Wl,--no-entry",
            "-Wl,--export-dynamic",
        ]);
    if !is_c {
        cmd.arg("-fno-exceptions");
    }
    cmd.arg("-I")
        .arg(&include_dir)
        .arg("-o")
        .arg(tmp_name)
        .arg(src_rel);

    match cmd.status() {
        Ok(st) if st.success() => {
            let bytes = fs::read(&tmp_path).map_err(|e| io_err(&tmp_path, &e));
            let _ = fs::remove_file(&tmp_path);
            let bytes = bytes?;
            verify_wasm_bytes(&tmp_path, &bytes)?;
            if fs::read(out_wasm).ok().as_deref() != Some(bytes.as_slice()) {
                let _ = fs::write(out_wasm, &bytes);
            }
            return Ok(bytes);
        }
        Ok(st) => {
            let _ = fs::remove_file(&tmp_path);
            if out_wasm.is_file()
                && let Ok(bytes) = fs::read(out_wasm)
                && verify_wasm_bytes(out_wasm, &bytes).is_ok()
            {
                eprintln!(
                    "warning: {compiler} exited with {st} building `{}`; using verified prebuilt `{}`",
                    src_path.display(),
                    out_wasm.display()
                );
                return Ok(bytes);
            }
            return Err(format!(
                "{compiler} failed building `{}` with {st} and no valid prebuilt `{}` present",
                src_path.display(),
                out_wasm.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = fs::remove_file(&tmp_path);
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("failed to invoke `{compiler}`: {e}"));
        }
    }

    // Fallback: verified prebuilt `.wasm` when Clang is not installed.
    if out_wasm.is_file() {
        let bytes = fs::read(out_wasm).map_err(|e| io_err(out_wasm, &e))?;
        verify_wasm_bytes(out_wasm, &bytes)?;
        return Ok(bytes);
    }

    Err(format!(
        "`{compiler}` not found and no prebuilt `{}` present in `{}`",
        out_wasm.display(),
        dir.display()
    ))
}

fn verify_wasm_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 8 || &bytes[0..4] != b"\0asm" {
        return Err(format!(
            "`{}` is not a valid WebAssembly binary (missing \\0asm header)",
            path.display()
        ));
    }
    Ok(())
}

/// Pure-Rust syntax + dry-load check for zero-compile Python mod scripts via `cs-mod-py`.
pub fn validate_python_syntax(src: &str) -> Result<(), String> {
    if src.trim().is_empty() {
        return Err("Python script is empty".to_string());
    }
    cs_mod_py::PySourceSyntaxCheck::check("<script>.py", src)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn parse_cargo_package_name(manifest_path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(manifest_path).map_err(|e| io_err(manifest_path, &e))?;
    let parsed: toml::Table = text
        .parse()
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    parsed
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{}: missing [package].name", manifest_path.display()))
}

fn collect_assets_recursive(
    base: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| io_err(dir, &e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_assets_recursive(base, &path, out)?;
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(base)
        {
            let norm = rel.to_string_lossy().replace('\\', "/");
            if let std::collections::btree_map::Entry::Vacant(slot) = out.entry(norm) {
                let data = fs::read(&path).map_err(|e| io_err(&path, &e))?;
                slot.insert(data);
            }
        }
    }
    Ok(())
}

fn title_case_id(id: &str) -> String {
    id.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn io_err(path: &Path, e: &std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

const FALLBACK_CS16_PYI: &str = r#"# PEP 484 type stubs for cs16-rs zero-compile Python mods
from typing import Any, Callable, Optional, Tuple

class HookAction:
    CONTINUE: int
    HANDLED: int
    OVERRIDE: int
    SUPERCEDE: int

class Color:
    DEFAULT: str
    TEAM: str
    GREEN: str
    WHITE: Tuple[int, int, int, int]
    RED: Tuple[int, int, int, int]
    CYAN: Tuple[int, int, int, int]
    AMBER: Tuple[int, int, int, int]

class Player:
    slot: int
    name: str
    health: float
    armor: int
    money: int
    team: int
    alive: bool
    origin: Tuple[float, float, float]
    def tr(self, key: str, **kwargs: Any) -> str: ...
    def say(self, text: str) -> None: ...
    def give_item(self, item: str) -> None: ...
    def has_flag(self, flag: str) -> bool: ...

class _VfxApi:
    def shockwave_ring(self, origin: Tuple[float, float, float], radius: float, rgba: Tuple[int, int, int, int], life: int = 8, width: int = 10, slot: Optional[int] = None) -> None: ...
    def laser_beam(self, start: Tuple[float, float, float], end: Tuple[float, float, float], rgba: Tuple[int, int, int, int], life: int = 5, width: int = 4, noise: int = 0, slot: Optional[int] = None) -> None: ...
    def aura_light(self, origin: Tuple[float, float, float], radius: int, rgb: Tuple[int, int, int], life: int = 10, decay: int = 5, slot: Optional[int] = None) -> None: ...

class _ModApi:
    vfx: _VfxApi
    def info(self, id: str, name: str = "", version: str = "0.1.0") -> Callable[..., Any]: ...
    def hook(self, name: str, phase: str = "pre", priority: int = 0) -> Callable[..., Any]: ...
    def chat_command(self, *triggers: str) -> Callable[..., Any]: ...
    def console_command(self, *names: str) -> Callable[..., Any]: ...
    def timer(self, interval_ms: int) -> Callable[..., Any]: ...

mod: _ModApi
vfx: _VfxApi
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "cs16_xtask_mod_cli_{tag}_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn scaffold_generates_valid_manifests_and_ide_configs_for_all_languages() {
        let tmp = temp_test_dir("scaffold");
        let root = workspace_root();

        for (lang, expected_ide_file) in [
            (ModLang::Rust, "Cargo.toml"),
            (ModLang::Cpp, "compile_commands.json"),
            (ModLang::Go, "go.mod"),
            (ModLang::Python, "cs16.pyi"),
        ] {
            let id = format!("demo_{}", lang.as_str());
            let dir = tmp.join(&id);
            let files =
                scaffold_project(&dir, &id, "Demo Mod", lang, ModSide::Server, &root).unwrap();
            assert!(!files.is_empty());

            let manifest_raw = fs::read_to_string(dir.join("mod.toml")).unwrap();
            let parsed = ModManifest::parse_toml(&manifest_raw).unwrap();
            assert_eq!(parsed.id, id);
            assert_eq!(parsed.api_version, ABI_VERSION);

            let lang_raw = fs::read_to_string(dir.join("lang.toml")).unwrap();
            assert!(lang_raw.contains("[en]") && lang_raw.contains("[ru]"));
            assert!(dir.join(expected_ide_file).is_file());
        }

        // Rejects unsafe IDs
        assert!(
            scaffold_project(&tmp.join("bad"), "../escape", "Bad", ModLang::Rust, ModSide::Server, &root)
                .is_err()
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn build_validates_python_and_packs_csmod_and_cpp_prebuilt_fallback() {
        let tmp = temp_test_dir("build");
        let out = tmp.join("dist");
        let root = workspace_root();

        // 1. Python mod builds cleanly into .csmod
        let py_dir = tmp.join("py_demo");
        scaffold_project(&py_dir, "py_demo", "Py Demo", ModLang::Python, ModSide::Server, &root)
            .unwrap();
        let py_built = build_mods_in(&py_dir, &out, &root).unwrap();
        assert_eq!(py_built.len(), 1);
        assert!(out.join("py_demo.csmod").is_file());
        let zip_bytes = fs::read(out.join("py_demo.csmod")).unwrap();
        let entries = CsModArchive::read_zip(&zip_bytes).unwrap();
        assert!(entries.contains_key("mod.toml"));
        assert!(entries.contains_key("main.py"));
        assert!(entries.contains_key("lang.toml"));

        // Broken Python syntax fails cleanly
        assert!(validate_python_syntax("def f(x:\n    return x\n").is_err());
        assert!(validate_python_syntax("s = \"unclosed\n").is_err());

        // 2. C++ mod with verified prebuilt `.wasm` packs cleanly even when clang++ is absent
        let cpp_dir = tmp.join("cpp_demo");
        scaffold_project(&cpp_dir, "cpp_demo", "Cpp Demo", ModLang::Cpp, ModSide::Server, &root)
            .unwrap();
        // Minimal valid empty WASM binary header (`\0asm` v1)
        let minimal_wasm = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        fs::write(cpp_dir.join("server.wasm"), minimal_wasm).unwrap();
        let cpp_built = build_mods_in(&cpp_dir, &out, &root).unwrap();
        assert_eq!(cpp_built.len(), 1);
        assert!(out.join("cpp_demo.csmod").is_file());

        // Corrupt non-WASM binary is rejected deterministically (remove main.cpp so hosts
        // with a working clang++ --target=wasm32 don't overwrite the corrupt prebuilt file).
        let _ = fs::remove_file(cpp_dir.join("main.cpp"));
        fs::write(cpp_dir.join("server.wasm"), b"not wasm").unwrap();
        assert!(build_mods_in(&cpp_dir, &out, &root).is_err());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn build_cpp_mod_from_relative_path_does_not_double_paths() {
        let tmp = temp_test_dir("cpp_rel");
        let root = workspace_root();
        let rel_semiclip = if Path::new("mods/gamemodes/semiclip").is_dir() {
            Path::new("mods/gamemodes/semiclip")
        } else {
            Path::new("../mods/gamemodes/semiclip")
        };
        assert!(rel_semiclip.is_relative());
        assert!(rel_semiclip.is_dir());

        let built = build_mods_in(rel_semiclip, &tmp, &root).unwrap();
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].mod_id, "semiclip");
        assert!(tmp.join("semiclip.csmod").is_file());
        assert!(!rel_semiclip.join(".cs16_cpp_tmp.wasm").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn build_packs_existing_repo_mods_via_compat_path() {
        let tmp = temp_test_dir("repo_mods");
        let root = workspace_root();
        let built = build_mods_in(&root.join("mods"), &tmp, &root).unwrap();
        assert!(!built.is_empty());
        for item in &built {
            assert!(item.output_path.is_file());
            assert!(item.byte_len > 0);
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn replay_dump_file_reproduces_and_verifies_fix_for_python_plugin() {
        use cs_mod_api::{HookId, HookPhase};
        use cs_mod_host::crash_dump::{
            DEFAULT_RNG_SEED, FAULT_DUMP_SCHEMA_VERSION, InvocationTrigger, ModFaultDump,
            PluginLanguage,
        };
        use cs_mod_host::natives::{ActiveEvent, ClientSnap, EngineSnapshot};

        let tmp = temp_test_dir("py_replay");
        let buggy_py = tmp.join("buggy.py");
        let fixed_py = tmp.join("fixed.py");
        fs::write(
            &buggy_py,
            r#"from cs16 import mod, HookAction
@mod.hook("player_take_damage", phase="pre")
def on_dmg(ev):
    ratio = ev.damage / 0
    return HookAction.CONTINUE
"#,
        )
        .unwrap();
        fs::write(
            &fixed_py,
            r#"from cs16 import mod, HookAction
@mod.hook("player_take_damage", phase="pre")
def on_dmg(ev):
    ev.damage = ev.damage * 0.5
    return HookAction.OVERRIDE
"#,
        )
        .unwrap();

        let mut snap = EngineSnapshot {
            time_secs: 42.0,
            max_clients: 4,
            map_name: "de_dust2".to_string(),
            ..Default::default()
        };
        snap.clients.push(ClientSnap {
            slot: 0,
            active: true,
            name: "Victim".to_string(),
            alive: true,
            health: 100.0,
            ..Default::default()
        });
        let ev = ActiveEvent {
            i32_args: [0, 1, 2, 28, 0, 0],
            f32_args: [50.0, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let dump = ModFaultDump {
            schema_version: FAULT_DUMP_SCHEMA_VERSION,
            dump_id: "test_py_div0".to_string(),
            timestamp_unix_ms: 1_700_000_000_000,
            mod_id: "buggy".to_string(),
            version: "1.0.0".to_string(),
            language: PluginLanguage::Python,
            binary_sha256: "00".to_string(),
            source_path: Some(buggy_py.to_string_lossy().into_owned()),
            permissions_mask: cs_mod_py::default_python_permissions().bits(),
            http_hosts: Vec::new(),
            max_memory_bytes: 4 * 1024 * 1024,
            fuel_budget: 500_000,
            fuel_consumed_before_trap: 120,
            rng_seed: DEFAULT_RNG_SEED,
            trigger: InvocationTrigger::Hook {
                hook: HookId::PlayerTakeDamage,
                phase: HookPhase::Pre,
            },
            event: ev,
            snapshot: snap,
            storage_cache: BTreeMap::new(),
            error: "ZeroDivisionError: division by zero".to_string(),
            stack_trace: vec!["buggy.py:4".to_string()],
            truncated: false,
        };
        let dump_path = tmp.join("dump.json");
        fs::write(&dump_path, dump.to_bounded_json().1).unwrap();

        let rep_buggy = replay_dump_file(&dump_path, None).unwrap();
        assert!(rep_buggy.outcome.is_fault(), "expected fault on buggy.py: {rep_buggy}");

        let rep_fixed = replay_dump_file(&dump_path, Some(&fixed_py)).unwrap();
        assert!(rep_fixed.outcome.is_pass(), "expected pass on fixed.py: {rep_fixed}");
        assert_eq!(rep_fixed.override_f32, Some(25.0));

        let _ = fs::remove_dir_all(&tmp);
    }
}
