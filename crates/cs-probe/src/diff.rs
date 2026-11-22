//! Difference between two decoded captures.
//!
//! The captures are cut into phases at every string command the probe sent (`new`, `sendres`,
//! `spawn`, `sendents`, `jointeam`, ...), so a phase is what the server sent in answer to one
//! command, and phases are matched by command word and occurrence (`jointeam#0`). Inside a
//! phase the messages that are not sent every frame are aligned by a longest common
//! subsequence over their kinds; matched pairs are compared field by field, the rest are
//! reported as present in one capture only or as in a different place. The per-frame messages
//! (time, client data, entity updates) are counted, and the first and last client data of the
//! phase are compared.
//!
//! Normalised away: player names, timestamps (`svc_time`), `random_seed` fields, connectionless
//! text, the arguments of string commands, and entity numbers above the players.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::capture::Direction;
use crate::dump::{Decoded, DumpLine};

/// Longest detail text of a message that is in one capture only.
const MAX_DETAIL: usize = 140;
/// More moved messages than this in a phase are summarised in one line.
const MAX_MOVED_LISTED: usize = 8;
/// What stands for a player name in compared values.
const PLAYER: &str = "<player>";

#[derive(Debug, Clone, Copy)]
pub struct DiffOptions {
    /// Compare what the entity updates carry (player fields changed, non-player entity count).
    pub entities: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self { entities: true }
    }
}

/// One difference inside a phase.
#[derive(Debug, Clone, PartialEq)]
pub enum Difference {
    OnlyInA {
        kind: String,
        detail: String,
    },
    OnlyInB {
        kind: String,
        detail: String,
    },
    /// The message is in both captures, at different places.
    Moved {
        kind: String,
        pos_a: usize,
        pos_b: usize,
    },
    Field {
        kind: String,
        name: String,
        a: String,
        b: String,
    },
    /// A field that is a set differs.
    Set {
        kind: String,
        name: String,
        only_a: Vec<String>,
        only_b: Vec<String>,
    },
}

/// What one capture did in one phase.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhaseSide {
    pub present: bool,
    pub first_time: f64,
    pub last_time: f64,
    /// Server datagrams that carried a message of the phase.
    pub datagrams: usize,
    /// Per-frame messages, by kind.
    pub per_update: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhaseDiff {
    /// `new#0`, `jointeam#0`, ...; `connect#0` before the first command.
    pub name: String,
    pub a: PhaseSide,
    pub b: PhaseSide,
    pub differences: Vec<Difference>,
}

/// The totals of one capture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub received: usize,
    pub sent: usize,
    pub seconds: f64,
    pub errors: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffReport {
    pub a: Totals,
    pub b: Totals,
    pub phases: Vec<PhaseDiff>,
}

impl DiffReport {
    pub fn difference_count(&self) -> usize {
        self.phases.iter().map(|p| p.differences.len()).sum()
    }

