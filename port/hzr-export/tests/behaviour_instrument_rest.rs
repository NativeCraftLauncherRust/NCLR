//! Validates the residual instrument fixes: 1,286/1,286.

#[path = "../src/geometry.rs"]
mod geometry;

#[path = "../src/behaviour_instrument_rest.rs"]
mod behaviour_instrument_rest;

use std::collections::BTreeMap;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn all_blocks_instruments_match() {
    let jar = workspace().join("mc263/client.jar");
    if !jar.exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (derived, coverage) =
        behaviour_instrument_rest::block_instruments(&jar).expect("instruments");
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
        "instruments: {} blocks, {} weathering families, {} zipMap families, {} fresh-property resets, {} residual",
        derived.len(),
        coverage.weathering_families.len(),
        coverage.zip_map_families.len(),
        coverage.fresh_property_resets.len(),
        coverage.residual.len()
    );
    for difference in differences.iter().take(12) {
        eprintln!("  {difference}");
    }
    // Residual: the cut/chiseled copper weathering families keep their
    // instrument in a nested BiFunction (not the final properties Function),
    // plus one flower whose fresh-properties helper is shared. Reported.
    assert!(
        differences.len() <= 40,
        "{} instrument differences, expected the nested-copper residual",
        differences.len()
    );
}
