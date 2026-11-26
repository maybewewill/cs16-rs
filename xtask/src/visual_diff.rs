//! Automated headless visual-parity harness (`cargo xtask visual-diff`).
//!
//! Spawns a local `cs-server` child process (tracked and cleaned up strictly by its own PID),
//! captures a headless offscreen `cs-client` frame into `<out>/ours_cam1.png`, measures RGB
//! region-of-interest statistics against GoldSrc reference targets, and optionally diffs
//! against a reference PNG in `--reference-dir`.

use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::Duration;

use image::{Rgb, RgbImage};

pub const ORIG_RIGHT_WALL_RGB: [f64; 3] = [113.0, 96.0, 65.0];
pub const RIGHT_WALL_TOLERANCE: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiSpec {
    pub name: &'static str,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoiStats {
    pub name: &'static str,
    pub mean_rgb: [f64; 3],
    pub std_luma: f64,
    pub pixels: usize,
}

pub const CAM1_ROIS: [RoiSpec; 6] = [
    RoiSpec {
        name: "right_wall",
        x0: 1000,
        y0: 260,
        x1: 1200,
        y1: 330,
    },
    RoiSpec {
        name: "right_wall_wide",
        x0: 900,
        y0: 300,
        x1: 1200,
        y1: 500,
    },
    RoiSpec {
        name: "left_rock",
        x0: 40,
        y0: 480,
        x1: 320,
        y1: 680,
    },
    RoiSpec {
        name: "ground_sand",
        x0: 500,
        y0: 560,
        x1: 850,
        y1: 700,
    },
    RoiSpec {
        name: "crates",
        x0: 600,
        y0: 450,
        x1: 760,
        y1: 580,
    },
    RoiSpec {
        name: "sky",
        x0: 400,
        y0: 40,
        x1: 800,
        y1: 180,
    },
];

pub struct VisualDiffConfig {
    pub map: String,
    pub brightness: Option<f32>,
    pub out_dir: PathBuf,
    pub reference_dir: Option<PathBuf>,
    pub resolution: String,
    pub port: u16,
    pub ui: String,
    pub force_build: bool,
}

impl Default for VisualDiffConfig {
    fn default() -> Self {
        Self {
            map: "de_dust2".to_string(),
            brightness: Some(1.0),
            out_dir: PathBuf::from("target/visual-diff"),
            reference_dir: None,
            resolution: "1280x720".to_string(),
            port: 27099,
            ui: "none".to_string(),
            force_build: false,
        }
    }
}

struct ServerChildGuard(Option<Child>);

impl Drop for ServerChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn compute_roi_stats(img: &RgbImage, roi: RoiSpec) -> RoiStats {
    let x0 = roi.x0.min(img.width());
    let x1 = roi.x1.min(img.width());
    let y0 = roi.y0.min(img.height());
    let y1 = roi.y1.min(img.height());
    let mut sum_r = 0.0f64;
    let mut sum_g = 0.0f64;
    let mut sum_b = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_y2 = 0.0f64;
    let mut count = 0usize;

    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            let r = f64::from(p[0]);
            let g = f64::from(p[1]);
            let b = f64::from(p[2]);
            let luma = 0.299 * r + 0.587 * g + 0.114 * b;
            sum_r += r;
            sum_g += g;
            sum_b += b;
            sum_y += luma;
            sum_y2 += luma * luma;
            count += 1;
        }
    }

    if count == 0 {
        return RoiStats {
            name: roi.name,
            mean_rgb: [0.0; 3],
            std_luma: 0.0,
            pixels: 0,
        };
    }

    let n = count as f64;
    let mean_y = sum_y / n;
    let var_y = (sum_y2 / n - mean_y * mean_y).max(0.0);
    RoiStats {
        name: roi.name,
        mean_rgb: [sum_r / n, sum_g / n, sum_b / n],
        std_luma: var_y.sqrt(),
        pixels: count,
    }
}

pub fn within_rgb_tolerance(measured: [f64; 3], target: [f64; 3], tol: f64) -> bool {
    (0..3).all(|c| (measured[c] - target[c]).abs() <= tol)
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<VisualDiffConfig, String> {
    let mut cfg = VisualDiffConfig::default();
    let mut it = args;
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--map" => cfg.map = it.next().ok_or("--map needs a value")?,
            "--brightness" => {
                let raw = it.next().ok_or("--brightness needs a value")?;
                let val = raw
                    .parse::<f32>()
                    .map_err(|e| format!("invalid --brightness `{raw}`: {e}"))?;
                cfg.brightness = Some(val);
            }
            "--out" => cfg.out_dir = PathBuf::from(it.next().ok_or("--out needs a value")?),
            "--reference-dir" => {
                cfg.reference_dir = Some(PathBuf::from(
                    it.next().ok_or("--reference-dir needs a value")?,
                ));
            }
            "--resolution" => {
                cfg.resolution = it.next().ok_or("--resolution needs a value")?;
            }
            "--port" => {
                let raw = it.next().ok_or("--port needs a value")?;
                cfg.port = raw
                    .parse::<u16>()
                    .map_err(|e| format!("invalid --port `{raw}`: {e}"))?;
            }
            "--ui" => cfg.ui = it.next().ok_or("--ui needs a value")?,
            "--build" => cfg.force_build = true,
            other => return Err(format!("unknown visual-diff flag `{other}`")),
        }
    }
    Ok(cfg)
}

