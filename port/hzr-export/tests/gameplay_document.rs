//! Does `gameplay_document` reproduce the whole `_shared/gameplay.compact.json`?

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "../src/gameplay_document.rs"]
mod gameplay_document;

use std::path::{Path, PathBuf};

use serde_json::Value;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn jar() -> PathBuf {
    workspace().join("mc263/client.jar")
}

fn prepared() -> PathBuf {
    workspace().join("tmp/ship-extract/_shared/gameplay.compact.json")
}

fn reference() -> Option<Value> {
    if !jar().is_file() || !prepared().is_file() {
        eprintln!("skipping: the jar or the prepared table is not there");
        return None;
    }
    let text = std::fs::read_to_string(prepared()).expect("the prepared table");
    Some(serde_json::from_str(&text).expect("the prepared table is JSON"))
}

/// First differing JSON path, so a mismatch is diagnosable.
fn difference(actual: &Value, expected: &Value, path: &str, out: &mut Vec<String>) {
    match (actual, expected) {
        (Value::Object(left), Value::Object(right)) => {
            let mut keys: Vec<&String> = left.keys().chain(right.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let next = format!("{path}.{key}");
                match (left.get(key), right.get(key)) {
                    (Some(a), Some(b)) => difference(a, b, &next, out),
                    (Some(_), None) => out.push(format!("{next}: only in actual")),
                    (None, Some(_)) => out.push(format!("{next}: only in expected")),
                    (None, None) => {}
                }
                if out.len() >= 20 {
                    return;
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                out.push(format!(
                    "{path}: array length {} != {}",
                    left.len(),
                    right.len()
                ));
                return;
            }
            for (index, (a, b)) in left.iter().zip(right.iter()).enumerate() {
                difference(a, b, &format!("{path}[{index}]"), out);
                if out.len() >= 20 {
                    return;
                }
            }
        }
        (a, b) if a != b => {
            out.push(format!("{path}: {a} != {b}"));
        }
        _ => {}
    }
}

#[test]
fn the_whole_document_matches_the_prepared_copy() {
    let Some(reference) = reference() else {
        return;
    };
    let document = gameplay_document::document(&jar()).expect("the document builds");
    if document != reference {
        let mut differences = Vec::new();
        difference(&document, &reference, "$", &mut differences);
        let report = differences.join("\n");
        let out = workspace().join("tmp/gp4/document_report.txt");
        std::fs::write(&out, &report).expect("report written");
        panic!("the whole document differs:\n{report}");
    }
}

#[test]
fn write_produces_the_document() {
    if reference().is_none() {
        return;
    }
    let out = std::env::temp_dir().join(format!(
        "hzr-export-gameplay-document-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    gameplay_document::write(&jar(), &out).expect("the document is written");
    let text = std::fs::read_to_string(out.join(gameplay_document::TABLE)).expect("the file");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(
        document["blocks"].as_array().map(Vec::len),
        Some(1286),
        "every block"
    );
    assert_eq!(
        document["fields"].as_array().map(Vec::len),
        Some(27),
        "every field"
    );
    let Some(reference) = reference() else { return };
    assert_eq!(document, reference, "the written document matches the prepared copy");
    let _ = std::fs::remove_dir_all(&out);
}
