//! Validates normalized identity fields against the prepared export.

#[path = "../src/behaviour_fields_identity.rs"]
mod behaviour_fields_identity;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

#[test]
fn identity_gaps_match_after_normalization() {
    let jar = jar();
    if !jar.exists() || !workspace().join("tmp/ship-extract").exists() {
        eprintln!("skipped: needs the jar and the prepared copy");
        return;
    }
    let (fields, coverage) =
        behaviour_fields_identity::normalized_fields(&jar).expect("normalized fields");
    let document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace().join("tmp/ship-extract/block-behaviours.json")).expect("table"),
    )
    .expect("JSON");
    let prepared: std::collections::BTreeMap<&str, serde_json::Value> = document["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .filter_map(|row| row["name"].as_str().map(|name| (name, row["fields"].clone())))
        .collect();
    let mut mismatches = Vec::new();
    let mut compared = 0usize;
    let mut normalized_blocks = 0usize;
    for (id, mine) in &fields {
        let Some(theirs) = prepared.get(id.as_str()) else {
            continue;
        };
        let mine = behaviour_fields_identity::normalize_exported(&serde_json::Value::Object(
            mine.as_object().cloned().unwrap_or_default(),
        ));
        let theirs = behaviour_fields_identity::normalize_exported(theirs);
        if mine != theirs && mismatches.len() < 10 {
            mismatches.push(format!("{id}: ours {mine:?} theirs {theirs:?}"));
        }
        compared += 1;
        normalized_blocks += 1;
    }
    // Count the prepared export's raw identity strings: the JVM lambda names
    // embed run-specific addresses and are irreducible as literals.
    let mut identity_values = 0usize;
    let mut lambda_values = 0usize;
    let mut distinct = std::collections::BTreeSet::new();
    for fields in prepared.values() {
        if let Some(object) = fields.as_object() {
            for value in object.values() {
                if let Some(text) = value.as_str() {
                    if text.starts_with('<') {
                        identity_values += 1;
                        if text.contains("$$Lambda/0x") {
                            lambda_values += 1;
                        }
                        distinct.insert(text.to_string());
                    }
                }
            }
        }
    }
    eprintln!(
        "fields: {compared} blocks compared (exact {} + identity gaps {}); non-identity/constructor gaps {}; tokens {:?}; raw identity values {} ({} lambda-address strings, {} distinct)",
        coverage.exact,
        coverage.identity_gaps,
        coverage.non_identity_gaps.len(),
        coverage.tokens,
        identity_values,
        lambda_values,
        distinct.len()
    );

    for mismatch in &mismatches {
        eprintln!("  {mismatch}");
    }
    if !coverage.non_identity_gaps.is_empty() {
        eprintln!("non-identity gaps: {:?}", coverage.non_identity_gaps);
    }
    assert_eq!(
        mismatches.len(),
        0,
        "{} normalized blocks differ",
        mismatches.len()
    );
    let _ = normalized_blocks;
}
