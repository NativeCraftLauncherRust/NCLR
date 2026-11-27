//! Tests for [`creative_search`]: the complete 14-tab creative inventory.
//!
//! `ItemStack.tags()` iteration order is JVM identity-hash dependent and not
//! reproducible from the jar, so both sides are normalized by sorting each
//! stack's tag list before comparison; everything else (items, tooltip lines,
//! metadata, permissions_only) is compared exactly.
//!
//! Needs `mc263/client.jar` and the Java export; skips without the jar.

#[path = "../src/creative_search.rs"]
mod creative_search;

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

fn sorted_tags(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|entry| match entry {
                    Value::Array(tags) => {
                        let mut tags: Vec<Value> = tags.clone();
                        tags.sort_by_key(|tag| tag.as_str().unwrap_or("").to_owned());
                        Value::Array(tags)
                    }
                    other => other.clone(),
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn metadata(tab: &creative_search::Tab) -> Map<String, Value> {
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

fn first_array_diff(got: &[Value], want: &[Value], path: &str) -> Option<String> {
    if got.len() != want.len() {
        return Some(format!("{path}: length {} != {}", got.len(), want.len()));
    }
    for (index, (got, want)) in got.iter().zip(want).enumerate() {
        if got != want {
            return Some(format!("{path}[{index}]: {got:?} != {want:?}"));
        }
    }
    None
}

#[test]
fn all_fourteen_tabs_match_java_export() {
    if !jar().exists() {
        eprintln!("skipping: {} not present", jar().display());
        return;
    }
    let export = creative_search::generate_all(jar()).expect("generate creative inventory");
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
        let want_items = expected_tab["items"].as_array().expect("items");
        let got_items: Vec<Value> = ours
            .items
            .as_ref()
            .expect("items")
            .iter()
            .cloned()
            .map(Value::String)
            .collect();
        if let Some(diff) = first_array_diff(&got_items, want_items, &format!("{id}/items")) {
            panic!("{diff}");
        }
        if let Some(want_lines) = expected_tab.get("lines").and_then(Value::as_array) {
            let got_lines: Vec<Value> = ours
                .lines
                .as_ref()
                .expect("lines")
                .iter()
                .map(|line| Value::Array(line.iter().cloned().map(Value::String).collect()))
                .collect();
            if let Some(diff) = first_array_diff(&got_lines, want_lines, &format!("{id}/lines")) {
                panic!("{diff}");
            }
        }
        if let Some(want_tags) = expected_tab.get("tags").and_then(Value::as_array) {
            let got_tags: Vec<Value> = ours
                .tags
                .as_ref()
                .expect("tags")
                .iter()
                .map(|tags| {
                    let mut tags = tags.clone();
                    tags.sort();
                    Value::Array(tags.into_iter().map(Value::String).collect())
                })
                .collect();
            let want_tags = sorted_tags(&Value::Array(want_tags.clone()));
            let want_tags = want_tags.as_array().expect("tags");
            if let Some(diff) = first_array_diff(&got_tags, want_tags, &format!("{id}/tags")) {
                panic!("{diff}");
            }
        }
        if let Some(want) = expected_tab
            .get("permissions_only")
            .and_then(Value::as_array)
        {
            let got: Vec<Value> = ours
                .permissions_only
                .as_ref()
                .expect("permissions_only")
                .iter()
                .copied()
                .map(Value::from)
                .collect();
            assert_eq!(&got, want, "{id}: permissions_only");
        }
    }
}

#[test]
fn writes_table_for_external_comparison() {
    if !jar().exists() {
        eprintln!("skipping: {} not present", jar().display());
        return;
    }
    let export = creative_search::generate_all(jar()).expect("generate creative inventory");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("creative-search");
    fs::create_dir_all(&dir).expect("create dir");
    fs::write(
        dir.join("creative-inventory.json"),
        serde_json::to_string(&export.to_value()).expect("serialize") + "\n",
    )
    .expect("write");
}
