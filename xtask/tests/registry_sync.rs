//! Keeps the parity registries under `docs/parity/` in sync with the
//! constants and tables they describe.

use std::collections::BTreeSet;
use std::path::PathBuf;

use cs_cvar::CVarRegistry;
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::messages::{clc, svc};
use cs_proto48::usermsg::UserMsgRegistry;
use xtask::parity::{self, Category};

fn load(category: &str) -> Category {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/parity")
        .join(format!("{category}.toml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parity::parse_category(category, &text).unwrap_or_else(|e| panic!("{category}: {e}"))
}

fn ids(category: &Category) -> BTreeSet<String> {
    category.items.iter().map(|i| i.id.clone()).collect()
}

#[test]
fn wire_registry_matches_opcode_constants() {
    let expected: BTreeSet<String> = svc::ALL
        .iter()
        .map(|(name, _)| format!("svc_{name}"))
        .chain(clc::ALL.iter().map(|(name, _)| format!("clc_{name}")))
        .collect();
    assert_eq!(expected.len(), 72);
    assert_eq!(ids(&load("wire")), expected);
}

#[test]
fn usermsg_registry_matches_cs16_default() {
    let expected: BTreeSet<String> = UserMsgRegistry::cs16_default()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert_eq!(expected.len(), 73);
    assert_eq!(ids(&load("usermsg")), expected);
}

#[test]
fn delta_registry_matches_builtin_delta_lst() {
    let expected: BTreeSet<String> = DeltaRegistry::cs16_default()
        .iter()
        .flat_map(|schema| {
            schema
                .fields
                .iter()
                .map(move |field| format!("{}.{}", schema.name, field.name))
        })
        .collect();
    assert_eq!(ids(&load("delta")), expected);
}

#[test]
fn every_registered_cvar_is_in_registry() {
    let registry = ids(&load("cvars"));
    let cvars = CVarRegistry::new_with_defaults();
    let missing: Vec<&String> = cvars
        .vars()
        .keys()
        .filter(|name| !registry.contains(*name))
        .collect();
    assert!(
        missing.is_empty(),
        "cvars absent from cvars.toml: {missing:?}"
    );
}
