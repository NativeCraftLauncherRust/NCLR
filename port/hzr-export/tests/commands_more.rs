//! Does the extended exporter reproduce vanilla's `commands.json`?
//!
//! The reference is `tmp/ship-extract/commands.json`, the file vanilla's data
//! generator (`CommandsReport`) writes and `ship.hzr3` carries. `commands_more`
//! re-assembles it from the player's `mc263/client.jar` by executing the 97
//! `XCommand.register` calls with a symbolic JVM (see the module docs), so every
//! subtree present in the output was fully assembled; a registration the evaluator
//! cannot follow is rolled back.

#[path = "../src/commands_more.rs"]
mod commands_more;

use std::path::PathBuf;

use serde_json::Value;

fn workspace() -> PathBuf {
    // <workspace>/route3/crates/hzr-export
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference_table() -> PathBuf {
    workspace().join("tmp/ship-extract/commands.json")
}

fn describe_diff(path: &str, mine: &Value, theirs: &Value) -> String {
    let kind = |value: &serde_json::Map<String, Value>| {
        value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    };
    match (mine, theirs) {
        (Value::Object(mine), Value::Object(theirs)) => {
            let mine_children = mine.get("children").and_then(Value::as_object);
            let their_children = theirs.get("children").and_then(Value::as_object);
            let mut extra = Vec::new();
            let mut missing = Vec::new();
            let mut differing = Vec::new();
            if let (Some(mine_children), Some(their_children)) = (mine_children, their_children) {
                for (name, value) in mine_children {
                    match their_children.get(name) {
                        Some(other) if other == value => {}
                        Some(_) => differing.push(name.clone()),
                        None => extra.push(name.clone()),
                    }
                }
                for name in their_children.keys() {
                    if !mine_children.contains_key(name) {
                        missing.push(name.clone());
                    }
                }
            }
            let fields: Vec<&String> = mine
                .keys()
                .filter(|key| key.as_str() != "children" && theirs.get(*key) != Some(&mine[*key]))
                .collect();
            format!(
                "{path}: type {} vs {}; fields {fields:?}; children extra {extra:?} missing {missing:?} differing {differing:?}",
                kind(mine),
                kind(theirs)
            )
        }
        _ => format!("{path}: value differs"),
    }
}

/// The one registration the evaluator cannot follow yet.
const KNOWN_GAPS: [&str; 1] = ["gamerule"];

#[test]
fn every_produced_command_is_identical_to_the_reference() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let (document, coverage) = commands_more::commands_tables(&jar()).expect("the jar is readable");
    let reference: Value =
        serde_json::from_slice(&std::fs::read(reference_table()).unwrap()).expect("reference JSON");

    assert_eq!(document.get("type").and_then(Value::as_str), Some("root"));
    let mine = document
        .get("children")
        .and_then(Value::as_object)
        .expect("root children");
    let theirs = reference
        .get("children")
        .and_then(Value::as_object)
        .expect("reference children");

    let mut failures = Vec::new();
    let mut identical = 0usize;
    for (name, value) in mine {
        match theirs.get(name) {
            Some(expected) if expected == value => identical += 1,
            Some(expected) => failures.push(describe_diff(name, value, expected)),
            None => failures.push(format!("{name}: not a command in the reference")),
        }
    }
    let missing: Vec<&String> = theirs.keys().filter(|name| !mine.contains_key(*name)).collect();
    eprintln!(
        "root commands: {identical} identical of {}, {} missing, {} registrations rolled back",
        mine.len(),
        missing.len(),
        coverage.skipped.len()
    );
    eprintln!("  missing: {missing:?}");
    eprintln!("  skipped: {:?}", coverage.skipped);
    for note in &coverage.notes {
        eprintln!("  {note}");
    }
    assert!(
        failures.is_empty(),
        "{}",
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        identical >= 93,
        "only {identical} root commands identical; the interpreter regressed"
    );
}

#[test]
fn only_the_known_gap_is_missing_and_it_is_reported() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let (document, coverage) = commands_more::commands_tables(&jar()).expect("export");
    let produced = document
        .get("children")
        .and_then(Value::as_object)
        .expect("root children");
    let reference: Value =
        serde_json::from_slice(&std::fs::read(reference_table()).unwrap()).expect("reference JSON");
    let theirs = reference
        .get("children")
        .and_then(Value::as_object)
        .expect("reference children");
    let missing: Vec<&String> = theirs.keys().filter(|name| !produced.contains_key(*name)).collect();
    for name in &missing {
        assert!(
            KNOWN_GAPS.contains(&name.as_str()),
            "{name} is missing but is not a known gap"
        );
    }
    assert_eq!(coverage.skipped.len(), 1, "exactly one gap is reported");
    let skipped = &coverage.skipped[0];
    assert!(skipped.contains("GameRuleCommand"), "{skipped}");
    assert!(skipped.contains("incomplete"), "{skipped}");
}

#[test]
fn write_puts_a_parsable_table_in_the_output_directory() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let named = std::env::var_os("HZR_EXPORT_OUT").map(PathBuf::from);
    let directory = named.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hzr-export-commands-more-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&directory);
    let coverage = commands_more::write(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(commands_more::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("valid JSON");
    let object = document.as_object().expect("an object");
    assert_eq!(object.get("type").and_then(Value::as_str), Some("root"));
    let children = object
        .get("children")
        .and_then(Value::as_object)
        .expect("children");
    assert_eq!(children.len(), coverage.produced.len());
    if named.is_none() {
        let _ = std::fs::remove_dir_all(&directory);
    }
}
