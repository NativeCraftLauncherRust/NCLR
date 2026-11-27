//! Does `items_full` reproduce the Java registry tables from the player's jar?
//!
//! Reference: the Java-produced tables under `tmp/ship-extract/` (`registries.json` and
//! `command-registries.json`). The reader only reads `mc263/client.jar`; no `ship.hzr3`
//! value is used.
//!
//! The heavy readers (the whole class scan behind `registries.json`, and the merged
//! `command-registries.json`, which also runs `states.rs`'s state table) are cached for
//! the test process so the oracle checks don't redo them per test.

#[path = "../src/items_full.rs"]
mod items_full;

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;

fn workspace() -> PathBuf {
    // <workspace>/route3/crates/hzr-export
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference(name: &str) -> Value {
    let path = workspace().join("tmp/ship-extract").join(name);
    serde_json::from_slice(&std::fs::read(&path).expect("reference table")).expect("JSON")
}

struct RegistryCache {
    document: Value,
    produced: Vec<String>,
    missing: Vec<String>,
    notes: Vec<String>,
    registries: Vec<String>,
    absent_registries: Vec<String>,
}

fn registries_cached() -> &'static RegistryCache {
    static CACHE: OnceLock<RegistryCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        let (document, coverage) = items_full::registries(&jar()).expect("registries");
        RegistryCache {
            document,
            produced: coverage.produced,
            missing: coverage.missing,
            notes: coverage.notes,
            registries: coverage.registries,
            absent_registries: coverage.absent_registries,
        }
    })
}

struct CommandCache {
    document: Value,
    produced: Vec<String>,
    missing: Vec<String>,
    notes: Vec<String>,
}

fn command_cached() -> &'static CommandCache {
    static CACHE: OnceLock<CommandCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        let (document, coverage) =
            items_full::command_registries(&jar()).expect("command registries");
        CommandCache {
            document,
            produced: coverage.produced,
            missing: coverage.missing,
            notes: coverage.notes,
        }
    })
}

/// Which top-level keys differ between two documents.
fn describe_document_diff(mine: &Value, reference: &Value) -> String {
    let mine = mine.as_object().expect("document");
    let reference = reference.as_object().expect("document");
    let extra: Vec<&String> = mine
        .keys()
        .filter(|key| !reference.contains_key(*key))
        .collect();
    let missing: Vec<&String> = reference
        .keys()
        .filter(|key| !mine.contains_key(*key))
        .collect();
    let differing: Vec<&String> = mine
        .keys()
        .filter(|key| {
            reference
                .get(*key)
                .is_some_and(|value| value != &mine[*key])
        })
        .collect();
    format!("extra {extra:?}; missing {missing:?}; differing {differing:?}")
}

#[test]
fn every_emitted_registry_matches_the_java_report() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let cache = registries_cached();
    let generated = cache.document.as_object().expect("an object");
    let expected = reference("registries.json");
    let expected = expected.as_object().expect("an object");

    let mut failures = Vec::new();
    for (key, value) in generated {
        match expected.get(key) {
            Some(reference) if reference == value => {}
            Some(_) => failures.push(format!("{key}: differs")),
            None => failures.push(format!("{key}: not in the reference")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(generated.len(), 95, "emitted registry count");
    assert_eq!(cache.registries.len(), 95);
    assert!(cache.absent_registries.is_empty());
}

#[test]
fn the_vanilla_code_only_tables_are_reported_as_missing() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let cache = registries_cached();
    for table in ["items.json", "commands.json", "creative-inventory.json"] {
        assert!(
            cache.missing.iter().any(|line| line == table),
            "{table} is not reported as missing"
        );
        assert!(
            cache.notes.iter().any(|note| note.contains(table)),
            "{table} has no reason"
        );
    }
    // `blocks` is produced now that states.rs is green.
    assert!(cache
        .produced
        .iter()
        .any(|line| line == "command-registries.json#blocks"));
}

#[test]
fn additions_are_a_superset_of_the_base_module() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (base, _) = items_full::base_registries(&jar()).expect("base");
    let full = &registries_cached().document;
    let full = full.as_object().expect("object");
    let base = base.as_object().expect("object");
    for (key, value) in base {
        assert_eq!(
            full.get(key),
            Some(value),
            "{key} changed from the base module"
        );
    }
    assert_eq!(full.len() - base.len(), items_full::ADDED_REGISTRIES.len());
}

#[test]
fn blocks_fragment_matches_the_oracle() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let document = &command_cached().document;
    let blocks = document.get("blocks").expect("blocks");
    let reference = reference("command-registries.json");
    assert!(
        blocks == &reference["blocks"],
        "command-registries#blocks differs: {}",
        describe_document_diff(blocks, &reference["blocks"])
    );
    assert_eq!(blocks.as_object().map(|map| map.len()), Some(1286));
}

#[test]
fn command_registries_is_complete_and_matches_the_oracle() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let document = &command_cached().document;
    let reference = reference("command-registries.json");
    assert!(
        document == &reference,
        "command-registries differs: {}",
        describe_document_diff(document, &reference)
    );
    assert_eq!(
        document.as_object().map(|object| object.len()),
        Some(32),
        "all 32 top-level keys"
    );
}

#[test]
fn write_puts_a_parsable_registries_table_in_the_output_directory() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let directory =
        std::env::temp_dir().join(format!("hzr-export-items-full-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let coverage = items_full::write(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(items_full::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("valid JSON");
    let object = document.as_object().expect("an object");
    assert_eq!(object.len(), coverage.registries.len());
    assert_eq!(object.len(), 95);
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn write_command_registries_puts_the_merged_table_on_disk() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let directory = std::env::temp_dir().join(format!(
        "hzr-export-command-registries-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    items_full::write_command_registries(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(items_full::COMMAND_TABLE)).expect("file");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let object = document.as_object().expect("object");
    assert_eq!(object.len(), 32, "command-registries top-level keys");
    assert!(object.contains_key("items"));
    assert!(object.contains_key("blocks"));
    assert!(object.contains_key("block_entities"));
    assert_eq!(
        object["by_registry"].as_object().map(|map| map.len()),
        Some(155)
    );
    let _ = std::fs::remove_dir_all(&directory);
}
