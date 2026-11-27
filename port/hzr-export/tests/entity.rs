//! Semantic parity tests for [`entity`], the jar-driven port of the Java
//! `VanillaEntityTypesExporter` and `VanillaEntityAttributeOracle` exporters.
//!
//! `src/main.rs` is a binary and is not modified by this task, so the module is
//! included directly instead of through the crate root.
//!
//! The tests need the player's own client jar and the Java/prepared tables:
//! * `mc263/client.jar`
//! * `tmp/ship-extract/entity-types.json`
//! * `tmp/ship-extract/attributes.json`
//!
//! They are skipped (with a message) when the jar is absent, so a checkout
//! without the (never committed) Mojang files still builds.

#[path = "../src/entity.rs"]
mod entity;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate lives at <root>/route3/crates/hzr-export")
        .to_path_buf()
}

fn jar() -> PathBuf {
    workspace_root().join("mc263").join("client.jar")
}

fn reference(name: &str) -> PathBuf {
    workspace_root().join("tmp").join("ship-extract").join(name)
}

fn load(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Semantic equality with a tiny tolerance for finite JSON numbers.
///
/// `serde_json` is used here without its `float_roundtrip` feature (that would
/// need a `Cargo.toml` edit this task may not make), so parsing the reference
/// file can shift a long decimal such as `0.23000000417232513` by one ulp.
/// The generated side is built from exact `f64`/`f32` constants. Integer and
/// string values (including attribute bit patterns) still compare exactly.
fn values_equal(mine: &Value, expected: &Value) -> bool {
    match (mine, expected) {
        (Value::Number(mine), Value::Number(expected)) => {
            match (mine.as_f64(), expected.as_f64()) {
                (Some(mine), Some(expected)) => {
                    (mine - expected).abs() <= 1e-12 * (1.0 + mine.abs().max(expected.abs()))
                }
                _ => mine == expected,
            }
        }
        (Value::Object(mine), Value::Object(expected)) => {
            mine.len() == expected.len()
                && mine.iter().all(|(key, value)| {
                    expected
                        .get(key)
                        .is_some_and(|other| values_equal(value, other))
                })
        }
        (Value::Array(mine), Value::Array(expected)) => {
            mine.len() == expected.len()
                && mine
                    .iter()
                    .zip(expected)
                    .all(|(value, other)| values_equal(value, other))
        }
        _ => mine == expected,
    }
}

fn first_difference(mine: &Value, expected: &Value, path: &str) -> Option<String> {
    match (mine, expected) {
        (Value::Object(mine), Value::Object(expected)) => {
            if let Some(key) = expected.keys().find(|key| !mine.contains_key(*key)) {
                return Some(format!("{path}/{key}: missing"));
            }
            if let Some(key) = mine.keys().find(|key| !expected.contains_key(*key)) {
                return Some(format!("{path}/{key}: extra"));
            }
            for key in expected.keys() {
                if !values_equal(mine.get(key).unwrap(), expected.get(key).unwrap()) {
                    return first_difference(
                        mine.get(key).unwrap(),
                        expected.get(key).unwrap(),
                        &format!("{path}/{key}"),
                    );
                }
            }
            None
        }
        (Value::Array(mine), Value::Array(expected)) => {
            if mine.len() != expected.len() {
                return Some(format!(
                    "{path}: array length {} != {}",
                    mine.len(),
                    expected.len()
                ));
            }
            for (index, (mine, expected)) in mine.iter().zip(expected).enumerate() {
                if !values_equal(mine, expected) {
                    return first_difference(mine, expected, &format!("{path}[{index}]"));
                }
            }
            None
        }
        _ => Some(format!("{path}: {mine} != {expected}")),
    }
}

fn describe_diff(mine: &Value, expected: &Value) -> String {
    match (mine.as_object(), expected.as_object()) {
        (Some(mine), Some(expected)) => {
            let missing: Vec<&String> =
                expected.keys().filter(|k| !mine.contains_key(*k)).collect();
            let extra: Vec<&String> = mine.keys().filter(|k| !expected.contains_key(*k)).collect();
            let changed: Vec<&String> = expected
                .keys()
                .filter(|k| !values_equal(mine.get(*k).unwrap(), expected.get(*k).unwrap()))
                .collect();
            let details: Vec<String> = changed
                .iter()
                .filter_map(|key| {
                    first_difference(mine.get(*key).unwrap(), expected.get(*key).unwrap(), key)
                })
                .collect();
            format!(
                "missing {missing:?}, extra {extra:?}, changed {} keys: {:?}; first differences: {:?}",
                changed.len(),
                &changed[..changed.len().min(5)],
                &details[..details.len().min(5)]
            )
        }
        _ => "values are not both objects".to_string(),
    }
}

fn write_for_external_comparison(name: &str, value: &Value) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("entity-parity");
    fs::create_dir_all(&dir).expect("create parity output dir");
    let text = serde_json::to_string_pretty(value).expect("serialize generated table");
    fs::write(dir.join(name), text + "\n").expect("write generated table");
}

#[test]
fn entity_types_matches_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let (mine, _) = entity::generate(jar()).expect("generate entity tables");
    let expected = load(&reference("entity-types.json"));
    write_for_external_comparison("entity-types.json", &mine);
    assert!(
        values_equal(&mine, &expected),
        "entity-types.json differs: {}",
        describe_diff(&mine, &expected)
    );
}

#[test]
fn attributes_matches_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let (_, mine) = entity::generate(jar()).expect("generate entity tables");
    let expected = load(&reference("attributes.json"));
    write_for_external_comparison("attributes.json", &mine);
    assert!(
        values_equal(&mine, &expected),
        "attributes.json differs: {}",
        describe_diff(&mine, &expected)
    );
}

#[test]
fn table_shapes_match_vanilla_registries() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let (entity_types, attributes) = entity::generate(jar()).expect("generate entity tables");

    // Vanilla 26.3 registers 161 entity types, 40 attributes and 93 living
    // entity types with a default attribute supplier.
    let entity_types = entity_types.as_object().expect("entity-types object");
    assert_eq!(entity_types.len(), 161, "entity type registry size");
    let definitions = attributes
        .get("attributes")
        .and_then(Value::as_array)
        .expect("attributes array");
    assert_eq!(definitions.len(), 40, "attribute registry size");
    let entities = attributes
        .get("entities")
        .and_then(Value::as_object)
        .expect("entities object");
    assert_eq!(
        entities.len(),
        93,
        "living entities with attribute suppliers"
    );
    assert_eq!(attributes.get("data_version"), Some(&Value::from(5023u64)));

    // Every living entity's attribute table uses registry attribute ids and
    // hex-encoded double bits.
    let ids: Vec<&str> = definitions
        .iter()
        .map(|def| def.get("id").and_then(Value::as_str).expect("attribute id"))
        .collect();
    for (entity, table) in entities {
        let table = table.as_object().expect("entity attribute table");
        for (attribute, value) in table {
            assert!(
                ids.contains(&attribute.as_str()),
                "{entity}: {attribute} is not a registered attribute"
            );
            let bits = value.as_str().expect("attribute bits");
            assert_eq!(bits.len(), 16, "{entity}: {attribute} bits length");
            assert!(bits.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
}
