//! Does `behaviour_header` reproduce the non-state sections of the Java
//! `block-behaviours.json` from the player's `client.jar`?
//!
//! Reference: `tmp/ship-extract/block-behaviours.json`. The two reflective `fields`
//! maps and `fire.shapes` are other tasks / a JVM identity string; the tests compare the
//! parts this module owns and pin the reported gaps.

#[path = "../src/behaviour_header.rs"]
mod behaviour_header;

use std::path::PathBuf;

use serde_json::Value;

fn workspace() -> PathBuf {
    // <workspace>/route3/crates/hzr-export
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference() -> Value {
    let path = workspace().join("tmp/ship-extract/block-behaviours.json");
    serde_json::from_slice(&std::fs::read(path).expect("block-behaviours.json")).expect("JSON")
}

#[test]
fn blocks_header_matches_the_java_export() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let mine = behaviour_header::blocks(&jar()).expect("blocks");
    let mine = mine.as_array().expect("array");
    let reference = reference();
    let expected = reference["blocks"].as_array().expect("array");
    assert_eq!(mine.len(), 1286, "block count");
    assert_eq!(mine.len(), expected.len());
    let mut failures = Vec::new();
    for (ours, theirs) in mine.iter().zip(expected.iter()) {
        let name = theirs["name"].as_str().unwrap_or("");
        for key in ["name", "class", "interfaces", "dynamic_shape", "item"] {
            if ours.get(key) != theirs.get(key) {
                failures.push(format!(
                    "{name}: {key} differs (ours {:?}, reference {:?})",
                    ours.get(key),
                    theirs.get(key)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn items_class_and_block_field_match_the_java_export() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let mine = behaviour_header::items(&jar()).expect("items");
    let mine = mine.as_object().expect("object");
    let reference = reference();
    let expected = reference["items"].as_object().expect("object");
    eprintln!("items ours {} reference {}", mine.len(), expected.len());
    let mut failures = Vec::new();
    let mut missing = Vec::new();
    for (id, theirs) in expected {
        let Some(ours) = mine.get(id) else {
            missing.push(id.clone());
            continue;
        };
        if ours.get("class") != theirs.get("class") {
            failures.push(format!(
                "{id}: class differs (ours {:?}, reference {:?})",
                ours.get("class"),
                theirs.get("class")
            ));
        }
        if ours["fields"].get("block") != theirs["fields"].get("block") {
            failures.push(format!(
                "{id}: block field differs (ours {:?}, reference {:?})",
                ours["fields"].get("block"),
                theirs["fields"].get("block")
            ));
        }
    }
    assert!(
        failures.is_empty() && missing.is_empty(),
        "{} class/block failures, {} missing (first: {:?})\n{}",
        failures.len(),
        missing.len(),
        &missing[..missing.len().min(12)],
        failures
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn fluids_match_the_registry_order() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let mine = behaviour_header::fluids(&jar()).expect("fluids");
    let expected = reference();
    assert_eq!(mine, expected["fluids"]);
    assert_eq!(mine.as_array().map(|list| list.len()), Some(5));
}

#[test]
fn fire_odds_and_damage_match_while_shapes_is_reported() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let mine = behaviour_header::fire(&jar()).expect("fire");
    let reference = reference();
    let theirs = &reference["fire"];
    assert_eq!(
        mine.get("igniteOdds"),
        theirs.get("igniteOdds"),
        "igniteOdds"
    );
    assert_eq!(mine.get("burnOdds"), theirs.get("burnOdds"), "burnOdds");
    assert_eq!(
        mine.get("fireDamage"),
        theirs.get("fireDamage"),
        "fireDamage"
    );
    assert!(
        !mine
            .as_object()
            .is_some_and(|object| object.contains_key("shapes")),
        "shapes must stay unreported rather than invented"
    );
    let (_, coverage) = behaviour_header::sections(&jar()).expect("sections");
    assert!(coverage
        .notes
        .iter()
        .any(|note| note.contains("fire.shapes")));
}

#[test]
fn sections_and_coverage_report_every_gap() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, coverage) = behaviour_header::sections(&jar()).expect("sections");
    let object = document.as_object().expect("object");
    for key in behaviour_header::SECTIONS {
        assert!(object.contains_key(key), "{key} missing");
    }
    assert_eq!(object.len(), 4);
    assert!(coverage.missing.iter().any(|line| line == "blocks.fields"));
    assert!(coverage
        .missing
        .iter()
        .any(|line| line.contains("items.fields")));
}

#[test]
fn write_puts_a_parsable_document_on_disk() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let out =
        std::env::temp_dir().join(format!("hzr-behaviour-header-{}.json", std::process::id()));
    let coverage = behaviour_header::write(&jar(), &out).expect("write");
    let text = std::fs::read_to_string(&out).expect("file");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(document.as_object().map(|object| object.len()), Some(4));
    assert_eq!(coverage.produced.len(), 4);
    let _ = std::fs::remove_file(out);
}
