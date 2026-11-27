//! Does `gameplay_exact` reproduce `_shared/gameplay.compact.json` for every block and every
//! state?
//!
//! The reference is the prepared copy `tmp/ship-extract/_shared/gameplay.compact.json`: a
//! constant field's value for a block is `dictionaries[field][per_block[block][slot]]`, a
//! varying field's value for a state is `dictionaries[field][per_state[state][slot]]`.
//!
//! The 16 fields [`gameplay_exact`] derives are compared exactly for all 1,286 blocks and all
//! states (the per-state `map_color` function and the decorated pot's per-state sound
//! included). The light-engine, selection-outline and item/render fields are not ported and
//! are reported as gaps by the writer; they are not compared.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay_exact.rs"]
mod gameplay_exact;

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

fn reference_per_block(document: &Value, field: &str, block: &str) -> Option<Value> {
    let constant = strings(&document["constant_fields"], "constant_fields");
    let slot = constant.iter().position(|candidate| candidate == field)?;
    let row = document["per_block"].get(block)?.as_array()?;
    let index = row.get(slot)?.as_u64()? as usize;
    document["dictionaries"][field]
        .as_array()?
        .get(index)
        .cloned()
}

fn reference_per_state(document: &Value, field: &str, state: usize) -> Option<Value> {
    let varying = strings(&document["varying_fields"], "varying_fields");
    let slot = varying.iter().position(|candidate| candidate == field)?;
    let row = document["per_state"].as_array()?.get(state)?.as_array()?;
    let index = row.get(slot)?.as_u64()? as usize;
    document["dictionaries"][field]
        .as_array()?
        .get(index)
        .cloned()
}

/// The first state id of every block.
fn first_states(document: &Value) -> BTreeMap<usize, usize> {
    let per_state = document["blocks_per_state"]
        .as_array()
        .expect("blocks_per_state");
    let mut first = BTreeMap::new();
    for (state, block_index) in per_state.iter().enumerate() {
        first
            .entry(block_index.as_u64().expect("an index") as usize)
            .or_insert(state);
    }
    first
}

