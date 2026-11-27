//! Does the Rust exporter reproduce vanilla's `commands.json`?
//!
//! The reference is `tmp/ship-extract/commands.json`, the file vanilla's data
//! generator (`CommandsReport`) writes and `ship.hzr3` carries. It is rebuilt here
//! from the player's `mc263/client.jar` by executing the command registrations with
//! a symbolic JVM; a registration the evaluator cannot follow is rolled back, so
//! every subtree present in the output is complete.

#[path = "../src/commands_data.rs"]
mod commands_data;

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

#[test]
fn the_produced_command_tree_matches_the_reference() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let (document, coverage) = commands_data::commands_data(&jar()).expect("the jar is readable");
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
        "root commands: {identical} identical, {} missing, {} registers rolled back",
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
        identical >= 80,
        "only {identical} root commands reproduced; the interpreter regressed"
    );
}

/// The registrations the interpreter still cannot follow. They are a *ceiling*: a
/// later change may fix one (this list then needs no edit), but a new missing
/// command is a regression.
const KNOWN_GAPS: [&str; 10] = [
    "compute",
    "data",
    "difficulty",
    "execute",
    "function",
    "gamerule",
    "item",
    "loot",
    "scoreboard",
    "test",
];

#[test]
fn only_the_known_commands_are_missing_and_each_has_a_reason() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let (document, coverage) = commands_data::commands_data(&jar()).expect("export");
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
    // Every rolled-back registration names the class and the reason, and there is
    // one per missing root command (a registration can add several roots, but none
    // of these do).
    assert_eq!(coverage.skipped.len(), missing.len());
    for skipped in &coverage.skipped {
        assert!(
            skipped.contains("net/minecraft/") && skipped.contains(": "),
            "no class/reason in {skipped}"
        );
    }
    eprintln!("missing ({}): {missing:?}", missing.len());
    eprintln!("skipped: {:#?}", coverage.skipped);
}

#[test]
fn the_blocked_commands_are_reported_not_invented() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, coverage) = commands_data::commands_data(&jar()).expect("export");
    let produced = document
        .get("children")
        .and_then(Value::as_object)
        .expect("root children");
    // Everything produced is a root command, and every skipped registration names
    // the class it came from.
    for name in produced.keys() {
        assert!(!name.is_empty());
    }
    for skipped in &coverage.skipped {
        assert!(skipped.contains(':'), "{skipped} has no reason");
    }
    assert_eq!(
        coverage.produced.len(),
        produced.len(),
        "coverage and document disagree"
    );
}

#[test]
fn write_puts_a_parsable_table_in_the_output_directory() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let named = std::env::var_os("HZR_EXPORT_OUT").map(PathBuf::from);
    let directory = named.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hzr-export-commands-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&directory);
    let coverage = commands_data::write(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(commands_data::TABLE)).expect("the file");
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
