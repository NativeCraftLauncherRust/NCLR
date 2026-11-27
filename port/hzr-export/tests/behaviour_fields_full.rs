//! Validates the reconstructed reflective `fields` of blocks and block items
//! against `horizon-prepare`'s `block-behaviours.json`.
//!
//! The module renders every field it can prove from the jar and reports the
//! rest as gaps; these tests compare every block/item the module claims exact
//! and pin the coverage so a regression cannot quietly hide behind the gaps.

#[path = "../src/behaviour_fields_full.rs"]
mod behaviour_fields_full;

use std::collections::BTreeSet;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference() -> serde_json::Value {
    let path = workspace().join("tmp/ship-extract/block-behaviours.json");
    serde_json::from_slice(&std::fs::read(path).expect("block-behaviours.json")).expect("JSON")
}

#[test]
fn every_exact_block_field_map_matches_the_export() {
    let jar = jar();
    if !jar.exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let (exact, all, coverage) =
        behaviour_fields_full::block_fields_report(&jar).expect("block fields");
    let document = reference();
    let reference_blocks = document["blocks"].as_array().expect("blocks");
    let prepared: std::collections::BTreeMap<String, serde_json::Value> = reference_blocks
        .iter()
        .map(|row| {
            (
                row["name"].as_str().expect("name").to_string(),
                row["fields"].clone(),
            )
        })
        .collect();
    let value_bearing = prepared
        .values()
        .filter(|fields| fields.as_object().map(|o| !o.is_empty()).unwrap_or(false))
        .count();
    let mut mismatches = Vec::new();
    for (id, fields) in &exact {
        if let Some(expected) = prepared.get(id) {
            if expected != fields {
                mismatches.push((id.clone(), expected.clone(), fields.clone()));
            }
        }
    }
    for (id, expected, actual) in mismatches.iter().take(15) {
        eprintln!("MISMATCH {id}\n  expected {expected}\n  actual   {actual}");
    }
    eprintln!(
        "blocks: {} rendered, {} claimed exact (of {} value-bearing, {} total); gaps {}",
        all.len(),
        coverage.blocks_fields_exact,
        value_bearing,
        coverage.blocks,
        coverage.gaps.len(),
    );
    assert!(mismatches.is_empty(), "{} claimed-exact block field maps differ from the export", mismatches.len());
    assert_eq!(coverage.blocks, 1_286);
    assert_eq!(all.len(), 1_286, "every block gets a rendered map");
    // 573 blocks declare no exported instance fields and are exact; the rest of
    // the exact set are the value-bearing blocks whose constructor state is
    // fully reproducible. The remaining gaps are lambda/VoxelShape identities.
    assert_eq!(coverage.blocks_fields_exact, 1_158);
    assert_eq!(coverage.gaps.len(), 128);
    assert!(coverage.gaps.keys().all(|key| {
        key.contains("constructor incomplete")
            || key.contains("lambda")
            || key.contains("shape object identity")
            || key.contains("blockEntityType")
            || key.contains("map not reconstructed")
            || key.contains("null map")
    }));
}

#[test]
fn every_block_item_field_map_matches_the_export() {
    let jar = jar();
    if !jar.exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let (rendered, coverage) = behaviour_fields_full::item_fields(&jar).expect("item fields");
    let document = reference();
    let expected = document["items"].as_object().expect("items");
    let mut mismatches = Vec::new();
    for (id, fields) in &rendered {
        if let Some(item) = expected.get(id) {
            if &item["fields"] != fields {
                mismatches.push((id.clone(), item["fields"].clone(), fields.clone()));
            }
        }
    }
    let missing: Vec<&String> = expected
        .keys()
        .filter(|id| !rendered.contains_key(*id))
        .collect();
    for (id, expected, actual) in mismatches.iter().take(10) {
        eprintln!("MISMATCH {id}\n  expected {expected}\n  actual   {actual}");
    }
    eprintln!(
        "items: {} rendered, {} claimed exact (of {}); missing {}; mismatches {}",
        rendered.len(),
        coverage.items_fields_exact,
        expected.len(),
        missing.len(),
        mismatches.len()
    );
    assert!(mismatches.is_empty(), "{} item field maps differ", mismatches.len());
    assert_eq!(coverage.items, 1_141);
    assert_eq!(coverage.items_fields_exact, 1_141);
    let expected_ids: BTreeSet<&String> = expected.keys().collect();
    let rendered_ids: BTreeSet<&String> = rendered.keys().collect();
    assert_eq!(expected_ids, rendered_ids, "item ids missing or extra");
}
