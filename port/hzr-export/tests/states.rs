//! Does the Rust exporter reproduce Horizon's block-state table from the jar?
//!
//! The reference is the prepared copy `tmp/ship-extract/_shared/` — the same two
//! files `ship.hzr3` carries, written by `horizon-prepare` from the real 26.3
//! jars: `states.bin` (the `states` NBT trailer, one compound per state in id
//! order) and `block-order.json` (the block order and default state ids).

#[path = "../src/states.rs"]
mod states;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/_shared")
}

// ---------------------------------------------------------------------------
// A small NBT reader: the states list is compounds of strings only.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tag {
    Byte(i8),
    String(String),
    Int(i32),
    Long(i64),
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u1(&mut self) -> u8 {
        let value = self.bytes[self.at];
        self.at += 1;
        value
    }
    fn u2(&mut self) -> u16 {
        let value = u16::from_be_bytes([self.bytes[self.at], self.bytes[self.at + 1]]);
        self.at += 2;
        value
    }
    fn u4(&mut self) -> u32 {
        let value = u32::from_be_bytes([
            self.bytes[self.at],
            self.bytes[self.at + 1],
            self.bytes[self.at + 2],
            self.bytes[self.at + 3],
        ]);
        self.at += 4;
        value
    }
    fn text(&mut self) -> String {
        let length = self.u2() as usize;
        let value = String::from_utf8_lossy(&self.bytes[self.at..self.at + length]).into_owned();
        self.at += length;
        value
    }
    fn payload(&mut self, tag: u8) -> Tag {
        match tag {
            1 => Tag::Byte(self.u1() as i8),
            3 => Tag::Int(self.u4() as i32),
            4 => {
                let high = self.u4() as u64;
                let low = self.u4() as u64;
                Tag::Long(((high << 32) | low) as i64)
            }
            7 => {
                let length = self.u4() as usize;
                self.at += length;
                Tag::List(Vec::new())
            }
            8 => Tag::String(self.text()),
            9 => {
                let element = self.u1();
                let count = self.u4() as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(self.payload(element));
                }
                Tag::List(items)
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let tag = self.u1();
                    if tag == 0 {
                        return Tag::Compound(entries);
                    }
                    let name = self.text();
                    let value = self.payload(tag);
                    entries.push((name, value));
                }
            }
            other => panic!("unsupported NBT tag {other}"),
        }
    }
}

