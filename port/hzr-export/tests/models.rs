//! Tests for [`models`]: rebuilding `entity-models.json` by evaluating
//! `LayerDefinitions.createRoots()` and baking every layer.
//!
//! The module needs the shared class-file reader at the crate root (the same
//! wiring `hzr-bytecode`'s examples use), so it is declared here and included by
//! `src/models.rs` as `crate::classfile`.
//!
//! The tests need the player's own client jar and the Java table:
//! * `mc263/client.jar`
//! * `tmp/ship-extract/entity-models.json`
//!
//! They are skipped (with a message) when the jar is absent. Numbers are
//! compared at `f32` precision (the precision Java wrote them at).

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/models.rs"]
mod models;

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

/// Semantic equality. Numbers are compared as `f32` (Java wrote floats), with
/// `0.0`/`-0.0` treated as equal; everything else must be identical.
fn values_equal(mine: &Value, expected: &Value) -> bool {
    match (mine, expected) {
        (Value::Number(mine), Value::Number(expected)) => {
            match (mine.as_f64(), expected.as_f64()) {
                (Some(mine), Some(expected)) => {
                    let mine = mine as f32;
                    let expected = expected as f32;
                    mine.to_bits() == expected.to_bits() || mine == expected
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
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("models-parity");
    fs::create_dir_all(&dir).expect("create parity output dir");
    let text = serde_json::to_string(value).expect("serialize generated table");
    fs::write(dir.join(name), text).expect("write generated table");
}

fn count_parts(value: &Value) -> (usize, usize) {
    let mut parts = 1usize;
    let mut polygons = value["polygons"].as_array().map(Vec::len).unwrap_or(0);
    for child in value["children"]
        .as_object()
        .map(|map| map.values())
        .into_iter()
        .flatten()
    {
        let (child_parts, child_polygons) = count_parts(child);
        parts += child_parts;
        polygons += child_polygons;
    }
    (parts, polygons)
}

#[test]
fn entity_models_matches_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let table = models::entity_models(&jar()).expect("entity models");
    assert!(
        table.gaps.is_empty(),
        "layers that could not be baked: {:?}",
        table.gaps
    );
    let mine = table.to_value();
    let expected = load(&reference("entity-models.json"));
    write_for_external_comparison("entity-models.json", &mine);
    assert!(
        values_equal(&mine, &expected),
        "entity-models.json differs: {}",
        describe_diff(&mine, &expected)
    );
}

#[test]
fn entity_models_shape_matches_vanilla() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let table = models::entity_models(&jar()).expect("entity models");
    assert_eq!(table.layers.len(), 361, "layer count");
    let keys: std::collections::BTreeSet<&str> = table.keys().into_iter().collect();
    for expected_key in [
        "minecraft:allay#main",
        "minecraft:player#main",
        "minecraft:armor_stand#boots",
        "minecraft:boat/oak#main",
    ] {
        assert!(keys.contains(expected_key), "missing {expected_key}");
    }
    let mut parts = 0usize;
    let mut polygons = 0usize;
    for (_, value) in &table.layers {
        let (layer_parts, layer_polygons) = count_parts(value);
        parts += layer_parts;
        polygons += layer_polygons;
        // Every baked part carries the nine pose floats.
        assert_eq!(
            value["pose"].as_array().map(Vec::len),
            Some(9),
            "root pose length"
        );
    }
    assert_eq!(parts, 3380, "part count");
    assert_eq!(polygons, 15070, "polygon count");
}

#[test]
fn written_table_round_trips() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let out =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("models/render-entity-models.json");
    let table = models::write_entity_models(&jar(), &out).expect("write");
    let written: Value =
        serde_json::from_slice(&fs::read(&out).expect("read back")).expect("parse written table");
    let expected = load(&reference("entity-models.json"));
    assert!(
        values_equal(&written, &expected),
        "written table differs: {}",
        describe_diff(&written, &expected)
    );
    assert_eq!(table.layers.len(), 361);
}
