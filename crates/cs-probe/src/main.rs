//! `cs-probe --connect <ip:port> [--name N] [--seconds S] [--capture FILE] [--walk]`
//!
//! Connects to a Protocol 48 server over UDP, runs for `S` seconds (default 10), optionally
//! walks forward and records every datagram to a capture file. Exits with status 0 only when
//! the probe reached `InGame`.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    match native::run(std::env::args().skip(1)) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {msg}\n{}", native::USAGE);
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::PathBuf;
    use std::process::ExitCode;
    use std::time::Duration;

    use cs_platform::{Clock, NativeClock};
    use cs_probe::{CaptureWriter, ProbeClient, ProbeConfig, ProbeState};
    use cs_proto48::crypto::md5_hex;
    use cs_proto48::delta::GoldSrcUserCmd;
    use cs_transport::{PeerAddr, UdpTransport};

    pub const USAGE: &str =
        "usage: cs-probe --connect <ip:port> [--name N] [--seconds S] [--capture FILE] [--walk]";

    /// Steam ID sent in the LAN auth ticket (`sv_lan 1` does not check it).
    const LAN_STEAM_ID: u64 = 76_561_197_960_265_729;
    /// `msec` of each walking command; one command is queued per 10 ms of clock time.
    const WALK_MSEC: u8 = 10;
    /// `cl_forwardspeed` default.
    const WALK_FORWARD: f32 = 400.0;

    struct Args {
        server: std::net::SocketAddr,
        name: String,
        seconds: f64,
        capture: Option<PathBuf>,
        walk: bool,
    }

    fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
        let mut server = None;
        let mut name = "cs-probe".to_string();
        let mut seconds = 10.0;
        let mut capture = None;
        let mut walk = false;
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("{arg} needs a value"));
            match arg.as_str() {
                "--connect" => {
                    let v = value()?;
                    server = Some(
                        v.parse()
                            .map_err(|_| format!("--connect: bad address {v}"))?,
                    );
                }
                "--name" => name = value()?,
                "--seconds" => {
                    let v = value()?;
                    seconds = v
                        .parse::<f64>()
                        .ok()
                        .filter(|s| s.is_finite() && *s > 0.0)
                        .ok_or(format!("--seconds: not a positive number: {v}"))?;
                }
                "--capture" => capture = Some(PathBuf::from(value()?)),
                "--walk" => walk = true,
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(Args {
            server: server.ok_or("--connect is required")?,
            name,
            seconds,
            capture,
            walk,
        })
    }

    pub fn run(args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
        let args = parse_args(args)?;
        let bind = if args.server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let transport = UdpTransport::bind(bind).map_err(|e| e.to_string())?;
        let cfg = ProbeConfig {
            name: args.name,
            steam_id: LAN_STEAM_ID,
            cdkey_md5_hex: md5_hex(b"cs-probe"),
        };
        let mut probe = ProbeClient::new(transport, PeerAddr::Udp(args.server), cfg);
        if let Some(path) = &args.capture {
            let writer =
                CaptureWriter::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            probe.set_capture(writer);
        }

        let clock = NativeClock::new();
        let mut reached_in_game = false;
        let mut next_walk_cmd = 0.0;
        probe.connect();
        loop {
            let now = clock.now();
            if now >= args.seconds {
                break;
            }
            probe.pump(now);
            match probe.state() {
                ProbeState::InGame => {
                    if !reached_in_game {
                        reached_in_game = true;
                        next_walk_cmd = now;
                        println!("{now:.3}s: in game");
                    }
                    while args.walk && next_walk_cmd <= now {
                        probe.queue_cmd(GoldSrcUserCmd {
                            msec: WALK_MSEC,
                            forwardmove: WALK_FORWARD,
                            ..Default::default()
                        });
                        next_walk_cmd += f64::from(WALK_MSEC) / 1000.0;
                    }
                }
                ProbeState::Rejected(_) | ProbeState::Disconnected(_) => break,
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        let stats = probe.stats();
        println!("state: {:?}", probe.state());
        println!(
            "datagrams in: {}  bytes in: {}  largest: {}  full updates: {}  delta updates: {}  \
             parse errors: {}  entities: {}",
            stats.datagrams_in,
            stats.bytes_in,
            stats.largest_datagram,
            stats.full_updates,
            stats.delta_updates,
            stats.parse_errors,
            probe.entities().len()
        );
        if let Some(e) = probe.capture_error() {
            eprintln!("capture failed: {e}");
            return Ok(ExitCode::FAILURE);
        }
        Ok(if reached_in_game {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        })
    }
}
