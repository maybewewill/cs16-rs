//! Parity ledger scorer.
//!
//! A ledger directory holds `weights.toml` (category name -> weight, summing
//! to 100) and one `<category>.toml` per category listing `[[item]]` entries
//! with `id`, `status` and optional `evidence`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// Name of the weights file inside a ledger directory.
const WEIGHTS_FILE: &str = "weights.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Missing,
    Implemented,
    Tested,
    Verified,
}

impl Status {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "missing" => Some(Self::Missing),
            "implemented" => Some(Self::Implemented),
            "tested" => Some(Self::Tested),
            "verified" => Some(Self::Verified),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Implemented => "implemented",
            Self::Tested => "tested",
            Self::Verified => "verified",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub status: Status,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub name: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Weights(pub BTreeMap<String, u32>);

#[derive(Debug, Clone, PartialEq)]
pub struct CategoryScore {
    pub name: String,
    pub weight: u32,
    pub total: usize,
    pub implemented: usize,
    pub tested: usize,
    pub verified: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub categories: Vec<CategoryScore>,
    pub verified_pct: f64,
    pub tested_pct: f64,
    pub implemented_pct: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParityError {
    Toml(String),
    UnknownStatus {
        id: String,
        value: String,
    },
    DuplicateId(String),
    MissingEvidence(String),
    WeightsSum(u32),
    Io(String),
    /// A category file whose name is not a key in `weights.toml`.
    UnweightedCategory(String),
}

impl fmt::Display for ParityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(msg) => write!(f, "invalid TOML: {msg}"),
            Self::UnknownStatus { id, value } => {
                write!(f, "item `{id}`: unknown status `{value}`")
            }
            Self::DuplicateId(id) => write!(f, "duplicate item id `{id}`"),
            Self::MissingEvidence(id) => {
                write!(f, "item `{id}` is verified but has no evidence")
            }
            Self::WeightsSum(sum) => write!(f, "weights sum to {sum}, expected 100"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::UnweightedCategory(name) => {
                write!(f, "category `{name}` has no weight in {WEIGHTS_FILE}")
            }
        }
    }
}

