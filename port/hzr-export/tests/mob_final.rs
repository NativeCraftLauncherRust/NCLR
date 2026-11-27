//! Validate the final `mob-data.json` export.
//!
//! Three gates in one pass (the export is expensive, so it runs once):
//! * every `states` row equals the Java exporter's row exactly;
//! * the whole `Value` is semantically equal to the reference file;
//! * the written file is byte equal (Java's Gson order and shortest float
//!   text), with a first-difference report otherwise.

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/mob_final.rs"]
mod mob_final;

use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn reference_text() -> String {
    let path = workspace().join("tmp/ship-extract/mob-data.json");
    std::fs::read_to_string(&path).expect("mob-data.json")
}

#[test]
fn whole_mob_data_matches_java_export() {
    let text = reference_text();
    let reference: serde_json::Value = serde_json::from_str(&text).expect("json");
    let data = mob_final::export_mob_data(&jar()).expect("export");
    let ours = data.to_value();

    // 1. every state row
    let java_states = reference
        .get("states")
        .and_then(|value| value.as_array())
        .expect("states");
    let our_states = ours
        .get("states")
        .and_then(|value| value.as_array())
        .expect("states");
    assert_eq!(our_states.len(), java_states.len(), "row count");
    let mut row_bad = Vec::new();
    let mut path_bad = 0usize;
    let mut flags_bad = 0usize;
    let mut height_bad = 0usize;
    for (index, (our, java)) in our_states.iter().zip(java_states.iter()).enumerate() {
        let our = our.as_array().expect("row");
        let java = java.as_array().expect("row");
        if our[0] != java[0] {
            path_bad += 1;
        }
        if our[1] != java[1] {
            flags_bad += 1;
        }
        if our[2] != java[2] {
            height_bad += 1;
        }
        if our != java {
            if row_bad.len() < 20 {
                row_bad.push(format!("#{index}: ours={our:?} java={java:?}"));
            }
        }
    }
    assert_eq!(
        (path_bad, flags_bad, height_bad),
        (0, 0, 0),
        "state column mismatches: {:?}",
        row_bad
    );

    // 2. the whole table, semantically
    assert_eq!(ours, reference, "whole table semantic mismatch");

    // 3. the whole file, byte for byte
    let written = data.to_json().expect("json");
    if written != text {
        let mut at = 0usize;
        let ours_bytes = written.as_bytes();
        let java_bytes = text.as_bytes();
        while at < ours_bytes.len() && at < java_bytes.len() && ours_bytes[at] == java_bytes[at] {
            at += 1;
        }
        let start = at.saturating_sub(120);
        let end = (at + 160).min(ours_bytes.len().min(java_bytes.len()));
        let ours_window = written.get(start..end).unwrap_or_default();
        let java_window = text.get(start..end).unwrap_or_default();
        eprintln!("byte mismatch at {at}");
        eprintln!("ours: {ours_window:?}");
        eprintln!("java: {java_window:?}");
    }
    assert_eq!(written.len(), text.len(), "file length");
    assert!(written == text, "file bytes");
}

