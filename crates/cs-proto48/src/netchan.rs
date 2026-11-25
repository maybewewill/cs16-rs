//! GoldSrc `netchan_t` reliable/unreliable transport, dual-stream fragmentation,
//! `COM_Munge2` packet obfuscation, and `-2` UDP split-packet reassembly
//! (`ReHLDS/engine/net_chan.cpp` & `net_ws.cpp`).

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::compress::decompress_stream;
use crate::crypto::{com_munge2, com_unmunge2};
use crate::SPLIT_PACKET_HEADER;

pub const MAX_STREAMS: usize = 2;
pub const FRAG_NORMAL_STREAM: usize = 0;
pub const FRAG_FILE_STREAM: usize = 1;

pub const MAX_RELIABLE_PAYLOAD: usize = 1200;
pub const MAX_MSGLEN: usize = 3990;
pub const FRAGMENT_S2C_MAX_SIZE: usize = 1024;
pub const FRAGMENT_C2S_SIZE: usize = 128;
/// Default limit, in bytes, on the fragments buffered for reassembly in one incoming stream.
pub const DEFAULT_MAX_INCOMING_STREAM: usize = 4 * 1024 * 1024;
/// Bytes each buffered fragment is charged against the incoming-stream limit at least, so that
/// tiny pieces cannot hold far more memory than the limit says.
pub const MIN_FRAGMENT_CHARGE: usize = 64;

const SVC_NOP: u8 = 1;

/// Bytes a buffered piece of `len` bytes is charged against the incoming-stream limit.
fn fragment_charge(len: usize) -> usize {
    len.max(MIN_FRAGMENT_CHARGE)
}

#[inline]
pub fn make_frag_id(id: u16, count: u16) -> u32 {
    ((id as u32) << 16) | (count as u32)
}

#[inline]
pub fn frag_get_id(fragid: u32) -> u16 {
    ((fragid >> 16) & 0xFFFF) as u16
}

#[inline]
pub fn frag_get_count(fragid: u32) -> u16 {
    (fragid & 0xFFFF) as u16
}

#[derive(Debug, Clone)]
struct OutFrag {
    buffer_id: u16,
    data: Vec<u8>,
}

/// Output produced by processing an inbound GoldSrc `NetChan` packet.
#[derive(Debug, Default, Clone)]
pub struct NetChanIncoming {
    /// Immediate payload (reliable + unreliable messages carried in this packet).
    pub payload: Vec<u8>,
    /// Fully reassembled normal-stream (`FRAG_NORMAL_STREAM`) payload, if completed on this packet,
    /// already BZ2-decompressed when it carried the `b"BZ2\0"` prefix.
    pub completed_normal_stream: Option<Vec<u8>>,
    /// Fully reassembled file-stream (`FRAG_FILE_STREAM`) payload, if completed on this packet.
    pub completed_file_stream: Option<Vec<u8>>,
    /// Number of dropped packets detected since the previous incoming sequence.
    pub dropped: u32,
    /// Parsed 30-bit incoming sequence number.
    pub sequence: u32,
    /// Parsed 30-bit sequence acknowledged by remote peer.
    pub sequence_ack: u32,
}

/// Stateful GoldSrc Protocol 48 `netchan_t` connection channel.
#[derive(Debug, Clone)]
pub struct NetChan {
    pub outgoing_sequence: u32,
    pub incoming_sequence: u32,
    pub incoming_acknowledged: u32,
    pub incoming_reliable_acknowledged: u32,
    pub incoming_reliable_sequence: u32,
    pub reliable_sequence: u32,
    pub last_reliable_sequence: u32,

    /// Queued outgoing reliable bytes (`chan->message`).
    reliable_queue: Vec<u8>,
    /// Currently in-flight reliable bytes awaiting acknowledgement (`chan->reliable_buf`).
    reliable_buf: Vec<u8>,
    /// Per-stream active fragment metadata for the in-flight reliable packet.
    reliable_fragment: [bool; MAX_STREAMS],
    reliable_fragid: [u32; MAX_STREAMS],
    frag_startpos: [u16; MAX_STREAMS],
    frag_length: [u16; MAX_STREAMS],

    /// Active outgoing fragment queue per stream (`chan->fragbufs`).
    fragbufs: [VecDeque<OutFrag>; MAX_STREAMS],
    fragbufcount: [u16; MAX_STREAMS],
    /// Waiting batches of outgoing fragments (`chan->waitlist`).
    waitlist: [VecDeque<(u16, VecDeque<OutFrag>)>; MAX_STREAMS],
    /// Filenames queued or actively sending on `FRAG_FILE_STREAM`, to reject duplicate requests.
    active_file_names: VecDeque<String>,

    /// Incoming fragment reassembly buffers per stream (`chan->incomingbufs`).
    incoming_frags: [BTreeMap<u16, Vec<u8>>; MAX_STREAMS],
    incoming_expected: [u16; MAX_STREAMS],
    /// Bytes charged for the pieces in `incoming_frags`, per stream (see [`fragment_charge`]).
    incoming_bytes: [usize; MAX_STREAMS],
    /// Limit on `incoming_bytes` of one stream.
    max_incoming_stream: usize,