    /// The report as text.
    pub fn format(&self) -> String {
        let mut out = String::new();
        for (label, t) in [("A", &self.a), ("B", &self.b)] {
            let rate = if t.seconds > 0.0 {
                t.received as f64 / t.seconds
            } else {
                0.0
            };
            let _ = writeln!(
                out,
                "{label}: {} datagrams from the server in {:.1} s ({rate:.1}/s), {} from the client, {} errors",
                t.received, t.seconds, t.sent, t.errors
            );
        }
        for phase in &self.phases {
            let _ = writeln!(
                out,
                "\n== phase {}   A {}   B {}",
                phase.name,
                side_text(&phase.a),
                side_text(&phase.b)
            );
            let kinds: BTreeSet<&String> = phase
                .a
                .per_update
                .keys()
                .chain(phase.b.per_update.keys())
                .collect();
            if !kinds.is_empty() {
                let list: Vec<String> = kinds
                    .into_iter()
                    .map(|k| {
                        format!(
                            "{k} {}/{}",
                            phase.a.per_update.get(k).unwrap_or(&0),
                            phase.b.per_update.get(k).unwrap_or(&0)
                        )
                    })
                    .collect();
                let _ = writeln!(out, "   per-frame messages (A/B): {}", list.join(", "));
            }
            let moved = phase
                .differences
                .iter()
                .filter(|d| matches!(d, Difference::Moved { .. }))
                .count();
            let mut lines: Vec<(String, usize)> = Vec::new();
            let mut push = |text: String| match lines.iter_mut().find(|(t, _)| *t == text) {
                Some((_, n)) => *n += 1,
                None => lines.push((text, 1)),
            };
            let mut moved_shown = false;
            for d in &phase.differences {
                match d {
                    Difference::OnlyInA { kind, detail } => {
                        push(format!("only in A: {kind} {detail}"));
                    }
                    Difference::OnlyInB { kind, detail } => {
                        push(format!("only in B: {kind} {detail}"));
                    }
                    Difference::Moved {
                        kind,
                        pos_a,
                        pos_b,
                    } => {
                        if moved <= MAX_MOVED_LISTED {
                            push(format!("order: {kind} is #{pos_a} in A, #{pos_b} in B"));
                        } else if !moved_shown {
                            moved_shown = true;
                            push(format!(
                                "order: {moved} messages are in a different place, the first is \
                                 {kind} (#{pos_a} in A, #{pos_b} in B)"
                            ));
                        }
                    }
                    Difference::Field { kind, name, a, b } => {
                        push(format!("{kind} {name}: A={a} B={b}"));
                    }
                    Difference::Set {
                        kind,
                        name,
                        only_a,
                        only_b,
                    } => {
                        push(format!(
                            "{kind} {name}: only in A {}; only in B {}",
                            summarise(only_a),
                            summarise(only_b)
                        ));
                    }
                }
            }
            for (text, n) in lines {
                if n > 1 {
                    let _ = writeln!(out, "   {text} (x{n})");
                } else {
                    let _ = writeln!(out, "   {text}");
                }
            }
        }
        let _ = writeln!(out, "\n{} differences", self.difference_count());
        out
    }
}

/// Longest list of names printed in full.
const MAX_LISTED: usize = 12;

/// `items` as a list; a long list is cut and, for `type:name` items, counted by type.
fn summarise(items: &[String]) -> String {
    if items.len() <= MAX_LISTED {
        return format!("{items:?}");
    }
    let mut by_type: BTreeMap<&str, usize> = BTreeMap::new();
    for item in items {
        if let Some((ty, _)) = item.split_once(':') {
            *by_type.entry(ty).or_insert(0) += 1;
        }
    }
    let mut out = format!("{} items", items.len());
    if !by_type.is_empty() {
        let list: Vec<String> = by_type.iter().map(|(t, n)| format!("type {t}: {n}")).collect();
        let _ = write!(out, " ({})", list.join(", "));
    }
    let _ = write!(out, ", first {:?}", &items[..MAX_LISTED]);
    out
}

fn side_text(side: &PhaseSide) -> String {
    if side.present {
        format!(
            "{:.3}-{:.3} s, {} datagrams",
            side.first_time, side.last_time, side.datagrams
        )
    } else {
        "absent".to_string()
    }
}

struct Phase<'a> {
    name: String,
    lines: Vec<&'a DumpLine>,
}

/// The server's lines and the probe's string commands, cut into phases at each string command.
fn split_phases(decoded: &Decoded) -> Vec<Phase<'_>> {
    let mut phases = vec![Phase {
        name: "connect#0".to_string(),
        lines: Vec::new(),
    }];
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for line in &decoded.lines {
        if line.dir == Direction::Sent {
            if line.kind != "clc_stringcmd" {
                continue;
            }
            let word = command_word(line);
            let n = seen.entry(word.clone()).or_insert(0);
            phases.push(Phase {
                name: format!("{word}#{n}"),
                lines: Vec::new(),
            });
            *n += 1;
        }
        if let Some(phase) = phases.last_mut() {
            phase.lines.push(line);
        }
    }
    phases
}

