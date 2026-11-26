//! Validates the reflective field walk: blocks whose instance fields are all
//! absent at export time produce `{}` in `block-behaviours.json`.
//!
//! `new Gson()` drops `JsonNull` values, so a block whose declared fields are
//! still null when the exporter runs serialises as `{}`. Three blocks have
//! declared fields that are null at that point (`carved_pumpkin`/`jack_o_lantern`
//! lazy golem patterns, plain `shulker_box`'s colour); modelling those needs
//! constructor-state evaluation, so they are reported rather than guessed.

#[path = "../src/behaviour_fields.rs"]
mod behaviour_fields;

use std::collections::BTreeSet;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

#[test]
fn empty_field_blocks_are_exactly_the_prepared_ones() {
    let jar = workspace().join("mc263/client.jar");
    let reference = workspace().join("tmp/ship-extract/block-behaviours.json");
    if !jar.exists() || !reference.exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (mine, coverage) = behaviour_fields::block_fields(&jar).expect("block fields");
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(reference).expect("reference")).expect("JSON");
    let prepared: BTreeSet<String> = document["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .filter(|row| row["fields"].as_object().expect("object").is_empty())
        .map(|row| row["name"].as_str().expect("name").to_string())
        .collect();
    let derived: BTreeSet<String> = mine.keys().cloned().collect();
    // The three null-field exceptions need constructor-state evaluation.
    let known_null_at_export: BTreeSet<String> = [
        "minecraft:carved_pumpkin",
        "minecraft:jack_o_lantern",
        "minecraft:shulker_box",
    ]
    .iter()
    .map(|id| id.to_string())
    .collect();
    let missing: Vec<&String> = prepared.difference(&derived).collect();
    let extra: Vec<&String> = derived.difference(&prepared).collect();
    assert!(extra.is_empty(), "derived blocks the prepared file does not have");
    assert_eq!(
        missing
            .iter()
            .map(|id| id.as_str())
            .collect::<BTreeSet<_>>(),
        known_null_at_export.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        "the only missing empty-field blocks are the null-at-export ones"
    );
    assert_eq!(coverage.blocks, 1_286);
    assert_eq!(coverage.empty_fields_exact, prepared.len() - 3);
    assert!(!coverage.gaps.is_empty(), "the gap inventory is reported");
    eprintln!(
        "empty fields: {} of {} blocks exact, 3 null-at-export exceptions, {} field kinds missing",
        coverage.empty_fields_exact,
        coverage.blocks,
        coverage.gaps.len()
    );
}

#[test]
fn known_classes_have_their_declared_instance_fields() {
    let jar = workspace().join("mc263/client.jar");
    if !jar.exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let jar = behaviour_fields::Jar::open(&jar).expect("jar");
    let field_names = |simple: &str| -> Vec<String> {
        let internal = jar.resolve(simple).unwrap_or_else(|| panic!("{simple}"));
        behaviour_fields::declared_fields(&jar, &internal)
            .expect("fields")
            .into_iter()
            .map(|field| field.name)
            .collect()
    };
    // `Block` is the walk stop; its own fields are not exported.
    assert!(behaviour_fields::declared_fields(&jar, "net/minecraft/world/level/block/Block")
        .expect("fields")
        .is_empty());
    // A subclass declares its own fields and inherits the chain's.
    let stairs = field_names("StairBlock");
    assert!(stairs.contains(&"base".to_string()), "{stairs:?}");
    // Plain subclasses carry no fields.
    assert!(field_names("AirBlock").is_empty());
}
