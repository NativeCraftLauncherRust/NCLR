//! Does the merged exporter reproduce `worldgen/blocks/structure-blocks.json` from the jar?
//!
//! The reference is the prepared copy `tmp/ship-extract/worldgen/blocks/structure-blocks.json`
//! — what `horizon-prepare` produced from the real 26.3 jars with
//! `VanillaStructureBlockExporter`.
//!
//! The test compares the rendered report byte for byte. The one entry the jar has and the
//! report does not — `minecraft:vault` — is checked to be exactly the vault block's states,
//! so the omission is a reproduced fact about the report rather than a lost row.

#[path = "../src/structure_final.rs"]
mod structure_final;

#[path = "../src/states.rs"]
mod states;

use std::path::{Path, PathBuf};

use serde_json::Value;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/worldgen/blocks/structure-blocks.json")
}

fn reference() -> Option<(String, Value)> {
    if !jar().is_file() || !prepared().is_file() {
        eprintln!(
            "skipping: {} or {} is not there",
            jar().display(),
            prepared().display()
        );
        return None;
    }
    let text = std::fs::read_to_string(prepared()).expect("the prepared table");
    let value = serde_json::from_str(&text).expect("the prepared table is JSON");
    Some((text, value))
}

fn ours() -> &'static structure_final::Merged {
    // The evaluation takes a minute; every test shares one.
    static MERGED: std::sync::OnceLock<structure_final::Merged> = std::sync::OnceLock::new();
    MERGED.get_or_init(|| structure_final::table(&jar()).expect("the table builds from the jar"))
}

#[test]
fn the_whole_report_matches_the_prepared_copy() {
    let Some((text, reference)) = reference() else {
        return;
    };
    let merged = ours();
    let rendered = structure_final::render(merged);
    if rendered != text {
        // Point at the section that differs.
        let ours: Value = serde_json::from_str(&rendered).expect("our table is JSON");
        let mut report = Vec::new();
        for (section, expected) in reference.as_object().expect("an object") {
            let actual = ours.get(section);
            if section == "full_block" {
                let expected_states = expected.as_array().expect("full block");
                let Some(actual_states) = actual.and_then(Value::as_array) else {
                    report.push("full_block: missing".to_string());
                    continue;
                };
                let mut wrong = 0usize;
                let mut first = Vec::new();
                for (state, flag) in actual_states.iter().enumerate() {
                    let theirs = expected_states
                        .get(state)
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let mine = flag.as_bool().unwrap_or(false);
                    if mine == theirs {
                        continue;
                    }
                    wrong += 1;
                    if first.len() < 20 {
                        first.push(format!("{state}: {mine} != {theirs}"));
                    }
                }
                report.push(format!(
                    "full_block: {wrong} wrong ({} states unknown); first: {}",
                    merged.unknown_full_block.len(),
                    first.join(", ")
                ));
            } else if actual != Some(expected) {
                report.push(format!("{section}: differs"));
            }
        }
        panic!("structure-blocks.json differs\n{}", report.join("\n"));
    }
    assert_eq!(rendered, text, "the bytes differ from the prepared copy");
    assert!(
        merged.unknown_full_block.is_empty(),
        "every state's full_block is derived"
    );
}

#[test]
fn the_block_entity_types_are_the_prepared_ones_plus_the_reported_vault() {
    let Some((_, reference)) = reference() else {
        return;
    };
    let merged = ours();
    let expected = reference["block_entities"]
        .as_object()
        .expect("block entities");
    let ours: std::collections::BTreeMap<&String, &Vec<u32>> = merged
        .table
        .block_entities
        .iter()
        .map(|(kind, states)| (kind, states))
        .collect();
    for (kind, states) in expected {
        let wanted: Vec<u64> = states
            .as_array()
            .expect("states")
            .iter()
            .map(|value| value.as_u64().expect("an id"))
            .collect();
        let actual: Vec<u64> = ours
            .get(kind)
            .unwrap_or_else(|| panic!("{kind}: missing"))
            .iter()
            .map(|state| *state as u64)
            .collect();
        assert_eq!(actual, wanted, "{kind}: the states that carry it");
    }
    assert_eq!(
        ours.len(),
        expected.len(),
        "no block entity type beyond the prepared ones"
    );
    // The one the jar adds is the vault block's own state range.
    let omitted = &merged.omitted_block_entities;
    assert_eq!(omitted.len(), 1, "only the vault type is left out");
    assert_eq!(omitted[0].0, "minecraft:vault");
    let declared = states::state_table(&jar()).expect("the state table");
    let mut start = 0u32;
    let mut vault = None;
    for block in &declared.blocks {
        if block.id == "minecraft:vault" {
            vault = Some((start, start + block.state_count as u32));
        }
        start += block.state_count as u32;
    }
    let (first, last) = vault.expect("the vault block");
    assert_eq!(
        omitted[0].1,
        (first..last).collect::<Vec<u32>>(),
        "the omitted states are exactly the vault block's states"
    );
}

#[test]
fn write_produces_the_exact_report() {
    let Some((text, _)) = reference() else { return };
    let out =
        std::env::temp_dir().join(format!("hzr-export-structure-final-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = structure_final::write_merged(ours(), &out).expect("the table is written");
    assert!(coverage
        .produced
        .contains(&"worldgen/blocks/structure-blocks.json".to_string()));
    assert!(coverage.missing.is_empty(), "no section is missing");
    let written =
        std::fs::read_to_string(out.join(structure_final::TABLE)).expect("the written table");
    assert_eq!(written, text, "the written bytes are the prepared report");
    let _ = std::fs::remove_dir_all(&out);
}
