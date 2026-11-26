//! Counter-Strike 1.6 (Rust) desktop client entry point.

mod app;
mod session;

use cs_client::audio;

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
    if launch.headless {
        if let Err(err) = Cs16App::new(launch).run_headless() {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
        return;
    }
    let event_loop = EventLoop::new().expect("create winit event loop");
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = Cs16App::new(launch);
    event_loop
        .run_app(&mut app)
        .expect("run cs16-rs event loop");
}

fn parse_args() -> LaunchConfig {
    parse_args_from(std::env::args().skip(1))
}

pub(crate) fn parse_args_from<I: IntoIterator<Item = String>>(iter: I) -> LaunchConfig {
    let args: Vec<String> = iter.into_iter().collect();
    let mut cfg = LaunchConfig::default();
    let mut i = 0usize;

    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--gamedir" | "-g" => {
                if let Some(val) = args.get(i + 1) {
                    cfg.gamedir = Some(PathBuf::from(val));
                    i += 1;
                }
            }
            "--map" | "-m" => {
                if let Some(val) = args.get(i + 1) {
                    cfg.startup_map = Some(val.clone());
                    i += 1;
                }
            }
            "--console" | "-console" => {
                cfg.open_console = true;
            }
            "--headless" => {
                cfg.headless = true;
            }
            "--demo" => {
                cfg.demo = true;
            }
            "--exec" | "-e" => {
                if let Some(first) = args.get(i + 1) {
                    let mut parts = vec![first.as_str()];
                    i += 1;
                    while let Some(next) = args.get(i + 1) {
                        if next.starts_with('-') || next.starts_with('+') {
                            break;
                        }
                        parts.push(next.as_str());
                        i += 1;
                    }
                    cfg.startup_exec.push(parts.join(" "));
                }
            }
            "--screenshot" => {
                if let Some(val) = args.get(i + 1) {
                    cfg.auto_screenshot = Some(PathBuf::from(val));
                    i += 1;
                }
            }
            "--ui" => {
                if let Some(val) = args.get(i + 1) {
                    cfg.startup_ui = Some(val.clone());
                    i += 1;
                }
            }
            "--resolution" | "-res" => {
                if let Some(val) = args.get(i + 1) {
                    if let Some((w, h)) = parse_resolution(val) {
                        cfg.width = w;
                        cfg.height = h;
                    }
                    i += 1;
                }
            }
            "-w" | "--width" => {
                if let Some(val) = args.get(i + 1)
                    && let Ok(w) = val.parse::<u32>()
                    && w > 0
                {
                    cfg.width = w;
                    i += 1;
                }
            }
            "-h" | "--height" => {
                if let Some(val) = args.get(i + 1)
                    && let Ok(h) = val.parse::<u32>()
                    && h > 0
                {
                    cfg.height = h;
                    i += 1;
                }
            }
            "--wait-camera" => {
                if let Some(val) = args.get(i + 1)
                    && let Ok(cam) = val.parse::<u32>()
                {
                    cfg.wait_camera = cam;
                    i += 1;
                }
            }
            "--wait-seconds" => {
                if let Some(val) = args.get(i + 1)
                    && let Ok(secs) = val.parse::<f32>()
                    && secs.is_finite()
                {
                    cfg.wait_seconds = Some(secs.max(0.0));
                    i += 1;
                }
            }
            "--record-dir" => {
                if let Some(val) = args.get(i + 1) {
                    cfg.record_dir = Some(PathBuf::from(val));
                    i += 1;
                }
            }
            "--record-frames" => {
                if let Some(val) = args.get(i + 1)
                    && let Ok(frames) = val.parse::<u32>()
                {
                    cfg.record_frames = frames;
                    i += 1;
                }
            }
            "--record-fps" => {
                if let Some(val) = args.get(i + 1)
                    && let Ok(fps) = val.parse::<f32>()
                    && fps.is_finite()
                    && fps > 0.0
                {
                    cfg.record_fps = fps.clamp(1.0, 120.0);
                    i += 1;
                }
            }
            plus if plus.starts_with('+') && plus.len() > 1 => {
                let mut parts = vec![&plus[1..]];
                while let Some(next) = args.get(i + 1) {
                    if next.starts_with('-') || next.starts_with('+') {
                        break;
                    }
                    parts.push(next.as_str());
                    i += 1;
                }
                cfg.startup_exec.push(parts.join(" "));
            }
            _ => {}
        }
        i += 1;
    }

    cfg
}

fn parse_resolution(spec: &str) -> Option<(u32, u32)> {
    let lower = spec.to_ascii_lowercase();
    let (w, h) = lower.split_once('x')?;
    let w = w.trim().parse::<u32>().ok().filter(|&v| v > 0)?;
    let h = h.trim().parse::<u32>().ok().filter(|&v| v > 0)?;
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_handles_exec_plus_connect_headless_and_resolution() {
        let cfg = parse_args_from([
            "--headless".to_string(),
            "--resolution".to_string(),
            "1280x720".to_string(),
            "--exec".to_string(),
            "connect 127.0.0.1:27015".to_string(),
            "+brightness".to_string(),
            "1.0".to_string(),
            "--wait-camera".to_string(),
            "2".to_string(),
            "--wait-seconds".to_string(),
            "1.5".to_string(),
            "--screenshot".to_string(),
            "out.png".to_string(),
        ]);
        assert!(cfg.headless);
        assert_eq!((cfg.width, cfg.height), (1280, 720));
        assert_eq!(
            cfg.startup_exec,
            ["connect 127.0.0.1:27015", "brightness 1.0"]
        );
        assert_eq!(cfg.wait_camera, 2);
        assert_eq!(cfg.wait_seconds, Some(1.5));
        assert_eq!(cfg.auto_screenshot, Some(PathBuf::from("out.png")));

        let unquoted = parse_args_from([
            "--exec".to_string(),
            "connect".to_string(),
            "127.0.0.1:27015".to_string(),
            "+connect".to_string(),
            "127.0.0.1:27016".to_string(),
            "-w".to_string(),
            "800".to_string(),
            "-h".to_string(),
            "600".to_string(),
        ]);
        assert_eq!(
            unquoted.startup_exec,
            ["connect 127.0.0.1:27015", "connect 127.0.0.1:27016"]
        );
        assert_eq!((unquoted.width, unquoted.height), (800, 600));

        let demo_cfg = parse_args_from([
            "--demo".to_string(),
            "--headless".to_string(),
            "--screenshot".to_string(),
            "demo.png".to_string(),
            "--record-dir".to_string(),
            "frames".to_string(),
            "--record-frames".to_string(),
            "72".to_string(),
            "--record-fps".to_string(),
            "24".to_string(),
        ]);
        assert!(demo_cfg.demo);
        assert!(demo_cfg.headless);
        assert_eq!(demo_cfg.auto_screenshot, Some(PathBuf::from("demo.png")));
        assert_eq!(demo_cfg.record_dir, Some(PathBuf::from("frames")));
        assert_eq!(demo_cfg.record_frames, 72);
        assert!((demo_cfg.record_fps - 24.0).abs() < 1e-6);
    }
}
