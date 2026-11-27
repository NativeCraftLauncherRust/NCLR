//! Tests for [`creative_final`]: the full creative inventory from the player's
//! jar. Complete tabs are compared stack-for-stack with the Java export;
//! remaining gaps are reported.
//!
//! Needs `mc263/client.jar` and the Java export; skips without the jar.

#[path = "../src/creative_final.rs"]
mod creative_final;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crate lives at <root>/route3/crates/hzr-export")
        .to_path_buf()
}

fn jar() -> PathBuf {
    workspace_root().join("mc263").join("client.jar")
}

fn expected() -> Value {
    let path = workspace_root()
        .join("tmp")
        .join("ship-extract")
        .join("creative-inventory.json");
    serde_json::from_str(&fs::read_to_string(&path).expect("read creative export"))
        .expect("parse creative export")
}

fn metadata(tab: &creative_final::Tab) -> Map<String, Value> {
    let mut object = Map::new();
    object.insert("id".into(), Value::String(tab.id.clone()));
    object.insert("type".into(), Value::String(tab.type_name.clone()));
    object.insert("row".into(), Value::String(tab.row.clone()));
    object.insert("column".into(), Value::from(tab.column));
    object.insert("title".into(), Value::String(tab.title.clone()));
    object.insert("icon".into(), Value::String(tab.icon.clone()));
    object.insert("visible".into(), Value::Bool(tab.visible));
    object
}

#[test]
fn complete_tabs_match_java_export() {
    if !jar().exists() {
        eprintln!("skipping: {} not present", jar().display());
        return;
    }
    let export = creative_final::generate_all(jar()).expect("generate creative inventory");
    assert_eq!(export.tabs.len(), 14);
    let expected = expected();
    let expected_tabs = expected["tabs"].as_array().expect("tabs array");

    for expected_tab in expected_tabs {
        let id = expected_tab["id"].as_str().expect("id");
        let ours = export
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .unwrap_or_else(|| panic!("missing tab {id}"));
        for key in ["id", "type", "row", "column", "title", "icon", "visible"] {
            assert_eq!(
                metadata(ours).get(key),
                expected_tab.get(key),
                "{id}: metadata {key}"
            );
        }
    }

    for gap in &export.gaps {
        eprintln!("gap: {gap}");
    }
    let complete: Vec<&creative_final::Tab> = export.complete();
    eprintln!(
        "complete ({}/14): {:?}",
        complete.len(),
        complete
            .iter()
            .map(|tab| tab.id.as_str())
            .collect::<Vec<_>>()
    );
    for tab in &complete {
        let expected_tab = expected_tabs
            .iter()
            .find(|other| other["id"] == tab.id.as_str())
            .unwrap_or_else(|| panic!("no expected {}", tab.id));
        let want_items = expected_tab["items"].as_array().expect("items");
        let got_items = tab.items.as_ref().expect("complete tab has items");
        let first = got_items
            .iter()
            .zip(want_items)
            .position(|(a, b)| Value::String(a.clone()) != *b);
        if first.is_some() || got_items.len() != want_items.len() {
            let detail = first.map(|index| {
                format!(
                    "first stack diff at {index}: got {:?}, want {:?}",
                    got_items.get(index),
                    want_items.get(index).map(|v| v.as_str().unwrap_or(""))
                )
            });
            assert_eq!(
                got_items.len(),
                want_items.len(),
                "{}: stack count differs ({detail:?})",
                tab.id
            );
            panic!("{}: stack list differs ({detail:?})", tab.id);
        }
        if let Some(indices) = &tab.permissions_only {
            let want = expected_tab["permissions_only"]
                .as_array()
                .expect("permissions_only");
            let got: Vec<Value> = indices.iter().copied().map(Value::from).collect();
            assert_eq!(got, *want, "{}: permissions_only", tab.id);
        }
    }
}

#[test]
fn writes_full_table_for_external_comparison() {
    if !jar().exists() {
        eprintln!("skipping: {} not present", jar().display());
        return;
    }
    let export = creative_final::generate_all(jar()).expect("generate creative inventory");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("creative-final");
    fs::create_dir_all(&dir).expect("create dir");
    fs::write(
        dir.join("creative-inventory.json"),
        serde_json::to_string(&export.to_value()).expect("serialize") + "\n",
    )
    .expect("write");
}