impl std::error::Error for ParityError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCategory {
    #[serde(default)]
    item: Vec<RawItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItem {
    id: String,
    status: String,
    evidence: Option<String>,
}

/// Parses one category registry file named `name`.
pub fn parse_category(name: &str, toml_text: &str) -> Result<Category, ParityError> {
    let raw: RawCategory =
        toml::from_str(toml_text).map_err(|e| ParityError::Toml(format!("{name}: {e}")))?;
    let mut seen = BTreeSet::new();
    let mut items = Vec::with_capacity(raw.item.len());
    for RawItem {
        id,
        status,
        evidence,
    } in raw.item
    {
        let Some(status) = Status::parse(&status) else {
            return Err(ParityError::UnknownStatus { id, value: status });
        };
        if !seen.insert(id.clone()) {
            return Err(ParityError::DuplicateId(id));
        }
        let has_evidence = evidence.as_deref().is_some_and(|e| !e.trim().is_empty());
        if status == Status::Verified && !has_evidence {
            return Err(ParityError::MissingEvidence(id));
        }
        items.push(Item {
            id,
            status,
            evidence,
        });
    }
    Ok(Category {
        name: name.to_owned(),
        items,
    })
}

/// Parses `weights.toml`; the weights must sum to exactly 100.
pub fn parse_weights(toml_text: &str) -> Result<Weights, ParityError> {
    let map: BTreeMap<String, u32> =
        toml::from_str(toml_text).map_err(|e| ParityError::Toml(format!("{WEIGHTS_FILE}: {e}")))?;
    let sum = map.values().fold(0u32, |acc, w| acc.saturating_add(*w));
    if sum != 100 {
        return Err(ParityError::WeightsSum(sum));
    }
    Ok(Weights(map))
}

/// Scores categories against weights.
///
/// Counters are cumulative: `verified` items also count as `tested`, and
/// `tested` items as `implemented`. A weighted category with no file or no
/// items scores 0 but stays in the denominator. Categories absent from the
/// weights are reported with weight 0 and do not affect the percentages.
pub fn score(cats: &[Category], weights: &Weights) -> Report {
    let mut categories: Vec<CategoryScore> = weights
        .0
        .iter()
        .map(|(name, &weight)| {
            let items = cats
                .iter()
                .find(|c| &c.name == name)
                .map_or(&[][..], |c| &c.items[..]);
            category_score(name, weight, items)
        })
        .collect();
    categories.extend(
        cats.iter()
            .filter(|c| !weights.0.contains_key(&c.name))
            .map(|c| category_score(&c.name, 0, &c.items)),
    );

    let weight_sum: u64 = weights.0.values().map(|&w| u64::from(w)).sum();
    let pct = |count: fn(&CategoryScore) -> usize| -> f64 {
        if weight_sum == 0 {
            return 0.0;
        }
        let weighted: f64 = categories
            .iter()
            .filter(|c| c.total > 0)
            .map(|c| f64::from(c.weight) * count(c) as f64 / c.total as f64)
            // Explicit +0.0 start: `Sum` for floats starts at -0.0, which
            // would print as "-0.0%" for an empty ledger.
            .fold(0.0, |acc, x| acc + x);
        100.0 * weighted / weight_sum as f64
    };
    let verified_pct = pct(|c| c.verified);
    let tested_pct = pct(|c| c.tested);
    let implemented_pct = pct(|c| c.implemented);

    Report {
        categories,
        verified_pct,
        tested_pct,
        implemented_pct,
    }
}

fn category_score(name: &str, weight: u32, items: &[Item]) -> CategoryScore {
    let count = |pred: fn(Status) -> bool| items.iter().filter(|i| pred(i.status)).count();
    CategoryScore {
        name: name.to_owned(),
        weight,
        total: items.len(),
        implemented: count(|s| s != Status::Missing),
        tested: count(|s| matches!(s, Status::Tested | Status::Verified)),
        verified: count(|s| s == Status::Verified),
    }
}

/// Renders a category back to registry TOML: one `[[item]]` table per item,
/// in the category's item order, separated by blank lines.
pub fn category_to_toml(cat: &Category) -> String {
    let quote = |s: &str| toml::Value::String(s.to_owned()).to_string();
    let tables: Vec<String> = cat
        .items
        .iter()
        .map(|item| {
            let mut table = format!(
                "[[item]]\nid = {}\nstatus = {}\n",
                quote(&item.id),
                quote(item.status.as_str())
            );
            if let Some(evidence) = &item.evidence {
                table.push_str(&format!("evidence = {}\n", quote(evidence)));
            }
            table
        })
        .collect();
    tables.join("\n")
}

/// Adds the names listed in a console dump (`cvarlist` / `cmdlist` output)
/// to `existing` as `missing` items.
///
/// Each line contributes its first whitespace-delimited token when that token
/// matches `[A-Za-z_+\-][A-Za-z0-9_+\-.]*`. Blank lines, lines of dashes and
/// lines whose first token is a number (the summary line) are skipped. When
/// the dump contains a line of dashes, everything up to and including the
/// first one is a header and is skipped too. Existing items keep their
/// status and evidence; the result is sorted by id without duplicates.
pub fn import_names(existing: Category, dump_text: &str) -> Category {
    let is_dashes = |line: &str| {
        let line = line.trim();
        !line.is_empty() && line.chars().all(|c| c == '-')
    };
    let body_start = dump_text
        .lines()
        .position(is_dashes)
        .map_or(0, |idx| idx + 1);

    let mut items: BTreeMap<String, Item> = existing
        .items
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();
    for line in dump_text.lines().skip(body_start) {
        if is_dashes(line) {
            continue;
        }
        let Some(token) = line.split_whitespace().next() else {
            continue;
        };
        if token.parse::<f64>().is_ok() || !is_dump_name(token) {
            continue;
        }
        items.entry(token.to_owned()).or_insert_with(|| Item {
            id: token.to_owned(),
            status: Status::Missing,
            evidence: None,
        });
    }
    Category {
        name: existing.name,
        items: items.into_values().collect(),
    }
}

/// `[A-Za-z_+\-][A-Za-z0-9_+\-.]*`
fn is_dump_name(token: &str) -> bool {
    let mut chars = token.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '_' | '+' | '-'));
    first_ok && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '.'))
}

