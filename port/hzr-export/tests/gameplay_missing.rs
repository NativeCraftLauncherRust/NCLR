//! Does `gameplay_missing` reproduce `_shared/gameplay.compact.json`'s 11 remaining fields?
//!
//! The reference is the prepared copy `tmp/ship-extract/_shared/gameplay.compact.json`.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay_missing.rs"]
mod gameplay_missing;

use std::collections::BTreeMap;
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

fn reference_per_state(document: &Value, field: &str, state: usize) -> Option<Value> {
    if let Some(slot) = strings(&document["varying_fields"], "varying_fields")
        .iter()
        .position(|candidate| candidate == field)
    {
        let row = document["per_state"].as_array()?.get(state)?.as_array()?;
        let index = row.get(slot)?.as_u64()? as usize;
        return document["dictionaries"][field]
            .as_array()?
            .get(index)
            .cloned();
    }
    let slot = strings(&document["constant_fields"], "constant_fields")
        .iter()
        .position(|candidate| candidate == field)?;
    let blocks = strings(&document["blocks"], "blocks");
    let block_index = document["blocks_per_state"]
        .as_array()?
        .get(state)?
        .as_u64()? as usize;
    let row = document["per_block"].get(&blocks[block_index])?.as_array()?;
    let index = row.get(slot)?.as_u64()? as usize;
    document["dictionaries"][field]
        .as_array()?
        .get(index)
        .cloned()
}

#[test]
fn report_gaps() {
    let Some(reference) = reference() else {
        return;
    };
    let document = gameplay_missing::document(&jar()).expect("the document builds from the jar");
    let blocks = strings(&document["blocks"], "blocks");
    assert_eq!(
        blocks,
        strings(&reference["blocks"], "blocks"),
        "the block list and order"
    );
    let fields = gameplay_missing::FIELDS;
    let mut report = String::new();
    for field in fields {
        let mut wrong = 0usize;
        let mut sample = Vec::new();
        let per_state = reference["blocks_per_state"]
            .as_array()
            .expect("blocks_per_state");
        for state in 0..per_state.len() {
            let block_index = per_state[state].as_u64().expect("index") as usize;
            let Some(actual) = reference_per_state(&document, field, state) else {
                continue;
            };
            let Some(expected) = reference_per_state(&reference, field, state) else {
                continue;
            };
            if actual != expected {
                wrong += 1;
                if sample.len() < 6 {
                    sample.push(format!(
                        "{} state {state}: {} != {}",
                        blocks[block_index],
                        short(&actual),
                        short(&expected)
                    ));
                }
            }
        }
        report.push_str(&format!("{field}: {wrong} wrong {:?}\n", sample));
    }
    let out = workspace().join("tmp/gp4/missing_report.txt");
    std::fs::write(&out, &report).expect("report written");
    eprintln!("report: {}", out.display());
}

fn short(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.len() > 90 {
        format!("{}...", &text[..90])
    } else {
        text
    }
}

#[test]
fn write_produces_the_document() {
    if reference().is_none() {
        return;
    }
    let out = std::env::temp_dir().join(format!(
        "hzr-export-gameplay-missing-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    gameplay_missing::write(&jar(), &out).expect("the document is written");
    let text = std::fs::read_to_string(out.join(gameplay_missing::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(
        document["blocks"].as_array().map(Vec::len),
        Some(1286),
        "every block"
    );
    let Some(reference) = reference() else { return };
    let mut diffs: BTreeMap<&str, usize> = BTreeMap::new();
    for field in gameplay_missing::FIELDS {
        diffs.insert(field, 0);
    }
    let per_state = reference["blocks_per_state"]
        .as_array()
        .expect("blocks_per_state")
        .len();
    for state in 0..per_state {
        for field in gameplay_missing::FIELDS.iter() {
            let Some(actual) = reference_per_state(&document, field, state) else {
                continue;
            };
            let Some(expected) = reference_per_state(&reference, field, state) else {
                continue;
            };
            if actual != expected {
                *diffs.get_mut(field).expect("field") += 1;
            }
        }
    }
    eprintln!("field differences from the prepared copy: {diffs:?}");
    assert_eq!(
        strings(&document["fields"], "fields").len(),
        gameplay_missing::FIELDS.len(),
        "the document carries every field"
    );
    let _ = std::fs::remove_dir_all(&out);
}

