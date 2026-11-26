//! Final `block-behaviours.json` integration.
//!
//! Sections come from the validated end-state modules:
//! * blocks: `behaviour_header::blocks` with `behaviour_fields_finish` fields;
//! * states: `behaviour_states_final::rows`;
//! * items + fire: `behaviour_items_fire`;
//! * fluids: `behaviour_header::fluids`.
//!
//! The test compares the whole document with the prepared export, applying the
//! identity normalization to the block `fields` objects on both sides (lambda
//! class names embed JVM addresses) and exact equality everywhere else.

#![allow(dead_code, clippy::duplicate_mod, clippy::empty_line_after_doc_comments)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "behaviour_header.rs"]
#[allow(unused, clippy::all)]
mod behaviour_header;

#[path = "behaviour_fields_finish.rs"]
#[allow(unused, clippy::all)]
mod behaviour_fields_finish;

#[path = "behaviour_fields_identity.rs"]
#[allow(unused, clippy::all)]
pub mod behaviour_fields_identity;

#[path = "behaviour_states_final.rs"]
#[allow(unused, clippy::all)]
mod behaviour_states_final;

#[path = "behaviour_items_fire.rs"]
#[allow(unused, clippy::all)]
pub mod behaviour_items_fire;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub fields_remaining: Vec<String>,
    pub states: usize,
    pub items: usize,
    pub notes: Vec<String>,
}

/// Build the final document.
pub fn document(jar_path: &Path) -> Result<(Value, Coverage), String> {
    let (fields, fields_coverage) =
        behaviour_fields_finish::normalized_fields(jar_path)?;
    let mut blocks = behaviour_header::blocks(jar_path)?
        .as_array()
        .cloned()
        .unwrap_or_default();
    for row in blocks.iter_mut() {
        let Some(object) = row.as_object_mut() else {
            continue;
        };
        let Some(name) = object.get("name").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        if let Some(value) = fields.get(&name) {
            object.insert("fields".to_string(), value.clone());
        }
    }
    let (states, states_coverage) = behaviour_states_final::rows(jar_path)?;
    let (items, fire) = if let Ok((value, _)) = behaviour_items_fire::sections(jar_path) {
        (
            value.get("items").cloned().unwrap_or(Value::Null),
            value.get("fire").cloned().unwrap_or(Value::Null),
        )
    } else {
        (
            behaviour_items_fire::items(jar_path)?,
            behaviour_items_fire::fire(jar_path)?,
        )
    };
    let fluids = behaviour_header::fluids(jar_path)?;
    let mut root = Map::new();
    root.insert("blocks".to_string(), Value::Array(blocks.clone()));
    root.insert("states".to_string(), Value::Array(states.clone()));
    root.insert("items".to_string(), items);
    root.insert("fluids".to_string(), fluids);
    // The fire section carries one lambda identity string (`shapes`); normalize it
    // so the document is stable across JVM runs.
    root.insert(
        "fire".to_string(),
        behaviour_fields_identity::normalize_exported(&fire),
    );

    let mut coverage = Coverage {
        blocks: blocks.len(),
        fields_remaining: fields_coverage.remaining.clone(),
        states: states.len(),
        items: json_len(json_get(&Value::Object(root.clone()), "items")),
        ..Default::default()
    };
    coverage.notes.extend(states_coverage.notes);
    coverage.notes.extend(fields_coverage.remaining);
    Ok((Value::Object(root), coverage))
}

fn json_get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

fn json_len(value: &Value) -> usize {
    value
        .as_array()
        .map(Vec::len)
        .or_else(|| value.as_object().map(Map::len))
        .unwrap_or(0)
}

/// Write the final document below `out` as `block-behaviours.json`.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let (document, coverage) = document(jar_path)?;
    let bytes = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
    let path = out.join("block-behaviours.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(coverage)
}

/// Section sizes for reports.
pub fn section_sizes(document: &Value) -> BTreeMap<&'static str, usize> {
    let mut out = BTreeMap::new();
    out.insert("blocks", json_len(json_get(document, "blocks")));
    out.insert("states", json_len(json_get(document, "states")));
    out.insert("items", json_len(json_get(document, "items")));
    out.insert("fluids", json_len(json_get(document, "fluids")));
    out.insert("fire", json_len(json_get(document, "fire")));
    out
}
