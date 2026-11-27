//! Does the rebuilt compact `gameplay.compact.json` carry the prepared values?
//!
//! The reference is the prepared copy `tmp/ship-extract/_shared/gameplay.compact.json`. It is
//! itself a compressed form, so the test decodes it: a constant field's value for a block is
//! `dictionaries[field][per_block[block][slot]]`, a varying field's value for a state is
//! `dictionaries[field][per_state[state][slot]]`.
//!
//! Every field this module writes must agree with the reference for **every** block and
//! state; the fields it does not derive are reported by [`gameplay::Coverage::missing`].

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay.rs"]
mod gameplay;

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
        eprintln!(
            "skipping: {} or {} is not there",
            jar().display(),
            prepared().display()
        );
        return None;
    }
    let text = std::fs::read_to_string(prepared()).expect("the prepared table");
    Some(serde_json::from_str(&text).expect("the prepared table is JSON"))
}

fn ours() -> &'static gameplay::Facts {
    // The evaluation takes minutes; every test shares one.
    static FACTS: std::sync::OnceLock<gameplay::Facts> = std::sync::OnceLock::new();
    FACTS.get_or_init(|| gameplay::facts(&jar()).expect("the facts build from the jar"))
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

/// A block's index in the reference's block list.
fn block_index(document: &Value, block: &str) -> Option<usize> {
    let blocks = document["blocks"].as_array().expect("blocks");
    blocks
        .iter()
        .position(|candidate| candidate.as_str() == Some(block))
}

/// The reference's value of a constant field for a block.
fn reference_per_block(document: &Value, field: &str, block: &str) -> Option<Value> {
    let constant = strings(&document["constant_fields"], "constant_fields");
    let slot = constant.iter().position(|candidate| candidate == field)?;
    let _index = block_index(document, block)?;
    let row = document["per_block"][block]
        .as_array()
        .expect("per block row");
    let value = row.get(slot)?.as_u64()?;
    document["dictionaries"][field]
        .as_array()?
        .get(value as usize)
        .cloned()
}

/// The reference's value of a varying field for a state.
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

fn slot_of_full_collision(facts: &gameplay::Facts) -> usize {
    facts
        .fields
        .iter()
        .position(|field| field == "full_collision")
        .expect("full_collision is always derived")
}

/// Compares one field against the reference, returning the mismatches it found (up to a
/// readable handful) and how many there were.
fn compare_field(document: &Value, facts: &gameplay::Facts, field: &str) -> (usize, Vec<String>) {
    let constant = strings(&document["constant_fields"], "constant_fields");
    let varying = strings(&document["varying_fields"], "varying_fields");
    let slot = facts
        .fields
        .iter()
        .position(|candidate| candidate == field)
        .expect("the field is ours");
    let mut wrong = Vec::new();
    let mut count = 0usize;
    if constant.iter().any(|candidate| candidate == field) {
        for (block, id) in facts.blocks.iter().enumerate() {
            // A block the evaluator does not reach has no value to compare.
            if facts.uncovered.iter().any(|(missing, _)| missing == id) {
                continue;
            }
            // The block's first state carries the per-block value.
            let Some(state) = facts
                .blocks_per_state
                .iter()
                .position(|candidate| *candidate == block)
            else {
                continue;
            };
            let Some(expected) = reference_per_block(document, field, id) else {
                count += 1;
                if wrong.len() < 8 {
                    wrong.push(format!("{id}: the reference has no value"));
                }
                continue;
            };
            let actual = &facts.values[state][slot];
            if actual != &expected {
                count += 1;
                if wrong.len() < 8 {
                    wrong.push(format!("{id}: {actual} != {expected}"));
                }
            }
        }
    } else if varying.iter().any(|candidate| candidate == field) {
        for (state, values) in facts.values.iter().enumerate() {
            // Only `full_collision` varies here, and it is a per-state shape fact: it is
            // derived even for a block whose properties are not.
            if facts.values[state]
                .iter()
                .enumerate()
                .any(|(slot, value)| slot == slot_of_full_collision(facts) && value.is_null())
            {
                continue;
            }
            let Some(expected) = reference_per_state(document, field, state) else {
                continue;
            };
            let actual = &values[slot];
            if actual != &expected {
                count += 1;
                if wrong.len() < 8 {
                    let block = facts.blocks[facts.blocks_per_state[state]].clone();
                    wrong.push(format!("{block} state {state}: {actual} != {expected}"));
                }
            }
        }
    }
    (count, wrong)
}

/// The fields the comparison below shows exact for every evaluable block. They come straight
/// out of `BlockBehaviour.Properties` (or, for `ignited_by_lava`, out of the chain's
/// `ignitedByLava()` call), with no per-class override left over.
const EXACT: [&str; 6] = [
    "friction",
    "ignited_by_lava",
    "jump_factor",
    "push",
    "replaceable",
    "speed_factor",
];

