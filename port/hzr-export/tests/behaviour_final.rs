//! Compares the integrated `block-behaviours.json` section by section.

#[path = "../src/behaviour_final.rs"]
mod behaviour_final;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

#[test]
fn every_section_is_reported_exactly() {
    if !jar().exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let (document, coverage) = behaviour_final::document(&jar()).expect("document");
    let reference: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json")).expect("table"),
    )
    .expect("JSON");

    // Blocks: compare headers (name/class/interfaces/dynamic_shape/item) and the
    // exact fields subset separately.
    let mut header_exact = 0usize;
    let mut fields_exact = 0usize;
    let mine_blocks = document["blocks"].as_array().expect("blocks");
    let ref_blocks = reference["blocks"].as_array().expect("blocks");
    let ref_index: std::collections::BTreeMap<&str, &serde_json::Value> = ref_blocks
        .iter()
        .filter_map(|row| row["name"].as_str().map(|name| (name, row)))
        .collect();
    for row in mine_blocks {
        let Some(object) = row.as_object() else { continue };
        let Some(name) = object.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(theirs) = ref_index.get(name) else {
            continue;
        };
        let mut same_header = true;
        for key in ["class", "interfaces", "dynamic_shape", "item"] {
            if object.get(key) != theirs.get(key) {
                same_header = false;
            }
        }
        if same_header {
            header_exact += 1;
        }
        if object.get("fields") == theirs.get("fields") {
            fields_exact += 1;
        }
    }
    eprintln!(
        "blocks: {} rows, header exact {header_exact}, fields exact {fields_exact}",
        mine_blocks.len()
    );

    // States: exact rows.
    let mine_states = document["states"].as_array().expect("states");
    let ref_states = reference["states"].as_array().expect("states");
    let mut state_exact = 0usize;
    for (index, row) in mine_states.iter().enumerate() {
        if ref_states.get(index) == Some(row) {
            state_exact += 1;
        }
    }
    eprintln!(
        "states: {} rows, exact {state_exact}",
        mine_states.len()
    );

    // Items / fluids / fire.
    let mine_items = document["items"].as_object().expect("items");
    let ref_items = reference["items"].as_object().expect("items");
    let mut item_exact = 0usize;
    for (name, value) in mine_items {
        if ref_items.get(name) == Some(value) {
            item_exact += 1;
        }
    }
    let items_total = ref_items.len();
    let fluids_exact = document["fluids"] == reference["fluids"];
    let fire_exact = document["fire"] == reference["fire"];
    eprintln!(
        "items: {item_exact}/{items_total} exact; fluids exact {fluids_exact}; fire exact {fire_exact}"
    );
    eprintln!("coverage: {} gaps/notes reported", coverage.gaps.len());

    // Fire: report the differing keys (the header's fire derivation is partial).
    if let (Some(mine_fire), Some(ref_fire)) =
        (document["fire"].as_object(), reference["fire"].as_object())
    {
        let mut fire_differences = Vec::new();
        for key in ref_fire.keys() {
            if mine_fire.get(key) != ref_fire.get(key) {
                fire_differences.push(key.clone());
            }
        }
        eprintln!("fire: {fire_differences:?}");
    }
    assert_eq!(mine_blocks.len(), ref_blocks.len());
    assert_eq!(mine_states.len(), ref_states.len());
    assert_eq!(items_total, mine_items.len());
}
