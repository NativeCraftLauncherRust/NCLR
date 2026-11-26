//! Tests for [`render_full`]: the jar-driven rebuild of
//! `render-definitions.json` (per-block skip-rendering rule, `can_occlude`,
//! `minecraft:bars` membership, default state id and state-id range).
//!
//! The module needs the shared class-file reader at the crate root (the same
//! wiring `hzr-bytecode`'s examples use), so it is declared here and included by
//! `src/render_full.rs` as `crate::classfile`.
//!
//! The tests need the player's own client jar and the Java table:
//! * `mc263/client.jar`
//! * `tmp/ship-extract/render-definitions.json`
//!
//! They are skipped (with a message) when the jar is absent.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/render_full.rs"]
mod render_full;

use std::collections::BTreeMap;
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

fn values_equal(mine: &Value, expected: &Value) -> bool {
    match (mine, expected) {
        (Value::Number(mine), Value::Number(expected)) => {
            mine.as_u64() == expected.as_u64() && mine.as_f64() == expected.as_f64()
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
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("render-full-parity");
    fs::create_dir_all(&dir).expect("create parity output dir");
    let text = serde_json::to_string_pretty(value).expect("serialize generated table");
    fs::write(dir.join(name), text + "\n").expect("write generated table");
}

#[test]
fn render_definitions_matches_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let mine = render_full::render_definitions_value(&jar()).expect("render definitions");
    let expected = load(&reference("render-definitions.json"));
    write_for_external_comparison("render-definitions.json", &mine);
    assert!(
        values_equal(&mine, &expected),
        "render-definitions.json differs: {}",
        describe_diff(&mine, &expected)
    );
}

#[test]
fn render_definitions_shape_matches_vanilla() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let table = render_full::render_definitions(&jar()).expect("render definitions");
    assert_eq!(table.entries.len(), 1286, "block count");
    assert!(table.notes.is_empty(), "report notes: {:?}", table.notes);

    let mut rules: BTreeMap<&str, usize> = BTreeMap::new();
    let mut bars = Vec::new();
    let mut next_state = 0u32;
    for (id, definition) in &table.entries {
        *rules.entry(definition.rule).or_default() += 1;
        if definition.bars_tag {
            bars.push(id.clone());
        }
        // State ids are contiguous across the registry, in registry order.
        assert_eq!(
            definition.states.first().copied(),
            Some(next_state),
            "{id}: first state id"
        );
        assert_eq!(
            definition.states.len(),
            definition.states.last().copied().unwrap_or(0) as usize - next_state as usize + 1,
            "{id}: state ids are contiguous"
        );
        assert!(
            definition.states.contains(&definition.default),
            "{id}: default state is inside the range"
        );
        next_state += definition.states.len() as u32;
    }
    assert_eq!(next_state, 35_723, "state registry size");
    assert_eq!(
        rules,
        BTreeMap::from([
            ("bars", 26),
            ("leaves", 14),
            ("liquid", 2),
            ("mangrove_roots", 1),
            ("never", 1211),
            ("same_block", 32),
        ]),
        "skip-rendering rule distribution"
    );
    // `minecraft:bars`: iron bars plus the eight copper bars.
    assert_eq!(bars.len(), 9, "bars tag members: {bars:?}");
    assert!(bars.contains(&"minecraft:iron_bars".to_string()));
    assert!(bars.contains(&"minecraft:waxed_oxidized_copper_bars".to_string()));
}

#[test]
fn can_occlude_matches_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let mine = render_full::can_occlude(&jar()).expect("can_occlude");
    let expected = load(&reference("render-definitions.json"));
    assert_eq!(mine.len(), 1286, "blocks with a can_occlude value");
    let mut wrong = Vec::new();
    for (id, row) in expected.as_object().unwrap() {
        let name = id.trim_start_matches("minecraft:");
        let value = mine
            .get(name)
            .unwrap_or_else(|| panic!("{id}: no can_occlude"));
        let expected_value = row["can_occlude"].as_bool().unwrap();
        if *value != expected_value && wrong.len() < 12 {
            wrong.push(format!("{id}: ours {value}, java {expected_value}"));
        }
    }
    assert!(wrong.is_empty(), "can_occlude mismatches: {wrong:?}");

    // The properties evaluator cannot execute the copper collections and falls
    // back on the call sites' property functions; the fill-in passes must have
    // produced a value for every block.
    let probe = render_full::probe(&jar()).expect("probe");
    assert_eq!(probe.registry, 1286);
    assert!(
        probe.class_conflicts.is_empty(),
        "{:?}",
        probe.class_conflicts
    );
}

#[test]
fn state_section_is_blocked_with_a_reason_per_column() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    // The state registry itself is available (ids/order/values)…
    assert_eq!(
        render_full::states_row_count(&jar()).expect("state count"),
        35_723,
        "state registry size"
    );
    // …but a `mob-data.json#states` row needs these facts, none of which come
    // from the jar's data.
    let blockers = render_full::blocked_tables();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].0, "mob-data.json#states");
    assert_eq!(
        render_full::STATES_BLOCKED.len(),
        9,
        "every blocked row column is named"
    );
    let names: Vec<&str> = render_full::STATES_BLOCKED
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert!(names.iter().any(|name| name.contains("path type")));
    assert!(names.iter().any(|name| name.contains("height")));
    assert!(names.iter().any(|name| name.contains("isSolidRender")));
    assert!(render_full::STATES_BLOCKED
        .iter()
        .all(|(_, why)| why.len() > 40));
}

#[test]
fn written_text_is_byte_identical_to_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render-full/render-definitions.json");
    let table = render_full::write_render_definitions(&jar(), &out).expect("write");
    let mine = fs::read(&out).expect("read back");
    let expected = fs::read(reference("render-definitions.json")).expect("read java");
    assert_eq!(
        mine.len(),
        expected.len(),
        "written table length differs from the Java export"
    );
    if mine != expected {
        let at = mine
            .iter()
            .zip(expected.iter())
            .position(|(left, right)| left != right)
            .unwrap_or(0);
        let start = at.saturating_sub(80);
        panic!(
            "written table differs at byte {at}:\n ours: {}\n java: {}",
            String::from_utf8_lossy(&mine[start..(start + 240).min(mine.len())]),
            String::from_utf8_lossy(&expected[start..(start + 240).min(expected.len())])
        );
    }
    // The written keys are sorted (a `TreeMap`); the in-memory entries keep
    // registry order (`minecraft:air` is block 0).
    assert_eq!(table.entries.first().unwrap().0, "minecraft:air");
}
