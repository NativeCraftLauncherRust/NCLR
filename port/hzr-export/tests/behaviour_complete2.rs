//! Compares the final integrated table with the prepared export.

#[path = "../src/behaviour_complete2.rs"]
mod behaviour_complete2;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn final_table_matches_after_normalization() {
    let jar = workspace().join("mc263/client.jar");
    if !jar.exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (document, coverage) = behaviour_complete2::document(&jar).expect("document");
    let reference: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json")).expect("table"),
    )
    .expect("JSON");

    // Blocks: headers exact, fields normalized equal.
    let mine_blocks = document["blocks"].as_array().expect("blocks");
    let ref_blocks = reference["blocks"].as_array().expect("blocks");
    let ref_index: std::collections::BTreeMap<&str, &serde_json::Value> = ref_blocks
        .iter()
        .filter_map(|row| row["name"].as_str().map(|name| (name, row)))
        .collect();
    let mut header_exact = 0usize;
    let mut fields_exact = 0usize;
    let mut field_mismatches = Vec::new();
    for row in mine_blocks {
        let Some(name) = row["name"].as_str() else { continue };
        let Some(theirs) = ref_index.get(name) else { continue };
        if ["class", "interfaces", "dynamic_shape", "item"]
            .iter()
            .all(|key| row.get(*key) == theirs.get(*key))
        {
            header_exact += 1;
        }
        let mine = behaviour_complete2::behaviour_fields_identity::normalize_exported(&row["fields"]);
        let theirs =
            behaviour_complete2::behaviour_fields_identity::normalize_exported(&theirs["fields"]);
        if mine == theirs {
            fields_exact += 1;
        } else if field_mismatches.len() < 8 {
            field_mismatches.push(format!("{name}: ours {mine:?} theirs {theirs:?}"));
        }
    }

    // States rows exact.
    let mine_states = document["states"].as_array().expect("states");
    let ref_states = reference["states"].as_array().expect("states");
    let state_exact = mine_states
        .iter()
        .zip(ref_states)
        .filter(|(mine, theirs)| mine == theirs)
        .count();

    // Items / fluids / fire exact.
    let mine_items = document["items"].as_object().expect("items");
    let ref_items = reference["items"].as_object().expect("items");
    let item_exact = mine_items
        .iter()
        .filter(|(name, value)| ref_items.get(*name) == Some(value))
        .count();
    let fluids_exact = document["fluids"] == reference["fluids"];
    let fire_exact = behaviour_complete2::behaviour_fields_identity::normalize_exported(
        &document["fire"],
    ) == behaviour_complete2::behaviour_fields_identity::normalize_exported(&reference["fire"]);


    eprintln!(
        "blocks: {} rows, header {header_exact} exact, fields {fields_exact} exact; states {state_exact}/{} exact; items {item_exact}/{}; fluids {fluids_exact}; fire {fire_exact}; fields remaining {}",
        mine_blocks.len(),
        mine_states.len(),
        ref_items.len(),
        coverage.fields_remaining.len()
    );
    for mismatch in &field_mismatches {
        eprintln!("  {mismatch}");
    }
    assert_eq!(mine_blocks.len(), 1_286);
    assert_eq!(mine_states.len(), 35_723);
    assert_eq!(header_exact, 1_286);
    assert_eq!(fields_exact, 1_286);
    assert_eq!(state_exact, 35_723, "all state rows exact");
    assert_eq!(item_exact, ref_items.len(), "all items exact");
    assert!(fluids_exact, "fluids section");
    assert!(fire_exact, "fire section");
}
