//! Readable dump of a golden capture: one line per message, in arrival order.
//!
//! [`decode_capture`] replays a capture through the same decoder the probe uses live and turns
//! every server message (and one summary per client datagram) into a [`DumpLine`]. The lines are
//! plain data so [`crate::diff`] can align two captures; [`format_dump`] prints them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::delta::{DeltaFieldDesc, DeltaRecord, DeltaSchema, DeltaValue};
use cs_proto48::messages::{
    ClcMessage, SvcMessage, SvcParseContext, entity_schema_name, parse_clc_stream,
    parse_svc_stream,
};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};
use cs_proto48::usermsg::CsUserMsg;

use crate::capture::{CaptureRecord, Direction};

/// Longest payload shown as hex before it is cut.
const MAX_HEX_BYTES: usize = 64;
/// Longest connectionless text shown.
const MAX_OOB_CHARS: usize = 48;

/// What happened to the entities in one `svc_packetentities` update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityInfo {
    /// Entities in the update.
    pub total: usize,
    /// Entities with a number above `max_clients`.
    pub non_player: usize,
    /// Names of the fields that changed on player entities (numbers `1..=max_clients`).
    pub player_fields: BTreeSet<String>,
}

/// One decoded message (or datagram summary) of a capture.
#[derive(Debug, Clone, PartialEq)]
pub struct DumpLine {
    pub time: f64,
    /// `Received` is server to client, `Sent` is client to server.
    pub dir: Direction,
    /// Index of the capture record the message came from.
    pub record: usize,
    /// `svc_stufftext`, `usermsg:Money`, `clc_stringcmd`, `oob:challenge`, ...
    pub kind: String,
    /// The payload as `(name, value)` pairs, values already formatted.
    pub fields: Vec<(String, String)>,
    /// For `svc_clientdata`: every field that is not zero, as the full state after the message.
    pub state: Vec<(String, String)>,
    /// Fields that are sets (the names in a resource list), compared as sets by the diff.
    pub sets: Vec<(String, BTreeSet<String>)>,
    /// Sent every frame (time, client data, entity updates, moves): counted, not aligned.
    pub per_update: bool,
    /// Set for entity updates only.
    pub entities: Option<EntityInfo>,
}

impl DumpLine {
    fn new(time: f64, dir: Direction, record: usize, kind: impl Into<String>) -> Self {
        Self {
            time,
            dir,
            record,
            kind: kind.into(),
            fields: Vec::new(),
            state: Vec::new(),
            sets: Vec::new(),
            per_update: false,
            entities: None,
        }
    }

    fn field(mut self, name: &str, value: impl Into<String>) -> Self {
        self.fields.push((name.to_string(), value.into()));
        self
    }

    /// The text of the line: time, direction, kind and `name=value` pairs.
    pub fn format(&self) -> String {
        let dir = match self.dir {
            Direction::Received => "S",
            Direction::Sent => "C",
        };
        let mut out = format!("{:9.3} {dir} {}", self.time, self.kind);
        for (name, value) in &self.fields {
            let _ = write!(out, " {name}={value}");
        }
        for (name, set) in &self.sets {
            let _ = write!(out, " {name}=[{} items]", set.len());
        }
        out
    }
}

/// A capture decoded into lines, with the counts a report needs.
#[derive(Debug, Clone, Default)]
pub struct Decoded {
    pub lines: Vec<DumpLine>,
    /// Records the server sent (every datagram, connectionless ones included).
    pub received: usize,
    /// Records the probe sent.
    pub sent: usize,
    /// Time of the first and last datagram the server sent.
    pub first_received: f64,
    pub last_received: f64,
    /// Every player name seen in `svc_updateuserinfo`, which the diff blanks out.
    pub names: BTreeSet<String>,
    /// Records that could not be decoded.
    pub errors: Vec<String>,
}

/// Which lines [`format_dump`] prints.
#[derive(Debug, Clone, Copy)]
pub struct DumpOptions {
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub entities: bool,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            from: None,
            to: None,
            entities: true,
        }
    }
}

