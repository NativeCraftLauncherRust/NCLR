//! Does the Rust exporter reproduce the world generator's block placement and survival
//! tables from the jar?
//!
//! The references are the prepared copies under `tmp/ship-extract/worldgen/blocks/`, what
//! `horizon-prepare` produced from the real 26.3 jars: `VanillaSimpleBlockOracle`'s export
//! mode writes `placement-definitions.json` and `survival-extra-definitions.json`, and
//! `survival::bind_definitions` turns `VanillaBlockSurvivalOracle`'s three intermediates into
//! `survival-rules.json`.
//!
//! All three are compared exactly: the values and the order the bundle writes them in.

#[path = "../src/worldgen_blocks.rs"]
mod worldgen_blocks;

use std::path::{Path, PathBuf};

use serde_json::Value;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/worldgen/blocks")
}

fn load(name: &str) -> Option<(String, Value)> {
    let path = prepared().join(name);
    if !jar().is_file() || !path.is_file() {
        eprintln!(
            "skipping: {} or {} is not there",
            jar().display(),
            path.display()
        );
        return None;
    }
    let text = std::fs::read_to_string(&path).expect("the prepared table");
    let value = serde_json::from_str(&text).expect("the prepared table is JSON");
    Some((text, value))
}

fn ours() -> worldgen_blocks::Tables {
    worldgen_blocks::tables(&jar()).expect("the tables build from the jar")
}

/// The keys and values of a flat `{"key":"value", ...}` object, in file order.
fn ordered_pairs(text: &str) -> Vec<(String, String)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some((key, after_key)) = quoted(bytes, at) {
        // The colon, then the value.
        let mut colon = after_key;
        while bytes.get(colon) != Some(&b':') {
            colon += 1;
            if colon >= bytes.len() {
                return out;
            }
        }
        let Some((value, after_value)) = quoted(bytes, colon + 1) else {
            return out;
        };
        out.push((key, value));
        at = after_value;
    }
    out
}

fn quoted(bytes: &[u8], mut at: usize) -> Option<(String, usize)> {
    while bytes.get(at) != Some(&b'"') {
        at += 1;
        if at >= bytes.len() {
            return None;
        }
    }
    let start = at + 1;
    let mut end = start;
    while bytes.get(end) != Some(&b'"') {
        end += 1;
        if end >= bytes.len() {
            return None;
        }
    }
    Some((
        String::from_utf8_lossy(&bytes[start..end]).into_owned(),
        end + 1,
    ))
}

#[test]
fn placement_definitions_match_the_prepared_copy() {
    let Some((text, reference)) = load("placement-definitions.json") else {
        return;
    };
    let tables = ours();
    let rendered = worldgen_blocks::render_placement(&tables);
    let ours: Value = serde_json::from_str(&rendered).expect("our table is JSON");
    assert_eq!(ours, reference, "placement-definitions.json differs");
    assert_eq!(rendered, text, "the bytes differ from the prepared copy");
}

#[test]
fn survival_rules_match_the_prepared_copy() {
    let Some((text, reference)) = load("survival-rules.json") else {
        return;
    };
    let tables = ours();
    let rendered = worldgen_blocks::render_survival(&tables);
    let ours: Value = serde_json::from_str(&rendered).expect("our table is JSON");
    if ours != reference {
        let expected = reference.as_object().expect("an object");
        let actual = ours.as_object().expect("an object");
        let mut missing = Vec::new();
        let mut extra = Vec::new();
        let mut wrong = Vec::new();
        for (block, rule) in expected {
            match actual.get(block) {
                None => missing.push(block.clone()),
                Some(value) if value != rule => {
                    wrong.push(format!("{block}: {} != {}", value, rule))
                }
                Some(_) => {}
            }
        }
        for block in actual.keys() {
            if !expected.contains_key(block) {
                extra.push(block.clone());
            }
        }
        panic!(
            "survival-rules.json differs: {} missing, {} extra, {} wrong\nmissing: {:?}\nextra: {:?}\nwrong: {:?}",
            missing.len(),
            extra.len(),
            wrong.len(),
            &missing[..missing.len().min(20)],
            &extra[..extra.len().min(20)],
            &wrong[..wrong.len().min(20)],
        );
    }
    let expected = ordered_pairs(&text);
    assert_eq!(
        tables.survival, expected,
        "the rules, in the order the bundle writes them"
    );
    assert_eq!(rendered, text, "the bytes differ from the prepared copy");
}

#[test]
fn survival_extra_definitions_match_the_prepared_copy() {
    let Some((text, reference)) = load("survival-extra-definitions.json") else {
        return;
    };
    let tables = ours();
    let rendered = worldgen_blocks::render_extra(&tables);
    let ours: Value = serde_json::from_str(&rendered).expect("our table is JSON");
    if ours != reference {
        let expected_gates: Vec<String> = reference["fence_gates"]
            .as_array()
            .expect("fence gates")
            .iter()
            .map(|value| value.as_str().expect("an id").to_string())
            .collect();
        let expected_odds = reference["ignition_odds"].as_object().expect("odds");
        let mut wrong = Vec::new();
        let mut missing = Vec::new();
        for (block, odds) in expected_odds {
            let wanted = odds.as_i64().expect("an odd");
            match tables
                .ignition_odds
                .iter()
                .find(|(candidate, _)| candidate == block)
            {
                None => missing.push(block.clone()),
                Some((_, ours)) if *ours as i64 != wanted => {
                    wrong.push(format!("{block}: {ours} != {wanted}"))
                }
                Some(_) => {}
            }
        }
        panic!(
            "survival-extra-definitions.json differs\nfence gates: ours {:?}\nexpected {:?}\nmissing odds: {:?}\nwrong odds ({}): {:?}",
            tables.fence_gates,
            expected_gates,
            &missing[..missing.len().min(10)],
            wrong.len(),
            &wrong[..wrong.len().min(20)],
        );
    }
    assert_eq!(rendered, text, "the bytes differ from the prepared copy");
}

#[test]
fn write_produces_the_three_paths() {
    if load("survival-rules.json").is_none() {
        return;
    }
    let out = std::env::temp_dir().join(format!("hzr-export-blocks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = worldgen_blocks::write(&jar(), &out).expect("the tables are written");
    assert_eq!(coverage.produced.len(), 3);
    assert!(coverage.missing.is_empty());
    for name in worldgen_blocks::TABLES {
        assert!(out.join(name).is_file(), "{name} should have been written");
    }
    let survival = std::fs::read_to_string(out.join(worldgen_blocks::TABLES[1]))
        .expect("the written survival rules");
    let rules: Value = serde_json::from_str(&survival).expect("JSON");
    assert_eq!(rules.as_object().map(|object| object.len()), Some(1_286));
    let _ = std::fs::remove_dir_all(&out);
}
