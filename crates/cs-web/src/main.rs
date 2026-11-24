//! Binary entrypoint for `cs-web` (`wasm32-unknown-unknown` attaches `window.cs16Web`).

#[cfg(target_arch = "wasm32")]
fn main() {
    cs_web::wasm_entry::install();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    println!("cs-web targets wasm32-unknown-unknown. Build the web bundle with: cargo xtask web-build");
}