/// Loads a ledger directory: `weights.toml` plus every other `*.toml` file as
/// a category named by its file stem. Subdirectories are ignored. A category
/// file with no entry in `weights.toml` is an error.
pub fn load_dir(dir: &Path) -> Result<(Vec<Category>, Weights), ParityError> {
    let io_err =
        |path: &Path, e: std::io::Error| ParityError::Io(format!("{}: {e}", path.display()));

    let weights_path = dir.join(WEIGHTS_FILE);
    let weights_text = fs::read_to_string(&weights_path).map_err(|e| io_err(&weights_path, e))?;
    let weights = parse_weights(&weights_text)?;

    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let entry = entry.map_err(|e| io_err(dir, e))?;
        let path = entry.path();
        let is_file = entry.file_type().map_err(|e| io_err(&path, e))?.is_file();
        let is_toml = path.extension().is_some_and(|ext| ext == "toml");
        let is_weights = path.file_name().is_some_and(|n| n == WEIGHTS_FILE);
        if is_file && is_toml && !is_weights {
            paths.push(path);
        }
    }
    paths.sort();

    let mut categories = Vec::with_capacity(paths.len());
    for path in paths {
        let text = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !weights.0.contains_key(&name) {
            return Err(ParityError::UnweightedCategory(name));
        }
        categories.push(parse_category(&name, &text)?);
    }
    Ok((categories, weights))
}

/// The `path.rs::name` citations of an evidence text, as `(file, fn name)`. After `path.rs::`
/// come one or more names, `tests::a_test`, then further names after commas (`, other_test`)
/// as long as each looks like a test name (it has an underscore) and ends at a comma, a
/// semicolon, a parenthesis or the end; only the last `::` segment of a name is the function.
pub fn cited_fns(evidence: &str) -> Vec<(String, String)> {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let is_path = |c: char| is_ident(c) || matches!(c, '/' | '.' | '-');
    let mut out = Vec::new();
    let mut rest = evidence;
    while let Some(at) = rest.find(".rs::") {
        let start = rest[..at]
            .rfind(|c: char| !is_path(c))
            .map_or(0, |i| i + 1);
        let file = format!("{}.rs", &rest[start..at]);
        let mut tail = &rest[at + ".rs::".len()..];
        loop {
            let len = tail
                .find(|c: char| !is_ident(c) && c != ':')
                .unwrap_or(tail.len());
            let name = tail[..len].rsplit("::").next().unwrap_or_default();
            // After a name: a list continues only after a comma and a space.
            let after = &tail[len..];
            if !name.is_empty() {
                out.push((file.clone(), name.to_string()));
            }
            let Some(next) = after.strip_prefix(", ") else {
                tail = after;
                break;
            };
            let next_len = next.find(|c: char| !is_ident(c)).unwrap_or(next.len());
            let token = &next[..next_len];
            let ends = next[next_len..].is_empty()
                || next[next_len..].starts_with([',', ';', ')', '.'])
                || next[next_len..].starts_with(" (");
            if token.contains('_') && ends {
                tail = next;
            } else {
                tail = after;
                break;
            }
        }
        rest = tail;
    }
    out
}