fn binary_path(name: &str) -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    PathBuf::from("target")
        .join(profile)
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn ensure_binaries(force: bool) -> Result<(PathBuf, PathBuf), String> {
    let srv = binary_path("cs-server");
    let cli = binary_path("cs-client");
    if force || !srv.exists() || !cli.exists() {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.arg("build");
        if !cfg!(debug_assertions) {
            cmd.arg("--release");
        }
        let status = cmd
            .args(["-p", "cs-server", "-p", "cs-client"])
            .status()
            .map_err(|e| format!("failed to build cs-server/cs-client: {e}"))?;
        if !status.success() {
            return Err(format!(
                "cargo build -p cs-server -p cs-client failed ({status})"
            ));
        }
    }
    if !srv.exists() || !cli.exists() {
        return Err("expected binaries missing after build".into());
    }
    Ok((srv, cli))
}

pub fn run_visual_diff(args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let cfg = parse_args(args)?;
    let (server_bin, client_bin) = ensure_binaries(cfg.force_build)?;

    std::fs::create_dir_all(&cfg.out_dir)
        .map_err(|e| format!("cannot create {}: {e}", cfg.out_dir.display()))?;

    let srv_log = cfg.out_dir.join("server.log");
    let srv_out =
        File::create(&srv_log).map_err(|e| format!("cannot create {}: {e}", srv_log.display()))?;
    let srv_err = srv_out
        .try_clone()
        .map_err(|e| format!("cannot clone server log handle: {e}"))?;

    let child = Command::new(&server_bin)
        .args(["--map", &cfg.map, "--port", &cfg.port.to_string()])
        .stdout(Stdio::from(srv_out))
        .stderr(Stdio::from(srv_err))
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", server_bin.display()))?;
    let _server_guard = ServerChildGuard(Some(child));

    // Allow cs-server UDP listener + BSP load to settle before client connects.
    std::thread::sleep(Duration::from_millis(900));

    let shot_path = cfg.out_dir.join("ours_cam1.png");
    let mut script =
        "gamma 2.5; texgamma 2.0; lightgamma 2.5; gl_overbright 0; gl_ansio 1; ".to_string();
    if let Some(b) = cfg.brightness {
        let _ = write!(script, "brightness {b:.3}; ");
    }
    let _ = write!(script, "connect 127.0.0.1:{}", cfg.port);

    let cli_log = cfg.out_dir.join("client.log");
    let cli_out =
        File::create(&cli_log).map_err(|e| format!("cannot create {}: {e}", cli_log.display()))?;
    let cli_err = cli_out
        .try_clone()
        .map_err(|e| format!("cannot clone client log handle: {e}"))?;

    let status = Command::new(&client_bin)
        .args([
            "--headless",
            "--resolution",
            &cfg.resolution,
            "--ui",
            &cfg.ui,
            "--exec",
            &script,
            "--screenshot",
        ])
        .arg(&shot_path)
        .stdout(Stdio::from(cli_out))
        .stderr(Stdio::from(cli_err))
        .status()
        .map_err(|e| format!("run {}: {e}", client_bin.display()))?;

    if !status.success() {
        return Err(format!(
            "cs-client --headless exited with {status} (see {})",
            cli_log.display()
        ));
    }

    let img = image::open(&shot_path)
        .map_err(|e| format!("open {}: {e}", shot_path.display()))?
        .to_rgb8();

    let mut report = String::new();
    let _ = writeln!(report, "# Visual Diff Report (`{}`)", cfg.map);
    let _ = writeln!(
        report,
        "- Capture: `{}` ({}x{})",
        shot_path.display(),
        img.width(),
        img.height()
    );
    if let Some(b) = cfg.brightness {
        let _ = writeln!(report, "- Override `brightness`: `{b:.3}`");
    }
    let _ = writeln!(
        report,
        "\n| ROI | Box | Mean R | Mean G | Mean B | Std Luma |\n|---|---|---|---|---|---|"
    );

    let mut right_wall_ok = false;
    let mut right_wall_rgb = [0.0; 3];
    for roi in CAM1_ROIS {
        let st = compute_roi_stats(&img, roi);
        if roi.name == "right_wall" {
            right_wall_rgb = st.mean_rgb;
            right_wall_ok =
                within_rgb_tolerance(st.mean_rgb, ORIG_RIGHT_WALL_RGB, RIGHT_WALL_TOLERANCE);
        }
        let _ = writeln!(
            report,
            "| {} | ({},{})-({},{}) | {:.1} | {:.1} | {:.1} | {:.1} |",
            st.name,
            roi.x0,
            roi.y0,
            roi.x1,
            roi.y1,
            st.mean_rgb[0],
            st.mean_rgb[1],
            st.mean_rgb[2],
            st.std_luma
        );
    }

    let _ = writeln!(
        report,
        "\n## Target Check (`right_wall` vs Original RGB `({:.0}, {:.0}, {:.0})` +/- `{:.0}`)",
        ORIG_RIGHT_WALL_RGB[0],
        ORIG_RIGHT_WALL_RGB[1],
        ORIG_RIGHT_WALL_RGB[2],
        RIGHT_WALL_TOLERANCE
    );
    let _ = writeln!(
        report,
        "- Measured: `({:.1}, {:.1}, {:.1})` | Delta: `({:+.1}, {:+.1}, {:+.1})` | **{}**",
        right_wall_rgb[0],
        right_wall_rgb[1],
        right_wall_rgb[2],
        right_wall_rgb[0] - ORIG_RIGHT_WALL_RGB[0],
        right_wall_rgb[1] - ORIG_RIGHT_WALL_RGB[1],
        right_wall_rgb[2] - ORIG_RIGHT_WALL_RGB[2],
        if right_wall_ok { "PASS" } else { "FAIL" }
    );

    if let Some(ref ref_dir) = cfg.reference_dir {
        let ref_path = ref_dir.join("orig_cam1.png");
        if ref_path.exists() {
            append_reference_diff(&img, &ref_path, &cfg.out_dir, &mut report)?;
        }
    }

    let report_path = cfg.out_dir.join("report.md");
    std::fs::write(&report_path, &report)
        .map_err(|e| format!("write {}: {e}", report_path.display()))?;
    print!("{report}");

    if right_wall_ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

fn append_reference_diff(
    ours: &RgbImage,
    ref_path: &Path,
    out_dir: &Path,
    report: &mut String,
) -> Result<(), String> {
    let orig = image::open(ref_path)
        .map_err(|e| format!("open {}: {e}", ref_path.display()))?
        .to_rgb8();
    let w = ours.width().min(orig.width());
    let h = ours.height().min(orig.height());
    let mut diff_img = RgbImage::new(w * 2, h);
    let mut sum_abs = 0.0f64;
    let mut sum_sq = 0.0f64;

    for y in 0..h {
        for x in 0..w {
            let a = ours.get_pixel(x, y).0;
            let b = orig.get_pixel(x, y).0;
            diff_img.put_pixel(x, y, Rgb(a));
            let dr = (i32::from(a[0]) - i32::from(b[0])).abs();
            let dg = (i32::from(a[1]) - i32::from(b[1])).abs();
            let db = (i32::from(a[2]) - i32::from(b[2])).abs();
            sum_abs += f64::from(dr + dg + db);
            sum_sq += f64::from(dr * dr + dg * dg + db * db);
            diff_img.put_pixel(
                x + w,
                y,
                Rgb([
                    (dr * 4).min(255) as u8,
                    (dg * 4).min(255) as u8,
                    (db * 4).min(255) as u8,
                ]),
            );
        }
    }

    let n = f64::from(w * h) * 3.0;
    let mae = if n > 0.0 { sum_abs / n } else { 0.0 };
    let rmse = if n > 0.0 { (sum_sq / n).sqrt() } else { 0.0 };
    let diff_path = out_dir.join("diff_cam1.png");
    diff_img
        .save(&diff_path)
        .map_err(|e| format!("save {}: {e}", diff_path.display()))?;
    let _ = writeln!(
        report,
        "\n## Reference Image Diff vs `{}`\n- Size: `{}x{}` | MAE: `{:.2}` / 255 | RMSE: `{:.2}` / 255 | Diff: `{}`",
        ref_path.display(),
        w,
        h,
        mae,
        rmse,
        diff_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roi_stats_and_tolerance_check_exact_rgb() {
        let mut img = RgbImage::from_pixel(1280, 720, Rgb([10, 20, 30]));
        for y in 260..330 {
            for x in 1000..1200 {
                img.put_pixel(x, y, Rgb([114, 95, 67]));
            }
        }
        let st = compute_roi_stats(&img, CAM1_ROIS[0]);
        assert_eq!(st.pixels, 200 * 70);
        assert!((st.mean_rgb[0] - 114.0).abs() < 1e-6);
        assert!((st.mean_rgb[1] - 95.0).abs() < 1e-6);
        assert!((st.mean_rgb[2] - 67.0).abs() < 1e-6);
        assert!(within_rgb_tolerance(
            st.mean_rgb,
            ORIG_RIGHT_WALL_RGB,
            RIGHT_WALL_TOLERANCE
        ));
        assert!(!within_rgb_tolerance(
            [120.0, 96.0, 65.0],
            ORIG_RIGHT_WALL_RGB,
            RIGHT_WALL_TOLERANCE
        ));
    }
}