/// The text of the dump of `decoded`: one line per message inside `[from, to]`, then a line for
/// each record that could not be decoded.
pub fn format_dump(decoded: &Decoded, opts: &DumpOptions) -> String {
    let mut out = String::new();
    for line in &decoded.lines {
        if !opts.entities && line.entities.is_some() {
            continue;
        }
        if opts.from.is_some_and(|from| line.time < from)
            || opts.to.is_some_and(|to| line.time > to)
        {
            continue;
        }
        out.push_str(&line.format());
        out.push('\n');
    }
    for err in &decoded.errors {
        let _ = writeln!(out, "error: {err}");
    }
    let _ = writeln!(
        out,
        "# {} datagrams from the server, {} from the client, {} errors",
        decoded.received,
        decoded.sent,
        decoded.errors.len()
    );
    out
}

/// Decoder state that spans messages: the last clientdata, weapon and entity records.
#[derive(Default)]
struct Tracker {
    clientdata: Option<DeltaRecord>,
    weapons: BTreeMap<u8, DeltaRecord>,
    entities: BTreeMap<u16, DeltaRecord>,
}

/// Replays `records` and decodes every message in them. A record that does not decode adds an
/// entry to [`Decoded::errors`] and the replay goes on.
pub fn decode_capture(records: &[CaptureRecord]) -> Decoded {
    let mut out = Decoded::default();
    let mut rx = NetChan::new(FRAGMENT_C2S_SIZE);
    let mut tx = NetChan::new(FRAGMENT_C2S_SIZE);
    let mut splits = SplitPacketAssembler::default();
    let mut ctx = SvcParseContext::new();
    let mut tracker = Tracker::default();
    let mut seen_received = false;

    for (i, record) in records.iter().enumerate() {
        match record.dir {
            Direction::Received => {
                out.received += 1;
                if !seen_received {
                    seen_received = true;
                    out.first_received = record.time;
                }
                out.last_received = record.time;
                let Some(datagram) = splits.push(&record.data) else {
                    continue;
                };
                if is_connectionless(&datagram) {
                    out.lines.push(oob_line(record, i, &datagram));
                    continue;
                }
                let incoming = match rx.process_packet(&datagram) {
                    Ok(Some(incoming)) => incoming,
                    Ok(None) => continue,
                    Err(e) => {
                        out.errors.push(format!("record {i}: netchan: {e}"));
                        continue;
                    }
                };
                ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;
                let streams =
                    std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
                for stream in streams {
                    match parse_svc_stream(&stream, &mut ctx) {
                        Ok(messages) => {
                            for msg in &messages {
                                // The netchan pads short packets with `svc_nop`.
                                if matches!(msg, SvcMessage::Nop) {
                                    continue;
                                }
                                let line = svc_line(msg, record.time, i, &ctx, &mut tracker);
                                if let SvcMessage::UpdateUserInfo { userinfo, .. } = msg
                                    && let Some(name) = userinfo_name(userinfo)
                                {
                                    out.names.insert(name);
                                }
                                out.lines.push(line);
                            }
                        }
                        Err(e) => out.errors.push(format!("record {i}: svc stream: {e}")),
                    }
                }
            }
            Direction::Sent => {
                out.sent += 1;
                if is_connectionless(&record.data) {
                    out.lines.push(oob_line(record, i, &record.data));
                    continue;
                }
                let incoming = match tx.process_packet(&record.data) {
                    Ok(Some(incoming)) => incoming,
                    Ok(None) => continue,
                    Err(e) => {
                        out.errors.push(format!("record {i}: client netchan: {e}"));
                        continue;
                    }
                };
                let Some(schema) = ctx.deltas.get("usercmd_t") else {
                    continue;
                };
                match parse_clc_stream(&incoming.payload, schema, incoming.sequence) {
                    Ok(messages) => {
                        for msg in &messages {
                            out.lines.push(clc_line(msg, record.time, i));
                        }
                    }
                    Err(e) => out.errors.push(format!("record {i}: clc stream: {e}")),
                }
            }
        }
    }
    out
}

