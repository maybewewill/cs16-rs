use std::path::PathBuf;
use std::process::ExitCode;

use xtask::parity;

const USAGE: &str = "usage: cargo xtask parity [--dir <path>] [--min <pct>]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("parity") => match run_parity(args) {
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

fn run_parity(mut args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let mut dir = PathBuf::from("docs/parity");
    let mut min: Option<f64> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = args.next().ok_or("--dir needs a value")?.into(),
            "--min" => {
                let value = args.next().ok_or("--min needs a value")?;
                min = Some(
                    value
                        .parse()
                        .map_err(|_| format!("--min: not a number: {value}"))?,
                );
            }
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
        "TOTAL verified: {:.1}%  implemented: {:.1}%",
        report.verified_pct, report.implemented_pct
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
