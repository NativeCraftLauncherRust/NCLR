//! Does `gameplay_final` reproduce `_shared/gameplay.compact.json`'s values for every block?
//!
//! The reference is the prepared copy `tmp/ship-extract/_shared/gameplay.compact.json`; a
//! constant field's value for a block is `dictionaries[field][per_block[block][slot]]`, a
//! varying field's value for a state is `dictionaries[field][per_state[state][slot]]`.
//!
//! The per-block test compares only the fields that do not need the collision shapes, so it
//! runs without the geometry evaluation; the full test then compares every state.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay_final.rs"]
mod gameplay_final;

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
    let value = row.get(slot)?.as_u64()?;
    document["dictionaries"][field]
        .as_array()?
        .get(value as usize)
        .cloned()
}

fn reference_per_state(document: &Value, field: &str, state: usize) -> Option<Value> {
    let varying = strings(&document["varying_fields"], "varying_fields");
    let slot = varying.iter().position(|candidate| candidate == field)?;
    let row = document["per_state"].as_array()?.get(state)?.as_array()?;
    let value = row.get(slot)?.as_u64()?;
    document["dictionaries"][field]
        .as_array()?
        .get(value as usize)
        .cloned()
}

#[test]
fn the_per_block_fields_match_the_prepared_copy() {
    let Some(document) = reference() else { return };
    let table = gameplay_final::rows(&jar()).expect("the rows build from the jar");
    let expected_blocks = strings(&document["blocks"], "blocks");
    assert_eq!(table.blocks, expected_blocks, "the block list and order");
    let constant = strings(&document["constant_fields"], "constant_fields");
    let mut failures = Vec::new();
    let mut report = Vec::new();
    for field in constant {
        // `can_occlude` needs the shapes; `full_collision` is per state.
        if field == "can_occlude" || field == "full_collision" {
            continue;
        }
        let mut wrong = Vec::new();
        let mut nulls = 0usize;
        let mut covered_wrong = Vec::new();
        let mut count = 0usize;
        for (index, block) in table.blocks.iter().enumerate() {
            let actual = match table.rows[index].get(field.as_str()) {
                Some(value) => value,
                None => continue,
            };
            if actual.is_null() {
                count += 1;
                nulls += 1;
                if wrong.len() < 4 {
                    wrong.push(format!("{block}: null"));
                }
                continue;
            }
            let Some(expected) = reference_per_block(&document, &field, block) else {
                count += 1;
                if covered_wrong.len() < 6 {
                    covered_wrong.push(format!("{block}: the reference has no value"));
                }
                continue;
            };
            if actual != &expected {
                count += 1;
                if covered_wrong.len() < 6 {
                    covered_wrong.push(format!("{block}: {actual} != {expected}"));
                }
            }
        }
        report.push(format!(
            "{field}: {count} wrong ({nulls} null, {} covered wrong)",
            count - nulls
        ));
        if count > 0 {
            failures.push(format!(
                "{field}: {count} wrong ({nulls} null; covered: {})",
                covered_wrong.join("; ")
            ));
        }
    }
    eprintln!(
        "per-block comparison ({} blocks, {} uncovered):\n  {}",
        table.blocks.len(),
        table.uncovered.len(),
        report.join("\n  ")
    );
    assert!(
        failures.is_empty(),
        "per-block fields differ from the prepared copy:\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_whole_document_matches_the_prepared_copy() {
    let Some(document) = reference() else { return };
    let facts = gameplay_final::facts(&jar()).expect("the facts build from the jar");
    let rendered = gameplay_final::render(&facts);
    let ours: Value = serde_json::from_str(&rendered).expect("our table is JSON");
    // Constant fields per block, varying fields per state.
    let mut failures = Vec::new();
    for field in strings(&document["constant_fields"], "constant_fields") {
        let mut count = 0usize;
        let mut wrong = Vec::new();
        for block in &facts.blocks {
            let Some(expected) = reference_per_block(&document, &field, block) else {
                continue;
            };
            let slot = facts
                .fields
                .iter()
                .position(|candidate| candidate == &field)
                .expect("the field is ours");
            let state = facts
                .blocks_per_state
                .iter()
                .position(|candidate| facts.blocks[*candidate] == *block)
                .expect("the block has a state");
            if facts.values[state][slot] != expected {
                count += 1;
                if wrong.len() < 4 {
                    wrong.push(format!(
                        "{block}: {} != {expected}",
                        facts.values[state][slot]
                    ));
                }
            }
        }
        if count > 0 {
            failures.push(format!("{field}: {count} wrong (first: {})", wrong.join("; ")));
        }
    }
    for field in strings(&document["varying_fields"], "varying_fields") {
        let mut count = 0usize;
        let mut wrong = Vec::new();
        let slot = facts
            .fields
            .iter()
            .position(|candidate| candidate == &field)
            .expect("the field is ours");
        for state in 0..facts.values.len() {
            let Some(expected) = reference_per_state(&document, &field, state) else {
                continue;
            };
            if facts.values[state][slot] != expected {
                count += 1;
                if wrong.len() < 4 {
                    wrong.push(format!(
                        "state {state}: {} != {expected}",
                        facts.values[state][slot]
                    ));
                }
            }
        }
        if count > 0 {
            failures.push(format!("{field}: {count} wrong (first: {})", wrong.join("; ")));
        }
    }
    eprintln!(
        "the whole document:\n  fields ours {:?}\n  reference {:?}",
        ours["fields"], document["fields"]
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
    let out = std::env::temp_dir().join(format!("hzr-export-gameplay-final-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = gameplay_final::write(&jar(), &out).expect("the document is written");
    assert!(coverage
        .produced
        .contains(&"_shared/gameplay.compact.json".to_string()));
    assert!(out.join(gameplay_final::TABLE).is_file(), "the file exists");
    let written = std::fs::read_to_string(out.join(gameplay_final::TABLE)).expect("the file");
    let value: Value = serde_json::from_str(&written).expect("JSON");
    assert_eq!(
        value["blocks"].as_array().map(Vec::len),
        Some(1286),
        "every block"
    );
    let _ = std::fs::remove_dir_all(&out);
}