fn is_connectionless(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == CONNECTIONLESS_HEADER.to_le_bytes()
}

/// A connectionless packet: its first word (`challenge`, `connect`, `getchallenge`, ...).
fn oob_line(record: &CaptureRecord, index: usize, datagram: &[u8]) -> DumpLine {
    let text: String = datagram
        .iter()
        .skip(4)
        .take(MAX_OOB_CHARS)
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { ' ' })
        .collect();
    let word = text.split([' ', '\n']).next().unwrap_or("").to_string();
    DumpLine::new(record.time, record.dir, index, format!("oob:{word}"))
}

fn clc_line(msg: &ClcMessage, time: f64, record: usize) -> DumpLine {
    let new = |kind: &str| DumpLine::new(time, Direction::Sent, record, kind);
    match msg {
        ClcMessage::Move {
            num_backup, cmds, ..
        } => {
            let mut line = new("clc_move").field(
                "cmds",
                cmds.len().saturating_sub(usize::from(*num_backup)).to_string(),
            );
            if let Some(newest) = cmds.first() {
                line = line
                    .field("msec", newest.msec.to_string())
                    .field("buttons", format!("0x{:x}", newest.buttons))
                    .field("impulse", newest.impulse.to_string())
                    .field("weaponselect", newest.weaponselect.to_string());
            }
            line.per_update = true;
            line
        }
        ClcMessage::StringCmd(cmd) => new("clc_stringcmd").field("cmd", format!("{cmd:?}")),
        ClcMessage::Delta(seq) => {
            let mut line = new("clc_delta").field("seq", seq.to_string());
            line.per_update = true;
            line
        }
        ClcMessage::ResourceList(list) => {
            new("clc_resourcelist").field("count", list.len().to_string())
        }
        ClcMessage::FileConsistency(data) => {
            new("clc_fileconsistency").field("bytes", data.len().to_string())
        }
        ClcMessage::VoiceData(data) => new("clc_voicedata").field("bytes", data.len().to_string()),
        ClcMessage::CvarValue(value) => new("clc_cvarvalue").field("value", format!("{value:?}")),
        ClcMessage::CvarValue2 {
            request_id,
            cvar_name,
            value,
        } => new("clc_cvarvalue2")
            .field("id", request_id.to_string())
            .field("cvar", cvar_name.clone())
            .field("value", format!("{value:?}")),
    }
}

/// The name in a `\key\value\...` userinfo string.
fn userinfo_name(userinfo: &str) -> Option<String> {
    let mut parts = userinfo.strip_prefix('\\').unwrap_or(userinfo).split('\\');
    while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
        if key == "name" {
            return Some(value.to_string());
        }
    }
    None
}

/// `userinfo` without its `name` pair.
fn userinfo_without_name(userinfo: &str) -> String {
    let mut out = String::new();
    let mut parts = userinfo.strip_prefix('\\').unwrap_or(userinfo).split('\\');
    while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
        if key != "name" {
            let _ = write!(out, "\\{key}\\{value}");
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in bytes.iter().take(MAX_HEX_BYTES) {
        let _ = write!(out, "{b:02x}");
    }
    if bytes.len() > MAX_HEX_BYTES {
        let _ = write!(out, "..(+{})", bytes.len() - MAX_HEX_BYTES);
    }
    if bytes.is_empty() {
        out.push_str("(empty)");
    }
    out
}

fn format_value(field: &DeltaFieldDesc, value: &DeltaValue) -> String {
    match value {
        DeltaValue::Float(v) => v.to_string(),
        DeltaValue::Str(s) => format!("{s:?}"),
        DeltaValue::Int(v) => v.to_string(),
        DeltaValue::Uint(v) if field.name == "weapons" => format!("0x{v:x}"),
        DeltaValue::Uint(v) => v.to_string(),
    }
}

/// The fields of `cur` that differ from `prev` (all non-zero fields when there is no `prev`),
/// named `prefix` + field name.
fn changed_fields(
    schema: &DeltaSchema,
    prev: Option<&DeltaRecord>,
    cur: &DeltaRecord,
    prefix: &str,
) -> Vec<(String, String)> {
    let zero = schema.zero_record();
    let prev = prev.unwrap_or(&zero);
    schema
        .fields
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let (a, b) = (prev.values.get(i)?, cur.values.get(i)?);
            (a != b).then(|| (format!("{prefix}{}", f.name), format_value(f, b)))
        })
        .collect()
}

