//! Validate the occupancy-aware height port and the ZERO-level dynamic shapes.
//!
//! The reference heights are the third column of
//! `tmp/ship-extract/mob-data.json#states` (the Java exporter's
//! `shape.isEmpty() ? 0.0 : shape.max(Direction.Axis.Y)`).

#[path = "../src/mob_height.rs"]
mod mob_height;

use std::collections::BTreeMap;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn expected_heights() -> Vec<f64> {
    let path = workspace().join("tmp/ship-extract/mob-data.json");
    let data = std::fs::read_to_string(&path).expect("mob-data.json");
    let value: serde_json::Value = serde_json::from_str(&data).expect("json");
    value
        .get("states")
        .and_then(|value| value.as_array())
        .expect("states")
        .iter()
        .map(|row| row.as_array().expect("row")[2].as_f64().expect("height"))
        .collect()
}

#[test]
fn heights_match_java_export() {
    let expected = expected_heights();
    let (heights, gaps) = mob_height::heights(&jar()).expect("heights");
    assert_eq!(heights.len(), expected.len(), "row count");
    assert!(gaps.is_empty(), "unresolved dynamic blocks: {gaps:?}");
    let mut bad = Vec::new();
    for (index, (ours, java)) in heights.iter().zip(expected.iter()).enumerate() {
        if ours != java {
            if bad.len() < 20 {
                bad.push(format!("#{index}: ours={ours} java={java}"));
            }
        }
    }
    let mismatches = heights
        .iter()
        .zip(expected.iter())
        .filter(|(ours, java)| ours != java)
        .count();
    assert_eq!(mismatches, 0, "height mismatches: {bad:#?}");
}

#[test]
fn dynamic_shapes_at_zero() {
    // The four classes the prepared collision table leaves uncached.
    let cases: &[(&str, BTreeMap<&str, &str>, f64)] = &[
        ("minecraft:bamboo", BTreeMap::new(), 1.0),
        ("minecraft:scaffolding", BTreeMap::new(), 1.0),
        (
            "minecraft:pointed_dripstone",
            BTreeMap::from([("thickness", "tip_merge"), ("vertical_direction", "down")]),
            1.0,
        ),
        (
            "minecraft:pointed_dripstone",
            BTreeMap::from([("thickness", "tip"), ("vertical_direction", "up")]),
            0.6875,
        ),
        (
            "minecraft:pointed_dripstone",
            BTreeMap::from([("thickness", "tip"), ("vertical_direction", "down")]),
            1.0,
        ),
        (
            "minecraft:pointed_dripstone",
            BTreeMap::from([("thickness", "base")]),
            1.0,
        ),
        (
            "minecraft:sulfur_spike",
            BTreeMap::from([("thickness", "tip"), ("vertical_direction", "up")]),
            0.6875,
        ),
        (
            "minecraft:sulfur_spike",
            BTreeMap::from([("thickness", "frustum")]),
            1.0,
        ),
    ];
    for (block, values, expected) in cases {
        let values: BTreeMap<String, String> = values
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let shape = mob_height::dynamic_shape(block, &values)
            .unwrap_or_else(|| panic!("{block} has no dynamic shape"));
        assert_eq!(
            mob_height::shape_height(&shape),
            *expected,
            "{block} {values:?}"
        );
    }
    assert!(mob_height::is_dynamic_block("minecraft:bamboo"));
    assert!(mob_height::is_dynamic_block("minecraft:white_shulker_box"));
    assert!(!mob_height::is_dynamic_block("minecraft:stone"));
}

#[test]
fn written_heights_round_trip() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mob-height");
    let out = dir.join("heights.json");
    let heights = mob_height::write_heights(&jar(), &out).expect("write heights");
    let data = std::fs::read_to_string(&out).expect("heights file");
    let value: serde_json::Value = serde_json::from_str(&data).expect("json");
    let rows = value.as_array().expect("array");
    assert_eq!(rows.len(), heights.len());
    assert_eq!(rows[0].as_f64(), Some(heights[0]));
}
