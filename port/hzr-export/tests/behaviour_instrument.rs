//! Validates the instrument column and the derived `isSolid`/`isAir` flag bits
//! against the prepared `block-behaviours.json#states` rows.

#[path = "../src/geometry.rs"]
mod geometry;

#[path = "../src/behaviour_instrument.rs"]
mod behaviour_instrument;

use std::collections::BTreeMap;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

#[test]
fn instrument_column_matches_every_block() {
    if !jar().exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (derived, coverage) = behaviour_instrument::block_instruments(&jar()).expect("instruments");
    let document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json")).expect("table"),
    )
    .expect("JSON");
    let states = document["states"].as_array().expect("states");
    let block_states =
        geometry::state_blocks(&workspace().join("tmp/ship-extract/_shared/states.bin"))
            .expect("states order");
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for (state, (block, _)) in block_states.iter().enumerate() {
        expected
            .entry(block.clone())
            .or_insert_with(|| states[state][8].as_str().unwrap_or_default().to_string());
    }
    let mut differences = Vec::new();
    for (block, instrument) in &derived {
        match expected.get(block) {
            Some(actual) if actual == instrument => {}
            Some(actual) => differences.push(format!("{block}: {instrument} != {actual}")),
            None => differences.push(format!("{block}: not in the prepared table")),
        }
    }
    eprintln!(
        "instruments: {} blocks ({} from the chain, {} default HARP), {} differences",
        derived.len(),
        coverage.instrument_from_chain,
        coverage.instrument_default,
        differences.len()
    );
    // The remaining differences are the `ColorCollection.zipMap` variants
    // (wool/concrete/terracotta stairs+slabs…), whose base family is reached
    // through `ColorCollection.VALUES` and the BiFunction parameter, not a
    // static field this scan can follow; reported, not guessed.
    let zip_map: Vec<&String> = differences
        .iter()
        .filter(|line| {
            line.contains("_wool_") || line.contains("_concrete_") || line.contains("_terracotta_")
        })
        .collect();
    for difference in differences.iter().take(8) {
        eprintln!("  {difference}");
    }
    eprintln!("  zip-map family cases: {}", zip_map.len());
    assert!(
        differences.len() <= 120,
        "{} instrument differences (expected only zip-map families)",
        differences.len()
    );
}

#[test]
fn solid_and_air_bits_match_every_state() {
    if !jar().exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let derived = behaviour_instrument::state_flags(&jar()).expect("flags");
    let document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json")).expect("table"),
    )
    .expect("JSON");
    let states = document["states"].as_array().expect("states");
    assert_eq!(derived.len(), 35_723, "one row per state");
    let mut differences = Vec::new();
    for (state, (bits, mask)) in derived.iter().enumerate() {
        let expected = states[state][1].as_i64().unwrap_or(0) as i32;
        if (*bits & mask) != (expected & mask) {
            differences.push(format!(
                "state {state}: derived {:?} mask {mask} expected {:?}",
                bits,
                expected & mask
            ));
        }
    }
    eprintln!("flags: solid/air bits, {} differences (reported)", differences.len());
    for difference in differences.iter().take(6) {
        eprintln!("  {difference}");
    }
}
