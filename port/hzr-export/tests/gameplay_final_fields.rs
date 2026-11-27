//! Does `gameplay_final_fields` reproduce every field of `_shared/gameplay.compact.json`?

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay_final_fields.rs"]
mod gameplay_final_fields;

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

fn value_at(document: &Value, field: &str, state: usize) -> Option<Value> {
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
fn every_field_matches_the_prepared_copy() {
    let Some(reference) = reference() else {
        return;
    };
    eprintln!("debug: {}", gameplay_final_fields::debug_info(&jar()));
    let document = gameplay_final_fields::document(&jar()).expect("the document builds");
    assert_eq!(
        strings(&document["blocks"], "blocks"),
        strings(&reference["blocks"], "blocks"),
        "the block list and order"
    );
    let states = reference["blocks_per_state"]
        .as_array()
        .expect("blocks_per_state")
        .len();
    let mut diffs = std::collections::BTreeMap::new();
    let mut samples: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut item_blocks: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for field in gameplay_final_fields::FIELDS {
        let mut wrong = 0usize;
        let mut sample = Vec::new();
        for state in 0..states {
            let actual = value_at(&document, field, state).unwrap_or(Value::Null);
            let expected = value_at(&reference, field, state).unwrap_or(Value::Null);
            if actual != expected {
                wrong += 1;
                if field == "item" {
                    if let Some(block) = strings(&document["blocks"], "blocks")
                        .get(
                            document["blocks_per_state"]
                                .as_array()
                                .and_then(|per_state| per_state.get(state))
                                .and_then(|value| value.as_u64())
                                .unwrap_or(0) as usize,
                        )
                    {
                        item_blocks.insert(block.clone());
                    }
                }
                if sample.len() < 6 {
                    let block = strings(&document["blocks"], "blocks")
                        .get(
                            document["blocks_per_state"]
                                .as_array()
                                .and_then(|per_state| per_state.get(state))
                                .and_then(|value| value.as_u64())
                                .unwrap_or(0) as usize,
                        )
                        .cloned()
                        .unwrap_or_default();
                    sample.push(format!(
                        "{block} state {state}: {} != {}",
                        short(&actual),
                        short(&expected)
                    ));
                }
            }
        }
        diffs.insert(field, wrong);
        if !sample.is_empty() {
            samples.insert(field.to_string(), sample);
        }
    }
    let report = format!("{diffs:?}\n{samples:?}\nitem blocks: {item_blocks:?}");
    let out = workspace().join("tmp/gp4/final_fields_report.txt");
    std::fs::write(&out, &report).expect("report written");
    assert!(
        diffs.values().all(|count| *count == 0),
        "fields differ from the prepared copy: {report}"
    );
}

#[test]
fn write_produces_the_document() {
    if reference().is_none() {
        return;
    }
    let out = std::env::temp_dir().join(format!(
        "hzr-export-gameplay-final-fields-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    gameplay_final_fields::write(&jar(), &out).expect("the document is written");
    let text = std::fs::read_to_string(out.join(gameplay_final_fields::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(
        document["blocks"].as_array().map(Vec::len),
        Some(1286),
        "every block"
    );
    assert_eq!(
        strings(&document["fields"], "fields"),
        gameplay_final_fields::FIELDS
            .iter()
            .map(|field| field.to_string())
            .collect::<Vec<_>>(),
        "every field"
    );
    let _ = std::fs::remove_dir_all(&out);
}

fn short(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    if text.len() > 100 {
        format!("{}...", &text[..100])
    } else {
        text
    }
}