/// The first word of a `clc_stringcmd` line's `cmd` field.
fn command_word(line: &DumpLine) -> String {
    line.fields
        .iter()
        .find(|(n, _)| n == "cmd")
        .and_then(|(_, v)| v.trim_matches('"').split_whitespace().next())
        .unwrap_or("")
        .to_string()
}

/// What messages are aligned by: the kind, plus the command word of a string command.
fn key(line: &DumpLine) -> String {
    if line.kind == "clc_stringcmd" {
        format!("clc_stringcmd:{}", command_word(line))
    } else {
        line.kind.clone()
    }
}

fn aligned(line: &DumpLine) -> bool {
    !line.per_update && line.entities.is_none()
}

/// The fields of `line` with what legitimately differs taken out.
fn normalised(line: &DumpLine, names: &BTreeSet<String>) -> Vec<(String, String)> {
    if line.kind.starts_with("oob:") || line.kind == "clc_stringcmd" {
        return Vec::new();
    }
    line.fields
        .iter()
        .filter(|(n, _)| !n.contains("random_seed"))
        .filter(|(n, _)| !(line.kind == "svc_updateuserinfo" && n == "name"))
        .map(|(n, v)| (n.clone(), blank_names(v, names)))
        .collect()
}

fn blank_names(value: &str, names: &BTreeSet<String>) -> String {
    let mut out = value.to_string();
    for name in names.iter().filter(|n| !n.is_empty()) {
        out = out.replace(name.as_str(), PLAYER);
    }
    out
}

fn detail(line: &DumpLine, names: &BTreeSet<String>) -> String {
    let mut out = String::new();
    for (n, v) in normalised(line, names) {
        let _ = write!(out, "{n}={v} ");
    }
    for (n, set) in &line.sets {
        let _ = write!(out, "{n}=[{} items] ", set.len());
    }
    let mut out = out.trim_end().to_string();
    if out.len() > MAX_DETAIL {
        let mut cut = MAX_DETAIL;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("...");
    }
    out
}