    /// Default fragment size used when slicing large payloads.
    pub fragment_size: usize,
}

impl NetChan {
    /// Initialize a fresh `NetChan` (`Netchan_Setup`: `outgoing_sequence = 1`).
    pub fn new(fragment_size: usize) -> Self {
        Self {
            outgoing_sequence: 1,
            incoming_sequence: 0,
            incoming_acknowledged: 0,
            incoming_reliable_acknowledged: 0,
            incoming_reliable_sequence: 0,
            reliable_sequence: 0,
            last_reliable_sequence: 0,
            reliable_queue: Vec::new(),
            reliable_buf: Vec::new(),
            reliable_fragment: [false; MAX_STREAMS],
            reliable_fragid: [0; MAX_STREAMS],
            frag_startpos: [0; MAX_STREAMS],
            frag_length: [0; MAX_STREAMS],
            fragbufs: [VecDeque::new(), VecDeque::new()],
            fragbufcount: [0; MAX_STREAMS],
            waitlist: [VecDeque::new(), VecDeque::new()],
            active_file_names: VecDeque::new(),
            incoming_frags: [BTreeMap::new(), BTreeMap::new()],
            incoming_expected: [0; MAX_STREAMS],
            incoming_bytes: [0; MAX_STREAMS],
            max_incoming_stream: DEFAULT_MAX_INCOMING_STREAM,
            fragment_size: fragment_size.clamp(16, FRAGMENT_S2C_MAX_SIZE),
        }
    }

    /// Reset fragment queues and in-flight buffers (`Netchan_Clear`).
    pub fn clear(&mut self) {
        self.reliable_queue.clear();
        if !self.reliable_buf.is_empty() {
            self.reliable_sequence ^= 1;
            self.reliable_buf.clear();
        }
        self.reliable_fragment = [false; MAX_STREAMS];
        self.reliable_fragid = [0; MAX_STREAMS];
        self.frag_startpos = [0; MAX_STREAMS];
        self.frag_length = [0; MAX_STREAMS];
        for s in 0..MAX_STREAMS {
            self.fragbufs[s].clear();
            self.fragbufcount[s] = 0;
            self.waitlist[s].clear();
            self.discard_incoming(s);
        }
        self.active_file_names.clear();
    }

    /// Limits the bytes buffered for reassembly in each incoming stream; a fragment that would
    /// take a stream past it discards that stream's pieces and is refused. The same limit bounds
    /// the output of a compressed normal stream, so no reassembled stream is larger than it.
    pub fn set_max_incoming_stream(&mut self, bytes: usize) {
        self.max_incoming_stream = bytes;
    }

    /// Bytes charged for the pieces buffered for reassembly in both incoming streams: each
    /// piece counts its length, and at least [`MIN_FRAGMENT_CHARGE`].
    pub fn incoming_buffered_bytes(&self) -> usize {
        self.incoming_bytes.iter().sum()
    }

    /// Drops the pieces buffered in incoming stream `s`.
    fn discard_incoming(&mut self, s: usize) {
        self.incoming_frags[s].clear();
        self.incoming_expected[s] = 0;
        self.incoming_bytes[s] = 0;
    }

