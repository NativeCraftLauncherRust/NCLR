//! Does the Rust exporter reproduce the Java tables?
//!
//! The reference is `tmp/ship-extract/command-registries.json`, written by
//! `VanillaCommandParseOracle` (`registries` mode) from the real 26.3 jars — the
//! same file `ship.hzr3` carries. These tests read the player's `mc263/client.jar`
//! and compare the generated document to it, key by key.
//!
//! The tables the exporter cannot build from the jar alone (`items.json`,
//! `commands.json`, `creative-inventory.json`, and the code-registry half of
//! `command-registries.json`) are listed in
//! [`registries::BLOCKED_TABLES`]/[`registries::Coverage`]; the last test pins
//! that they are reported rather than silently dropped.

#[path = "../src/registries.rs"]
mod registries;

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
    workspace().join("tmp/ship-extract/command-registries.json")
}

/// A short description of the first differences, for the failure message.
fn describe_diff(key: &str, mine: &Value, theirs: &Value) -> String {
    match (mine, theirs) {
        (Value::Array(mine), Value::Array(theirs)) => {
            let extra: Vec<&Value> = mine
                .iter()
                .filter(|value| !theirs.contains(value))
                .take(5)
                .collect();
            let missing: Vec<&Value> = theirs
                .iter()
                .filter(|value| !mine.contains(value))
                .take(5)
                .collect();
            format!(
                "{key}: {} vs {} entries; extra {extra:?}; missing {missing:?}",
                mine.len(),
                theirs.len()
            )
        }
        (Value::Object(mine), Value::Object(theirs)) => {
            let extra: Vec<&String> = mine.keys().filter(|name| !theirs.contains_key(*name)).take(5).collect();
            let missing: Vec<&String> = theirs.keys().filter(|name| !mine.contains_key(*name)).take(5).collect();
            let differing: Vec<&String> = mine
                .keys()
                .filter(|name| theirs.get(*name).is_some_and(|value| value != &mine[*name]))
                .take(5)
                .collect();
            format!(
                "{key}: {} vs {} entries; extra {extra:?}; missing {missing:?}; differing {differing:?}",
                mine.len(),
                theirs.len()
            )
        }
        _ => format!("{key}: value differs"),
    }
}

#[test]
fn command_registries_reproduces_the_java_export() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let (document, coverage) = registries::command_registries(&jar()).expect("the jar is readable");
    let generated = document.as_object().expect("an object");
    let reference: Value =
        serde_json::from_slice(&std::fs::read(reference_table()).unwrap()).expect("reference JSON");
    let reference = reference.as_object().expect("an object");

    let mut failures = Vec::new();
    let mut matched = 0usize;
    for (key, value) in generated {
        if key == "by_registry" {
            continue;
        }
        match reference.get(key) {
            Some(expected) if expected == value => matched += 1,
            Some(expected) => failures.push(describe_diff(key, value, expected)),
            None => failures.push(format!("{key}: not in the reference")),
        }
    }

    // `by_registry` is compared entry by entry: the code-registered half is out of
    // this module's reach, and only the registries it does know about have to match.
    if let (Some(Value::Object(mine)), Some(Value::Object(theirs))) =
        (generated.get("by_registry"), reference.get("by_registry"))
    {
        let mut identical = 0usize;
        for (id, entry) in mine {
            match theirs.get(id) {
                Some(expected) if expected == entry => identical += 1,
                Some(_) => failures.push(format!("by_registry[{id}]: differs")),
                None => failures.push(format!("by_registry[{id}]: not in the reference")),
            }
        }
        let absent = theirs.keys().filter(|id| !mine.contains_key(*id)).count();
        eprintln!("by_registry: {identical} registries identical, {absent} absent (code-only)");
    } else {
        failures.push("by_registry: absent".to_string());
    }

    eprintln!("exact keys: {matched} of {}", generated.len());
    eprintln!("keys this jar cannot produce: {:?}", coverage.missing);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_derivable_counts_match_vanilla_26_3() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, _) = registries::command_registries(&jar()).expect("export");
    let generated = document.as_object().expect("an object");
    let length = |key: &str| {
        generated
            .get(key)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{key} is not an array"))
            .len()
    };
    // Independent expected values: what a Java-produced table of 26.3 carries.
    assert_eq!(length("learnable_recipes"), 1739);
    assert_eq!(length("templates"), 1511);
    assert_eq!(length("item_tags"), 236);
    assert_eq!(length("block_tags"), 300);
    assert_eq!(length("entity_types"), 161);
    assert_eq!(length("summonable"), 159);
    assert_eq!(length("effects"), 40);
    assert_eq!(length("attributes"), 40);
    assert_eq!(length("sounds"), 1991);
    assert_eq!(length("enchantments"), 43);
    assert_eq!(length("structures"), 52);
    assert_eq!(length("biomes"), 67);
    assert_eq!(length("damage_types"), 51);
    assert_eq!(length("clocks"), 2);
    assert_eq!(length("timelines"), 4);
    assert_eq!(length("persistent_components"), 119);
    assert_eq!(
        generated
            .get("advancement_criteria")
            .and_then(Value::as_object)
            .map(|map| map.len()),
        Some(1866)
    );
    assert_eq!(
        generated
            .get("block_tag_members")
            .and_then(Value::as_object)
            .map(|map| map.len()),
        Some(300)
    );
    // The one entry the sounds' loop-built ids could be missed by.
    let sounds = generated["sounds"].as_array().unwrap();
    assert!(sounds.contains(&Value::String("minecraft:entity.cat.ambient".into())));
    // `player` and `fishing_bobber` are the only types nobody may summon.
    let summonable = generated["summonable"].as_array().unwrap();
    assert!(!summonable.contains(&Value::String("minecraft:player".into())));
    assert!(!summonable.contains(&Value::String("minecraft:fishing_bobber".into())));
}

#[test]
fn the_tables_that_need_vanillas_code_are_reported() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, coverage) = registries::command_registries(&jar()).expect("export");
    let generated = document.as_object().expect("an object");
    for key in ["items", "blocks", "block_entities"] {
        assert!(
            coverage.missing.iter().any(|name| name == key),
            "{key} is expected to be missing, produced: {:?}",
            coverage.produced
        );
        assert!(!generated.contains_key(key), "{key} should not be invented");
    }
    let notes = coverage.notes.join("\n");
    for (table, _) in registries::BLOCKED_TABLES {
        assert!(notes.contains(table), "{table} is not named in the notes");
    }
    // Every key is either produced or listed as missing.
    assert_eq!(coverage.produced.len() + coverage.missing.len(), registries::KEYS.len());
}

#[test]
fn write_puts_a_parsable_table_in_the_output_directory() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    // `HZR_EXPORT_OUT` keeps the table somewhere the caller named (used to diff it
    // against the Java export); otherwise it lives in a scratch directory.
    let named = std::env::var_os("HZR_EXPORT_OUT").map(PathBuf::from);
    let directory = named.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hzr-export-registries-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&directory);
    let coverage = registries::write(&jar(), &directory).expect("write");
    let text = std::fs::read_to_string(directory.join(registries::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("valid JSON");
    let object = document.as_object().expect("an object");
    assert_eq!(object.len(), coverage.produced.len());
    assert!(object.contains_key("sounds"));
    assert!(!object.contains_key("items"));
    if named.is_none() {
        let _ = std::fs::remove_dir_all(&directory);
    }
}
