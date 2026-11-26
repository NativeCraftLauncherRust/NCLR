//! Validation of `behaviour_items_fire`: all 1,141 item field maps exact and the
//! fire section exact after normalizing the JVM lambda identity string.

#[path = "../src/behaviour_items_fire.rs"]
#[allow(unused, clippy::duplicate_mod)]
mod behaviour_items_fire;

use std::path::PathBuf;

use serde_json::Value;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference() -> Value {
    serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json"))
            .expect("block-behaviours.json"),
    )
    .expect("JSON")
}

fn normalize_lambda(value: &Value) -> Value {
    match value {
        Value::String(text) if text.contains("$$Lambda/0x") => {
            Value::String("<Lambda>".to_string())
        }
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), normalize_lambda(value)))
                .collect(),
        ),
        Value::Array(array) => Value::Array(array.iter().map(normalize_lambda).collect()),
        other => other.clone(),
    }
}

#[test]
fn every_item_field_map_is_exact() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let mine = behaviour_items_fire::items(&jar()).expect("items");
    let mine = mine.as_object().expect("object");
    let reference = reference();
    let expected = reference["items"].as_object().expect("items");
    assert_eq!(mine.len(), 1141, "item count");
    assert_eq!(mine.len(), expected.len());
    let mut mismatches = Vec::new();
    for (id, want) in expected {
        match mine.get(id) {
            Some(got) if got == want => {}
            Some(got) => {
                if mismatches.len() < 10 {
                    mismatches.push(format!(
                        "{id}: ours {} want {}",
                        serde_json::to_string(got).unwrap_or_default(),
                        serde_json::to_string(want).unwrap_or_default()
                    ));
                }
            }
            None => mismatches.push(format!("{id}: missing")),
        }
    }
    for sample in &mismatches {
        eprintln!("  {sample}");
    }
    assert!(
        mismatches.is_empty(),
        "{} item rows differ",
        mismatches.len()
    );
}

#[test]
fn fire_section_matches_after_lambda_normalization() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let mine = behaviour_items_fire::fire(&jar()).expect("fire");
    let reference = reference();
    assert_eq!(
        normalize_lambda(&mine),
        normalize_lambda(&reference["fire"]),
        "fire section"
    );
    let object = mine.as_object().expect("object");
    assert_eq!(object.len(), 4);
    assert_eq!(object["fireDamage"], reference["fire"]["fireDamage"]);
    assert_eq!(object["igniteOdds"], reference["fire"]["igniteOdds"]);
    assert_eq!(object["burnOdds"], reference["fire"]["burnOdds"]);
    assert!(object["shapes"]
        .as_str()
        .is_some_and(|text| text.contains("$$Lambda/0x") || text == "<Lambda>"));
}

#[test]
fn coverage_reports_both_sections_exact() {
    if !jar().exists() {
        eprintln!("skipped: needs {}", jar().display());
        return;
    }
    let (document, coverage) = behaviour_items_fire::sections(&jar()).expect("sections");
    assert_eq!(coverage.items, 1141);
    assert_eq!(coverage.items_exact, 1141);
    assert_eq!(coverage.fire_fields, 4);
    assert!(coverage.gaps.is_empty(), "{:?}", coverage.gaps);
    assert!(document.get("items").is_some());
    assert!(document.get("fire").is_some());
}
