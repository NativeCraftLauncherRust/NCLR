//! Validation of `items_codec`: every emitted item must be exact.

#[path = "../src/items_codec.rs"]
mod items_codec;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}
fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}
fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/items.json")
}

#[test]
fn emitted_items_are_exact() {
    if !jar().exists() || !prepared().exists() {
        return;
    }
    let (mine, coverage) = items_codec::items_components(&jar()).expect("items");
    let text = std::fs::read_to_string(prepared()).expect("oracle");
    let expected: serde_json::Value = serde_json::from_str(&text).expect("json");
    let expected = expected.as_object().expect("object");
    let mut mismatches = Vec::new();
    for (id, ours) in &mine {
        match expected.get(id) {
            Some(want) if want == ours => {}
            Some(want) => {
                if mismatches.len() < 12 {
                    mismatches.push(format!(
                        "{id}: ours {} want {}",
                        serde_json::to_string(ours).unwrap_or_default(),
                        serde_json::to_string(want).unwrap_or_default()
                    ));
                }
            }
            None => mismatches.push(format!("{id}: not in oracle")),
        }
    }
    eprintln!(
        "emitted {}/{} items; gaps {:?}",
        mine.len(),
        expected.len(),
        coverage.gaps
    );
    for s in &mismatches {
        eprintln!("  {s}");
    }
    assert!(mismatches.is_empty(), "emitted items differ");
    assert_eq!(coverage.emitted, mine.len());
}
