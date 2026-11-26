//! Complete `block-behaviours.json` integration.
//!
//! Builds on `behaviour_final` (header blocks/items/fluids/fire + state rows with
//! the exact flags column) and replaces every block's `fields` object with the
//! normalized/finished derivation from `behaviour_fields_finish` (records,
//! static Maps, constructor values, identity tokens). The result is the table
//! compared after identity normalization in the test.

#![allow(dead_code, clippy::duplicate_mod, clippy::empty_line_after_doc_comments)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "behaviour_final.rs"]
#[allow(unused, clippy::all)]
mod behaviour_final;

#[path = "behaviour_fields_finish.rs"]
#[allow(unused, clippy::all)]
mod behaviour_fields_finish;

#[path = "behaviour_fields_identity.rs"]
#[allow(unused, clippy::all)]
pub mod behaviour_fields_identity;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub blocks_fields_derived: usize,
    pub blocks_fields_remaining: Vec<String>,
    pub states: usize,
    pub items: usize,
    pub notes: Vec<String>,
}

/// Build the complete document.
pub fn document(jar_path: &Path) -> Result<(Value, Coverage), String> {
    let (document, final_coverage) = behaviour_final::document(jar_path)?;
    let (fields, fields_coverage) =
        behaviour_fields_finish::normalized_fields(jar_path)?;
    let mut coverage = Coverage {
        blocks: fields.len(),
        blocks_fields_derived: fields.len(),
        blocks_fields_remaining: fields_coverage.remaining.clone(),
        states: document["states"].as_array().map(|array| array.len()).unwrap_or(0),
        items: document["items"].as_object().map(|map| map.len()).unwrap_or(0),
        ..Default::default()
    };
    let mut blocks = document["blocks"].as_array().cloned().unwrap_or_default();
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
    let mut root = Map::new();
    root.insert("blocks".to_string(), Value::Array(blocks));
    root.insert("states".to_string(), document["states"].clone());
    root.insert("items".to_string(), document["items"].clone());
    root.insert("fluids".to_string(), document["fluids"].clone());
    root.insert("fire".to_string(), document["fire"].clone());
    coverage.notes.extend(final_coverage.gaps);
    coverage.notes.extend(fields_coverage.remaining);
    Ok((Value::Object(root), coverage))
}

/// Write the complete document below `out` as `block-behaviours.json`.
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

/// Sector counts used by the test: `(blocks, states, items, fluids, fire)`.
pub fn section_sizes(document: &Value) -> BTreeMap<&'static str, usize> {
    let mut out = BTreeMap::new();
    out.insert("blocks", document["blocks"].as_array().map(Vec::len).unwrap_or(0));
    out.insert("states", document["states"].as_array().map(Vec::len).unwrap_or(0));
    out.insert("items", document["items"].as_object().map(Map::len).unwrap_or(0));
    out.insert("fluids", document["fluids"].as_array().map(Vec::len).unwrap_or(0));
    out.insert("fire", document["fire"].as_object().map(Map::len).unwrap_or(0));
    out
}