fn svc_line(
    msg: &SvcMessage,
    time: f64,
    record: usize,
    ctx: &SvcParseContext,
    tracker: &mut Tracker,
) -> DumpLine {
    let new = |kind: String| DumpLine::new(time, Direction::Received, record, kind);
    let (name, generic) = split_debug(&format!("{msg:?}"));
    let kind = if let SvcMessage::PacketEntities(update) = msg {
        if update.is_delta {
            "svc_deltapacketentities".to_string()
        } else {
            "svc_packetentities".to_string()
        }
    } else {
        format!("svc_{}", name.to_lowercase())
    };
    let mut line = new(kind);
    match msg {
        SvcMessage::UserMsg {
            name, decoded, ..
        } => {
            line.kind = format!("usermsg:{name}");
            line.fields = match decoded {
                CsUserMsg::Unknown { payload, .. } => vec![("payload".to_string(), hex(payload))],
                other => split_debug(&format!("{other:?}")).1,
            };
        }
        SvcMessage::StuffText(text) | SvcMessage::Print(text) | SvcMessage::CenterPrint(text) => {
            line = line.field("text", format!("{text:?}"));
        }
        SvcMessage::NewUserMsg(desc) => {
            line.kind = format!("svc_newusermsg:{}", desc.name);
            line = line
                .field("id", desc.id.to_string())
                .field("size", desc.size.to_string());
        }
        SvcMessage::DeltaDescription(schema) => {
            line.kind = format!("svc_deltadescription:{}", schema.name);
            line = line.field("fields", schema.fields.len().to_string());
        }
        SvcMessage::SpawnBaseline {
            baselines,
            instance_baselines,
        } => {
            line = line
                .field("baselines", baselines.len().to_string())
                .field("instance_baselines", instance_baselines.len().to_string());
        }
        SvcMessage::ResourceList {
            resources,
            consistency_indices,
        } => {
            line = line
                .field("count", resources.len().to_string())
                .field("consistency", consistency_indices.len().to_string());
            line.sets.push((
                "resources".to_string(),
                resources
                    .iter()
                    .map(|r| format!("{}:{}", r.res_type, r.filename))
                    .collect(),
            ));
        }
        SvcMessage::UpdateUserInfo {
            slot,
            userid,
            userinfo,
            ..
        } => {
            line = line
                .field("slot", slot.to_string())
                .field("userid", userid.to_string())
                .field("name", userinfo_name(userinfo).unwrap_or_default())
                .field("userinfo", format!("{:?}", userinfo_without_name(userinfo)));
        }
        SvcMessage::TempEntity { te_type, raw } => {
            line = line
                .field("type", te_type.to_string())
                .field("payload", hex(raw));
        }
        SvcMessage::VoiceData { player_slot, data } => {
            line = line
                .field("slot", player_slot.to_string())
                .field("bytes", data.len().to_string());
        }
        SvcMessage::Time(t) => {
            line = line.field("time", t.to_string());
            line.per_update = true;
        }
        SvcMessage::Choke | SvcMessage::Pings(_) => line.per_update = true,
        SvcMessage::Event(events) => {
            line = line.field("count", events.len().to_string());
            for (i, e) in events.iter().enumerate() {
                line = line.field(
                    &format!("event{i}"),
                    format!(
                        "index={} packet_index={:?} fire_time={:?} args={:?}",
                        e.index, e.packet_index, e.fire_time, e.args
                    ),
                );
            }
        }
        SvcMessage::ClientData {
            record: rec,
            weapon_deltas,
            delta_seq,
            ..
        } => {
            line.per_update = true;
            line = line.field(
                "delta",
                delta_seq.map_or("none".to_string(), |s| s.to_string()),
            );
            if let Some(schema) = ctx.deltas.get("clientdata_t") {
                line.fields
                    .extend(changed_fields(schema, tracker.clientdata.as_ref(), rec, ""));
                line.state = changed_fields(schema, None, rec, "");
                tracker.clientdata = Some(rec.clone());
            }
            if let Some(schema) = ctx.deltas.get("weapon_data_t") {
                for (idx, wrec) in weapon_deltas {
                    let prefix = format!("weapon{idx}.");
                    line.fields.extend(changed_fields(
                        schema,
                        tracker.weapons.get(idx),
                        wrec,
                        &prefix,
                    ));
                    line.state.extend(changed_fields(schema, None, wrec, &prefix));
                    tracker.weapons.insert(*idx, wrec.clone());
                }
            }
        }
        SvcMessage::PacketEntities(update) => {
            line.per_update = true;
            let mut info = EntityInfo {
                total: update.entities.len(),
                ..EntityInfo::default()
            };
            line = line
                .field("count", update.entities.len().to_string())
                .field("removed", update.removed.len().to_string());
            let mut now = BTreeMap::new();
            for (&number, (ty, rec)) in &update.entities {
                let schema_name = entity_schema_name(number, *ty, ctx.max_clients);
                let player = number >= 1 && number <= u16::from(ctx.max_clients);
                if !player {
                    info.non_player += 1;
                }
                if let Some(schema) = ctx.deltas.get(schema_name) {
                    let prev = tracker.entities.get(&number);
                    let changes = changed_fields(schema, prev, rec, "");
                    if player {
                        info.player_fields
                            .extend(changes.iter().map(|(n, _)| n.clone()));
                    }
                    if prev.is_none() || !changes.is_empty() {
                        let list: Vec<String> =
                            changes.iter().map(|(n, v)| format!("{n}={v}")).collect();
                        let tag = if prev.is_none() { "new" } else { "changed" };
                        line = line.field(
                            &format!("#{number}"),
                            format!("{tag} type={ty} [{}]", list.join(" ")),
                        );
                    }
                }
                now.insert(number, rec.clone());
            }
            tracker.entities = now;
            line.entities = Some(info);
        }
        _ => line.fields = generic,
    }
    line
}