fn read_states(path: &Path) -> Vec<(String, Vec<(String, String)>)> {
    let bytes = std::fs::read(path).expect("states.bin");
    let mut reader = Reader { bytes: &bytes, at: 0 };
    let tag = reader.u1();
    assert_eq!(tag, 9, "states.bin is a list");
    let _name = reader.text();
    let element = reader.u1();
    assert_eq!(element, 10, "of compounds");
    let count = reader.u4() as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Tag::Compound(entries) = reader.payload(10) else {
            panic!("a state is a compound");
        };
        let mut id = String::new();
        let mut properties = Vec::new();
        for (name, value) in entries {
            match (name.as_str(), value) {
                ("id", Tag::String(text)) => id = text,
                ("properties", Tag::Compound(pairs)) => {
                    for (key, value) in pairs {
                        let Tag::String(value) = value else {
                            panic!("a property value is a string");
                        };
                        properties.push((key, value));
                    }
                }
                other => panic!("unexpected state field {other:?}"),
            }
        }
        out.push((id, properties));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn the_registry_is_the_states_the_prepared_copy_carries() {
    if !jar().exists() || !prepared().join("states.bin").exists() {
        eprintln!("skipped: needs the jar and the prepared states.bin");
        return;
    }
    let table = states::state_table(&jar()).expect("the state table");
    let expected = read_states(&prepared().join("states.bin"));
    assert_eq!(
        table.state_count(),
        expected.len(),
        "the table has the wrong number of states"
    );
    assert_eq!(
        table.blocks.len(),
        1_286,
        "the table has the wrong number of blocks"
    );

    let sorted: Vec<Vec<&states::Property>> = table.blocks.iter().map(|block| block.sorted()).collect();
    let mut index = 0usize;
    for (block, properties) in table.blocks.iter().zip(&sorted) {
        let lists: Vec<Vec<String>> =
            properties.iter().map(|property| property.values.clone()).collect();
        for row in cartesian(&lists) {
            let (expected_id, expected_properties) = &expected[index];
            assert_eq!(&block.id, expected_id, "state {index}: block id");
            let mut mine: Vec<(String, String)> = properties
                .iter()
                .zip(&row)
                .map(|(property, value)| {
                    (property.name.clone(), property.values[*value].clone())
                })
                .collect();
            // The state's identity, not the bundle's NBT field order: that order
            // is checked by the byte comparison below.
            mine.sort();
            let mut expected = expected_properties.clone();
            expected.sort();
            assert_eq!(&mine, &expected, "state {index} ({expected_id}): properties");
            index += 1;
        }
    }
}

#[test]
fn block_order_json_matches_the_prepared_copy() {
    if !jar().exists() || !prepared().join("block-order.json").exists() {
        eprintln!("skipped: needs the jar and the prepared block-order.json");
        return;
    }
    let mine = states::block_order(&jar()).expect("the table");
    let expected: Value = serde_json::from_slice(
        &std::fs::read(prepared().join("block-order.json")).expect("the table"),
    )
    .expect("reference JSON");
    let mut differences = Vec::new();
    let mine_blocks = mine["blocks"].as_array().expect("blocks");
    let expected_blocks = expected["blocks"].as_array().expect("blocks");
    if mine_blocks.len() != expected_blocks.len() {
        differences.push(format!(
            "{} blocks vs {}",
            mine_blocks.len(),
            expected_blocks.len()
        ));
    }
    for (index, (mine, expected)) in mine_blocks.iter().zip(expected_blocks).enumerate() {
        if mine != expected && differences.len() < 10 {
            differences.push(format!("block {index}: {mine} vs {expected}"));
        }
    }
    let mine_defaults = mine["defaults"].as_object().expect("defaults");
    let expected_defaults = expected["defaults"].as_object().expect("defaults");
    let mut differing = 0usize;
    for (id, value) in expected_defaults {
        if mine_defaults.get(id) != Some(value) {
            differing += 1;
            if differences.len() < 20 {
                differences.push(format!(
                    "default {id}: {:?} vs {value}",
                    mine_defaults.get(id)
                ));
            }
        }
    }
    assert!(
        differences.is_empty(),
        "{differing} defaults differ: {}",
        differences.join("\n")
    );
}

#[test]
fn both_files_are_byte_identical_to_the_prepared_copies() {
    if !jar().exists() || !prepared().join("states.bin").exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let named = std::env::var_os("HZR_EXPORT_OUT").map(std::path::PathBuf::from);
    let out = named.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hzr-export-states-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&out);
    states::write(&jar(), &out).expect("write");
    for name in [states::TABLE, states::STATES] {
        let mine = std::fs::read(out.join(name)).expect("our file");
        // The bundle keeps both under `_shared/`, so `name` already carries it.
        let expected = std::fs::read(workspace().join("tmp/ship-extract").join(name))
            .expect("the prepared file");
        if mine != expected {
            let at = mine
                .iter()
                .zip(expected.iter())
                .position(|(left, right)| left != right)
                .unwrap_or_else(|| mine.len().min(expected.len()));
            panic!(
                "{name} differs at byte {at}: ours {} bytes, reference {} bytes; ours {:?} reference {:?}",
                mine.len(),
                expected.len(),
                &mine[at.saturating_sub(30)..(at + 30).min(mine.len())],
                &expected[at.saturating_sub(30)..(at + 30).min(expected.len())],
            );
        }
    }
    if named.is_none() {
        let _ = std::fs::remove_dir_all(&out);
    }
}