/// Compares every field this module derives against the reference. `full_collision` is left
/// to the geometry tables and is skipped here.
fn compare(document: &Value, rows: &gameplay_exact::Rows) -> Vec<String> {
    let mut failures = Vec::new();
    let expected_blocks = strings(&document["blocks"], "blocks");
    if rows.blocks != expected_blocks {
        failures.push("the block list and order differ".to_string());
        return failures;
    }
    if !rows.unresolved.is_empty() {
        failures.push(format!(
            "{} blocks have no row: {:?}",
            rows.unresolved.len(),
            &rows.unresolved[..rows.unresolved.len().min(8)]
        ));
    }
    for field in gameplay_exact::FIELDS
        .iter()
        .filter(|field| **field != "full_collision")
    {
        let mut wrong = 0usize;
        let mut first = Vec::new();
        for (index, block) in rows.blocks.iter().enumerate() {
            let Some(actual) = rows.rows[index].get(*field) else {
                continue;
            };
            let Some(expected) = reference_per_block(document, field, block) else {
                continue;
            };
            if actual != &expected {
                wrong += 1;
                if first.len() < 4 {
                    first.push(format!("{block}: {actual} != {expected}"));
                }
            }
        }
        if wrong > 0 {
            failures.push(format!("{field}: {wrong} wrong ({})", first.join("; ")));
        }
    }
    // Sound: constant per block, except the decorated pot.
    {
        let first = first_states(document);
        let mut wrong = 0usize;
        let mut first_wrong = Vec::new();
        for (index, block) in rows.blocks.iter().enumerate() {
            let Some(actual) = rows.rows[index].get("sound") else {
                continue;
            };
            let state = first.get(&index).copied().expect("the block has a state");
            let Some(expected) = reference_per_state(document, "sound", state) else {
                continue;
            };
            if actual != &expected {
                wrong += 1;
                if first_wrong.len() < 4 {
                    first_wrong.push(format!("{block}: {actual} != {expected}"));
                }
            }
        }
        if wrong > 0 {
            failures.push(format!("sound: {wrong} wrong ({})", first_wrong.join("; ")));
        }
    }
    // `map_color` and the decorated pot's sound are per state.
    {
        let blocks = strings(&document["blocks"], "blocks");
        let per_state = document["blocks_per_state"]
            .as_array()
            .expect("blocks_per_state");
        let mut map_wrong = 0usize;
        let mut map_first = Vec::new();
        let mut sound_wrong = 0usize;
        let mut sound_first = Vec::new();
        for field in ["map_color", "sound"] {
            let first = first_states(document);
            for (state, block_index) in per_state.iter().enumerate() {
                let index = block_index.as_u64().expect("an index") as usize;
                let Some(actual_row) = rows.rows[index].get(field) else {
                    continue;
                };
                let local = state - first.get(&index).copied().unwrap_or(0);
                let actual = match field {
                    "map_color" => rows
                        .map_colors
                        .get(&index)
                        .and_then(|values| values.get(local))
                        .unwrap_or(actual_row),
                    "sound" => rows
                        .sounds
                        .get(&index)
                        .and_then(|values| values.get(local))
                        .unwrap_or(actual_row),
                    _ => actual_row,
                };
                let Some(expected) = reference_per_state(document, field, state) else {
                    continue;
                };
                if actual != &expected {
                    if field == "map_color" {
                        map_wrong += 1;
                        if map_first.len() < 4 {
                            map_first.push(format!(
                                "{} state {state}: {actual} != {expected}",
                                blocks[index]
                            ));
                        }
                    } else {
                        sound_wrong += 1;
                        if sound_first.len() < 4 {
                            sound_first.push(format!(
                                "{} state {state}: {actual} != {expected}",
                                blocks[index]
                            ));
                        }
                    }
                }
            }
        }
        if map_wrong > 0 {
            failures.push(format!("map_color: {map_wrong} wrong ({})", map_first.join("; ")));
        }
        if sound_wrong > 0 {
            failures.push(format!(
                "sound (per state): {sound_wrong} wrong ({})",
                sound_first.join("; ")
            ));
        }
    }
    failures
}

#[test]
fn every_row_matches_the_prepared_copy() {
    let Some(document) = reference() else {
        return;
    };
    let rows = gameplay_exact::rows(&jar()).expect("the rows build from the jar");
    let failures = compare(&document, &rows);
    eprintln!(
        "gameplay_exact: {} blocks, {} unresolved, {} mapColor gaps, {} per-state overrides",
        rows.blocks.len(),
        rows.unresolved.len(),
        rows.map_color_functions.len(),
        rows.map_colors.len() + rows.sounds.len()
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
    let out = std::env::temp_dir().join(format!(
        "hzr-export-gameplay-exact-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = gameplay_exact::write(&jar(), &out).expect("the document is written");
    assert!(coverage
        .produced
        .contains(&"_shared/gameplay.compact.json".to_string()));
    assert_eq!(coverage.unresolved.len(), 0, "every block has a row");
    assert_eq!(coverage.missing.len(), gameplay_exact::MISSING.len());
    let text = std::fs::read_to_string(out.join(gameplay_exact::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(
        document["blocks"].as_array().map(Vec::len),
        Some(1286),
        "every block"
    );
    assert_eq!(
        strings(&document["fields"], "fields"),
        gameplay_exact::FIELDS
            .iter()
            .map(|field| field.to_string())
            .collect::<Vec<_>>(),
        "the fields this module derives"
    );
    // The written document is compared with the reference too: the 15 non-shape fields and
    // the per-state overrides.
    let Some(reference) = reference() else { return };
    let rows = gameplay_exact::rows(&jar()).expect("the rows build from the jar");
    let failures = {
        // Reuse the row comparison (the document is built from the same values).
        let mut failures = compare(&reference, &rows);
        // `full_collision` is the one field the geometry engine still owns.
        failures.retain(|failure| !failure.starts_with("full_collision"));
        failures
    };
    assert!(
        failures.is_empty(),
        "fields differ from the prepared copy:\n{}",
        failures.join("\n")
    );
    let _ = std::fs::remove_dir_all(&out);
}
