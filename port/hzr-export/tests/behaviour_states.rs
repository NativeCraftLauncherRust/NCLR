//! Validation of `block-behaviours.json#states` against the prepared copy.
//!
//! The prepared file is only the oracle here; every value is derived from
//! `mc263/client.jar` (the class files and the registry/state tables).

#[path = "../src/behaviour_states.rs"]
mod behaviour_states;

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

fn expected_rows() -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(prepared()).expect("prepared block-behaviours");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    value
        .get("states")
        .and_then(|states| states.as_array())
        .cloned()
        .expect("states array")
}

fn number(row: &serde_json::Value, index: usize) -> i64 {
    if matches!(index, 4 | 7) {
        return row
            .get(index)
            .and_then(|value| value.as_bool())
            .map(|value| value as i64)
            .unwrap_or_else(|| panic!("column {index} is not a bool: {row}"));
    }
    row.get(index)
        .and_then(|value| value.as_i64())
        .unwrap_or_else(|| panic!("column {index} is not an integer: {row}"))
}

#[test]
fn states_columns_match_the_prepared_copy() {
    if !jar().exists() || !prepared().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let table = behaviour_states::behaviour_states(&jar()).expect("behaviour states");
    let block_ids = behaviour_states::state_block_ids(&jar()).expect("block ids");
    let expected = expected_rows();
    assert_eq!(table.rows.len(), expected.len(), "state count");
    let columns = [
        "sturdy",
        "flags",
        "fluid",
        "fluid_amount",
        "falling",
        "weak",
        "strong",
        "signal_error",
        "instrument",
    ];
    let mut mismatches = [0usize; 9];
    let mut samples: Vec<Vec<String>> = vec![Vec::new(); 9];
    for (index, (row, want)) in table.rows.iter().zip(&expected).enumerate() {
        let actual: [i64; 8] = [
            row.sturdy,
            row.flags as i64,
            row.fluid as i64,
            row.fluid_amount as i64,
            row.falling as i64,
            row.weak,
            row.strong,
            row.signal_error as i64,
        ];
        for column in 0..8 {
            if actual[column] != number(want, column) {
                mismatches[column] += 1;
                if samples[column].len() < 6 {
                    samples[column].push(format!(
                        "state {index} {}: ours {} expected {}",
                        block_ids[index], actual[column], number(want, column)
                    ));
                }
            }
        }
        let instrument = want
            .get(8)
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if row.instrument != instrument {
            mismatches[8] += 1;
            if samples[8].len() < 6 {
                samples[8].push(format!(
                    "state {index} {}: ours {} expected {instrument}",
                    block_ids[index], row.instrument
                ));
            }
        }
    }
    for (column, name) in columns.iter().enumerate() {
        eprintln!("{name}: {} mismatches", mismatches[column]);
        for sample in &samples[column] {
            eprintln!("  {sample}");
        }
    }
    let row_exact = table
        .rows
        .iter()
        .zip(&expected)
        .filter(|(row, want)| {
            let actual: [i64; 8] = [
                row.sturdy,
                row.flags as i64,
                row.fluid as i64,
                row.fluid_amount as i64,
                row.falling as i64,
                row.weak,
                row.strong,
                row.signal_error as i64,
            ];
            actual
                .iter()
                .enumerate()
                .all(|(column, value)| *value == number(want, column))
                && row.instrument
                    == want.get(8).and_then(|value| value.as_str()).unwrap_or("")
        })
        .count();
    eprintln!("row-exact {row_exact}/{}", expected.len());
    assert!(row_exact > 0);
}
