//! Does the sticky-piston fix make all 35,723 `block-behaviours` flag rows exact?
//!
//! The prepared copy is only the oracle; every bit is derived from
//! `mc263/client.jar` (via `behaviour_flags` plus this module's sticky-piston
//! property evaluation).

#[path = "../src/behaviour_flags_fix.rs"]
mod behaviour_flags_fix;

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
fn all_flags_are_exact() {
    if !jar().exists() || !prepared().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (rows, coverage) = behaviour_flags_fix::flags(&jar()).expect("flags");
    let block_ids = behaviour_flags_fix::state_block_ids(&jar()).expect("block ids");
    let text = std::fs::read_to_string(prepared()).expect("prepared copy");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    let expected = value["states"].as_array().expect("states");
    assert_eq!(rows.len(), expected.len(), "state count");
    assert_eq!(block_ids.len(), rows.len());
    let mut bit_mismatches = [0usize; 7];
    let mut row_mismatches = 0usize;
    let mut samples = Vec::new();
    for (index, (ours, want)) in rows.iter().zip(expected).enumerate() {
        let want = want[1].as_i64().unwrap_or(0) as i32;
        if ours != &want {
            row_mismatches += 1;
            if samples.len() < 12 {
                samples.push(format!(
                    "state {index} {}: ours {ours} want {want}",
                    block_ids[index]
                ));
            }
        }
        for bit in 0..7 {
            if (ours >> bit) & 1 != (want >> bit) & 1 {
                bit_mismatches[bit] += 1;
            }
        }
    }
    for bit in 0..7 {
        eprintln!("bit {bit}: {} mismatches", bit_mismatches[bit]);
    }
    eprintln!("row mismatches: {row_mismatches}/{}", rows.len());
    for sample in &samples {
        eprintln!("  {sample}");
    }
    eprintln!("gaps: {:?}", coverage.gaps);
    assert_eq!(row_mismatches, 0, "flags differ (see samples)");
    assert!(coverage.gaps.is_empty());
    assert_eq!(coverage.states, 35_723);
}