    /// Buffers piece `idx` of a `total`-piece message in incoming stream `s` and returns the
    /// reassembled message when this piece completes it. Pieces of a message with another
    /// total are discarded first.
    fn store_fragment(
        &mut self,
        s: usize,
        idx: u16,
        total: u16,
        piece: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, ProtoError> {
        if idx == 0 || idx > total {
            return Err(ProtoError::InvalidPayload(format!(
                "fragment stream {s}: index {idx} of {total}"
            )));
        }
        if piece.is_empty() || piece.len() > FRAGMENT_S2C_MAX_SIZE {
            return Err(ProtoError::InvalidPayload(format!(
                "fragment stream {s}: piece of {} bytes",
                piece.len()
            )));
        }
        if self.incoming_expected[s] != total {
            self.discard_incoming(s);
        }
        let replaced = self.incoming_frags[s]
            .get(&idx)
            .map_or(0, |old| fragment_charge(old.len()));
        let buffered = self.incoming_bytes[s] - replaced + fragment_charge(piece.len());
        if buffered > self.max_incoming_stream {
            self.discard_incoming(s);
            return Err(ProtoError::StreamRefused(format!(
                "fragment stream {s}: {buffered} bytes buffered, limit {}",
                self.max_incoming_stream
            )));
        }
        self.incoming_expected[s] = total;
        self.incoming_bytes[s] = buffered;
        self.incoming_frags[s].insert(idx, piece);
        if self.incoming_frags[s].len() < usize::from(total) {
            return Ok(None);
        }
        let frags = std::mem::take(&mut self.incoming_frags[s]);
        self.discard_incoming(s);
        Ok(Some(frags.into_values().flatten().collect()))
    }

    /// Append bytes to the reliable outgoing stream (`chan->message`).
    pub fn write_reliable(&mut self, data: &[u8]) {
        self.reliable_queue.extend_from_slice(data);
    }

    /// Queue a large payload as a GoldSrc `FRAG_NORMAL_STREAM` fragment sequence
    /// (`Netchan_CreateFragments`).
    pub fn create_fragments(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let chunk_size = self.fragment_size.max(16);
        let mut frags = VecDeque::new();
        for (idx, chunk) in data.chunks(chunk_size).enumerate() {
            frags.push_back(OutFrag {
                buffer_id: (idx + 1) as u16,
                data: chunk.to_vec(),
            });
        }
        let total = frags.len() as u16;
        self.waitlist[FRAG_NORMAL_STREAM].push_back((total, frags));
    }

    /// Whether a file with `filename` is currently queued or transferring on `FRAG_FILE_STREAM`
    /// (`Netchan_IsFileTransferActive`, ReHLDS `net_chan.cpp:1145`, read 2026-11-25).
    pub fn is_file_transfer_active(&self, filename: &str) -> bool {
        self.active_file_names
            .iter()
            .any(|f| f.eq_ignore_ascii_case(filename))
    }

    /// Queues a file stream transfer on `FRAG_FILE_STREAM` using GoldSrc's `Netchan_CreateFileFragmentsFromBuffer`
    /// framing: `filename\0` + compressor (`""`) + `u32` uncompressed byte size + payload.
    /// Rejects empty payloads or duplicate active transfers for the same filename.
    pub fn create_file_fragments(&mut self, filename: &str, data: &[u8], _try_bz2: bool) -> bool {
        if filename.is_empty() || filename.contains('\0') || data.is_empty() {
            return false;
        }
        if self.is_file_transfer_active(filename) {
            return true;
        }
        let Ok(uncompressed_len) = u32::try_from(data.len()) else {
            return false;
        };
        let mut framed = Vec::with_capacity(filename.len() + 6 + data.len());
        framed.extend_from_slice(filename.as_bytes());
        framed.push(0);
        // Workspace `bzip2-rs` only decodes BZ2 streams cleanly without unsafe code, so send
        // uncompressed with `compressor = ""` (ReHLDS `net_chan.cpp:1156` fallback).
        framed.push(0);
        framed.extend_from_slice(&uncompressed_len.to_le_bytes());
        framed.extend_from_slice(data);

        let chunk_size = self.fragment_size.max(16);
        let mut frags = VecDeque::new();
        for (idx, chunk) in framed.chunks(chunk_size).enumerate() {
            let Ok(id) = u16::try_from(idx + 1) else {
                return false;
            };
            frags.push_back(OutFrag {
                buffer_id: id,
                data: chunk.to_vec(),
            });
        }
        let Ok(total) = u16::try_from(frags.len()) else {
            return false;
        };
        self.active_file_names.push_back(filename.to_string());
        self.waitlist[FRAG_FILE_STREAM].push_back((total, frags));
        true
    }

    /// `(received_chunks, expected_total_chunks)` for an in-progress `FRAG_FILE_STREAM` download.
    pub fn incoming_file_progress(&self) -> Option<(usize, usize)> {
        let total = usize::from(self.incoming_expected[FRAG_FILE_STREAM]);
        (total > 0).then(|| (self.incoming_frags[FRAG_FILE_STREAM].len(), total))
    }

    /// Whether there are still reliable bytes or fragments waiting to be transmitted or acked.
    pub fn has_pending_reliable(&self) -> bool {
        !self.reliable_queue.is_empty()
            || !self.reliable_buf.is_empty()
            || !self.fragbufs[0].is_empty()
            || !self.fragbufs[1].is_empty()
            || !self.waitlist[0].is_empty()
            || !self.waitlist[1].is_empty()
    }

    /// Reliable bytes not yet acknowledged: the queue, the in-flight buffer and every fragment
    /// still queued or waiting.
    pub fn pending_reliable_bytes(&self) -> usize {
        let fragments: usize = (0..MAX_STREAMS)
            .map(|s| {
                let active: usize = self.fragbufs[s].iter().map(|f| f.data.len()).sum();
                let waiting: usize = self.waitlist[s]
                    .iter()
                    .flat_map(|(_, frags)| frags.iter())
                    .map(|f| f.data.len())
                    .sum();
                active + waiting
            })
            .sum();
        self.reliable_queue.len() + self.reliable_buf.len() + fragments
    }

    /// Bytes the next [`build_packet`](Self::build_packet) writes ahead of its unreliable
    /// payload: the 8-byte header, the fragment descriptors and the reliable data it will carry.
    /// The packet is this plus the unreliable bytes, padded to at least 16 bytes.
    pub fn next_packet_reliable_len(&self) -> usize {
        const HEADER: usize = 8;
        let resend = self.incoming_acknowledged > self.last_reliable_sequence
            && self.incoming_reliable_acknowledged != self.reliable_sequence;
        if !self.reliable_buf.is_empty() {
            if !resend {
                return HEADER;
            }
            let fragments = self.reliable_fragment.iter().filter(|&&f| f).count();
            let descriptors = if fragments > 0 { 2 + 8 * fragments } else { 0 };
            return HEADER + descriptors + self.reliable_buf.len();
        }

        // Mirrors the new-data branch of `build_packet` without changing anything.
        let queue = self.reliable_queue.len();
        let queue_fragmented = queue > MAX_MSGLEN;
        let chunk = self.fragment_size.max(16);
        let first_fragment = |s: usize| -> Option<usize> {
            if let Some(front) = self.fragbufs[s].front() {
                return Some(front.data.len());
            }
            if let Some((_, frags)) = self.waitlist[s].front() {
                return frags.front().map(|f| f.data.len());
            }
            (s == FRAG_NORMAL_STREAM && queue_fragmented).then(|| queue.min(chunk))
        };
        let fronts = [first_fragment(0), first_fragment(1)];
        let regular = !queue_fragmented && queue > 0 && fronts[FRAG_NORMAL_STREAM].is_none();
        let mut reliable = if regular { queue } else { 0 };
        let mut fragments = 0;
        for len in fronts.into_iter().flatten() {
            if reliable + len < MAX_RELIABLE_PAYLOAD {
                reliable += len;
                fragments += 1;
            }
        }
        let descriptors = if fragments > 0 { 2 + 8 * fragments } else { 0 };
        HEADER + descriptors + reliable
    }

    fn promote_waitlist(&mut self) {
        for s in 0..MAX_STREAMS {
            if self.fragbufs[s].is_empty()
                && let Some((total, frags)) = self.waitlist[s].pop_front()
            {
                self.fragbufcount[s] = total;
                self.fragbufs[s] = frags;
            }
        }
    }

    /// Build the next wire-ready UDP packet (`Netchan_Transmit`), applying
    /// 16-byte `svc_nop` padding and `COM_Munge2` obfuscation from byte 8 onward.
    pub fn build_packet(&mut self, unreliable: &[u8]) -> Vec<u8> {
        let mut send_reliable = false;

        if self.incoming_acknowledged > self.last_reliable_sequence
            && self.incoming_reliable_acknowledged != self.reliable_sequence
        {
            send_reliable = true;
        }

        if self.reliable_buf.is_empty() {
            if self.reliable_queue.len() > MAX_MSGLEN {
                let overflow = std::mem::take(&mut self.reliable_queue);
                self.create_fragments(&overflow);
            }

            self.promote_waitlist();

            let mut send_from_regular = !self.reliable_queue.is_empty();
            let send_from_frag = [!self.fragbufs[0].is_empty(), !self.fragbufs[1].is_empty()];

            if send_from_regular && send_from_frag[FRAG_NORMAL_STREAM] {
                send_from_regular = false;
                if self.reliable_queue.len() > MAX_RELIABLE_PAYLOAD {
                    let overflow = std::mem::take(&mut self.reliable_queue);
                    self.create_fragments(&overflow);
                }
            }

            let mut send_frag = false;
            for (s, &from_frag) in send_from_frag.iter().enumerate() {
                self.frag_startpos[s] = 0;
                self.reliable_fragment[s] = false;
                self.reliable_fragid[s] = 0;
                self.frag_length[s] = 0;
                if from_frag {
                    send_frag = true;
                }
            }

            if send_from_regular || send_frag {
                self.reliable_sequence ^= 1;
                send_reliable = true;
            }

            if send_from_regular {
                self.reliable_buf = std::mem::take(&mut self.reliable_queue);
                let base_len = self.reliable_buf.len() as u16;
                self.frag_startpos.fill(base_len);
            }

            for (s, &from_frag) in send_from_frag.iter().enumerate() {
                if from_frag
                    && let Some(front) = self.fragbufs[s].front()
                    && (self.reliable_buf.len() + front.data.len()) < MAX_RELIABLE_PAYLOAD
                    && let Some(frag) = self.fragbufs[s].pop_front()
                {
                    self.reliable_fragid[s] = make_frag_id(frag.buffer_id, self.fragbufcount[s]);
                    let flen = frag.data.len() as u16;
                    self.reliable_buf.extend_from_slice(&frag.data);
                    self.frag_length[s] = flen;
                    self.reliable_fragment[s] = true;
                    for pos in self.frag_startpos.iter_mut().skip(s + 1) {
                        *pos = pos.saturating_add(flen);
                    }
                }
            }
        }

        let send_reliable_fragment = self.reliable_fragment[0] || self.reliable_fragment[1];
        let mut w1 = self.outgoing_sequence | ((send_reliable as u32) << 31);
        let w2 = self.incoming_sequence | (self.incoming_reliable_sequence << 31);
        if send_reliable && send_reliable_fragment {
            w1 |= 1 << 30;
        }

        let seq_this = self.outgoing_sequence;
        self.outgoing_sequence = self.outgoing_sequence.wrapping_add(1);

        let mut pkt = MsgWriter::with_capacity(64 + self.reliable_buf.len() + unreliable.len());
        pkt.write_u32(w1);
        pkt.write_u32(w2);

        if send_reliable && send_reliable_fragment {
            for s in 0..MAX_STREAMS {
                if self.reliable_fragment[s] {
                    pkt.write_u8(1);
                    pkt.write_u32(self.reliable_fragid[s]);
                    pkt.write_u16(self.frag_startpos[s]);
                    pkt.write_u16(self.frag_length[s]);
                } else {
                    pkt.write_u8(0);
                }
            }
        }

        if send_reliable {
            pkt.write_bytes(&self.reliable_buf);
            self.last_reliable_sequence = seq_this;
        }

        if !unreliable.is_empty() {
            pkt.write_bytes(unreliable);
        }

        while pkt.len() < 16 {
            pkt.write_u8(SVC_NOP);
        }

        let mut bytes = pkt.into_bytes();
        com_munge2(&mut bytes[8..], (seq_this as u8) as i32);
        bytes
    }

    /// Process an inbound GoldSrc `NetChan` UDP packet (`Netchan_Process`).
    ///
    /// Returns `Ok(None)` for duplicate or out-of-order packets (ignored cleanly).
    pub fn process_packet(&mut self, raw: &[u8]) -> Result<Option<NetChanIncoming>, ProtoError> {
        if raw.len() < 8 {
            return Err(ProtoError::UnexpectedEof);
        }
        let mut buf = raw.to_vec();
        let w1 = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let w2 = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);

        let reliable_message = w1 >> 31;
        let reliable_ack = w2 >> 31;
        let has_fragments = (w1 & (1 << 30)) != 0;

        let sequence = w1 & 0x3FFF_FFFF;
        let sequence_ack = w2 & 0x3FFF_FFFF;

        com_unmunge2(&mut buf[8..], (sequence & 0xFF) as i32);

        let mut reader = MsgReader::new(&buf);
        let _ = reader.read_u32()?;
        let _ = reader.read_u32()?;

        let mut frag_present = [false; MAX_STREAMS];
        let mut frag_id = [0u32; MAX_STREAMS];
        let mut frag_offset = [0usize; MAX_STREAMS];
        let mut frag_length = [0usize; MAX_STREAMS];

        if has_fragments {
            for s in 0..MAX_STREAMS {
                if reader.read_u8()? != 0 {
                    frag_present[s] = true;
                    frag_id[s] = reader.read_u32()?;
                    frag_offset[s] = reader.read_u16()? as usize;
                    frag_length[s] = reader.read_u16()? as usize;
                }
            }
        }

        if sequence <= self.incoming_sequence {
            return Ok(None);
        }

        let dropped = sequence.saturating_sub(self.incoming_sequence.saturating_add(1));

        if reliable_ack == self.reliable_sequence && sequence_ack >= self.last_reliable_sequence {
            self.reliable_buf.clear();
            if self.reliable_fragment[FRAG_FILE_STREAM] && self.fragbufs[FRAG_FILE_STREAM].is_empty()
            {
                let _ = self.active_file_names.pop_front();
            }
        }

        self.incoming_sequence = sequence;
        self.incoming_acknowledged = sequence_ack;
        self.incoming_reliable_acknowledged = reliable_ack;
        if reliable_message != 0 {
            self.incoming_reliable_sequence ^= 1;
        }

        let body_start = reader.pos();
        let mut body = buf[body_start..].to_vec();
        let mut completed = [None, None];

        if has_fragments {
            for s in 0..MAX_STREAMS {
                if !frag_present[s] {
                    continue;
                }
                let off = frag_offset[s];
                let len = frag_length[s];
                if off.saturating_add(len) > body.len() {
                    return Err(ProtoError::InvalidPayload(format!(
                        "fragment stream {s} out of bounds (off={off}, len={len}, body={})",
                        body.len()
                    )));
                }
                let piece = body[off..off + len].to_vec();
                body.drain(off..off + len);
                for next_s in (s + 1)..MAX_STREAMS {
                    if frag_present[next_s] && frag_offset[next_s] >= len {
                        frag_offset[next_s] -= len;
                    }
                }

                let idx = frag_get_id(frag_id[s]);
                let total = frag_get_count(frag_id[s]);
                completed[s] = self.store_fragment(s, idx, total, piece)?;
            }
        }

        let completed_normal_stream = completed[FRAG_NORMAL_STREAM]
            .take()
            .map(|stream| {
                decompress_stream(&stream, self.max_incoming_stream)
                    .map_err(|e| ProtoError::StreamRefused(e.to_string()))
            })
            .transpose()?;

        Ok(Some(NetChanIncoming {
            payload: body,
            completed_normal_stream,
            completed_file_stream: completed[FRAG_FILE_STREAM].take(),
            dropped,
            sequence,
            sequence_ack,
        }))
    }
}

