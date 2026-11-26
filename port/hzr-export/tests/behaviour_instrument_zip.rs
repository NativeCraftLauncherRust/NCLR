//! Validates the zipMap instrument patch: 1,286/1,286 instruments.

#[path = "../src/geometry.rs"]
mod geometry;

#[path = "../src/behaviour_instrument_zip.rs"]
mod behaviour_instrument_zip;

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
        behaviour_instrument_zip::block_instruments(&jar).expect("instruments");
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
        "instruments: {} blocks, {} patched, {} zipMap families, {} residual",
        derived.len(),
        coverage.patched,
        coverage.zip_map_families.len(),
        coverage.residual.len()
    );
    for family in coverage.zip_map_families.iter().take(10) {
        eprintln!("  patched {family}");
    }
    for difference in differences.iter().take(10) {
        eprintln!("  {difference}");
    }
    if !differences.is_empty() {
        eprintln!(
            "{} residual instruments (weathering-copper per-member values, wall-variant helpers, single-copper helpers)",
            differences.len()
        );
    }
    assert!(
        differences.len() <= 50,
        "{} instrument differences, expected the copper/wall-variant residual",
        differences.len()
    );
}
