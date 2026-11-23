use std::path::{Path, PathBuf};
use std::process::ExitCode;

use xtask::parity;

const USAGE: &str = "usage: cargo xtask parity [--dir <path>] [--min <pct>]\n       \
                     cargo xtask parity import-names <category> <file> [--dir <path>]\n       \
                     cargo xtask golden <ip:port> <name>";

const DEFAULT_DIR: &str = "docs/parity";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("parity") => {
            let mut args = args.peekable();
            let result = if args.peek().map(String::as_str) == Some("import-names") {
                args.next();
                run_import_names(args)
            } else {
                run_parity(args)
            };
            match result {
                Ok(code) => code,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    ExitCode::from(2)
                }
            }
        }
        Some("golden") => match run_golden(args) {
            Ok(code) => code,
            Err(msg) => {
                eprintln!("error: {msg}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Records a golden capture: runs `cs-probe` against `<ip:port>` for 30 s, walking, and writes
/// `$CS16_GOLDEN_DIR/<name>.cs48cap`.
fn run_golden(args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let positional: Vec<String> = args.collect();
    let [server, name] = positional.as_slice() else {
        return Err(format!("golden needs <ip:port> <name>\n{USAGE}"));
    };
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(format!("invalid capture name `{name}`"));
    }
    let dir = std::env::var_os("CS16_GOLDEN_DIR").ok_or(
        "CS16_GOLDEN_DIR is not set; point it at the directory that holds golden captures",
    )?;
    let capture = PathBuf::from(dir).join(format!("{name}.cs48cap"));

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = std::process::Command::new(cargo)
        .args([
            "run",
            "--release",
            "-p",
            "cs-probe",
            "--",
            "--connect",
            server,
        ])
        .arg("--capture")
        .arg(&capture)
        .args(["--seconds", "30", "--walk"])
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() {
        println!("capture written to {}", capture.display());
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

fn run_parity(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let mut dir = PathBuf::from(DEFAULT_DIR);
    let mut min: Option<f64> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = args.next().ok_or("--dir needs a value")?.into(),
            "--min" => min = Some(parse_min(&args.next().ok_or("--min needs a value")?)?),
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }

    let (categories, weights) = parity::load_dir(&dir).map_err(|e| e.to_string())?;
    let report = parity::score(&categories, &weights);

    for c in &report.categories {
        println!(
            "{:<20} weight {:>3}  total {:>4}  implemented {:>4}  tested {:>4}  verified {:>4}",
            c.name, c.weight, c.total, c.implemented, c.tested, c.verified
        );
    }
    println!(
        "TOTAL verified: {:.1}%  tested: {:.1}%  implemented: {:.1}%",
        report.verified_pct, report.tested_pct, report.implemented_pct
    );

    match min {
        Some(min) if report.verified_pct < min => {
            eprintln!(
                "verified {:.1}% is below the minimum {min:.1}%",
                report.verified_pct
            );
            Ok(ExitCode::FAILURE)
        }
        _ => Ok(ExitCode::SUCCESS),
    }
}

/// Parses the `--min` percentage; non-finite values (`nan`, `inf`) would make
/// the gate meaningless and are rejected.
fn parse_min(value: &str) -> Result<f64, String> {
    match value.parse::<f64>() {
        Ok(min) if min.is_finite() => Ok(min),
        _ => Err(format!("--min: not a finite number: {value}\n{USAGE}")),
    }
}

fn run_import_names(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let mut dir = PathBuf::from(DEFAULT_DIR);
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = args.next().ok_or("--dir needs a value")?.into(),
            _ => positional.push(arg),
        }
    }
    let [category, dump_file] = positional.as_slice() else {
        return Err(format!("import-names needs <category> <file>\n{USAGE}"));
    };
    if category.is_empty() || category.contains(['/', '\\', '.']) {
        return Err(format!("invalid category name `{category}`"));
    }

    let registry_path = dir.join(format!("{category}.toml"));
    let registry_text = match std::fs::read_to_string(&registry_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io_error(&registry_path, &e)),
    };
    let existing = parity::parse_category(category, &registry_text).map_err(|e| e.to_string())?;
    let before = existing.items.len();

    let dump_text =
        std::fs::read_to_string(dump_file).map_err(|e| io_error(Path::new(dump_file), &e))?;
    let updated = parity::import_names(existing, &dump_text);
    std::fs::write(&registry_path, parity::category_to_toml(&updated))
        .map_err(|e| io_error(&registry_path, &e))?;

    println!(
        "{}: {} items ({} added)",
        registry_path.display(),
        updated.items.len(),
        updated.items.len() - before
    );
    Ok(ExitCode::SUCCESS)
}

fn io_error(path: &Path, e: &std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

#[cfg(test)]
mod tests {
    use super::parse_min;

    #[test]
    fn min_rejects_non_finite_values() {
        for value in ["nan", "NaN", "inf", "-inf", "infinity"] {
            assert!(parse_min(value).is_err(), "{value} accepted");
        }
        assert!(parse_min("abc").is_err());
        assert_eq!(parse_min("12.5"), Ok(12.5));
    }
}
