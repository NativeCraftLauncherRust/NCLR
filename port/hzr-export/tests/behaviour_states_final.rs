//! Validation of the final `block-behaviours.json#states` rows.
//!
//! The prepared copy is only the oracle here: every value in the rows is
//! derived from `mc263/client.jar` (block registry, state table, class files,
//! and the signal/fluid rules). The test compares every column of all 35,723
//! rows and reports the first mismatches, so no column can hide behind another.

#[path = "../src/behaviour_states_final.rs"]
mod behaviour_states_final;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/block-behaviours.json")
}

#[test]
fn every_state_matches_the_prepared_block_behaviours() {
    if !jar().exists() || !prepared().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (rows, coverage) = behaviour_states_final::rows(&jar()).expect("state rows");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared()).expect("block-behaviours.json"))
            .expect("JSON");
    let reference = document["states"].as_array().expect("states");
    assert_eq!(rows.len(), reference.len(), "state row count");

    let mut columns = vec![0usize; 9];
    let mut exact = 0usize;
    let mut samples: Vec<String> = Vec::new();
    for (index, (mine, theirs)) in rows.iter().zip(reference).enumerate() {
        let (Some(mine), Some(theirs)) = (mine.as_array(), theirs.as_array()) else {
            samples.push(format!("state {index}: not an array"));
            continue;
        };
        let mut same = mine.len() == theirs.len();
        for column in 0..9 {
            if mine.get(column) == theirs.get(column) {
                columns[column] += 1;
            } else {
                same = false;
                if samples.len() < 12 {
                    samples.push(format!(
                        "state {index} column {column}: ours {:?} expected {:?}",
                        mine.get(column),
                        theirs.get(column)
                    ));
                }
            }
        }
        if same {
            exact += 1;
        }
    }
    eprintln!(
        "block behaviours: rows={} exact={exact} signals_patched={} fluid_fixed={} at {:?} signal_error_kept={} columns={columns:?} notes={:?}",
        rows.len(),
        coverage.signals_patched,
        coverage.fluid_fixed,
        coverage.fluid_fixed_states,
        coverage.signal_error_kept,
        coverage.notes,
    );
    for (column, count) in columns.iter().enumerate() {
        eprintln!("  column {column}: {count}");
    }
    for sample in &samples {
        eprintln!("  {sample}");
    }

    assert_eq!(coverage.states, rows.len(), "coverage state count");
    // The signal replacement must actually have been applied, and the base
    // module's signalError must already agree everywhere.
    assert!(
        coverage.signals_patched > 0,
        "no signal column was patched — behaviour_signal was not applied"
    );
    assert_eq!(
        coverage.signal_error_kept,
        rows.len(),
        "signalError changed somewhere"
    );
    // The one fluid rule: exactly one state (water level 0) is fixed.
    assert_eq!(coverage.fluid_fixed, 1, "fluid fix count");
    assert_eq!(
        coverage.fluid_fixed_states.len(),
        1,
        "fluid fix state indices"
    );
    assert_eq!(exact, rows.len(), "row-exact");
    assert!(
        columns.iter().all(|count| *count == rows.len()),
        "column agreement: {columns:?}"
    );
}
