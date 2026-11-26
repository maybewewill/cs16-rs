//! Dedicated headless 100 Hz Counter-Strike 1.6 Rust server (`cs-server`).
//!
//! It is the Protocol 48 server for the original client and for `cs-client` (see
//! `proto48.rs`): `cargo run -p cs-server -- --map de_dust2` runs the game of
//! `cs-game`; add `--walk-game` for the older walking-only `WalkGame` of `cs-sim`. The
//! `--proto48` flag of the earlier two-mode server is still accepted and does nothing.

mod admin_http;
mod console_io;
mod proto48;
mod replays;
mod voice_bridge;

use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    // Run on an 8 MiB stack thread (matching Linux's default main stack) so unoptimized
    // Windows dev builds loading unified WASM/Python/AMXX mod hosts do not overflow the
    // OS's 1 MiB default main-thread stack.
    let handle = std::thread::Builder::new()
        .name("cs-server-main".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || proto48::run(&args))
        .expect("spawn cs-server main thread");
    if handle.join().is_err() {
        std::process::exit(1);
    }
}
