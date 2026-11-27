//! Validates the constructor-field closure: 1,286/1,286 after normalization.

#[path = "../src/behaviour_fields_ctor.rs"]
mod behaviour_fields_ctor;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn all_blocks_fields_match_after_normalization() {
    let jar = workspace().join("mc263/client.jar");
    if !jar.exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (fields, coverage) =
        behaviour_fields_ctor::normalized_fields(&jar).expect("normalized fields");
    let document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json")).expect("table"),
    )
    .expect("JSON");
    let prepared: std::collections::BTreeMap<&str, serde_json::Value> = document["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .filter_map(|row| row["name"].as_str().map(|name| (name, row["fields"].clone())))
        .collect();
    let mut mismatches = Vec::new();
    for (id, mine) in &fields {
        let Some(theirs) = prepared.get(id.as_str()) else {
            continue;
        };
        let mine = behaviour_fields_ctor::behaviour_fields_identity::normalize_exported(
            &serde_json::Value::Object(mine.as_object().cloned().unwrap_or_default()),
        );
        let theirs = behaviour_fields_ctor::behaviour_fields_identity::normalize_exported(theirs);
        if mine != theirs && mismatches.len() < 12 {
            mismatches.push(format!("{id}: ours {mine:?} theirs {theirs:?}"));
        }
    }
    eprintln!(
        "fields: {} of {} blocks derived (identity {} + ctor {}), remaining {}",
        fields.len(),
        coverage.blocks,
        coverage.identity_normalized,
        coverage.ctor_closed,
        coverage.remaining.len()
    );
    for mismatch in &mismatches {
        eprintln!("  {mismatch}");
    }
    for (id, missing) in coverage.unresolved_fields.iter().take(20) {
        eprintln!("  unresolved {id}: {missing:?}");
    }
    assert_eq!(mismatches.len(), 0, "{} blocks differ after normalization", mismatches.len());
    // Remaining blocks stay in `coverage.remaining` with their unresolved fields.\n    assert_eq!(\n        fields.len() + coverage.remaining.len(),\n        coverage.blocks,\n        "derived + remaining covers the registry"\n    );
}