/// Indices of a longest common subsequence of `a` and `b`, as pairs in order.
fn lcs_pairs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let width = m + 1;
    let mut len = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i * width + j] = if a[i] == b[j] {
                len[(i + 1) * width + j + 1] + 1
            } else {
                len[(i + 1) * width + j].max(len[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut pairs = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if len[(i + 1) * width + j] >= len[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

fn side(lines: &[&DumpLine], present: bool) -> PhaseSide {
    let received: Vec<&&DumpLine> = lines
        .iter()
        .filter(|l| l.dir == Direction::Received)
        .collect();
    let mut s = PhaseSide {
        present,
        first_time: received.first().map_or(0.0, |l| l.time),
        last_time: received.last().map_or(0.0, |l| l.time),
        datagrams: received
            .iter()
            .map(|l| l.record)
            .collect::<BTreeSet<_>>()
            .len(),
        per_update: BTreeMap::new(),
    };
    for l in received.iter().filter(|l| l.per_update) {
        *s.per_update.entry(l.kind.clone()).or_insert(0) += 1;
    }
    s
}

/// Compares two sets of `(name, value)` pairs; a name in one only has the value `0`, the value
/// of a field that is not in a non-zero state list.
fn compare_states(
    kind: &str,
    a: &[(String, String)],
    b: &[(String, String)],
    out: &mut Vec<Difference>,
) {
    let a: BTreeMap<&String, &String> = a.iter().map(|(n, v)| (n, v)).collect();
    let b: BTreeMap<&String, &String> = b.iter().map(|(n, v)| (n, v)).collect();
    let names: BTreeSet<&&String> = a.keys().chain(b.keys()).collect();
    let zero = "0".to_string();
    for name in names {
        let (va, vb) = (a.get(*name).copied().unwrap_or(&zero), b.get(*name).copied().unwrap_or(&zero));
        if va != vb {
            out.push(Difference::Field {
                kind: kind.to_string(),
                name: (**name).clone(),
                a: va.clone(),
                b: vb.clone(),
            });
        }
    }
}

fn client_data_lines<'a>(p: &Phase<'a>) -> Vec<&'a DumpLine> {
    p.lines
        .iter()
        .copied()
        .filter(|l| l.kind == "svc_clientdata")
        .collect()
}

fn diff_phase(
    a: &Phase<'_>,
    b: &Phase<'_>,
    names_a: &BTreeSet<String>,
    names_b: &BTreeSet<String>,
    opts: &DiffOptions,
) -> Vec<Difference> {
    let mut out = Vec::new();
    let al: Vec<&DumpLine> = a.lines.iter().copied().filter(|l| aligned(l)).collect();
    let bl: Vec<&DumpLine> = b.lines.iter().copied().filter(|l| aligned(l)).collect();
    let ka: Vec<String> = al.iter().map(|l| key(l)).collect();
    let kb: Vec<String> = bl.iter().map(|l| key(l)).collect();

    let pairs = lcs_pairs(&ka, &kb);
    let matched_a: BTreeSet<usize> = pairs.iter().map(|p| p.0).collect();
    let matched_b: BTreeSet<usize> = pairs.iter().map(|p| p.1).collect();

    // Unmatched messages: a kind unmatched on both sides is in a different place.
    let mut free_b: Vec<usize> = (0..bl.len()).filter(|j| !matched_b.contains(j)).collect();
    for i in (0..al.len()).filter(|i| !matched_a.contains(i)) {
        if let Some(pos) = free_b.iter().position(|&j| kb[j] == ka[i]) {
            let j = free_b.remove(pos);
            out.push(Difference::Moved {
                kind: ka[i].clone(),
                pos_a: i,
                pos_b: j,
            });
        } else {
            out.push(Difference::OnlyInA {
                kind: ka[i].clone(),
                detail: detail(al[i], names_a),
            });
        }
    }
    for j in free_b {
        out.push(Difference::OnlyInB {
            kind: kb[j].clone(),
            detail: detail(bl[j], names_b),
        });
    }

    for &(i, j) in &pairs {
        let (la, lb) = (al[i], bl[j]);
        let fa = normalised(la, names_a);
        let fb = normalised(lb, names_b);
        let order: Vec<&String> = fa
            .iter()
            .map(|(n, _)| n)
            .chain(fb.iter().map(|(n, _)| n))
            .collect();
        let mut done = BTreeSet::new();
        for name in order {
            if !done.insert(name) {
                continue;
            }
            let va = fa.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
            let vb = fb.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
            if va != vb {
                out.push(Difference::Field {
                    kind: ka[i].clone(),
                    name: name.clone(),
                    a: va.unwrap_or("(absent)").to_string(),
                    b: vb.unwrap_or("(absent)").to_string(),
                });
            }
        }
        for (name, set_a) in &la.sets {
            let empty = BTreeSet::new();
            let set_b = lb
                .sets
                .iter()
                .find(|(n, _)| n == name)
                .map_or(&empty, |(_, s)| s);
            if set_a != set_b {
                out.push(Difference::Set {
                    kind: ka[i].clone(),
                    name: name.clone(),
                    only_a: set_a.difference(set_b).cloned().collect(),
                    only_b: set_b.difference(set_a).cloned().collect(),
                });
            }
        }
    }

    // Client data: the first and the last of the phase.
    let (ca, cb) = (client_data_lines(a), client_data_lines(b));
    match (ca.first(), cb.first()) {
        (Some(fa), Some(fb)) => {
            compare_states("svc_clientdata[first]", &fa.state, &fb.state, &mut out);
            if let (Some(la), Some(lb)) = (ca.last(), cb.last()) {
                compare_states("svc_clientdata[last]", &la.state, &lb.state, &mut out);
            }
        }
        (Some(_), None) => out.push(Difference::OnlyInA {
            kind: "svc_clientdata".to_string(),
            detail: String::new(),
        }),
        (None, Some(_)) => out.push(Difference::OnlyInB {
            kind: "svc_clientdata".to_string(),
            detail: String::new(),
        }),
        (None, None) => {}
    }

    if opts.entities {
        let info = |p: &Phase<'_>| {
            let mut non_player = 0;
            let mut fields = BTreeSet::new();
            for e in p.lines.iter().filter_map(|l| l.entities.as_ref()) {
                non_player = non_player.max(e.non_player);
                fields.extend(e.player_fields.iter().cloned());
            }
            (non_player, fields)
        };
        let ((na, fa), (nb, fb)) = (info(a), info(b));
        if na != nb {
            out.push(Difference::Field {
                kind: "svc_packetentities".to_string(),
                name: "non-player entities (most in one update)".to_string(),
                a: na.to_string(),
                b: nb.to_string(),
            });
        }
        if fa != fb {
            out.push(Difference::Set {
                kind: "svc_packetentities".to_string(),
                name: "player fields that changed".to_string(),
                only_a: fa.difference(&fb).cloned().collect(),
                only_b: fb.difference(&fa).cloned().collect(),
            });
        }
    }
    out
}

fn totals(d: &Decoded) -> Totals {
    Totals {
        received: d.received,
        sent: d.sent,
        seconds: (d.last_received - d.first_received).max(0.0),
        errors: d.errors.len(),
    }
}

/// Compares `a` with `b`.
pub fn diff_captures(a: &Decoded, b: &Decoded, opts: &DiffOptions) -> DiffReport {
    let pa = split_phases(a);
    let pb = split_phases(b);
    let mut names: Vec<&String> = pa.iter().map(|p| &p.name).collect();
    for p in &pb {
        if !names.contains(&&p.name) {
            names.push(&p.name);
        }
    }
    let empty = |name: &str| Phase {
        name: name.to_string(),
        lines: Vec::new(),
    };
    let phases = names
        .into_iter()
        .map(|name| {
            let fa = pa.iter().find(|p| &p.name == name);
            let fb = pb.iter().find(|p| &p.name == name);
            let (ea, eb) = (empty(name), empty(name));
            let (xa, xb) = (fa.unwrap_or(&ea), fb.unwrap_or(&eb));
            PhaseDiff {
                name: name.clone(),
                a: side(&xa.lines, fa.is_some()),
                b: side(&xb.lines, fb.is_some()),
                differences: diff_phase(xa, xb, &a.names, &b.names, opts),
            }
        })
        .collect();
    DiffReport {
        a: totals(a),
        b: totals(b),
        phases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::decode_capture;
    use crate::test_support::*;

    fn decoded(cap: SyntheticCapture) -> Decoded {
        let d = decode_capture(&cap.build());
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        d
    }

    fn base() -> SyntheticCapture {
        SyntheticCapture::new().register(0.1)
    }

    #[test]
    fn lcs_aligns_around_an_extra_message() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let pairs = lcs_pairs(&s(&["a", "b", "c"]), &s(&["a", "x", "b", "c"]));
        assert_eq!(pairs, [(0, 0), (1, 2), (2, 3)]);
        assert!(lcs_pairs(&s(&[]), &s(&["a"])).is_empty());
    }

    #[test]
    fn identical_captures_do_not_differ() {
        let make = || {
            decoded(
                base()
                    .server(0.2, |w, reg| write_usermsg(w, reg, &CsUserMsg::RoundTime(115)))
                    .client_stringcmd(0.3, "jointeam 2")
                    .server(0.4, |w, reg| write_usermsg(w, reg, &CsUserMsg::HideWeapon(0))),
            )
        };
        let report = diff_captures(&make(), &make(), &DiffOptions::default());
        assert_eq!(report.difference_count(), 0, "{}", report.format());
        let names: Vec<&str> = report.phases.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["connect#0", "jointeam#0"]);
    }

    #[test]
    fn a_missing_message_and_a_changed_field_are_reported() {
        let a = decoded(
            base()
                .server(0.2, |w, reg| {
                    write_usermsg(w, reg, &CsUserMsg::HideWeapon(1));
                    write_usermsg(w, reg, &CsUserMsg::Money { amount: 800, flash: false });
                })
                .client_stringcmd(0.3, "jointeam 2")
                .server(0.4, |w, reg| write_usermsg(w, reg, &CsUserMsg::RoundTime(115))),
        );
        let b = decoded(
            base()
                .server(0.2, |w, reg| {
                    write_usermsg(w, reg, &CsUserMsg::Money { amount: 16000, flash: false });
                })
                .client_stringcmd(0.3, "jointeam 2"),
        );
        let report = diff_captures(&a, &b, &DiffOptions::default());
        let connect = &report.phases[0].differences;
        assert!(connect.contains(&Difference::OnlyInA {
            kind: "usermsg:HideWeapon".to_string(),
            detail: "v=1".to_string()
        }), "{connect:?}");
        assert!(connect.contains(&Difference::Field {
            kind: "usermsg:Money".to_string(),
            name: "amount".to_string(),
            a: "800".to_string(),
            b: "16000".to_string()
        }), "{connect:?}");
        let join = &report.phases[1].differences;
        assert!(matches!(&join[..], [Difference::OnlyInA { kind, .. }] if kind == "usermsg:RoundTime"), "{join:?}");
        let text = report.format();
        assert!(text.contains("only in A: usermsg:HideWeapon v=1"), "{text}");
        assert!(text.contains("usermsg:Money amount: A=800 B=16000"), "{text}");
    }

    #[test]
    fn an_order_difference_is_reported_as_moved() {
        let a = decoded(base().server(0.2, |w, reg| {
            write_usermsg(w, reg, &CsUserMsg::HideWeapon(0));
            write_usermsg(w, reg, &CsUserMsg::RoundTime(1));
            write_usermsg(w, reg, &CsUserMsg::ResetHud);
        }));
        let b = decoded(base().server(0.2, |w, reg| {
            write_usermsg(w, reg, &CsUserMsg::ResetHud);
            write_usermsg(w, reg, &CsUserMsg::HideWeapon(0));
            write_usermsg(w, reg, &CsUserMsg::RoundTime(1));
        }));
        let report = diff_captures(&a, &b, &DiffOptions::default());
        let d = &report.phases[0].differences;
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(matches!(&d[0], Difference::Moved { kind, .. } if kind == "usermsg:ResetHUD"), "{d:?}");
    }

    #[test]
    fn times_and_arguments_of_commands_are_normalised() {
        let a = decoded(
            base()
                .server(0.2, |w, _| write_time(w, 1.0))
                .client_stringcmd(0.3, "spawn 1 -555")
                .oob_from_server(0.31, b"challenge 1"),
        );
        let b = decoded(
            base()
                .server(0.2, |w, _| write_time(w, 99.0))
                .client_stringcmd(0.3, "spawn 2 777")
                .oob_from_server(0.31, b"challenge 2"),
        );
        let report = diff_captures(&a, &b, &DiffOptions::default());
        assert_eq!(report.difference_count(), 0, "{}", report.format());
    }

    #[test]
    fn a_phase_in_one_capture_only_is_reported() {
        let a = decoded(base().client_stringcmd(0.3, "jointeam 2").server(0.4, |w, _| write_print(w, "hi")));
        let b = decoded(base());
        let report = diff_captures(&a, &b, &DiffOptions::default());
        let phase = report.phases.iter().find(|p| p.name == "jointeam#0").unwrap();
        assert!(phase.a.present && !phase.b.present);
        assert!(phase.differences.iter().any(|d| matches!(d, Difference::OnlyInA { kind, .. } if kind == "svc_print")));
    }

    #[test]
    fn repeated_lines_are_counted_and_a_long_set_is_summarised() {
        let items: Vec<String> = (0..20).map(|i| format!("0:sound{i}.wav")).collect();
        let text = summarise(&items);
        assert!(text.starts_with("20 items (type 0: 20), first ["), "{text}");
        let a = decoded(base().client_stringcmd(0.3, "x").server(0.4, |w, _| {
            write_print(w, "same");
            write_print(w, "same");
        }));
        let b = decoded(base().client_stringcmd(0.3, "x"));
        let text = diff_captures(&a, &b, &DiffOptions::default()).format();
        assert!(text.contains("only in A: svc_print text=\"same\" (x2)"), "{text}");
    }
}
