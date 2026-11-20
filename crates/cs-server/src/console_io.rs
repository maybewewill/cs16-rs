//! Operator input of the Protocol 48 server: stdin lines and Ctrl+C, handed to the main loop
//! without blocking it.

use std::io::{BufRead, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

/// Most bytes of one input line kept in memory; the rest of a longer line is dropped.
const MAX_READ_LINE: u64 = 64 * 1024;

/// Reads `input` line by line on a thread and sends every line to the returned channel.
///
/// End of input or a read error ends the thread quietly: the sender is dropped and the
/// receiver then yields nothing more, which is not a request to stop (a server run as a
/// service has no terminal). A line that is not valid UTF-8 is passed on with the bad bytes
/// replaced.
pub fn spawn_line_reader<R: BufRead + Send + 'static>(mut input: R) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("console-stdin".to_string())
        .spawn(move || {
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match (&mut input).take(MAX_READ_LINE).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                // An over-long line is cut here (the engine cuts far shorter) and the rest of
                // it dropped, so it cannot grow memory or turn into several commands.
                while buf.len() as u64 >= MAX_READ_LINE && buf.last() != Some(&b'\n') {
                    let mut skipped = Vec::new();
                    match (&mut input).take(MAX_READ_LINE).read_until(b'\n', &mut skipped) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if skipped.last() == Some(&b'\n') => break,
                        Ok(_) => {}
                    }
                }
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\n', '\r']);
                if tx.send(line.to_string()).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the console input thread: {e}");
    }
    rx
}

/// A flag set by Ctrl+C (and SIGTERM or console close where the platform maps them). If the
/// handler cannot be installed a warning is logged and the flag simply never rises.
pub fn interrupt_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let handler_flag = Arc::clone(&flag);
    if let Err(e) = ctrlc::set_handler(move || handler_flag.store(true, Ordering::SeqCst)) {
        tracing::warn!("cannot install the Ctrl+C handler: {e}");
    }
    flag
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::Duration;

    #[test]
    fn lines_arrive_in_order_and_eof_ends_the_stream() {
        let rx = spawn_line_reader(Cursor::new(b"status\r\nsay hi\n\nlast".to_vec()));
        let got: Vec<String> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(5)).ok())
            .collect();
        assert_eq!(got, ["status", "say hi", "", "last"]);
    }

    #[test]
    fn an_overlong_line_is_cut_and_its_rest_dropped() {
        let mut input = "x".repeat(200 * 1024).into_bytes();
        input.extend_from_slice(b"\nnext\n");
        let rx = spawn_line_reader(Cursor::new(input));
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first.len() as u64, MAX_READ_LINE);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "next");
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        let rx = spawn_line_reader(Cursor::new(vec![b'a', 0xFF, b'b', b'\n']));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "a\u{FFFD}b"
        );
    }
}