/// Splits the `Debug` text of a value into its type name and top-level fields: `Money { amount:
/// 800, flash: false }` gives `("Money", [("amount", "800"), ("flash", "false")])`; a tuple
/// variant's fields are named `v` (one field) or `0`, `1`, ...
pub(crate) fn split_debug(text: &str) -> (String, Vec<(String, String)>) {
    let name_end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .unwrap_or(text.len());
    let name = text[..name_end].to_string();
    let rest = text[name_end..].trim();
    let (open, close) = match rest.chars().next() {
        Some('{') => ('{', '}'),
        Some('(') => ('(', ')'),
        _ => return (name, Vec::new()),
    };
    let inner = rest
        .strip_prefix(open)
        .and_then(|r| r.trim_end().strip_suffix(close))
        .unwrap_or("")
        .trim();
    let parts = split_top_level(inner);
    let fields = if open == '{' {
        parts
            .into_iter()
            .map(|p| match p.split_once(':') {
                Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
                None => (String::new(), p),
            })
            .collect()
    } else if parts.len() == 1 {
        vec![("v".to_string(), parts[0].clone())]
    } else {
        parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| (i.to_string(), p))
            .collect()
    };
    (name, fields)
}

/// `text` cut at the commas that are outside brackets and strings.
fn split_top_level(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    let mut current = String::new();
    for c in text.chars() {
        if in_string {
            current.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                current.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let last = current.trim();
    if !last.is_empty() {
        parts.push(last.to_string());
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn debug_text_splits_into_fields() {
        let (name, fields) = split_debug("Money { amount: 800, flash: false }");
        assert_eq!(name, "Money");
        assert_eq!(
            fields,
            [
                ("amount".to_string(), "800".to_string()),
                ("flash".to_string(), "false".to_string())
            ]
        );
        let (_, fields) = split_debug("SayText { text: \"a, b: {c}\", params: [\"x\", \"y\"] }");
        assert_eq!(fields[0].1, "\"a, b: {c}\"");
        assert_eq!(fields[1].1, "[\"x\", \"y\"]");
        assert_eq!(split_debug("Health(100)").1, [("v".to_string(), "100".to_string())]);
        assert_eq!(split_debug("ResetHud"), ("ResetHud".to_string(), Vec::new()));
    }

    #[test]
    fn a_dump_lists_messages_in_arrival_order() {
        let cap = SyntheticCapture::new()
            .oob_from_server(0.0, b"challenge steam 1")
            .register(0.4)
            .server(0.5, |w, reg| {
                write_usermsg(w, reg, &CsUserMsg::Money { amount: 800, flash: true });
                write_stufftext(w, "cd stop\n");
            })
            .client_stringcmd(0.6, "jointeam 2")
            .server(0.7, |w, _| write_time(w, 1.5))
            .build();
        let decoded = decode_capture(&cap);
        assert!(decoded.errors.is_empty(), "{:?}", decoded.errors);
        let kinds: Vec<&str> = decoded.lines.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "oob:challenge",
                "svc_newusermsg:Money",
                "svc_newusermsg:RoundTime",
                "svc_newusermsg:HideWeapon",
                "svc_newusermsg:ResetHUD",
                "usermsg:Money",
                "svc_stufftext",
                "clc_stringcmd",
                "svc_time"
            ]
        );
        let text = format_dump(&decoded, &DumpOptions::default());
        assert!(text.contains("usermsg:Money amount=800 flash=true"), "{text}");
        assert!(text.contains("svc_stufftext text=\"cd stop\\n\""), "{text}");
        assert!(text.contains("clc_stringcmd cmd=\"jointeam 2\""), "{text}");
        assert!(text.contains("# 4 datagrams from the server, 1 from the client"), "{text}");
    }

    #[test]
    fn from_and_to_cut_the_dump_by_time() {
        let cap = SyntheticCapture::new()
            .server(1.0, |w, _| write_print(w, "one"))
            .server(2.0, |w, _| write_print(w, "two"))
            .server(3.0, |w, _| write_print(w, "three"))
            .build();
        let decoded = decode_capture(&cap);
        let text = format_dump(
            &decoded,
            &DumpOptions {
                from: Some(1.5),
                to: Some(2.5),
                entities: true,
            },
        );
        assert!(text.contains("two") && !text.contains("one") && !text.contains("three"), "{text}");
    }

    #[test]
    fn an_undecodable_record_is_reported_and_the_replay_goes_on() {
        let mut cap = SyntheticCapture::new()
            .server(1.0, |w, _| write_print(w, "before"))
            .build();
        // A server datagram that is not a netchan packet.
        cap.push(CaptureRecord {
            dir: Direction::Received,
            time: 1.5,
            data: vec![1, 2, 3],
        });
        let decoded = decode_capture(&cap);
        assert_eq!(decoded.lines.len(), 1);
        let text = format_dump(&decoded, &DumpOptions::default());
        assert!(text.contains("before"), "{text}");
    }

    #[test]
    fn user_name_comes_out_of_the_userinfo() {
        assert_eq!(userinfo_name("\\name\\Bob\\model\\gign"), Some("Bob".to_string()));
        assert_eq!(userinfo_without_name("\\name\\Bob\\model\\gign"), "\\model\\gign");
    }
}
