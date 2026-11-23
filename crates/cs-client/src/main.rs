//! Counter-Strike 1.6 (Rust) desktop client entry point.

mod app;
mod audio;
mod camera;
mod hudtext;
mod input;
mod render;
mod session;
mod ui;
mod view;

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;
use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::{Cs16App, LaunchConfig};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,wgpu=warn,wgpu_hal=error,naga=warn")),
        )
        .init();

    let launch = parse_args();
    let event_loop = EventLoop::new().expect("create winit event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = Cs16App::new(launch);
    event_loop.run_app(&mut app).expect("run cs16-rs event loop");
}

fn parse_args() -> LaunchConfig {
    let mut args = std::env::args().skip(1);
    let mut gamedir = None;
    let mut startup_map = None;
    let mut open_console = false;
    let mut startup_exec = Vec::new();
    let mut auto_screenshot = None;
    let mut startup_ui = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--gamedir" | "-g" => {
                gamedir = args.next().map(PathBuf::from);
            }
            "--map" | "-m" => {
                startup_map = args.next();
            }
            "--console" | "-console" => {
                open_console = true;
            }
            "--exec" | "-e" => {
                if let Some(cmd) = args.next() {
                    startup_exec.push(cmd);
                }
            }
            "--screenshot" => {
                auto_screenshot = args.next().map(PathBuf::from);
            }
            "--ui" => {
                startup_ui = args.next();
            }
            _ => {}
        }
    }

    LaunchConfig {
        gamedir,
        startup_map,
        open_console,
        startup_exec,
        auto_screenshot,
        startup_ui,
    }
}
