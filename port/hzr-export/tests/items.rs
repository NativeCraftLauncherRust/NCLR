//! Does the Rust reader reproduce the Java registry tables from the player's jar?
//!
//! The reference values are the Java-produced tables under `tmp/ship-extract/`
//! (`registries.json` from the data generator, `command-registries.json` from
//! `VanillaCommandParseOracle`). The reader itself reads only `mc263/client.jar`.
//!
//! The tables that need vanilla's code to run (`items.json`, `commands.json`,
//! `creative-inventory.json`) and the state-property half of `blocks` are pinned as
//! reported gaps, never invented: a partial table is not written.

#[path = "../src/items.rs"]
mod items;

use std::path::PathBuf;

use serde_json::Value;

fn workspace() -> PathBuf {
    // <workspace>/route3/crates/hzr-export
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference_registries() -> Value {
    let path = workspace().join("tmp/ship-extract/registries.json");
    serde_json::from_slice(&std::fs::read(path).expect("reports/registries.json")).expect("JSON")
}

fn reference_command_registries() -> Value {
    let path = workspace().join("tmp/ship-extract/command-registries.json");
    serde_json::from_slice(&std::fs::read(path).expect("command-registries.json")).expect("JSON")
}

fn ids(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("an array")
        .iter()
        .map(|item| item.as_str().expect("a string").to_string())
        .collect()
}

/// A short description of where two id lists first differ, for the failure message.
fn first_diff(mine: &[String], expected: &[String]) -> String {
    if mine == expected {
        return "no difference".to_string();
    }
    for (index, (a, b)) in mine.iter().zip(expected.iter()).enumerate() {
        if a != b {
            let start = index.saturating_sub(3);
            return format!(
                "first difference at {index}: ours {a:?}, reference {b:?}\n  ours  {:?}\n  ref   {:?}",
                &mine[start..(index + 4).min(mine.len())],
                &expected[start..(index + 4).min(expected.len())]
            );
        }
    }
    let extra = &mine[expected.len().min(mine.len())..];
    let missing = &expected[mine.len().min(expected.len())..];
    format!(
        "lengths {} vs {}; extra {:?}; missing {:?}",
        mine.len(),
        expected.len(),
        &extra[..extra.len().min(5)],
        &missing[..missing.len().min(5)]
    )
}

#[test]
fn items_come_out_in_registry_order() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let mine = items::item_ids(&jar()).expect("items");
    let reference = reference_command_registries();
    let expected = ids(&reference["items"]);
    // Independent count: 26.3's item registry.
    assert_eq!(mine.len(), 1658, "item count");
    assert!(
        mine == expected,
        "item order or ids differ: {}",
        first_diff(&mine, &expected)
    );
}

#[test]
fn blocks_and_block_entities_come_out_in_registry_order() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let blocks = items::block_ids(&jar()).expect("blocks");
    let entities = items::block_entities(&jar()).expect("block entities");
    let reference = reference_command_registries();
    // The `blocks` dict comparison is order-insensitive; the order itself is pinned by
    // the `minecraft:block` protocol ids in `registries.json` below.
    let expected_blocks: Vec<String> = reference["blocks"]
        .as_object()
        .expect("blocks object")
        .keys()
        .cloned()
        .collect();
    let mine: std::collections::BTreeSet<String> = blocks.iter().cloned().collect();
    let expected: std::collections::BTreeSet<String> = expected_blocks.into_iter().collect();
    assert_eq!(blocks.len(), 1286, "block count");
    assert_eq!(mine, expected);
    assert_eq!(entities.len(), 191, "block entity count");
    assert_eq!(entities, ids(&reference["block_entities"]));
}

#[test]
fn registries_reproduces_the_verified_subset_exactly() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, coverage) = items::registries(&jar()).expect("registries");
    let generated = document.as_object().expect("an object");
    let reference = reference_registries();
    let reference = reference.as_object().expect("an object");

    // `minecraft:item` and `minecraft:block` come from the registration traces; the rest
    // are the verified registries.
    let mut expected_keys: std::collections::BTreeSet<String> = items::VERIFIED_REGISTRIES
        .iter()
        .map(|key| key.to_string())
        .collect();
    expected_keys.insert("minecraft:item".into());
    expected_keys.insert("minecraft:block".into());
    assert_eq!(
        generated
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        expected_keys
    );
    assert_eq!(generated.len(), 68, "verified registry count");

    let mut failures = Vec::new();
    for (key, value) in generated {
        match reference.get(key) {
            Some(expected) if expected == value => {}
            Some(_) => failures.push(format!("{key}: differs")),
            None => failures.push(format!("{key}: not in the reference")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    // 95 registries in vanilla 26.3's report; 27 are not derivable and are reported.
    assert_eq!(coverage.registries.len(), 68);
    assert_eq!(coverage.absent_registries.len(), 27);
    assert!(coverage.notes.iter().any(|note| note.contains("68 of 95")));
}

#[test]
fn command_registry_fragments_fill_the_missing_arrays() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let fragments = items::command_fragments(&jar()).expect("fragments");
    let reference = reference_command_registries();
    let object = fragments.as_object().expect("an object");
    assert_eq!(object["items"], reference["items"]);
    assert_eq!(object["block_entities"], reference["block_entities"]);
    assert!(
        !object.contains_key("blocks"),
        "blocks must stay absent until state properties can be read exactly"
    );

    // Every code-registered `by_registry` entry this module fills must be identical to
    // the oracle's, including its tag list.
    let reference_by = reference["by_registry"].as_object().expect("by_registry");
    let by = object["by_registry"].as_object().expect("by_registry");
    assert!(!by.is_empty());
    let mut failures = Vec::new();
    for (key, value) in by {
        match reference_by.get(key) {
            Some(expected) if expected == value => {}
            Some(_) => failures.push(format!("by_registry[{key}]: differs")),
            None => failures.push(format!("by_registry[{key}]: not in the reference")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_tables_that_need_vanillas_code_are_reported_not_invented() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, coverage) = items::registries(&jar()).expect("registries");
    assert!(document.is_object());
    for (table, _) in items::BLOCKED_TABLES {
        assert!(
            coverage.missing.iter().any(|name| name == table),
            "{table} is not reported as missing"
        );
        assert!(
            coverage.notes.iter().any(|note| note.contains(table)),
            "{table} has no reason"
        );
    }
    assert!(coverage
        .missing
        .iter()
        .any(|name| name == "command-registries.json#blocks"));
    for key in ["items", "blocks", "block_entities"] {
        let reported = coverage
            .produced
            .iter()
            .chain(coverage.missing.iter())
            .any(|name| name.contains(key));
        if key != "blocks" {
            assert!(reported, "{key} is neither produced nor missing");
        }
    }
}

#[test]
fn write_puts_a_parsable_registries_table_in_the_output_directory() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let directory = std::env::temp_dir().join(format!("hzr-export-items-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    let coverage = items::write(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(items::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("valid JSON");
    let object = document.as_object().expect("an object");
    assert_eq!(object.len(), coverage.registries.len());
    assert!(object.contains_key("minecraft:block"));
    assert!(object.contains_key("minecraft:item"));
    let _ = std::fs::remove_dir_all(&directory);
}
