//! Does `gameplay_base`'s fixed base layer match `_shared/gameplay.compact.json`?
//!
//! Constant fields are compared per block (`dictionaries[field][per_block[block][slot]]`),
//! varying fields per state (`dictionaries[field][per_state[state][slot]]`).

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay_base.rs"]
mod gameplay_base;

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

fn reference_per_block(document: &Value, field: &str, block: &str) -> Option<Value> {
    let constant = strings(&document["constant_fields"], "constant_fields");
    let slot = constant.iter().position(|candidate| candidate == field)?;
    let row = document["per_block"].get(block)?.as_array()?;
    let index = row.get(slot)?.as_u64()? as usize;
    document["dictionaries"][field].as_array()?.get(index).cloned()
}

fn value_at(document: &Value, field: &str, block: &str, state: usize) -> Option<Value> {
    if strings(&document["constant_fields"], "constant_fields")
        .iter()
        .any(|candidate| candidate == field)
    {
        return reference_per_block(document, field, block);
    }
    let varying = strings(&document["varying_fields"], "varying_fields");
    let slot = varying.iter().position(|candidate| candidate == field)?;
    let row = document["per_state"].as_array()?.get(state)?.as_array()?;
    let index = row.get(slot)?.as_u64()? as usize;
    document["dictionaries"][field].as_array()?.get(index).cloned()
}

#[test]
fn the_per_block_fields_match_the_prepared_copy() {
    let Some(document) = reference() else { return };
    let rows = gameplay_base::rows(&jar()).expect("the rows build from the jar");
    assert_eq!(
        rows.blocks,
        strings(&document["blocks"], "blocks"),
        "the block list and order"
    );
    let mut failures = Vec::new();
    let mut report = Vec::new();
    for field in strings(&document["constant_fields"], "constant_fields") {
        let mut wrong = 0usize;
        let mut nulls = 0usize;
        let mut first = Vec::new();
        for (index, block) in rows.blocks.iter().enumerate() {
            let Some(actual) = rows.rows[index].get(field.as_str()) else {
                continue;
            };
            if actual.is_null() {
                nulls += 1;
                continue;
            }
            let Some(expected) = reference_per_block(&document, &field, block) else {
                continue;
            };
            if actual != &expected {
                wrong += 1;
                if first.len() < 4 {
                    first.push(format!("{block}: {actual} != {expected}"));
                }
            }
        }
        report.push(format!("{field}: {wrong} wrong, {nulls} null"));
        if wrong > 0 {
            failures.push(format!("{field}: {wrong} wrong, {nulls} null ({})", first.join("; ")));
        }
    }
    // sound is a varying field in the reference (only the decorated pot varies per state),
    // so it is compared at each block's first state.
    {
        let first_states = {
            let mut first = std::collections::BTreeMap::new();
            for (state, block) in strings(&document["blocks"], "blocks").iter().enumerate() {
                let index = document["blocks_per_state"].as_array().expect("blocks_per_state")
                    [state].as_u64().expect("an index") as usize;
                let _ = index;
                first.entry(block.clone()).or_insert(state);
            }
            first
        };
        let mut wrong = 0usize;
        let mut nulls = 0usize;
        let mut first = Vec::new();
        for (index, block) in rows.blocks.iter().enumerate() {
            let Some(actual) = rows.rows[index].get("sound") else { continue };
            if actual.is_null() {
                nulls += 1;
                continue;
            }
            let state = *first_states.get(block).expect("the block has a state");
            if let Some(expected) = value_at(&document, "sound", block, state) {
                if actual != &expected {
                    wrong += 1;
                    if first.len() < 4 {
                        first.push(format!("{block}: {actual} != {expected}"));
                    }
                }
            }
        }
        report.push(format!("sound: {wrong} wrong, {nulls} null"));
        if wrong > 0 {
            failures.push(format!("sound: {wrong} wrong ({})", first.join("; ")));
        }
    }
    eprintln!(
        "per-block comparison ({} blocks, {} without a row):\n  {}\n  unresolved sample: {:?}",
        rows.blocks.len(),
        rows.unresolved.len(),
        report.join("\n  "),
        &rows.unresolved[..rows.unresolved.len().min(16)]
    );
    assert!(
        failures.is_empty(),
        "per-block fields differ:\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_whole_document_matches_the_prepared_copy() {
    let Some(document) = reference() else { return };
    let ours = gameplay_base::document(&jar()).expect("the document builds from the jar");
    let expected_blocks = strings(&document["blocks"], "blocks");
    assert_eq!(strings(&ours["blocks"], "blocks"), expected_blocks, "blocks");
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
            let constant = strings(&ours["constant_fields"], "constant_fields");
            let ours_value = if constant.iter().any(|candidate| candidate == &field) {
                reference_per_block(&ours, field.as_str(), block)
            } else {
                let varying = strings(&ours["varying_fields"], "varying_fields");
                varying
                    .iter()
                    .position(|candidate| candidate == &field)
                    .and_then(|slot| {
                        let row = ours["per_state"].as_array()?.get(state)?.as_array()?;
                        let index = row.get(slot)?.as_u64()? as usize;
                        ours["dictionaries"][field.as_str()].as_array()?.get(index).cloned()
                    })
            };
            let (Some(mine), Some(theirs)) = (ours_value, value_at(&document, &field, block, state))
            else {
                continue;
            };
            if mine != theirs {
                wrong += 1;
                if first.len() < 4 {
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
        "whole document ({} blocks):\n  {}",
        expected_blocks.len(),
        report.join("\n  ")
    );
    assert!(
        failures.is_empty(),
        "fields differ:\n{}",
        failures.join("\n")
    );
}

#[test]
fn write_reports_the_gaps() {
    if reference().is_none() {
        return;
    }
    let out = std::env::temp_dir().join(format!("hzr-export-gameplay-base-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = gameplay_base::write(&jar(), &out).expect("the document is written");
    assert!(coverage
        .produced
        .contains(&"_shared/gameplay.compact.json".to_string()));
    eprintln!(
        "write: {} unresolved, {} mapColor functions, {} missing fields",
        coverage.unresolved.len(),
        coverage.map_color_functions.len(),
        coverage.missing.len()
    );
    assert!(out.join(gameplay_base::TABLE).is_file());
    let _ = std::fs::remove_dir_all(&out);
}
