//! Does the Rust exporter reproduce `datafix-tables.json`?
//!
//! The reference is `tmp/ship-extract/datafix-tables.json`, written by
//! `VanillaDataFixTablesExporter` from the real 26.3 jars — the file `ship.hzr3`
//! carries and `horizon-world`'s `datafix/tables.rs` reads. These tests re-derive
//! it from the player's `mc263/client.jar` and compare key by key.

#[path = "../src/datafix.rs"]
mod datafix;

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
    workspace().join("tmp/ship-extract/datafix-tables.json")
}

fn describe_diff(key: &str, mine: &Value, theirs: &Value) -> String {
    match (mine, theirs) {
        (Value::Array(mine), Value::Array(theirs)) => {
            let extra: Vec<&Value> = mine.iter().filter(|v| !theirs.contains(v)).take(4).collect();
            let missing: Vec<&Value> = theirs.iter().filter(|v| !mine.contains(v)).take(4).collect();
            let first_difference = mine
                .iter()
                .zip(theirs.iter())
                .position(|(a, b)| a != b)
                .map(|index| {
                    format!(
                        "first difference at {index}: ours {:?} reference {:?}",
                        &mine[index], &theirs[index]
                    )
                })
                .unwrap_or_default();
            format!(
                "{key}: {} vs {} entries; extra {extra:?}; missing {missing:?}; {first_difference}",
                mine.len(),
                theirs.len()
            )
        }
        (Value::Object(mine), Value::Object(theirs)) => {
            let missing: Vec<&String> = theirs.keys().filter(|k| !mine.contains_key(*k)).take(4).collect();
            format!("{key}: {} vs {} keys; missing {missing:?}", mine.len(), theirs.len())
        }
        _ => format!("{key}: value differs"),
    }
}

#[test]
fn datafix_tables_reproduces_the_java_export() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let (document, coverage) = datafix::datafix_tables(&jar()).expect("the jar is readable");
    let generated = document.as_object().expect("an object");
    let reference: Value =
        serde_json::from_slice(&std::fs::read(reference_table()).unwrap()).expect("reference JSON");
    let reference = reference.as_object().expect("an object");

    let mut failures = Vec::new();
    let mut matched = 0usize;
    for (key, value) in generated {
        match reference.get(key) {
            Some(expected) if expected == value => matched += 1,
            Some(expected) => failures.push(describe_diff(key, value, expected)),
            None => failures.push(format!("{key}: not in the reference")),
        }
    }
    eprintln!("exact keys: {matched} of {}", generated.len());
    eprintln!("keys this jar cannot produce: {:?}", coverage.missing);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_table_is_the_one_horizon_world_reads() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, coverage) = datafix::datafix_tables(&jar()).expect("export");
    let generated = document.as_object().expect("an object");
    // Every key of the exported shape is either produced or reported missing.
    assert_eq!(
        coverage.produced.len() + coverage.missing.len(),
        datafix::KEYS.len()
    );
    // Independent expected values: what the Java-produced table of 26.3 carries.
    let length = |key: &str| {
        generated
            .get(key)
            .and_then(|value| value.as_array())
            .unwrap_or_else(|| panic!("{key} is not an array"))
            .len()
    };
    assert_eq!(length("ITEM_FLATTENING"), 320);
    assert_eq!(length("ITEM_DAMAGE_IDS"), 52);
    assert_eq!(length("BLOCK_IDS"), 254);
    assert_eq!(length("ITEM_IDS"), 315);
    assert_eq!(length("SPECIAL_OBJECTIVE_CRITERIA"), 43);
    assert_eq!(length("STATS_SKIP"), 5);
    assert_eq!(length("STATS_CUSTOM"), 50);
    assert_eq!(length("STATS_ITEM_KEYS"), 5);
    assert_eq!(length("STATS_ENTITY_KEYS"), 2);
    assert_eq!(length("STATS_ENTITIES"), 43);
    assert_eq!(length("RECIPES"), 54);
    assert_eq!(length("RECIPES_RENAMENING"), 6);
    assert_eq!(length("ADVANCEMENTS"), 55);
    assert_eq!(length("GENERATOR_BIOMES"), 73);
    assert_eq!(length("SPAWN_EGGS"), 49);
    assert_eq!(length("ENCHANTMENTS"), 34);
    assert_eq!(length("CORAL"), 20);
    assert_eq!(length("CORAL_FANS"), 5);
    assert_eq!(length("THE_RENAMENING_ENTITIES"), 12);
    assert_eq!(length("THE_RENAMENING_BLOCKS"), 14);
    assert_eq!(length("COD_SALMON"), 2);
    assert_eq!(length("COD_SALMON_EGGS"), 2);
    assert_eq!(length("ENTITIES_V1460"), 83);
    assert_eq!(length("ENTITIES_V99"), 67);
    assert_eq!(length("BLOCK_ENTITIES_V99"), 23);
    assert_eq!(length("SPAWN_EGG_ENTITIES"), 65);
    assert_eq!(length("ENTITY_IDS"), 75);
    assert_eq!(length("BLOCK_ENTITY_IDS"), 23);
    assert_eq!(length("ITEM_TO_BLOCK_ENTITY_V99"), 34);
    assert_eq!(length("ITEM_POTIONS"), 110);
    assert_eq!(length("STRUCTURE_TEMPLATE_RENAMES"), 4);
    assert_eq!(length("BLOCK_STATE_DATA"), 1695);
    // Spot checks that pin the semantics, not just the counts.
    assert_eq!(
        generated["ITEM_FLATTENING"][0],
        serde_json::json!(["minecraft:anvil.0", "minecraft:anvil"])
    );
    let state = generated["BLOCK_STATE_DATA"][0].as_str().unwrap();
    assert_eq!(state, "0\tminecraft:air\t\tminecraft:air\t");
    let first = &generated["BLOCK_STATE_DATA"][1].as_str().unwrap();
    assert!(first.starts_with("16\tminecraft:stone\t\t"), "{first}");
    assert_eq!(
        generated["STRUCTURE_TEMPLATE_RENAMES"][0][1],
        Value::String("ECP".into())
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
        std::env::temp_dir().join(format!("hzr-export-datafix-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&directory);
    let coverage = datafix::write(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(datafix::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("valid JSON");
    let object = document.as_object().expect("an object");
    assert_eq!(object.len(), coverage.produced.len());
    assert!(object.contains_key("BLOCK_STATE_DATA"));
    if named.is_none() {
        let _ = std::fs::remove_dir_all(&directory);
    }
}
