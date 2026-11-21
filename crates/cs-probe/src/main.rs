//! `cs-probe --connect <ip:port> [--name N] [--seconds S] [--capture FILE] [--walk]
//! [--cmd "<line>"]... [--cmd-gap N]`
//!
//! Connects to a Protocol 48 server over UDP, runs for `S` seconds (default 10), optionally
//! walks forward and records every datagram to a capture file. Each `--cmd` line is sent to the
//! server as a console command (`stringcmd`) once the probe is in game, in the order given,
//! each after the server has sent `N` more datagrams (default 10) than when the one before it
//! was sent: a count of what the server did, not of time, so a command is not sent before the
//! server has had frames to show the menu it answers. Exits with status 0 only when the probe
//! reached `InGame`.

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
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::process::ExitCode;
    use std::time::Duration;

    use cs_platform::{Clock, NativeClock};
    use cs_probe::{CaptureWriter, ProbeClient, ProbeConfig, ProbeState};
    use cs_proto48::crypto::md5_hex;
    use cs_proto48::delta::GoldSrcUserCmd;
    use cs_transport::{PeerAddr, UdpTransport};

    pub const USAGE: &str =
        "usage: cs-probe --connect <ip:port> [--name N] [--seconds S] [--capture FILE] [--walk] \
         [--cmd \"<line>\"]... [--cmd-gap N]";

    /// Steam ID sent in the LAN auth ticket (`sv_lan 1` does not check it).
    const LAN_STEAM_ID: u64 = 76_561_197_960_265_729;
    /// `msec` of each walking command; one command is queued per 10 ms of clock time.
    const WALK_MSEC: u8 = 10;
    /// `cl_forwardspeed` default.
    const WALK_FORWARD: f32 = 400.0;
    /// Datagrams the server sends between two `--cmd` lines unless `--cmd-gap` says otherwise.
    const DEFAULT_CMD_GAP: u64 = 10;
    /// Longest `--cmd` line (the engine reads a command of at most 255 bytes).
    const MAX_CMD_LEN: usize = 255;

    struct Args {
        server: std::net::SocketAddr,
        name: String,
        seconds: f64,
        capture: Option<PathBuf>,
        walk: bool,
        cmds: Vec<String>,
        cmd_gap: u64,
    }

    fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
        let mut server = None;
        let mut name = "cs-probe".to_string();
        let mut seconds = 10.0;
        let mut capture = None;
        let mut walk = false;
        let mut cmds = Vec::new();
        let mut cmd_gap = DEFAULT_CMD_GAP;
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
                "--cmd" => {
                    let v = value()?;
                    if v.is_empty() || v.len() > MAX_CMD_LEN {
                        return Err(format!("--cmd: a line of 1 to {MAX_CMD_LEN} bytes is needed"));
                    }
                    cmds.push(v);
                }
                "--cmd-gap" => {
                    let v = value()?;
                    cmd_gap = v
                        .parse::<u64>()
                        .map_err(|_| format!("--cmd-gap: not a number of datagrams: {v}"))?;
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(Args {
            server: server.ok_or("--connect is required")?,
            name,
            seconds,
            capture,
            walk,
            cmds,
            cmd_gap,
        })
    }

    /// What the tests read of the parsed arguments: the `--cmd` lines and the gap.
    #[cfg(test)]
    pub fn parse_args_for_tests(
        args: impl Iterator<Item = String>,
    ) -> Result<(Vec<String>, u64), String> {
        parse_args(args).map(|a| (a.cmds, a.cmd_gap))
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
        let mut pending_cmds: VecDeque<String> = args.cmds.iter().cloned().collect();
        // Datagrams the server had sent when the last command went out (or the probe got in game).
        let mut cmd_baseline = 0;
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
                        cmd_baseline = probe.stats().datagrams_in;
                        println!("{now:.3}s: in game");
                    }
                    if probe.stats().datagrams_in >= cmd_baseline + args.cmd_gap
                        && let Some(line) = pending_cmds.pop_front()
                    {
                        probe.send_stringcmd(&line);
                        cmd_baseline = probe.stats().datagrams_in;
                        println!("{now:.3}s: sent `{line}`");
                    }
                    // A client sends a command with time in it every frame, walking or
                    // not: the game's per-player logic (menus, joining) runs from them.
                    while next_walk_cmd <= now {
                        probe.queue_cmd(GoldSrcUserCmd {
                            msec: WALK_MSEC,
                            forwardmove: if args.walk { WALK_FORWARD } else { 0.0 },
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
        if let Some(cd) = probe.client_data() {
            println!(
                "player: origin ({:.1} {:.1} {:.1})  health {}  deadflag {}",
                cd.origin.x, cd.origin.y, cd.origin.z, cd.health, cd.deadflag
            );
        }
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::native::parse_args_for_tests;

    fn args(list: &[&str]) -> impl Iterator<Item = String> + use<> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn cmd_lines_are_kept_in_order() {
        let (cmds, gap) = parse_args_for_tests(args(&[
            "--connect",
            "127.0.0.1:27015",
            "--cmd",
            "jointeam 1",
            "--cmd",
            "joinclass 1",
            "--cmd-gap",
            "3",
        ]))
        .unwrap();
        assert_eq!(cmds, ["jointeam 1", "joinclass 1"]);
        assert_eq!(gap, 3);
    }

    #[test]
    fn bad_cmd_arguments_are_refused() {
        let base = ["--connect", "127.0.0.1:27015"];
        for extra in [
            &["--cmd"][..],
            &["--cmd", ""],
            &["--cmd-gap", "x"],
            &["--cmd-gap", "-1"],
        ] {
            let list: Vec<&str> = base.iter().chain(extra).copied().collect();
            assert!(parse_args_for_tests(args(&list)).is_err(), "{extra:?}");
        }
        let long = "x".repeat(256);
        assert!(
            parse_args_for_tests(args(&["--connect", "127.0.0.1:1", "--cmd", &long])).is_err()
        );
        assert_eq!(
            parse_args_for_tests(args(&["--connect", "127.0.0.1:1"])).unwrap(),
            (Vec::new(), 10)
        );
    }
}
