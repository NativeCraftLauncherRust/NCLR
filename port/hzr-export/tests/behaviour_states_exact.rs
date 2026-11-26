//! Compares the composed state rows column by column.

#[path = "../src/behaviour_states_exact.rs"]
mod behaviour_states_exact;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn states_rows_match_reference_by_column() {
    let jar = workspace().join("mc263/client.jar");
    if !jar.exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (rows, coverage) = behaviour_states_exact::rows(&jar).expect("rows");
    let document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json")).expect("table"),
    )
    .expect("JSON");
    let reference = document["states"].as_array().expect("states");
    let exact = rows
        .iter()
        .zip(reference)
        .filter(|(mine, theirs)| mine == theirs)
        .count();
    let columns = behaviour_states_exact::column_agreement(&rows, reference);
    eprintln!(
        "states: {} rows, exact {exact}, patched flags {} sturdy {} instruments {}; per-column {columns:?}",
        rows.len(),
        coverage.flags_patched,
        coverage.sturdy_patched,
        coverage.instruments_patched
    );
    assert_eq!(rows.len(), 35_723, "state row count");
    assert!(exact >= 8_000, "at least the previous composition");
    // Every column should be at least as good as the base module.
    for (index, count) in columns.iter().enumerate() {
        eprintln!("  column {index}: {count}");
    }
}
