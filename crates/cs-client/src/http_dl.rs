//! Pure-Rust HTTP/1.1 FastDL client (`sv_downloadurl`) implementing [`HttpFetcher`].
//!
//! Runs requests on a single background worker thread using blocking `std::net::TcpStream`
//! with strict header (`16 KiB`), body (`64 MiB`), redirect (`3`), and timeout caps.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use cs_clientcore::{HttpFetchResult, HttpFetcher};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(15);

struct FetchJob {
    base_url: String,
    rel_path: String,
    is_bz2: bool,
}

/// Asynchronous background HTTP/1.1 FastDL fetcher backed by a dedicated worker thread.
pub struct NativeHttpFetcher {
    tx: Sender<FetchJob>,
    rx: Receiver<HttpFetchResult>,
    pending: Arc<AtomicUsize>,
}

impl Default for NativeHttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeHttpFetcher {
    pub fn new() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<FetchJob>();
        let (res_tx, res_rx) = mpsc::channel::<HttpFetchResult>();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending);
        let _ = std::thread::Builder::new()
            .name("cs-fastdl-http".to_string())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let url = join_fastdl_url(&job.base_url, &job.rel_path, job.is_bz2);
                    let data = fetch_http_url(&url, MAX_REDIRECTS);
                    worker_pending.fetch_sub(1, Ordering::Relaxed);
                    if res_tx
                        .send(HttpFetchResult {
                            path: job.rel_path,
                            is_bz2: job.is_bz2,
                            data,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        Self {
            tx: job_tx,
            rx: res_rx,
            pending,
        }
    }
}

impl HttpFetcher for NativeHttpFetcher {
    fn start_fetch(&mut self, base_url: &str, rel_path: &str, is_bz2: bool) -> bool {
        if !base_url.trim_start().to_ascii_lowercase().starts_with("http://") {
            return false;
        }
        self.pending.fetch_add(1, Ordering::Relaxed);
        if self
            .tx
            .send(FetchJob {
                base_url: base_url.to_string(),
                rel_path: rel_path.to_string(),
                is_bz2,
            })
            .is_err()
        {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    fn poll_completed(&mut self) -> Vec<HttpFetchResult> {
        let mut out = Vec::new();
        while let Ok(item) = self.rx.try_recv() {
            out.push(item);
        }
        out
    }

    fn has_pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed) > 0
    }
}

fn join_fastdl_url(base: &str, rel: &str, is_bz2: bool) -> String {
    let base = base.trim().trim_end_matches('/');
    let rel = rel.trim_start_matches('/');
    if is_bz2 && !rel.to_ascii_lowercase().ends_with(".bz2") {
        format!("{base}/{rel}.bz2")
    } else {
        format!("{base}/{rel}")
    }
}

fn parse_http_url(url: &str) -> Result<(String, u16, String, String), String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("HTTP://"))
        .ok_or_else(|| "only plain http:// FastDL URLs are supported".to_string())?;
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err("invalid HTTP URL authority".to_string());
    }
    let (host, port) = if let Some(h) = authority.strip_prefix('[') {
        let end = h.find(']').ok_or_else(|| "invalid IPv6 host".to_string())?;
        let ip = &h[..end];
        let after = &h[end + 1..];
        let port = if let Some(p) = after.strip_prefix(':') {
            p.parse::<u16>().map_err(|_| "invalid port".to_string())?
        } else if after.is_empty() {
            80
        } else {
            return Err("invalid host suffix".to_string());
        };
        (ip.to_string(), port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        let port = p.parse::<u16>().map_err(|_| "invalid port".to_string())?;
        (h.to_string(), port)
    } else {
        (authority.to_string(), 80)
    };
    let safe_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Ok((host, port, safe_path, authority.to_string()))
}

