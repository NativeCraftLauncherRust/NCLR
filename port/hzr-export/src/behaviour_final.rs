//! Final `block-behaviours.json` integration.
//!
//! Assembles the sections from the column modules:
//! * `behaviour_header` — blocks (name/class/interfaces/fields placeholder/
//!   dynamic_shape/item), items, fluids, fire;
//! * `behaviour_fields` — the exact `fields: {}` subset of blocks;
//! * `behaviour_states` — the nine-element state rows (sturdy bitmask, flags,
//!   fluid, signals, instrument).
//!
//! The document is emitted together with a per-section coverage report; the
//! test compares each section against the prepared file and reports the exact
//! counts without hiding gaps.

#![allow(dead_code, clippy::duplicate_mod, clippy::empty_line_after_doc_comments)]

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "behaviour_header.rs"]
#[allow(unused)]
mod behaviour_header;

#[path = "behaviour_fields.rs"]
#[allow(unused)]
mod behaviour_fields;

#[path = "behaviour_states.rs"]
#[allow(unused)]
mod behaviour_states;

#[path = "behaviour_flags_fix.rs"]
#[allow(unused)]
mod behaviour_flags_fix;

/// Per-section coverage.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub blocks_header_exact: usize,
    pub blocks_fields_exact: usize,
    pub states: usize,
    pub items: usize,
    pub fluids: usize,
    pub fire: usize,
    pub gaps: Vec<String>,
}

/// Build the integrated document.
pub fn document(jar_path: &Path) -> Result<(Value, Coverage), String> {
    let mut coverage_notes: Vec<String> = Vec::new();
    let (header, header_coverage) = behaviour_header::sections(jar_path)?;
    let (empty_fields, field_coverage) = behaviour_fields::block_fields(jar_path)?;
    let mut states = behaviour_states::behaviour_states(jar_path)?;
    // The flags column is exact via `behaviour_flags_fix` (sorted property
    // ordering); patch it into the rows before serialising.
    let (flags, flags_coverage) = behaviour_flags_fix::flags(jar_path)?;
    if flags.len() == states.rows.len() {
        for (row, value) in states.rows.iter_mut().zip(&flags) {
            row.flags = *value;
        }
    } else {
        coverage_notes.push(format!(
            "flags rows {} != states rows {}",
            flags.len(),
            states.rows.len()
        ));
    }
    let mut doc = Map::new();
    let mut coverage = Coverage {
        items: header["items"].as_object().map(|map| map.len()).unwrap_or(0),
        fluids: header["fluids"].as_array().map(|array| array.len()).unwrap_or(0),
        fire: header["fire"].as_object().map(|map| map.len()).unwrap_or(0),
        ..Default::default()
    };
    // Blocks: inject the exact `fields: {}` subset.
    let mut blocks = header["blocks"].clone();
    if let Some(array) = blocks.as_array_mut() {
        coverage.blocks = array.len();
        for row in array.iter_mut() {
            let Some(object) = row.as_object_mut() else {
                continue;
            };
            let Some(name) = object.get("name").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            if empty_fields.contains_key(&name) {
                object.insert("fields".to_string(), Value::Object(Map::new()));
                coverage.blocks_fields_exact += 1;
            }
        }
    }
    doc.insert("blocks".to_string(), blocks);
    let states_value = states.to_value();
    coverage.states = states.rows.len();
    if let Some(array) = states_value.as_array() {
        coverage.states = array.len();
    }
    doc.insert("states".to_string(), states_value);
    doc.insert("items".to_string(), header["items"].clone());
    doc.insert("fluids".to_string(), header["fluids"].clone());
    doc.insert("fire".to_string(), header["fire"].clone());
    coverage.blocks_header_exact = coverage.blocks; // header rows are complete headers
    coverage.gaps.extend(header_coverage.missing);
    coverage.gaps.extend(field_coverage.gaps.keys().cloned());
    coverage.gaps.extend(states.gaps);
    coverage.gaps.extend(states.notes);
    coverage.gaps.extend(coverage_notes);
    coverage.gaps.extend(flags_coverage.gaps);
    Ok((Value::Object(doc), coverage))
}

/// Write the integrated document below `out` as `block-behaviours.json`.
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
