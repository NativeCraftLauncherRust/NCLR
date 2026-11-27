//! Semantic parity tests for [`render`], the jar-driven port of the Java
//! `VanillaMobDataExporter` sections this module can rebuild from the client
//! jar: the client's keyframe animation definitions and every entity type's
//! spawn placement.
//!
//! `src/main.rs` is a binary and is not modified by this task, so the module is
//! included directly instead of through the crate root.
//!
//! The tests need the player's own client jar and the Java table:
//! * `mc263/client.jar`
//! * `tmp/ship-extract/mob-data.json`
//!
//! They are skipped (with a message) when the jar is absent, so a checkout
//! without the (never committed) Mojang files still builds. The Java export's
//! numbers are floats: they are compared at `f32` precision, the precision Java
//! wrote them at; strings, booleans and object/array shapes still compare
//! exactly.

#[path = "../src/render.rs"]
mod render;

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
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("render-parity");
    fs::create_dir_all(&dir).expect("create parity output dir");
    let text = serde_json::to_string_pretty(value).expect("serialize generated table");
    fs::write(dir.join(name), text + "\n").expect("write generated table");
}

/// The implemented sections of `mob-data.json`, as one object. The Java table's
/// `states` section is blocked (see `render::blocked_tables`), so it is removed
/// from the expected table before comparing.
fn implemented(mine: &render::MobData, expected: &Value) -> (Value, Value) {
    let mut expected = expected.clone();
    expected
        .as_object_mut()
        .expect("mob-data is an object")
        .remove("states");
    let mine = mine.to_value();
    let mut mine_object = mine
        .as_object()
        .cloned()
        .expect("generated table is an object");
    mine_object.remove("states");
    (Value::Object(mine_object), expected)
}

#[test]
fn mob_data_animations_match_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let mine = render::export_mob_data(jar()).expect("generate render tables");
    let expected = load(&reference("mob-data.json"));
    write_for_external_comparison("animations.json", &mine.animations);
    assert!(
        values_equal(&mine.animations, &expected["animations"]),
        "mob-data.json#animations differs: {}",
        describe_diff(&mine.animations, &expected["animations"])
    );
}

#[test]
fn mob_data_placements_match_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let mine = render::export_mob_data(jar()).expect("generate render tables");
    let expected = load(&reference("mob-data.json"));
    write_for_external_comparison("placements.json", &mine.placements);

    // Registry coverage/order source: the entity-type registry order this
    // module derives must name exactly the entity types Java wrote.
    let mine_order: Vec<String> = render::entity_type_order(jar()).expect("registry order");
    let expected_order: std::collections::BTreeSet<String> = expected["placements"]
        .as_object()
        .unwrap()
        .keys()
        .map(|key| key.trim_start_matches("minecraft:").to_string())
        .collect();
    assert_eq!(mine_order.len(), 161, "entity type registry size");
    let mine_set: std::collections::BTreeSet<String> = mine_order.iter().cloned().collect();
    assert_eq!(
        mine_set, expected_order,
        "entity type registry members differ"
    );
    assert!(
        values_equal(&mine.placements, &expected["placements"]),
        "mob-data.json#placements differs: {}",
        describe_diff(&mine.placements, &expected["placements"])
    );
}

#[test]
fn mob_data_implemented_sections_match_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let mine = render::export_mob_data(jar()).expect("generate render tables");
    let expected = load(&reference("mob-data.json"));
    let (mine, expected) = implemented(&mine, &expected);
    write_for_external_comparison("mob-data-implemented.json", &mine);
    assert!(
        values_equal(&mine, &expected),
        "mob-data.json (implemented sections) differs: {}",
        describe_diff(&mine, &expected)
    );
}

#[test]
fn animation_shapes_match_vanilla_counts() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let mine = render::export_mob_data(jar()).expect("generate render tables");
    let animations = mine.animations.as_object().expect("animations object");
    // 16 definition classes, 74 static fields, 848 channels, 5,070 keyframes,
    // 39 looping definitions in 26.3.
    assert_eq!(animations.len(), 74, "animation definition count");
    let mut channels = 0usize;
    let mut keyframes = 0usize;
    let mut looping = 0usize;
    for definition in animations.values() {
        if definition["looping"].as_bool() == Some(true) {
            looping += 1;
        }
        for channel_list in definition["bones"].as_object().unwrap().values() {
            for channel in channel_list.as_array().unwrap() {
                channels += 1;
                assert!(
                    matches!(
                        channel["target"].as_str(),
                        Some("position" | "rotation" | "scale")
                    ),
                    "channel target: {channel}"
                );
                keyframes += channel["keyframes"].as_array().unwrap().len();
            }
        }
    }
    assert_eq!(channels, 848, "animation channel count");
    assert_eq!(keyframes, 5070, "keyframe count");
    assert_eq!(looping, 39, "looping animation count");
}

#[test]
fn partial_table_is_labelled_and_blockers_are_explicit() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    // The partial table must only carry sections this module actually rebuilds;
    // adding a placeholder `states` would silently break the consumer.
    let mine = render::export_mob_data(jar()).expect("generate render tables");
    let table = mine.to_value();
    let mut keys: Vec<String> = table.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["animations", "placements"],
        "partial table sections"
    );

    let blockers = render::blocked_tables();
    let names: Vec<&str> = blockers.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        vec![
            "mob-data.json#states",
            "render-definitions.json",
            "entity-models.json"
        ],
        "blocked tables are documented"
    );
    assert!(
        blockers.iter().all(|(_, reason)| reason.len() > 40),
        "every blocked table carries a concrete reason"
    );
}