fn fetch_http_url(url: &str, redirects_left: usize) -> Result<Vec<u8>, String> {
    let (host, port, path, host_header) = parse_http_url(url)?;
    let addrs: Vec<_> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("DNS error for {host}: {e}"))?
        .collect();
    let addr = addrs
        .into_iter()
        .next()
        .ok_or_else(|| format!("no address resolved for {host}"))?;

    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| format!("connect {addr}: {e}"))?;
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: Half-Life\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("send HTTP request: {e}"))?;

    let mut reader = BufReader::new(stream);
    let mut header_bytes = 0usize;
    let mut status_line = String::new();
    loop {
        let n = reader
            .read_line(&mut status_line)
            .map_err(|e| format!("read status line: {e}"))?;
        if n == 0 {
            return Err("empty HTTP response".to_string());
        }
        header_bytes += n;
        if header_bytes > MAX_HEADER_BYTES {
            return Err("HTTP response headers exceed 16 KiB limit".to_string());
        }
        if status_line != "\r\n" && status_line != "\n" {
            break;
        }
        status_line.clear();
    }
    let code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("malformed HTTP status line: {}", status_line.trim()))?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    let mut location: Option<String> = None;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("read header: {e}"))?;
        if n == 0 {
            return Err("truncated HTTP headers".to_string());
        }
        header_bytes += n;
        if header_bytes > MAX_HEADER_BYTES {
            return Err("HTTP response headers exceed 16 KiB limit".to_string());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            if key.eq_ignore_ascii_case("content-length") {
                let len = val
                    .parse::<usize>()
                    .map_err(|_| "invalid Content-Length header".to_string())?;
                if len > MAX_BODY_BYTES {
                    return Err(format!("Content-Length {len} exceeds 64 MiB cap"));
                }
                content_length = Some(len);
            } else if key.eq_ignore_ascii_case("transfer-encoding")
                && val.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            } else if key.eq_ignore_ascii_case("location") {
                location = Some(val.to_string());
            }
        }
    }

    if matches!(code, 301 | 302 | 303 | 307 | 308) {
        if redirects_left == 0 {
            return Err("too many HTTP redirects".to_string());
        }
        let target = location.ok_or_else(|| "redirect without Location header".to_string())?;
        let next_url = if target.starts_with('/') {
            if port == 80 {
                format!("http://{host}{target}")
            } else {
                format!("http://{host}:{port}{target}")
            }
        } else {
            target
        };
        return fetch_http_url(&next_url, redirects_left - 1);
    }

    if code != 200 {
        return Err(format!("HTTP status {code}"));
    }

    if chunked {
        read_chunked_body(&mut reader)
    } else if let Some(len) = content_length {
        let mut out = vec![0u8; len];
        reader
            .read_exact(&mut out)
            .map_err(|e| format!("read HTTP body: {e}"))?;
        Ok(out)
    } else {
        read_until_eof(&mut reader)
    }
}

fn read_until_eof(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("read body: {e}"))?;
        if n == 0 {
            break;
        }
        if out.len() + n > MAX_BODY_BYTES {
            return Err("HTTP response body exceeds 64 MiB cap".to_string());
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

fn read_chunked_body(reader: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        let n = reader
            .read_line(&mut size_line)
            .map_err(|e| format!("read chunk size: {e}"))?;
        if n == 0 {
            return Err("truncated chunked body".to_string());
        }
        let hex = size_line.split(';').next().unwrap_or("").trim();
        let chunk_len =
            usize::from_str_radix(hex, 16).map_err(|_| "invalid chunked length".to_string())?;
        if chunk_len == 0 {
            break;
        }
        if out.len() + chunk_len > MAX_BODY_BYTES {
            return Err("chunked HTTP body exceeds 64 MiB cap".to_string());
        }
        let start = out.len();
        out.resize(start + chunk_len, 0);
        reader
            .read_exact(&mut out[start..])
            .map_err(|e| format!("read chunk bytes: {e}"))?;
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|e| format!("read chunk terminator: {e}"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    fn serve_once(routes: HashMap<String, Vec<u8>>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let Ok(mut s) = stream else {
                    break;
                };
                let mut reader = BufReader::new(&mut s);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" {
                        break;
                    }
                }
                if let Some(resp) = routes.get(&path) {
                    let _ = s.write_all(resp);
                } else {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                }
            }
        });
        (format!("http://127.0.0.1:{port}/cstrike"), handle)
    }

    #[test]
    fn fastdl_fetches_raw_and_chunked_and_rejects_oversized_content_length() {
        let mut routes = HashMap::new();
        routes.insert(
            "/cstrike/models/test.mdl".to_string(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nIDST".to_vec(),
        );
        routes.insert(
            "/cstrike/sound/hit.wav".to_string(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nRIFF\r\n0\r\n\r\n"
                .to_vec(),
        );
        routes.insert(
            "/cstrike/maps/huge.bsp".to_string(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n".to_vec(),
        );

        let (base, handle) = serve_once(routes);
        let mut fetcher = NativeHttpFetcher::new();
        assert!(fetcher.start_fetch(&base, "models/test.mdl", false));
        assert!(fetcher.start_fetch(&base, "sound/hit.wav", false));
        assert!(fetcher.start_fetch(&base, "maps/huge.bsp", false));

        let mut results = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while results.len() < 3 && std::time::Instant::now() < deadline {
            results.extend(fetcher.poll_completed());
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(results.len(), 3);
        let by_path: HashMap<String, Result<Vec<u8>, String>> =
            results.into_iter().map(|r| (r.path, r.data)).collect();
        assert_eq!(by_path["models/test.mdl"].as_deref(), Ok(b"IDST".as_slice()));
        assert_eq!(by_path["sound/hit.wav"].as_deref(), Ok(b"RIFF".as_slice()));
        assert!(by_path["maps/huge.bsp"].is_err());
        let _ = handle.join();
    }
}
