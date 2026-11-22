//! Builds small synthetic captures for the dump and diff tests: server datagrams carrying
//! messages written with the `cs-proto48` writers, and client datagrams carrying string
//! commands, written to a capture file with [`CaptureWriter`] and read back.

use std::sync::atomic::{AtomicU32, Ordering};

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan};
use cs_proto48::usermsg::UserMsgRegistry;

pub use cs_proto48::messages::{
    write_svc_print as write_print, write_svc_stufftext as write_stufftext,
    write_svc_time as write_time,
};
pub use cs_proto48::usermsg::CsUserMsg;

use crate::capture::{CaptureRecord, CaptureWriter, Direction, read_capture};

/// The user messages the synthetic server registers with [`SyntheticCapture::register`].
const REGISTERED: &[(u8, i8, &str)] = &[
    (64, 5, "Money"),
    (65, 2, "RoundTime"),
    (66, 1, "HideWeapon"),
    (67, 0, "ResetHUD"),
];

pub fn write_usermsg(w: &mut MsgWriter, reg: &UserMsgRegistry, msg: &CsUserMsg) {
    assert!(
        cs_proto48::messages::write_svc_usermsg(w, reg, msg),
        "{msg:?} is not registered in the synthetic registry"
    );
}

pub struct SyntheticCapture {
    reg: UserMsgRegistry,
    server: NetChan,
    client: NetChan,
    records: Vec<CaptureRecord>,
}

impl SyntheticCapture {
    pub fn new() -> Self {
        let mut reg = UserMsgRegistry::new();
        for &(id, size, name) in REGISTERED {
            reg.register(id, size, name);
        }
        Self {
            reg,
            server: NetChan::new(FRAGMENT_C2S_SIZE),
            client: NetChan::new(FRAGMENT_C2S_SIZE),
            records: Vec::new(),
        }
    }

    /// A server datagram at `time` with the `svc_newusermsg` of every registered message.
    pub fn register(self, time: f64) -> Self {
        self.server(time, cs_proto48::messages::write_svc_userregs)
    }

    /// A server datagram at `time`; `write` adds the messages to it.
    pub fn server(
        mut self,
        time: f64,
        write: impl FnOnce(&mut MsgWriter, &UserMsgRegistry),
    ) -> Self {
        let mut w = MsgWriter::new();
        write(&mut w, &self.reg);
        let data = self.server.build_packet(&w.into_bytes());
        self.records.push(CaptureRecord {
            dir: Direction::Received,
            time,
            data,
        });
        self
    }

    /// A connectionless datagram from the server.
    pub fn oob_from_server(mut self, time: f64, text: &[u8]) -> Self {
        let mut data = vec![0xFF; 4];
        data.extend_from_slice(text);
        self.records.push(CaptureRecord {
            dir: Direction::Received,
            time,
            data,
        });
        self
    }

    /// A client datagram carrying one `clc_stringcmd`.
    pub fn client_stringcmd(mut self, time: f64, cmd: &str) -> Self {
        let mut w = MsgWriter::new();
        cs_proto48::messages::write_clc_stringcmd(&mut w, cmd);
        let data = self.client.build_packet(&w.into_bytes());
        self.records.push(CaptureRecord {
            dir: Direction::Sent,
            time,
            data,
        });
        self
    }

    /// The records, after a round trip through a capture file.
    pub fn build(self) -> Vec<CaptureRecord> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "cs-probe-synthetic-{}-{}.cs48cap",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut writer = CaptureWriter::create(&path).unwrap();
        for record in &self.records {
            writer.push(record).unwrap();
        }
        drop(writer);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        read_capture(&bytes).unwrap()
    }
}
