//! Validation of `items_data`: every emitted item must match
//! `tmp/ship-extract/items.json` exactly; the rest are reported as gaps.

#[path = "../src/items_data.rs"]
mod items_data;

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
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (mine, coverage) = items_data::items_components(&jar()).expect("items");
    let text = std::fs::read_to_string(prepared()).expect("prepared copy");
    let expected: serde_json::Value = serde_json::from_str(&text).expect("json");
    let expected = expected.as_object().expect("object");
    let mut mismatches = Vec::new();
    for (id, ours) in &mine {
        match expected.get(id) {
            Some(want) if want == ours => {}
            Some(want) => {
                if mismatches.len() < 8 {
                    mismatches.push(format!(
                        "{id}: ours {} want {}",
                        serde_json::to_string(ours).unwrap_or_default(),
                        serde_json::to_string(want).unwrap_or_default()
                    ));
                }
            }
            None => mismatches.push(format!("{id}: not in the prepared copy")),
        }
    }
    eprintln!(
        "emitted {}/{} items, oracle-only {}",
        mine.len(),
        expected.len(),
        expected.len().saturating_sub(mine.len())
    );
    eprintln!("coverage gaps: {:?}", coverage.gaps);
    // Diagnostics: default-component items the scanner did not emit.
    let universal = [
        "minecraft:attack_animation",
        "minecraft:attribute_modifiers",
        "minecraft:break_sound",
        "minecraft:enchantments",
        "minecraft:interact_animation",
        "minecraft:item_model",
        "minecraft:item_name",
        "minecraft:lore",
        "minecraft:max_stack_size",
        "minecraft:rarity",
        "minecraft:repair_cost",
        "minecraft:tooltip_display",
        "minecraft:use_effects",
    ];
    let mut default_missing = Vec::new();
    for (id, want) in expected {
        if let Some(object) = want.as_object() {
            if object.len() == universal.len()
                && universal.iter().all(|key| object.contains_key(*key))
                && !mine.contains_key(id)
            {
                default_missing.push(id.clone());
            }
        }
    }
    eprintln!(
        "default-component items missing: {} (first: {:?})",
        default_missing.len(),
        &default_missing[..default_missing.len().min(20)]
    );
    for sample in &mismatches {
        eprintln!("  {sample}");
    }
    assert!(
        mismatches.is_empty(),
        "emitted items differ from the oracle"
    );
    assert_eq!(coverage.emitted, mine.len());
    assert_eq!(mine.len(), 1312, "exact items.json subset");
}
