//! Validates `_shared/model-states.json` against the prepared copy.
//!
//! Everything the exporter uses comes from `client.jar` (state registry from
//! `states.rs` + the blockstate assets for the two client-only blocks); the
//! prepared file is only the oracle for the test.

#[path = "../src/model_states.rs"]
mod model_states;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference() -> PathBuf {
    workspace().join("tmp/ship-extract/_shared/model-states.json")
}

#[test]
fn the_document_is_byte_identical_to_the_prepared_copy() {
    if !jar().exists() || !reference().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let ours = model_states::model_states_json(&jar()).expect("the document");
    let theirs = std::fs::read(reference()).expect("the prepared copy");
    if ours != theirs {
        let at = ours
            .iter()
            .zip(theirs.iter())
            .position(|(left, right)| left != right)
            .unwrap_or_else(|| ours.len().min(theirs.len()));
        panic!(
            "differs at byte {at}: ours {} bytes, reference {} bytes; ours {:?} theirs {:?}",
            ours.len(),
            theirs.len(),
            &ours[at.saturating_sub(80)..(at + 200).min(ours.len())],
            &theirs[at.saturating_sub(80)..(at + 200).min(theirs.len())],
        );
    }
}

#[test]
fn every_block_has_its_schema_and_states() {
    if !jar().exists() || !reference().exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (ours, coverage) = model_states::model_states(&jar()).expect("the document");
    let theirs: serde_json::Value =
        serde_json::from_slice(&std::fs::read(reference()).expect("the prepared copy"))
            .expect("reference JSON");
    assert_eq!(ours, theirs, "the derived document differs semantically");
    assert_eq!(coverage.blocks, 1_288, "blockstate assets in the jar");
    assert_eq!(coverage.from_registry, 1_286, "blocks from the state registry");
    assert_eq!(
        coverage.synthesized,
        vec![
            "minecraft:glow_item_frame".to_string(),
            "minecraft:item_frame".to_string()
        ],
        "the two client-only blocks are rebuilt from their blockstate assets"
    );
    assert!(coverage.notes.is_empty(), "{:?}", coverage.notes);

    let entry = |id: &str| {
        ours.get(id)
            .unwrap_or_else(|| panic!("{id} is in the document"))
            .clone()
    };
    // The two client-only blocks: one boolean property, true first.
    assert_eq!(
        entry("minecraft:item_frame"),
        serde_json::json!({
            "schema": {"map": {"named": ["false", "true"]}},
            "states": [{"map": "true"}, {"map": "false"}],
        })
    );
    // Integer properties collapse to a range.
    assert_eq!(
        entry("minecraft:candle")["schema"]["candles"],
        serde_json::json!({"integer": [1, 4]})
    );
    // Enum properties are sorted names.
    assert_eq!(
        entry("minecraft:oak_slab")["schema"]["type"],
        serde_json::json!({"named": ["bottom", "double", "top"]})
    );
    // Blocks without properties have one empty state.
    assert_eq!(
        entry("minecraft:stone"),
        serde_json::json!({"schema": {}, "states": [{}]})
    );
    // The states stay in registry order: acacia_button starts at the default.
    assert_eq!(
        entry("minecraft:acacia_button")["states"][0],
        serde_json::json!({"face": "floor", "facing": "north", "powered": "true"})
    );
}