#[test]
#[allow(clippy::type_complexity)]
fn every_block_property_list_is_the_one_the_state_file_shows() {
    if !jar().exists() || !prepared().join("states.bin").exists() {
        eprintln!("skipped: needs the jar and the prepared copies");
        return;
    }
    let table = states::state_table(&jar()).expect("the state table");
    let expected = read_states(&prepared().join("states.bin"));
    // Group the reference into per-block runs, then compare block by block: id,
    // state count, property names and each property's value set.
    type StateRow = (String, Vec<(String, String)>);
    type BlockRun = (String, Vec<StateRow>);
    let mut runs: Vec<BlockRun> = Vec::new();
    for (id, properties) in &expected {
        match runs.last_mut() {
            Some((last, states)) if last == id => states.push((id.clone(), properties.clone())),
            _ => runs.push((id.clone(), vec![(id.clone(), properties.clone())])),
        }
    }
    assert_eq!(runs.len(), table.blocks.len(), "the block count differs");
    for (block, (id, slice)) in table.blocks.iter().zip(&runs) {
        assert_eq!(&block.id, id, "the block order differs");
        assert_eq!(
            block.state_count,
            slice.len(),
            "{}: state count",
            block.id
        );
        let mut observed: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (_, properties) in slice.iter() {
            for (name, value) in properties {
                observed
                    .entry(name.as_str())
                    .or_default()
                    .push(value.as_str());
            }
        }
        for (name, values) in observed {
            let property = block
                .properties
                .iter()
                .find(|property| property.name == name)
                .unwrap_or_else(|| panic!("{}: reference has property {name}, ours does not", block.id));
            let mut unique: Vec<&str> = values.clone();
            unique.sort_unstable();
            unique.dedup();
            let mut mine: Vec<&str> = property.values.iter().map(String::as_str).collect();
            mine.sort_unstable();
            assert_eq!(
                unique, mine,
                "{}: property {name} value set",
                block.id
            );
        }
        assert_eq!(
            block.properties.len(),
            slice[0].1.len(),
            "{}: property count",
            block.id
        );
    }

}

#[test]
fn the_coverage_reports_the_unblocked_tables() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let out = std::env::temp_dir().join(format!("hzr-export-states-cov-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = states::write(&jar(), &out).expect("write");
    let _ = std::fs::remove_dir_all(&out);
    assert_eq!(coverage.produced, vec![states::TABLE.to_string(), states::STATES.to_string()]);
    assert!(coverage.notes.join("\n").contains("states.bin"));
    assert_eq!(states::TABLES.len(), 2);
}

fn cartesian(values: &[Vec<String>]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new()];
    for list in values {
        let mut next = Vec::new();
        for prefix in &out {
            for index in 0..list.len() {
                let mut row = prefix.clone();
                row.push(index);
                next.push(row);
            }
        }
        out = next;
    }
    out
}
#[test]
fn the_property_table_is_what_downstream_reads() {
    if !jar().exists() {
        eprintln!("skipped: needs the jar");
        return;
    }
    let table = states::block_properties(&jar()).expect("the property table");
    assert_eq!(table.len(), 1_286, "every block has an entry");
    let with_properties = table.values().filter(|properties| !properties.is_empty()).count();
    assert_eq!(with_properties, 884, "the reference has 884 property-carrying blocks");
    // Boolean properties list `true` before `false` (26.3's `BooleanProperty`).
    let furnace = &table["minecraft:furnace"];
    let lit = furnace
        .iter()
        .find(|(name, _)| name == "lit")
        .expect("furnace lit");
    assert_eq!(lit.1, vec!["true".to_string(), "false".to_string()]);
    // Enums use `StringRepresentable` names, which need not be the constant:
    // `PistonType.DEFAULT` serialises as `normal`.
    let piston = &table["minecraft:piston_head"];
    let kind = piston
        .iter()
        .find(|(name, _)| name == "type")
        .expect("piston_head type");
    assert_eq!(kind.1, vec!["normal".to_string(), "sticky".to_string()]);
    // Integer properties run from their minimum to their maximum.
    let oak = &table["minecraft:oak_stairs"];
    let shape = oak
        .iter()
        .find(|(name, _)| name == "shape")
        .expect("oak_stairs shape");
    assert_eq!(
        shape.1,
        vec![
            "straight",
            "inner_left",
            "inner_right",
            "outer_left",
            "outer_right"
        ]
    );
}