/// Reassembles GoldSrc `-2` (`0xFFFF_FFFE`) split UDP datagrams (used by large A2S_RULES /
/// multi-MTU packets).
#[derive(Debug, Default)]
pub struct SplitPacketAssembler {
    in_flight: HashMap<i32, (u8, BTreeMap<u8, Vec<u8>>)>,
}

impl SplitPacketAssembler {
    /// Feed a raw UDP packet. Returns `Some(full_packet)` if the packet was not split or if
    /// this chunk completed a split packet.
    pub fn push(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        if packet.len() < 4 {
            return Some(packet.to_vec());
        }
        let header = i32::from_le_bytes([packet[0], packet[1], packet[2], packet[3]]);
        if header != SPLIT_PACKET_HEADER {
            return Some(packet.to_vec());
        }
        if packet.len() < 9 {
            return None;
        }
        let seq_id = i32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
        let pbyte = packet[8];
        let pkt_idx = (pbyte >> 4) & 0x0F;
        let pkt_total = pbyte & 0x0F;
        if pkt_total == 0 {
            return None;
        }
        let entry = self
            .in_flight
            .entry(seq_id)
            .or_insert_with(|| (pkt_total, BTreeMap::new()));
        entry.1.insert(pkt_idx, packet[9..].to_vec());
        if entry.1.len() == entry.0 as usize {
            let (_, parts) = self.in_flight.remove(&seq_id)?;
            let mut full = Vec::new();
            for (_, chunk) in parts {
                full.extend_from_slice(&chunk);
            }
            Some(full)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netchan_roundtrips_reliable_unreliable_and_multi_fragment_streams() {
        let mut server = NetChan::new(64);
        let mut client = NetChan::new(64);

        let large_signon: Vec<u8> = (0..220u16).map(|i| ((i * 7 + 13) & 0xFF) as u8).collect();
        server.create_fragments(&large_signon);

        let mut assembled = None;
        for step in 0..10 {
            let pkt = server.build_packet(&[0x07, step]);
            let inc = client.process_packet(&pkt).unwrap().unwrap();
            if let Some(full) = inc.completed_normal_stream {
                assembled = Some(full);
                break;
            }
            // Client sends ACK packet back so server advances reliable fragment buffer.
            let ack = client.build_packet(&[]);
            let _ = server.process_packet(&ack).unwrap();
        }

        assert_eq!(assembled, Some(large_signon));
    }

    #[test]
    fn pending_reliable_bytes_counts_queue_in_flight_and_fragments() {
        let mut server = NetChan::new(64);
        let mut client = NetChan::new(64);
        assert_eq!(server.pending_reliable_bytes(), 0);

        server.write_reliable(&[1; 10]);
        assert_eq!(server.pending_reliable_bytes(), 10);
        // In flight after sending, still pending until acknowledged.
        let pkt = server.build_packet(&[]);
        assert_eq!(server.pending_reliable_bytes(), 10);
        server.write_reliable(&[2; 5]);
        assert_eq!(server.pending_reliable_bytes(), 15);

        client.process_packet(&pkt).unwrap().unwrap();
        server
            .process_packet(&client.build_packet(&[]))
            .unwrap()
            .unwrap();
        assert_eq!(server.pending_reliable_bytes(), 5);

        // Two fragment batches: one becomes active, the other waits.
        server.create_fragments(&[3; 100]);
        server.create_fragments(&[4; 70]);
        assert_eq!(server.pending_reliable_bytes(), 175);
        server.build_packet(&[]);
        assert_eq!(server.pending_reliable_bytes(), 175);
    }

    /// A packet with sequence `seq` carrying `piece` as fragment `idx` of `total` in the
    /// normal stream, munged like `build_packet` does.
    fn fragment_packet(seq: u32, idx: u16, total: u16, piece: &[u8]) -> Vec<u8> {
        let mut w = MsgWriter::new();
        w.write_u32(seq | (1 << 31) | (1 << 30));
        w.write_u32(0);
        w.write_u8(1);
        w.write_u32(make_frag_id(idx, total));
        w.write_u16(0);
        w.write_u16(piece.len() as u16);
        w.write_u8(0);
        w.write_bytes(piece);
        let mut bytes = w.into_bytes();
        com_munge2(&mut bytes[8..], (seq as u8) as i32);
        bytes
    }

    #[test]
    fn fragment_index_zero_is_refused() {
        let mut chan = NetChan::new(64);
        assert!(
            chan.process_packet(&fragment_packet(1, 0, 2, &[1; 4]))
                .is_err()
        );
        assert_eq!(chan.incoming_buffered_bytes(), 0);
    }

    #[test]
    fn fragment_index_above_total_is_refused() {
        let mut chan = NetChan::new(64);
        assert!(
            chan.process_packet(&fragment_packet(1, 3, 2, &[1; 4]))
                .is_err()
        );
        assert_eq!(chan.incoming_buffered_bytes(), 0);
    }

    #[test]
    fn oversized_fragment_piece_is_refused() {
        let mut chan = NetChan::new(64);
        let big = vec![1; FRAGMENT_S2C_MAX_SIZE + 1];
        assert!(
            chan.process_packet(&fragment_packet(1, 1, 2, &big))
                .is_err()
        );
        assert_eq!(chan.incoming_buffered_bytes(), 0);
        let max = vec![1; FRAGMENT_S2C_MAX_SIZE];
        assert!(chan.process_packet(&fragment_packet(2, 1, 2, &max)).is_ok());
        assert_eq!(chan.incoming_buffered_bytes(), FRAGMENT_S2C_MAX_SIZE);
    }

    #[test]
    fn empty_fragment_piece_is_refused() {
        let mut chan = NetChan::new(64);
        assert!(chan.process_packet(&fragment_packet(1, 1, 2, &[])).is_err());
        assert_eq!(chan.incoming_buffered_bytes(), 0);
        // A piece of a message already being buffered is refused too and leaves it intact.
        chan.process_packet(&fragment_packet(2, 1, 3, &[1; 4]))
            .unwrap();
        assert!(chan.process_packet(&fragment_packet(3, 2, 3, &[])).is_err());
        assert_eq!(chan.incoming_buffered_bytes(), MIN_FRAGMENT_CHARGE);
    }

    /// `next_packet_reliable_len` matches what `build_packet(&[])` then writes, padding aside.
    fn assert_next_len_matches(chan: &NetChan) {
        let predicted = chan.next_packet_reliable_len();
        let built = chan.clone().build_packet(&[]).len();
        assert_eq!(
            built,
            predicted.max(16),
            "predicted {predicted}, built {built}"
        );
    }

    #[test]
    fn next_packet_reliable_len_predicts_build_packet() {
        let mut server = NetChan::new(FRAGMENT_S2C_MAX_SIZE);
        let mut client = NetChan::new(FRAGMENT_C2S_SIZE);
        assert_eq!(server.next_packet_reliable_len(), 8);
        assert_next_len_matches(&server);

        // A plain reliable message.
        server.write_reliable(&[1; 300]);
        assert_eq!(server.next_packet_reliable_len(), 308);
        assert_next_len_matches(&server);
        let pkt = server.build_packet(&[]);
        // In flight and not yet acknowledged: nothing reliable goes out.
        assert_eq!(server.next_packet_reliable_len(), 8);
        assert_next_len_matches(&server);

        // The packet is lost; the client acknowledges a later one: the server resends it.
        drop(pkt);
        let later = server.build_packet(&[]);
        client.process_packet(&later).unwrap().unwrap();
        server
            .process_packet(&client.build_packet(&[]))
            .unwrap()
            .unwrap();
        assert_eq!(server.next_packet_reliable_len(), 308);
        assert_next_len_matches(&server);

        // Acknowledged: a queue bigger than MAX_MSGLEN goes out as fragments.
        let pkt = server.build_packet(&[]);
        client.process_packet(&pkt).unwrap().unwrap();
        server
            .process_packet(&client.build_packet(&[]))
            .unwrap()
            .unwrap();
        assert_eq!(server.pending_reliable_bytes(), 0);
        server.write_reliable(&[2; MAX_MSGLEN + 500]);
        assert_eq!(
            server.next_packet_reliable_len(),
            8 + 2 + 8 + FRAGMENT_S2C_MAX_SIZE
        );
        assert_next_len_matches(&server);
        let pkt = server.build_packet(&[]);
        client.process_packet(&pkt).unwrap().unwrap();
        server
            .process_packet(&client.build_packet(&[]))
            .unwrap()
            .unwrap();

        // Fragments under way plus a small queued message: the next fragment goes first.
        server.write_reliable(&[3; 50]);
        assert_next_len_matches(&server);
        // Fragments of both streams at once, and a queue too large to wait.
        let mut both = NetChan::new(100);
        both.create_fragments(&[4; 250]);
        both.waitlist[FRAG_FILE_STREAM].push_back((
            1,
            VecDeque::from([OutFrag {
                buffer_id: 1,
                data: vec![5; 40],
            }]),
        ));
        both.write_reliable(&[6; MAX_RELIABLE_PAYLOAD + 10]);
        assert_next_len_matches(&both);
        assert_eq!(both.next_packet_reliable_len(), 8 + 2 + 16 + 100 + 40);
    }

    #[test]
    fn fragment_of_another_message_discards_buffered_pieces() {
        let mut chan = NetChan::new(64);
        let inc = chan.process_packet(&fragment_packet(1, 1, 3, &[9; 10]));
        assert!(inc.unwrap().unwrap().completed_normal_stream.is_none());
        assert_eq!(chan.incoming_buffered_bytes(), MIN_FRAGMENT_CHARGE);

        // A two-piece message starts: the piece of the three-piece one is dropped.
        chan.process_packet(&fragment_packet(2, 1, 2, &[1; 5]))
            .unwrap();
        assert_eq!(chan.incoming_buffered_bytes(), MIN_FRAGMENT_CHARGE);
        let inc = chan
            .process_packet(&fragment_packet(3, 2, 2, &[2; 3]))
            .unwrap()
            .unwrap();
        assert_eq!(
            inc.completed_normal_stream,
            Some(vec![1, 1, 1, 1, 1, 2, 2, 2])
        );
        assert_eq!(chan.incoming_buffered_bytes(), 0);
    }

    #[test]
    fn exceeding_the_incoming_limit_empties_the_stream() {
        let mut chan = NetChan::new(64);
        chan.set_max_incoming_stream(150);
        chan.process_packet(&fragment_packet(1, 1, 3, &[1; 80]))
            .unwrap();
        assert_eq!(chan.incoming_buffered_bytes(), 80);
        assert!(
            chan.process_packet(&fragment_packet(2, 2, 3, &[2; 80]))
                .is_err()
        );
        assert_eq!(chan.incoming_buffered_bytes(), 0);

        // The channel still works: a fragment and a plain packet go through.
        let inc = chan
            .process_packet(&fragment_packet(3, 1, 1, &[3; 30]))
            .unwrap()
            .unwrap();
        assert_eq!(inc.completed_normal_stream, Some(vec![3; 30]));
        let mut peer = NetChan::new(64);
        peer.outgoing_sequence = 4;
        let inc = chan
            .process_packet(&peer.build_packet(&[7]))
            .unwrap()
            .unwrap();
        assert_eq!(inc.payload[0], 7);
    }

    #[test]
    fn tiny_fragments_are_charged_the_minimum() {
        let mut chan = NetChan::new(64);
        let limit = 10 * MIN_FRAGMENT_CHARGE;
        chan.set_max_incoming_stream(limit);
        // One-byte pieces of a message that never completes: the stream holds
        // `limit / MIN_FRAGMENT_CHARGE` of them, not `limit`.
        for idx in 1..=10u16 {
            chan.process_packet(&fragment_packet(u32::from(idx), idx, 1000, &[idx as u8]))
                .unwrap();
            assert_eq!(
                chan.incoming_buffered_bytes(),
                usize::from(idx) * MIN_FRAGMENT_CHARGE
            );
        }
        assert!(
            chan.process_packet(&fragment_packet(11, 11, 1000, &[11]))
                .is_err()
        );
        assert_eq!(chan.incoming_buffered_bytes(), 0);
    }

    #[test]
    fn file_stream_fragments_deduplicate_and_report_progress() {
        let mut server = NetChan::new(FRAGMENT_S2C_MAX_SIZE);
        let mut client = NetChan::new(FRAGMENT_C2S_SIZE);
        let payload = vec![0xABu8; 2500];
        assert!(server.create_file_fragments("maps/de_test.bsp", &payload, false));
        assert!(server.is_file_transfer_active("MAPS/DE_TEST.BSP"));
        assert!(!server.create_file_fragments("", &payload, false));
        assert!(!server.create_file_fragments("bad\0name", &payload, false));
        assert!(!server.create_file_fragments("empty.txt", &[], false));
        // Duplicate active transfer returns true without queuing a second copy.
        assert!(server.create_file_fragments("maps/de_test.bsp", &payload, false));
        assert_eq!(server.waitlist[FRAG_FILE_STREAM].len(), 1);

        let mut completed_file: Option<Vec<u8>> = None;
        let mut saw_progress = false;
        for _ in 0..32 {
            let pkt = server.build_packet(&[]);
            let ev = client.process_packet(&pkt).unwrap();
            if let Some(inc) = ev
                && let Some(f) = inc.completed_file_stream
            {
                completed_file = Some(f);
            }
            if let Some((got, total)) = client.incoming_file_progress() {
                assert!(total >= 2);
                assert!(got <= total);
                saw_progress = true;
            }
            let ack = client.build_packet(&[]);
            server.process_packet(&ack).unwrap();
            if completed_file.is_some() && !server.is_file_transfer_active("maps/de_test.bsp") {
                break;
            }
        }
        assert!(saw_progress);
        assert!(!server.is_file_transfer_active("maps/de_test.bsp"));
        let raw = completed_file.expect("client must reassemble FRAG_FILE_STREAM");
        let name_end = raw.iter().position(|&b| b == 0).unwrap();
        assert_eq!(&raw[..name_end], b"maps/de_test.bsp");
        let comp_end = name_end + 1 + raw[name_end + 1..].iter().position(|&b| b == 0).unwrap();
        assert_eq!(&raw[name_end + 1..comp_end], b"");
        let size_start = comp_end + 1;
        let declared_len = u32::from_le_bytes(raw[size_start..size_start + 4].try_into().unwrap());
        assert_eq!(declared_len as usize, payload.len());
        assert_eq!(&raw[size_start + 4..], payload.as_slice());
    }
}