/// Every `path.rs::fn_name` an item's evidence cites that is not a `fn` of that file under
/// `root`: `category/id: file::name`. An evidence text that cites a function that was renamed
/// or removed would otherwise keep claiming a test that no longer exists.
pub fn missing_cited_fns(category: &Category, root: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    for item in &category.items {
        let Some(evidence) = &item.evidence else {
            continue;
        };
        for (file, name) in cited_fns(evidence) {
            let found = fs::read_to_string(root.join(&file)).is_ok_and(|text| {
                text.match_indices(&format!("fn {name}")).any(|(at, m)| {
                    !text[at + m.len()..]
                        .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
                })
            });
            if !found {
                missing.push(format!("{}/{}: {file}::{name}", category.name, item.id));
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_only_counts_in_headline() {
        let w = parse_weights("a = 60\nb = 40").unwrap();
        let a = parse_category(
            "a",
            r#"
            [[item]]
            id = "x"
            status = "verified"
            evidence = "t::x"

            [[item]]
            id = "y"
            status = "tested""#,
        )
        .unwrap();
        let r = score(&[a], &w);
        assert_eq!(r.verified_pct, 30.0);
        assert_eq!(r.tested_pct, 60.0);
        assert_eq!(r.implemented_pct, 60.0);
    }

    #[test]
    fn verified_without_evidence_is_rejected() {
        let r = parse_category(
            "a",
            r#"
            [[item]]
            id = "x"
            status = "verified""#,
        );
        assert_eq!(r.unwrap_err(), ParityError::MissingEvidence("x".into()));
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let r = parse_category(
            "a",
            r#"
            [[item]]
            id = "x"
            status = "missing"

            [[item]]
            id = "x"
            status = "implemented""#,
        );
        assert_eq!(r.unwrap_err(), ParityError::DuplicateId("x".into()));
    }

    #[test]
    fn unknown_status_is_rejected() {
        let r = parse_category(
            "a",
            r#"
            [[item]]
            id = "x"
            status = "done""#,
        );
        assert_eq!(
            r.unwrap_err(),
            ParityError::UnknownStatus {
                id: "x".into(),
                value: "done".into()
            }
        );
    }

    #[test]
    fn empty_ledger_scores_positive_zero() {
        let w = parse_weights("a = 60\nb = 40").unwrap();
        let r = score(&[], &w);
        assert_eq!(r.verified_pct.to_bits(), 0.0f64.to_bits());
        assert_eq!(r.tested_pct.to_bits(), 0.0f64.to_bits());
        assert_eq!(r.implemented_pct.to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn weights_must_sum_to_100() {
        let r = parse_weights("a = 60\nb = 30");
        assert_eq!(r.unwrap_err(), ParityError::WeightsSum(90));
    }

    /// Temporary ledger directory under `std::env::temp_dir()`, removed on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("xtask-parity-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_dir_names_categories_by_file_stem() {
        let dir = TempDir::new("stem");
        fs::write(dir.0.join(WEIGHTS_FILE), "alpha = 100\n").unwrap();
        fs::write(
            dir.0.join("alpha.toml"),
            "[[item]]\nid = \"x\"\nstatus = \"tested\"\n",
        )
        .unwrap();
        let sub = dir.0.join("nested.toml");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("beta.toml"), "this is = not [valid").unwrap();

        let (cats, weights) = load_dir(&dir.0).unwrap();
        let names: Vec<&str> = cats.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["alpha"]);
        assert_eq!(cats[0].items.len(), 1);
        assert_eq!(weights.0.get("alpha"), Some(&100));
    }

    #[test]
    fn load_dir_on_missing_directory_is_io_error() {
        let path = std::env::temp_dir().join(format!(
            "xtask-parity-does-not-exist-{}",
            std::process::id()
        ));
        assert!(matches!(load_dir(&path), Err(ParityError::Io(_))));
    }

    #[test]
    fn load_dir_rejects_category_without_weight() {
        let dir = TempDir::new("unweighted");
        fs::write(dir.0.join(WEIGHTS_FILE), "alpha = 100\n").unwrap();
        fs::write(dir.0.join("alpha.toml"), "").unwrap();
        fs::write(dir.0.join("gamma.toml"), "").unwrap();
        assert_eq!(
            load_dir(&dir.0).unwrap_err(),
            ParityError::UnweightedCategory("gamma".into())
        );
    }

    #[test]
    fn import_names_parses_synthetic_dump() {
        let existing = parse_category(
            "cvars",
            r#"
            [[item]]
            id = "sv_gravity"
            status = "tested"
            evidence = "t::gravity"

            [[item]]
            id = "zz_custom"
            status = "implemented""#,
        )
        .unwrap();
        let dump = "CVar List\n\
                    --------------\n\
                    sv_gravity                       : 800      : , \"sv\"\n\
                    _cl_autowepswitch                : 1        : , \"a\", \"user\"\n\
                    \n\
                    +attack\n\
                    cl_righthand : 1\n\
                    gl_vsync 0\n\
                    --------------\n\
                    5 Total CVars\n";

        let cat = import_names(existing, dump);

        assert_eq!(cat.name, "cvars");
        let ids: Vec<&str> = cat.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "+attack",
                "_cl_autowepswitch",
                "cl_righthand",
                "gl_vsync",
                "sv_gravity",
                "zz_custom"
            ]
        );
        let get = |id: &str| cat.items.iter().find(|i| i.id == id).unwrap();
        assert_eq!(get("sv_gravity").status, Status::Tested);
        assert_eq!(get("sv_gravity").evidence.as_deref(), Some("t::gravity"));
        assert_eq!(get("zz_custom").status, Status::Implemented);
        assert_eq!(get("gl_vsync").status, Status::Missing);
        assert_eq!(get("gl_vsync").evidence, None);
    }

    #[test]
    fn import_names_without_separator_keeps_first_line() {
        let empty = Category {
            name: "cmds".into(),
            items: Vec::new(),
        };
        let cat = import_names(empty, "quit\nmap de_dust2\nquit\n3 commands\n");
        let ids: Vec<&str> = cat.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["map", "quit"]);
    }

    #[test]
    fn category_toml_roundtrips() {
        let cat = Category {
            name: "wire".into(),
            items: vec![
                Item {
                    id: "svc_bad".into(),
                    status: Status::Missing,
                    evidence: None,
                },
                Item {
                    id: "odd \"id\" \\ x".into(),
                    status: Status::Verified,
                    evidence: Some("payload не разбирается".into()),
                },
            ],
        };
        let text = category_to_toml(&cat);
        assert_eq!(parse_category("wire", &text).unwrap(), cat);
    }

    #[test]
    fn cited_fns_follow_the_lists_and_stop_at_prose() {
        let evidence = "crates/a/src/x.rs::tests::one_test, two_test, three_test (A2A_PING);              crates/b/tests/y.rs::solo_test (8 clients, every 17th); unverified, see docs/z.md;              crates/c/src/z.rs::last_test, unverified against a real server";
        let cited: Vec<_> = cited_fns(evidence)
            .into_iter()
            .map(|(f, n)| format!("{f}:{n}"))
            .collect();
        assert_eq!(
            cited,
            [
                "crates/a/src/x.rs:one_test",
                "crates/a/src/x.rs:two_test",
                "crates/a/src/x.rs:three_test",
                "crates/b/tests/y.rs:solo_test",
                "crates/c/src/z.rs:last_test",
            ]
        );
    }

    #[test]
    fn missing_cited_fns_reports_renamed_tests() {
        let dir = TempDir::new("cited");
        fs::create_dir_all(dir.0.join("crates/a")).unwrap();
        fs::write(
            dir.0.join("crates/a/t.rs"),
            "#[test]
fn present_test() {}
fn present_test_longer() {}
",
        )
        .unwrap();
        let cat = parse_category(
            "cat",
            "[[item]]
id = \"i\"
status = \"tested\"
             evidence = \"crates/a/t.rs::present_test, gone_test; crates/a/none.rs::x_test\"
",
        )
        .unwrap();
        assert_eq!(
            missing_cited_fns(&cat, &dir.0),
            ["cat/i: crates/a/t.rs::gone_test", "cat/i: crates/a/none.rs::x_test"]
        );
    }

    /// The registry of the repository cites tests and functions that exist.
    #[test]
    fn the_registry_cites_only_existing_functions() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let (categories, _) = load_dir(&root.join("docs/parity")).unwrap();
        let missing: Vec<String> = categories
            .iter()
            .flat_map(|c| missing_cited_fns(c, &root))
            .collect();
        assert!(missing.is_empty(), "evidence cites missing functions:
{}", missing.join("
"));
    }
}
