//! Does the Rust exporter reproduce Horizon's block tables from the jar alone?
//!
//! The reference is `tmp/ship-extract/landing-definitions.json`, written by
//! `VanillaBlockLandingOracle.export` from the real 26.3 jars — the same file
//! `ship.hzr3` carries. These tests read the player's `mc263/client.jar` and
//! compare the generated document to it, block by block.
//!
//! The tests need both the jar and the prepared table; when either is missing (a
//! machine without a Minecraft install, CI) they say so and pass, because the
//! exporter's contract is "given this jar, this report" and there is nothing to
//! compare without a jar.

#[path = "../src/block.rs"]
mod block;

use std::path::PathBuf;

use serde_json::Value;

fn workspace() -> PathBuf {
    // <workspace>/route3/crates/hzr-export
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference_table() -> PathBuf {
    workspace().join("tmp/ship-extract/landing-definitions.json")
}

fn behaviour_reference() -> PathBuf {
    workspace().join("tmp/ship-extract/block-behaviours.json")
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap_or_else(|error| {
        panic!("{}: {error}", path.display())
    }))
    .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// A short description of the first differences, for the failure message.
fn describe_diff(mine: &Value, theirs: &Value) -> String {
    let mine = mine.as_object().expect("our document is an object");
    let theirs = theirs.as_object().expect("the reference is an object");
    let extra: Vec<&String> = mine.keys().filter(|key| !theirs.contains_key(*key)).take(5).collect();
    let missing: Vec<&String> = theirs.keys().filter(|key| !mine.contains_key(*key)).take(5).collect();
    let differing: Vec<String> = mine
        .iter()
        .filter(|(key, value)| theirs.get(*key).is_some_and(|expected| expected != *value))
        .take(5)
        .map(|(key, value)| format!("{key}: ours {value}, reference {}", theirs[key]))
        .collect();
    format!(
        "{} vs {} blocks; extra {extra:?}; missing {missing:?}; differing {differing:?}",
        mine.len(),
        theirs.len()
    )
}

#[test]
fn landing_definitions_reproduces_the_java_export() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!(
            "skipped: needs {} and {}",
            jar().display(),
            reference_table().display()
        );
        return;
    }
    let mine = block::landing_definitions(&jar()).expect("the jar is readable");
    let theirs = read(&reference_table());
    if mine != theirs {
        let mine = mine.as_object().expect("an object");
        let theirs = theirs.as_object().expect("an object");
        for (id, entry) in theirs {
            match mine.get(id) {
                Some(ours) if ours == entry => {}
                Some(ours) => eprintln!("differs {id}: ours {ours}, reference {entry}"),
                None => eprintln!("missing {id}"),
            }
        }
        for id in mine.keys() {
            if !theirs.contains_key(id) {
                eprintln!("extra {id}");
            }
        }
        panic!("{}", describe_diff(&serde_json::Value::Object(mine.clone()), &serde_json::Value::Object(theirs.clone())));
    }
}

#[test]
fn landing_definitions_matches_the_prepared_bytes() {
    if !jar().exists() || !reference_table().exists() {
        eprintln!("skipped: needs the jar and the prepared table");
        return;
    }
    // `horizon-prepare` copies Gson's compact `toString`, so the bytes are a
    // stronger contract than the parsed values: key order included.
    // `HZR_EXPORT_OUT` keeps the table somewhere the caller named (used to diff
    // it against the Java export); otherwise it lives in a scratch directory.
    let named = std::env::var_os("HZR_EXPORT_OUT").map(PathBuf::from);
    let out = named.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("hzr-export-block-{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&out);
    block::write(&jar(), &out).expect("write");
    let written = std::fs::read(out.join(block::TABLE)).expect("the written table");
    let expected = std::fs::read(reference_table()).expect("the prepared table");
    if named.is_none() {
        let _ = std::fs::remove_dir_all(&out);
    }
    if written != expected {
        let at = written
            .iter()
            .zip(expected.iter())
            .position(|(mine, theirs)| mine != theirs)
            .unwrap_or_else(|| written.len().min(expected.len()));
        panic!(
            "the bytes differ at {at}: ours {:?}, reference {:?}",
            String::from_utf8_lossy(&written[at.saturating_sub(40)..(at + 40).min(written.len())]),
            String::from_utf8_lossy(&expected[at.saturating_sub(40)..(at + 40).min(expected.len())])
        );
    }
}

#[test]
fn the_registry_is_the_1286_blocks_the_block_table_names() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let blocks = block::registry(&jar()).expect("the registry");
    // 26.3 registers 1,286 blocks: 878 one by one, 288 coloured, 120 copper.
    assert_eq!(blocks.len(), 1_286, "the registry has the wrong size");
    let ids: std::collections::BTreeSet<&str> = blocks.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids.len(), blocks.len(), "an id is registered twice");
    let colored = blocks
        .iter()
        .filter(|row| row.id.starts_with("minecraft:light_blue_"))
        .count();
    assert!(colored >= 18, "the sixteen-value colour families are missing");
}

