//! Does `gameplay_zip` close `_shared/gameplay.compact.json`?
//!
//! Both documents are compact (dictionaries + `per_block`/`per_state` indices), so the test
//! decodes them the same way: a field's value for a state is
//! `dictionaries[field][per_state[state][slot]]` for a varying field, and
//! `dictionaries[field][per_block[block][slot]]` for a constant one.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay_zip.rs"]
mod gameplay_zip;

use std::path::{Path, PathBuf};

use serde_json::Value;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/_shared/gameplay.compact.json")
}

fn reference() -> Option<Value> {
    if !jar().is_file() || !prepared().is_file() {
        eprintln!("skipping: the jar or the prepared table is not there");
        return None;
    }
    let text = std::fs::read_to_string(prepared()).expect("the prepared table");
    Some(serde_json::from_str(&text).expect("the prepared table is JSON"))
}

fn strings(value: &Value, what: &str) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("{what} is not an array"))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("{what} holds a non-string"))
                .to_string()
        })
        .collect()
}

/// The decoded value of `field` for global state `state` (whose block is `block`).
fn value_at(document: &Value, field: &str, block: &str, state: usize) -> Option<Value> {
    if strings(&document["fields"], "fields")
        .iter()
        .all(|candidate| candidate != field)
    {
        return None;
    }
    let constant = strings(&document["constant_fields"], "constant_fields");
    let index = if let Some(slot) = constant.iter().position(|candidate| candidate == field) {
        let row = document["per_block"].get(block)?.as_array()?;
        row.get(slot)?.as_u64()? as usize
    } else {
        let varying = strings(&document["varying_fields"], "varying_fields");
        let slot = varying.iter().position(|candidate| candidate == field)?;
        let row = document["per_state"].as_array()?.get(state)?.as_array()?;
        row.get(slot)?.as_u64()? as usize
    };
    document["dictionaries"][field].as_array()?.get(index).cloned()
}

#[test]
fn the_scan_finds_the_bases() {
    if !jar().is_file() {
        return;
    }
    let report = gameplay_zip::scan_report(&jar()).expect("the scan runs");
    eprintln!("scan: {report}");
    assert!(report.contains("zip_families="));
}

#[test]
fn every_field_matches_the_prepared_copy() {
    let Some(document) = reference() else { return };
    let ours = gameplay_zip::document(&jar()).expect("the document builds from the jar");
    let expected_blocks = strings(&document["blocks"], "blocks");
    assert_eq!(strings(&ours["blocks"], "blocks"), expected_blocks, "blocks");
    // The state -> block mapping must be the same in both.
    let expected_states: Vec<u64> = document["blocks_per_state"]
        .as_array()
        .expect("blocks_per_state")
        .iter()
        .map(|value| value.as_u64().expect("an index"))
        .collect();
    let our_states: Vec<u64> = ours["blocks_per_state"]
        .as_array()
        .expect("blocks_per_state")
        .iter()
        .map(|value| value.as_u64().expect("an index"))
        .collect();
    assert_eq!(our_states, expected_states, "state -> block");
    let mut failures = Vec::new();
    let mut report = Vec::new();
    for field in strings(&ours["fields"], "fields") {
        let mut wrong = 0usize;
        let mut first = Vec::new();
        for (state, block_index) in expected_states.iter().enumerate() {
            let block = &expected_blocks[*block_index as usize];
            let (Some(mine), Some(theirs)) = (
                value_at(&ours, &field, block, state),
                value_at(&document, &field, block, state),
            ) else {
                continue;
            };
            if mine != theirs {
                wrong += 1;
                if first.len() < 5 {
                    first.push(format!("{block} state {state}: {mine} != {theirs}"));
                }
            }
        }
        report.push(format!("{field}: {wrong} wrong"));
        if wrong > 0 {
            failures.push(format!("{field}: {wrong} wrong ({})", first.join("; ")));
        }
    }
    eprintln!(
        "coverage versus the prepared copy ({} blocks):\n  {}",
        expected_blocks.len(),
        report.join("\n  ")
    );
    assert!(
        failures.is_empty(),
        "fields differ from the prepared copy:\n{}",
        failures.join("\n")
    );
}

#[test]
fn write_produces_the_document() {
    if reference().is_none() {
        return;
    }
    let out = std::env::temp_dir().join(format!("hzr-export-gameplay-zip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = gameplay_zip::write(&jar(), &out).expect("the document is written");
    assert!(coverage
        .produced
        .contains(&"_shared/gameplay.compact.json".to_string()));
    assert!(
        coverage.unresolved.is_empty(),
        "{} blocks have no row: {:?}",
        coverage.unresolved.len(),
        &coverage.unresolved[..coverage.unresolved.len().min(8)]
    );
    let written =
        std::fs::read_to_string(out.join(gameplay_zip::TABLE)).expect("the written table");
    let value: Value = serde_json::from_str(&written).expect("JSON");
    assert_eq!(
        value["blocks"].as_array().map(Vec::len),
        Some(1286),
        "every block"
    );
    let _ = std::fs::remove_dir_all(&out);
}
