//! Do the `mob-data.json#states` predicate flags come out of the jar exactly?
//!
//! The oracle is the prepared `tmp/ship-extract/mob-data.json`: one row per
//! registry state, column 1 holding the flag mask. This test compares the six
//! predicate bits (`8 zombie | 16 ocelot | 32 polar bear | 64 redstone |
//! 512 solid | 1024 solid render`) and reports `flags_bad`.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/mob_flags.rs"]
mod mob_flags;

use std::collections::BTreeMap;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/mob-data.json")
}

fn prepared_flags() -> Vec<i32> {
    let text = std::fs::read_to_string(prepared()).expect("mob-data.json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    value
        .get("states")
        .and_then(|value| value.as_array())
        .expect("states")
        .iter()
        .map(|row| row.as_array().expect("row")[1].as_i64().expect("flags") as i32)
        .collect()
}

#[test]
fn the_initialiser_scan_registers_every_block() {
    if !jar().exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let (registered, registry, gaps, notes) =
        mob_flags::scan_counts(&jar()).expect("the Blocks initialiser scan");
    for note in &notes {
        eprintln!("note: {note}");
    }
    for gap in &gaps {
        eprintln!("gap: {gap}");
    }
    assert_eq!(
        registered, registry,
        "Blocks.<clinit> registration count must match the registry"
    );
    assert!(gaps.is_empty(), "scan gaps: {gaps:?}");
}

#[test]
fn every_state_matches_the_prepared_predicate_flags() {
    if !jar().exists() || !prepared().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (rows, coverage) = mob_flags::flags(&jar()).expect("predicate flags");
    let expected = prepared_flags();
    assert_eq!(rows.len(), expected.len(), "state count");
    let block_ids = mob_flags::state_block_ids(&jar()).expect("state block ids");
    assert_eq!(block_ids.len(), rows.len());

    let mask = mob_flags::PREDICATE_FLAGS;
    let mut flags_bad = 0usize;
    let mut bit_bad: BTreeMap<usize, usize> = BTreeMap::new();
    let mut by_block: BTreeMap<String, (usize, i32)> = BTreeMap::new();
    let mut samples: Vec<String> = Vec::new();
    for (index, (actual, wanted)) in rows.iter().zip(expected.iter()).enumerate() {
        let diff = (actual ^ wanted) & mask;
        if diff == 0 {
            continue;
        }
        flags_bad += 1;
        for bit in 0..11 {
            if diff >> bit & 1 != 0 {
                *bit_bad.entry(bit).or_default() += 1;
            }
        }
        let entry = by_block.entry(block_ids[index].clone()).or_insert((0, 0));
        entry.0 += 1;
        entry.1 |= diff;
        if samples.len() < 12 {
            samples.push(format!(
                "state {index} {}: ours {:#x} wanted {:#x} diff {:#x}",
                block_ids[index],
                actual & mask,
                wanted & mask,
                diff
            ));
        }
    }
    let mut report = String::new();
    for (block, (bad, diff)) in &by_block {
        report.push_str(&format!(
            "  {block}: {bad} states, bits {:?}\n",
            (0..11)
                .filter(|bit| diff >> bit & 1 != 0)
                .collect::<Vec<_>>()
        ));
    }
    eprintln!(
        "mob flags: rows={} exact={} flags_bad={flags_bad} bit_bad={bit_bad:?} gaps={} notes={:?}",
        rows.len(),
        rows.len() - flags_bad,
        coverage.gaps.len(),
        coverage.notes
    );
    if !by_block.is_empty() {
        eprintln!(
            "blocks with predicate flag mismatches ({}):\n{report}",
            by_block.len()
        );
    }
    for sample in &samples {
        eprintln!("  {sample}");
    }
    assert!(coverage.gaps.is_empty(), "gaps: {:?}", coverage.gaps);
    assert!(
        flags_bad == 0,
        "{flags_bad} states differ; first samples:\n{}",
        samples.join("\n")
    );
}