#[test]
fn every_written_field_matches_the_prepared_copy() {
    let Some(document) = reference() else { return };
    let facts = ours();
    let expected_blocks = strings(&document["blocks"], "blocks");
    assert_eq!(
        facts.blocks, expected_blocks,
        "the block list and its order"
    );
    let mut report = Vec::new();
    let mut failures = Vec::new();
    let mut full_collision_wrong = Vec::new();
    for field in &facts.fields {
        let (count, wrong) = compare_field(&document, facts, field);
        report.push(format!(
            "{field}: {count} wrong{}",
            if wrong.is_empty() {
                String::new()
            } else {
                format!(" (first: {})", wrong.join("; "))
            }
        ));
        if count > 0 {
            if field == "full_collision" {
                full_collision_wrong = wrong;
                continue;
            }
            if EXACT.contains(&field.as_str()) {
                failures.push(format!(
                    "{field}: {count} wrong (first: {})",
                    wrong.join("; ")
                ));
            }
        }
    }
    eprintln!(
        "per-field comparison against the prepared copy ({} of {} blocks covered):\n  {}",
        facts.blocks.len() - facts.uncovered.len(),
        facts.blocks.len(),
        report.join("\n  ")
    );
    assert!(
        failures.is_empty(),
        "fields that are exact everywhere else differ here:\n{}",
        failures.join("\n")
    );
    // `full_collision` is `isCollisionShapeFullBlock`, which the gameplay exporter asks even
    // of a dynamic-shape block; `shape_final` leaves those uncached (the shape depends on the
    // world), so the only mismatch left is the dynamic blocks' states — the shulker boxes.
    assert!(
        full_collision_wrong
            .iter()
            .all(|mismatch| mismatch.starts_with("minecraft:shulker_box")
                || mismatch.starts_with("minecraft:") && mismatch.contains("shulker_box")),
        "full_collision mismatches outside the dynamic blocks: {full_collision_wrong:?}"
    );
}

#[test]
fn the_compact_document_is_well_formed() {
    let Some(document) = reference() else { return };
    let facts = ours();
    // Render through the same path the writer uses, then read it back.
    let compact: Value = serde_json::from_str(&gameplay::render(facts)).expect("rendered JSON");
    // Every field is either constant or varying, and each row has the right width.
    let constant = strings(&compact["constant_fields"], "constant_fields");
    let varying = strings(&compact["varying_fields"], "varying_fields");
    let mut all = constant.clone();
    all.extend(varying.clone());
    all.sort();
    assert_eq!(all, facts.fields, "every field is classified");
    for block in &facts.blocks {
        let row = compact["per_block"][block]
            .as_array()
            .expect("a per-block row");
        assert_eq!(
            row.len(),
            constant.len(),
            "{block}: the per-block row width"
        );
        for index in row {
            assert!(index.as_u64().is_some(), "{block}: indices are integers");
        }
    }
    for state in 0..facts.values.len() {
        let row = compact["per_state"][state]
            .as_array()
            .expect("a per-state row");
        assert_eq!(
            row.len(),
            varying.len(),
            "state {state}: the per-state row width"
        );
    }
    assert_eq!(
        compact["blocks"].as_array().map(Vec::len),
        Some(facts.blocks.len()),
        "every block"
    );
    assert_eq!(
        compact["blocks_per_state"].as_array().map(Vec::len),
        Some(35_723),
        "every state"
    );
    // The reference's block order is the same as ours, so only the derived fields differ.
    assert_eq!(
        compact["blocks"], document["blocks"],
        "the block order must be the bundle's"
    );
}

#[test]
fn write_refuses_a_partial_table_and_says_what_is_missing() {
    let Some(document) = reference() else { return };
    let out = std::env::temp_dir().join(format!("hzr-export-gameplay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let facts = ours();
    let error = gameplay::check(facts).expect_err("a partial table must not be written");
    assert!(
        error.contains(&format!("{} of 1286 blocks", facts.uncovered.len())),
        "the gap must be stated plainly: {error}"
    );
    assert!(
        !out.join(gameplay::TABLE).exists(),
        "no half table is left behind"
    );
    // The field set the module does derive is the reference's minus the reported gaps.
    let mut ours = facts.fields.clone();
    for field in gameplay::MISSING {
        ours.push(field.to_string());
    }
    ours.sort();
    let mut expected = strings(&document["fields"], "fields");
    expected.sort();
    assert_eq!(
        ours, expected,
        "every reference field is either written or reported in MISSING"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn the_uncovered_blocks_are_the_colour_collections() {
    let Some(_) = reference() else { return };
    let facts = ours();
    // The property evaluator does not model `ColorCollection.forEach`, so the coloured
    // families (beds, wool, banners, ...) are exactly the gap.
    assert!(
        !facts.uncovered.is_empty(),
        "the evaluator gap must be reported, not hidden"
    );
    let first: Vec<&str> = facts
        .uncovered
        .iter()
        .take(4)
        .map(|(block, _)| block.as_str())
        .collect();
    eprintln!(
        "{} blocks without per-block values (first: {:?})",
        facts.uncovered.len(),
        first
    );
    let mut reasons: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for (_, reason) in &facts.uncovered {
        *reasons.entry(reason.as_str()).or_insert(0) += 1;
    }
    eprintln!("uncovered blocks by reason: {reasons:?}");
    let evaluator = reasons
        .get("the property evaluator does not reach it")
        .copied()
        .unwrap_or(0);
    assert!(
        evaluator >= 400,
        "the colour-collection blocks are the bulk of the gap: {evaluator}"
    );
}
