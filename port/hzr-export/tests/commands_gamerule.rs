//! Does the gamerule closure reproduce the whole `commands.json`?
//!
//! `commands_more` reproduces 93 root commands; this module adds `gamerule` from
//! `GameRules.<clinit>`. The reference is `tmp/ship-extract/commands.json`, the file
//! vanilla's data generator writes and `ship.hzr3` carries.

#[path = "../src/commands_gamerule.rs"]
mod commands_gamerule;

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
fn the_whole_command_tree_matches_the_reference() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let (document, coverage) = commands_gamerule::commands_tables(&jar()).expect("the jar is readable");
    let reference: Value =
        serde_json::from_slice(&std::fs::read(reference_table()).unwrap()).expect("reference JSON");

    assert_eq!(document, reference, "the produced tree differs from the reference");
    assert!(coverage.skipped.is_empty(), "skipped: {:?}", coverage.skipped);
    let produced = document
        .get("children")
        .and_then(Value::as_object)
        .expect("root children");
    assert_eq!(produced.len(), 94, "all 94 root commands are present");
    eprintln!("root commands: {} identical to the reference", produced.len());
}

#[test]
fn gamerule_carries_every_rule_under_both_names() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!("skipped: needs the jar and the reference");
        return;
    }
    let (document, _) = commands_gamerule::commands_tables(&jar()).expect("export");
    let gamerule = &document["children"]["gamerule"];
    let children = gamerule["children"].as_object().expect("gamerule children");
    assert_eq!(children.len(), 118);
    let reference: Value =
        serde_json::from_slice(&std::fs::read(reference_table()).unwrap()).expect("reference JSON");
    let theirs = reference["children"]["gamerule"]["children"]
        .as_object()
        .expect("reference gamerule children");
    let mut failures = Vec::new();
    for (name, value) in children {
        match theirs.get(name) {
            Some(expected) if expected == value => {}
            Some(expected) => failures.push(describe_diff(name, value, expected)),
            None => failures.push(format!("{name}: not in the reference")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(gamerule["permissions"], reference["children"]["gamerule"]["permissions"]);
}

#[test]
fn write_puts_a_parsable_table_in_the_output_directory() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let named = std::env::var_os("HZR_EXPORT_OUT").map(PathBuf::from);
    let directory = named.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hzr-export-gamerule-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&directory);
    let coverage = commands_gamerule::write(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(commands_gamerule::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("valid JSON");
    let object = document.as_object().expect("an object");
    let children = object
        .get("children")
        .and_then(Value::as_object)
        .expect("children");
    assert_eq!(children.len(), coverage.produced.len());
    assert!(children.contains_key("gamerule"));
    if named.is_none() {
        let _ = std::fs::remove_dir_all(&directory);
    }
}
