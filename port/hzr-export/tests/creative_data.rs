//! Semantic tests for [`creative_data`], the jar-driven port of the Java
//! `VanillaCreativeTabsExporter`.
//!
//! The port reproduces tab metadata for every creative tab, and each tab's
//! item list only when its `CreativeModeTabs` generator uses constructs this
//! evaluator models. The tests check:
//! * every tab's metadata (id/type/row/column/title/icon/visible) exactly;
//! * every fully generated tab (whole `items` array) exactly;
//! * that the tabs known to be reproducible are reported as complete, and the
//!   reasons the rest are incomplete are surfaced (printed when `--nocapture`).
//!
//! Needs `mc263/client.jar` and `tmp/ship-extract/creative-inventory.json`; the
//! tests skip when the jar is absent (player files are not committed).

#[path = "../src/creative_data.rs"]
mod creative_data;

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

fn reference() -> PathBuf {
    workspace_root()
        .join("tmp")
        .join("ship-extract")
        .join("creative-inventory.json")
}

fn load_expected() -> Value {
    let path = reference();
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn tab_metadata(tab: &creative_data::Tab) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), Value::String(tab.id.clone()));
    object.insert("type".into(), Value::String(tab.type_name.clone()));
    object.insert("row".into(), Value::String(tab.row.clone()));
    object.insert("column".into(), Value::from(tab.column));
    object.insert("title".into(), Value::String(tab.title.clone()));
    object.insert("icon".into(), Value::String(tab.icon.clone()));
    object.insert("visible".into(), Value::Bool(tab.visible));
    Value::Object(object)
}

#[test]
fn every_tab_metadata_matches_java_export() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let export = creative_data::generate(jar()).expect("generate creative inventory");
    assert_eq!(export.tabs.len(), 14, "creative tab count");

    let expected = load_expected();
    let expected_tabs = expected["tabs"].as_array().expect("tabs array");
    assert_eq!(expected_tabs.len(), 14);

    for expected_tab in expected_tabs {
        let id = expected_tab["id"].as_str().expect("tab id");
        let ours = export
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .unwrap_or_else(|| panic!("missing tab {id}"));
        for key in ["id", "type", "row", "column", "title", "icon", "visible"] {
            assert_eq!(
                tab_metadata(ours).get(key),
                expected_tab.get(key),
                "{id}: metadata field {key}"
            );
        }
    }
}

#[test]
fn complete_tabs_match_java_export_exactly() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let export = creative_data::generate(jar()).expect("generate creative inventory");
    let expected = load_expected();
    let expected_tabs = expected["tabs"].as_array().expect("tabs array");

    for gap in &export.gaps {
        eprintln!("gap: {gap}");
    }
    let complete: Vec<&creative_data::Tab> = export.complete();
    let complete_ids: Vec<&str> = complete.iter().map(|tab| tab.id.as_str()).collect();
    eprintln!("complete tabs ({}): {:?}", complete.len(), complete_ids);
    assert_eq!(
        complete.len() + export.gaps.len(),
        export.tabs.len(),
        "every tab is either complete or carries a gap"
    );
    for id in [
        "minecraft:building_blocks",
        "minecraft:colored_blocks",
        "minecraft:natural_blocks",
        "minecraft:redstone_blocks",
        "minecraft:spawn_eggs",
    ] {
        assert!(
            complete_ids.contains(&id),
            "{id} should be reproducible, complete were {complete_ids:?}"
        );
    }

    for tab in complete {
        let expected_tab = expected_tabs
            .iter()
            .find(|other| other["id"] == tab.id.as_str())
            .unwrap_or_else(|| panic!("no expected tab {}", tab.id));
        let mut object = match tab_metadata(tab) {
            Value::Object(object) => object,
            _ => unreachable!(),
        };
        object.insert(
            "items".into(),
            Value::Array(
                tab.items
                    .clone()
                    .expect("complete tab has items")
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        assert_eq!(
            Value::Object(object),
            *expected_tab,
            "{}: complete tab differs from the Java export",
            tab.id
        );
    }
}

#[test]
fn writes_subset_for_external_comparison() {
    if !jar().exists() {
        eprintln!(
            "skipping: {} not present (player files are not committed)",
            jar().display()
        );
        return;
    }
    let export = creative_data::generate(jar()).expect("generate creative inventory");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("creative-parity");
    fs::create_dir_all(&dir).expect("create output dir");
    let text = serde_json::to_string(&export.to_value()).expect("serialize subset");
    fs::write(dir.join("creative-inventory.json"), text + "\n").expect("write subset");
    let metadata = serde_json::to_string(&export.metadata_value()).expect("serialize metadata");
    fs::write(dir.join("metadata.json"), metadata + "\n").expect("write metadata");
}
