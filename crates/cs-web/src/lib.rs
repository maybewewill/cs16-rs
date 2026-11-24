//! Browser (`wasm32-unknown-unknown`) client runtime and JS/DOM bridge for `cs16-rs`.
//!
//! All `#![forbid(unsafe_code)]`: input mapping and session/VFS/audio orchestration live in
//! [`input_web`] and [`state`] so they test natively without a browser, while [`wasm_entry`]
//! binds them to `window.cs16Web`, WebGPU/WebGL2 `wgpu::Surface`, and DOM events on `wasm32`.

pub mod input_web;
pub mod state;

#[cfg(target_arch = "wasm32")]
pub mod wasm_entry;
