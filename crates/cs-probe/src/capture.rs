//! Golden-capture file format: every datagram a probe exchanged with a server.
//!
//! Layout: [`CAPTURE_MAGIC`], then records of `u8 dir`, `f64 time` (LE), `u32 len` (LE) and
//! `len` raw datagram bytes as they crossed the wire (before `COM_UnMunge2`).

use cs_proto48::bitbuf::ProtoError;

pub const CAPTURE_MAGIC: &[u8; 8] = b"CS48CAP1";

/// Which way a captured datagram travelled, seen from the probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Received = 0,
    Sent = 1,
}

/// One captured datagram.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureRecord {
    pub dir: Direction,
    /// Seconds on the probe's clock when the datagram was sent or received.
    pub time: f64,
    pub data: Vec<u8>,
}

/// Appends records to a capture file. Each record is flushed as it is pushed.
#[cfg(not(target_arch = "wasm32"))]
pub struct CaptureWriter {
    file: std::io::BufWriter<std::fs::File>,
}

#[cfg(not(target_arch = "wasm32"))]
impl CaptureWriter {
    /// Creates (or truncates) `path` and writes the magic.
    pub fn create(path: &std::path::Path) -> std::io::Result<Self> {
        use std::io::Write;
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        file.write_all(CAPTURE_MAGIC)?;
        file.flush()?;
        Ok(Self { file })
    }

    pub fn push(&mut self, r: &CaptureRecord) -> std::io::Result<()> {
        use std::io::Write;
        let len = u32::try_from(r.data.len()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "datagram too large")
        })?;
        self.file.write_all(&[r.dir as u8])?;
        self.file.write_all(&r.time.to_le_bytes())?;
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&r.data)?;
        self.file.flush()
    }
}

/// Parses a whole capture file.
pub fn read_capture(bytes: &[u8]) -> Result<Vec<CaptureRecord>, ProtoError> {
    let rest = bytes
        .strip_prefix(CAPTURE_MAGIC.as_slice())
        .ok_or_else(|| ProtoError::InvalidPayload("not a CS48CAP1 capture".to_string()))?;
    let mut rest = rest;
    let mut records = Vec::new();
    while !rest.is_empty() {
        let (header, after) = rest.split_at_checked(13).ok_or(ProtoError::UnexpectedEof)?;
        let dir = match header[0] {
            0 => Direction::Received,
            1 => Direction::Sent,
            other => {
                return Err(ProtoError::InvalidPayload(format!(
                    "capture record direction {other}"
                )));
            }
        };
        let time = f64::from_le_bytes(header[1..9].try_into().expect("8 bytes"));
        let len = u32::from_le_bytes(header[9..13].try_into().expect("4 bytes")) as usize;
        let (data, after) = after
            .split_at_checked(len)
            .ok_or(ProtoError::UnexpectedEof)?;
        records.push(CaptureRecord {
            dir,
            time,
            data: data.to_vec(),
        });
        rest = after;
    }
    Ok(records)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("cs-probe-{}-{name}", std::process::id()))
    }

    fn sample_records() -> Vec<CaptureRecord> {
        vec![
            CaptureRecord {
                dir: Direction::Sent,
                time: 0.0,
                data: b"\xFF\xFF\xFF\xFFgetchallenge steam\n".to_vec(),
            },
            CaptureRecord {
                dir: Direction::Received,
                time: 0.015625,
                data: vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            },
            CaptureRecord {
                dir: Direction::Received,
                time: 12.5,
                data: Vec::new(),
            },
        ]
    }

    fn write_file(name: &str, records: &[CaptureRecord]) -> Vec<u8> {
        let path = temp_path(name);
        let mut w = CaptureWriter::create(&path).unwrap();
        for r in records {
            w.push(r).unwrap();
        }
        drop(w);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        bytes
    }

    #[test]
    fn capture_roundtrips() {
        let records = sample_records();
        let bytes = write_file("roundtrip.cs48cap", &records);
        assert_eq!(&bytes[..8], CAPTURE_MAGIC);
        assert_eq!(read_capture(&bytes).unwrap(), records);

        let mut wrong_magic = bytes.clone();
        wrong_magic[7] = b'2';
        assert!(read_capture(&wrong_magic).is_err());
    }

    #[test]
    fn truncated_capture_is_an_error() {
        let bytes = write_file("truncated.cs48cap", &sample_records());
        // Magic alone is an empty, valid capture.
        assert_eq!(read_capture(&bytes[..8]).unwrap(), Vec::new());
        // Cutting anywhere inside a record (header or data) is an error.
        let first_record_end = 8 + 13 + 23;
        for cut in [3, 9, 8 + 12, 8 + 13 + 5, first_record_end + 13 + 4] {
            assert!(
                read_capture(&bytes[..cut]).is_err(),
                "cut at {cut} accepted"
            );
        }
        // An unknown direction byte is an error too.
        let mut bad_dir = bytes.clone();
        bad_dir[8] = 2;
        assert!(read_capture(&bad_dir).is_err());
    }
}