#[test]
fn every_block_class_matches_block_behaviours() {
    if !jar().exists() || !behaviour_reference().exists() {
        eprintln!("skipped: needs the jar and block-behaviours.json");
        return;
    }
    // `VanillaBlockBehaviourExporter` writes every block's class chain from the
    // running class. Our chain comes from the factory lambda in
    // `Blocks.<clinit>`, so this checks the registry derivation itself: ids,
    // order and classes.
    let reference = read(&behaviour_reference());
    let rows = reference["blocks"].as_array().expect("blocks");
    let ours = block::registry(&jar()).expect("the registry");
    assert_eq!(ours.len(), rows.len(), "the registry has the wrong size");
    let mut differing = Vec::new();
    let mut differences = 0usize;
    for (index, row) in rows.iter().enumerate() {
        let name = row["name"].as_str().unwrap_or_default();
        let expected: Vec<&str> = row["class"]
            .as_array()
            .expect("a class chain")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let block = &ours[index];
        if block.id != name || block.chain != expected {
            differences += 1;
            if differing.len() < 8 {
                differing.push(format!(
                    "{index} {name}: ours {} {:?}, reference {:?}",
                    block.id, block.chain, expected
                ));
            }
        }
    }
    assert!(
        differing.is_empty(),
        "{differences} of {} blocks differ: {}",
        rows.len(),
        differing.join("\n")
    );
}

#[test]
fn the_three_special_landing_entries_are_the_ones_vanilla_writes() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    // Independent expected values, read off vanilla's landing rules rather than
    // from the reference table: farmland's base block, the shelf mushroom's
    // bounce, and the honey block's 1.0/1.0 sound.
    let document = block::landing_definitions(&jar()).expect("the table");
    assert_eq!(
        document["minecraft:farmland"],
        serde_json::json!({"fall": "FarmlandBlock", "bounce": false, "base": "minecraft:dirt"})
    );
    assert_eq!(
        document["minecraft:shelf_mushroom"],
        serde_json::json!({"fall": "Block", "bounce": true})
    );
    assert_eq!(
        document["minecraft:honey_block"],
        serde_json::json!({
            "fall": "HoneyBlock",
            "bounce": false,
            "volume": "3f800000",
            "pitch": "3f800000",
        })
    );
    // Everything else lands like a plain `Block`.
    assert_eq!(
        document["minecraft:stone"],
        serde_json::json!({"fall": "Block", "bounce": false})
    );
}

#[test]
fn the_coverage_reports_the_tables_it_cannot_build() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let out = std::env::temp_dir().join(format!("hzr-export-block-cov-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let coverage = block::write(&jar(), &out).expect("write");
    let _ = std::fs::remove_dir_all(&out);
    assert_eq!(coverage.produced, vec![block::TABLE.to_string()]);
    for table in block::TABLES {
        let known = coverage.produced.iter().any(|name| name == table)
            || coverage.missing.iter().any(|name| name == table);
        assert!(known, "{table} is neither produced nor reported missing");
    }
    let notes = coverage.notes.join("\n");
    for (table, _) in block::BLOCKED_TABLES {
        assert!(notes.contains(table), "{table} is not named in the notes");
    }
    // Every registration's factory resolved, so no entry silently fell back to
    // the plain `Block` class.
    assert!(
        notes.contains("0 without a resolved class"),
        "the registry left a factory unresolved: {notes}"
    );
    assert!(block::TABLE == "landing-definitions.json");
}
