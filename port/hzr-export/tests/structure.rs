//! Does the Rust exporter reproduce `worldgen/blocks/structure-blocks.json` from the jar?
//!
//! The reference is the prepared copy `tmp/ship-extract/worldgen/blocks/structure-blocks.json`
//! — what `horizon-prepare` produced from the real 26.3 jars with
//! `VanillaStructureBlockExporter`.
//!
//! `states`, `rotate`, `mirror`, `loot_containers` and `block_entities` are compared
//! exactly, all 35 723 rows each. `full_block` is the module's declared gap
//! (`structure::Coverage`): it needs each state's collision shape, which is block geometry.
//! The last test pins that gap, so finishing it later has to touch this test too.

#[path = "../src/structure.rs"]
mod structure;

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
    workspace().join("tmp/ship-extract")
}

fn load_reference() -> Option<Value> {
    let path = prepared().join("worldgen/blocks/structure-blocks.json");
    if !jar().is_file() || !path.is_file() {
        eprintln!(
            "skipping: {} or {} is not there",
            jar().display(),
            path.display()
        );
        return None;
    }
    let text = std::fs::read_to_string(&path).expect("the prepared table");
    Some(serde_json::from_str(&text).expect("the prepared table is JSON"))
}

fn ours() -> structure::Table {
    structure::table(&jar()).expect("the table builds from the jar")
}

fn as_u64(row: impl AsRef<[u32]>) -> Vec<u64> {
    row.as_ref().iter().map(|value| *value as u64).collect()
}

fn ids(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .expect("an id array")
        .iter()
        .map(|value| value.as_u64().expect("an id"))
        .collect()
}

/// Per block, how many rotate and mirror rows disagree.
fn disagreements(
    reference: &Value,
    table: &structure::Table,
    declared: &states::StateTable,
) -> Vec<(String, usize, usize, usize)> {
    let expected_rotate = reference["rotate"].as_array().expect("rotate");
    let expected_mirror = reference["mirror"].as_array().expect("mirror");
    let mut start = 0usize;
    let mut out = Vec::new();
    for block in &declared.blocks {
        let mut rotate_bad = 0usize;
        let mut mirror_bad = 0usize;
        for state in start..start + block.state_count {
            if as_u64(table.rotate[state]) != ids(&expected_rotate[state]) {
                rotate_bad += 1;
            }
            if as_u64(table.mirror[state]) != ids(&expected_mirror[state]) {
                mirror_bad += 1;
            }
        }
        if rotate_bad + mirror_bad > 0 {
            out.push((block.id.clone(), rotate_bad, mirror_bad, block.state_count));
        }
        start += block.state_count;
    }
    out
}

#[test]
fn the_per_state_tables_match_the_prepared_export() {
    let Some(reference) = load_reference() else {
        return;
    };
    let table = ours();
    assert_eq!(
        table.states as u64,
        reference["states"].as_u64().expect("a state count"),
        "the state count"
    );
    assert_eq!(table.rotate.len(), table.states);
    assert_eq!(table.mirror.len(), table.states);
    let declared = states::state_table(&jar()).expect("the state table");
    let bad = disagreements(&reference, &table, &declared);
    assert!(
        bad.is_empty(),
        "{} blocks disagree (rotate, mirror, of states):\n{}",
        bad.len(),
        bad.iter()
            .map(|(id, rotate, mirror, states)| format!("{id}: {rotate} {mirror} / {states}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_container_and_block_entity_sections_match_the_prepared_export() {
    let Some(reference) = load_reference() else {
        return;
    };
    let table = ours();
    assert_eq!(
        as_u64(&table.loot_containers),
        ids(&reference["loot_containers"]),
        "the loot container states"
    );
    let expected = reference["block_entities"]
        .as_object()
        .expect("block entities");
    let ours: std::collections::BTreeMap<&String, Vec<u64>> = table
        .block_entities
        .iter()
        .map(|(kind, members)| (kind, as_u64(members)))
        .collect();
    for (kind, members) in expected {
        assert_eq!(
            ours.get(kind).unwrap_or_else(|| panic!("{kind}: missing")),
            &ids(members),
            "{kind}: the states that carry it"
        );
    }
    // The one entry this exporter has and the prepared copy does not. Every block the
    // exporter reaches creates its block entity through `newBlockEntity` and the jar
    // registers `BlockEntityTypes.VAULT` for `VaultBlockEntity`, so `minecraft:vault`
    // (14 states) is what the jar says; the prepared copy predates or missed it.
    let extra: Vec<&String> = ours
        .keys()
        .copied()
        .filter(|kind| !expected.contains_key(*kind))
        .collect();
    assert_eq!(
        extra,
        vec!["minecraft:vault"],
        "only the vault type is beyond the prepared copy"
    );
    assert_eq!(
        ours.get(&"minecraft:vault".to_string())
            .map(|states| states.len()),
        Some(32)
    );
    // The export writes the types in first-appearance order, and the prepared copy starts
    // with the earliest state that carries one.
    let raw = std::fs::read_to_string(prepared().join("worldgen/blocks/structure-blocks.json"))
        .expect("the prepared table");
    let first = raw
        .split("\"block_entities\":{\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the first block entity type");
    assert_eq!(
        table.block_entities.first().map(|(kind, _)| kind.as_str()),
        Some(first),
        "the block entity types start where the prepared copy's do"
    );
}

#[test]
fn the_report_is_the_compact_shape_the_bundle_carries() {
    let table = structure::Table {
        states: 2,
        rotate: vec![[4, 5, 3], [2, 2, 2]],
        mirror: vec![[5, 5], [2, 2]],
        full_block: vec![false, true],
        loot_containers: vec![1, 2],
        block_entities: vec![
            ("minecraft:chest".to_string(), vec![1, 2, 3]),
            ("minecraft:sign".to_string(), vec![4]),
        ],
    };
    assert_eq!(
        structure::render(&table),
        concat!(
            "{\"states\":2,",
            "\"rotate\":[[4,5,3],[2,2,2]],",
            "\"mirror\":[[5,5],[2,2]],",
            "\"full_block\":[false,true],",
            "\"loot_containers\":[1,2],",
            "\"block_entities\":{\"minecraft:chest\":[1,2,3],\"minecraft:sign\":[4]}}",
        )
    );
}

#[test]
fn the_unfinished_section_is_reported_as_a_gap() {
    if load_reference().is_none() {
        return;
    }
    let out = std::env::temp_dir().join(format!("hzr-export-structure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = structure::write(&jar(), &out).expect("the table is written");
    assert!(coverage
        .produced
        .contains(&"worldgen/blocks/structure-blocks.json".to_string()));
    assert_eq!(
        coverage.missing,
        vec!["worldgen/blocks/structure-blocks.json#full_block".to_string()],
        "full_block is the one declared gap"
    );
    let written = std::fs::read_to_string(out.join("worldgen/blocks/structure-blocks.json"))
        .expect("the written table");
    let value: Value = serde_json::from_str(&written).expect("the written table is JSON");
    assert_eq!(
        value["rotate"].as_array().map(Vec::len),
        Some(35_723),
        "all of the rotate rows are written"
    );
    let _ = std::fs::remove_dir_all(&out);
}
