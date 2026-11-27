//! Re-serialises the exact 27-field gameplay table in the bundle's canonical compact form.
//!
//! `gameplay_final_fields` patches the compact document in place, so its rows carry the right
//! values but the dictionaries keep the values that were replaced and the indices are no longer
//! the bundle's first-appearance order. This module decodes the patched table back into the
//! semantic `[state][field]` matrix and then re-compresses it exactly like
//! `gameplay::compact` does, which produces the canonical dictionaries, `per_block` rows and
//! `per_state` rows of `_shared/gameplay.compact.json`.
//!
//! Nothing here reads `ship.hzr3`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "gameplay_final_fields.rs"]
#[allow(dead_code)]
mod final_fields;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = final_fields::TABLE;

/// The 27 field names, in the bundle's order.
pub const FIELDS: [&str; 27] = final_fields::FIELDS;

/// What was produced, and what could not be.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
}

/// Builds the canonical whole document from the jar.
pub fn document(jar_path: &Path) -> Result<Value, String> {
    let patched = final_fields::document(jar_path)?;
    let facts = decode(&patched)?;
    Ok(compact(&facts))
}

/// Writes the canonical document below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let document = document(jar_path)?;
    let text = serde_json::to_string(&document).map_err(|error| error.to_string())?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Coverage {
        produced: vec![TABLE.to_string()],
    })
}

// ---------------------------------------------------------------------------
// The compact document
// ---------------------------------------------------------------------------

/// The `Facts` shape `gameplay::compact` compresses.
struct Facts {
    fields: Vec<String>,
    blocks: Vec<String>,
    values: Vec<Vec<Value>>,
    blocks_per_state: Vec<usize>,
}

fn strings(value: &Value, what: &str) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{what} is not an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{what} holds a non-string"))
        })
        .collect()
}

/// Turns the compact table back into the semantic `[state][field]` matrix.
fn decode(document: &Value) -> Result<Facts, String> {
    let fields = strings(&document["fields"], "fields")?;
    let blocks = strings(&document["blocks"], "blocks")?;
    let constant_fields = strings(&document["constant_fields"], "constant_fields")?;
    let varying_fields = strings(&document["varying_fields"], "varying_fields")?;
    let blocks_per_state = document["blocks_per_state"]
        .as_array()
        .ok_or("blocks_per_state is not an array")?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .map(|index| index as usize)
                .ok_or_else(|| "blocks_per_state holds a non-number".to_string())
        })
        .collect::<Result<Vec<usize>, String>>()?;
    let mut constant_slots: BTreeMap<&str, usize> = BTreeMap::new();
    for (slot, field) in constant_fields.iter().enumerate() {
        constant_slots.insert(field.as_str(), slot);
    }
    let mut varying_slots: BTreeMap<&str, usize> = BTreeMap::new();
    for (slot, field) in varying_fields.iter().enumerate() {
        varying_slots.insert(field.as_str(), slot);
    }
    // The state a block starts at (its first state), so constant rows can be found per block.
    let mut first_state: BTreeMap<usize, usize> = BTreeMap::new();
    for (state, block) in blocks_per_state.iter().enumerate() {
        first_state.entry(*block).or_insert(state);
    }
    let per_state = document["per_state"]
        .as_array()
        .ok_or("per_state is not an array")?;
    let dictionaries = document["dictionaries"]
        .as_object()
        .ok_or("dictionaries is not an object")?;
    let mut values = Vec::with_capacity(blocks_per_state.len());
    for (state, block_index) in blocks_per_state.iter().enumerate() {
        let mut row = Vec::with_capacity(fields.len());
        for field in &fields {
            let value = if let Some(slot) = constant_slots.get(field.as_str()) {
                let block = blocks
                    .get(*block_index)
                    .ok_or_else(|| format!("state {state} names block {block_index}"))?;
                let row = document["per_block"]
                    .get(block)
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("per_block has no {block}"))?;
                let index = row
                    .get(*slot)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("{block} slot {slot} is not a number"))?
                    as usize;
                dictionaries
                    .get(field)
                    .and_then(Value::as_array)
                    .and_then(|dictionary| dictionary.get(index))
                    .cloned()
                    .ok_or_else(|| format!("{field} has no entry {index}"))?
            } else if let Some(slot) = varying_slots.get(field.as_str()) {
                let index = per_state
                    .get(state)
                    .and_then(Value::as_array)
                    .and_then(|row| row.get(*slot))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("state {state} slot {slot} is not a number"))?
                    as usize;
                dictionaries
                    .get(field)
                    .and_then(Value::as_array)
                    .and_then(|dictionary| dictionary.get(index))
                    .cloned()
                    .ok_or_else(|| format!("{field} has no entry {index}"))?
            } else {
                return Err(format!("{field} is neither constant nor varying"));
            };
            row.push(value);
        }
        values.push(row);
    }
    let _ = first_state;
    Ok(Facts {
        fields,
        blocks,
        values,
        blocks_per_state,
    })
}

fn key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// `gameplay::compact`, verbatim in behaviour: dictionaries are in first-appearance order.
fn compact(facts: &Facts) -> Value {
    let fields = facts.fields.len();
    let mut constant = vec![true; fields];
    let mut first_seen: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for (state, block) in facts.blocks_per_state.iter().enumerate() {
        let block = facts.blocks[*block].as_str();
        let row = first_seen.entry(block).or_default();
        if row.is_empty() {
            row.extend(facts.values[state].iter());
        } else {
            for (field, value) in row.iter().enumerate() {
                if key(value) != key(&facts.values[state][field]) {
                    constant[field] = false;
                }
            }
        }
    }
    let mut blocks: Vec<String> = Vec::new();
    let mut position: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_state: BTreeMap<String, usize> = BTreeMap::new();
    for (state, block) in facts.blocks_per_state.iter().enumerate() {
        let name = &facts.blocks[*block];
        first_state.entry(name.clone()).or_insert(state);
        if !position.contains_key(name) {
            position.insert(name.clone(), blocks.len());
            blocks.push(name.clone());
        }
    }
    let mut dictionaries = Map::new();
    let mut index_of: Vec<BTreeMap<String, usize>> = Vec::with_capacity(fields);
    for field in 0..fields {
        let mut order: Vec<String> = Vec::new();
        let mut map: BTreeMap<String, usize> = BTreeMap::new();
        let mut walk = |value: &Value| {
            let text = key(value);
            if !map.contains_key(&text) {
                map.insert(text.clone(), order.len());
                order.push(text);
            }
        };
        if constant[field] {
            for block in &blocks {
                walk(&facts.values[first_state[block]][field]);
            }
        } else {
            for row in &facts.values {
                walk(&row[field]);
            }
        }
        let mut list = Vec::with_capacity(order.len());
        for text in &order {
            list.push(serde_json::from_str::<Value>(text).unwrap_or(Value::Null));
        }
        dictionaries.insert(facts.fields[field].clone(), Value::Array(list));
        index_of.push(map);
    }
    let constant_fields: Vec<usize> = (0..fields).filter(|field| constant[*field]).collect();
    let mut per_block = Map::new();
    for block in &blocks {
        let state = first_state[block];
        let mut row = Vec::with_capacity(constant_fields.len());
        for field in &constant_fields {
            row.push(Value::from(
                index_of[*field][&key(&facts.values[state][*field])] as u64,
            ));
        }
        per_block.insert(block.clone(), Value::Array(row));
    }
    let varying_fields: Vec<usize> = (0..fields).filter(|field| !constant[*field]).collect();
    let mut per_state = Vec::with_capacity(facts.values.len());
    for state in 0..facts.values.len() {
        let mut row = Vec::with_capacity(varying_fields.len());
        for field in &varying_fields {
            row.push(Value::from(
                index_of[*field][&key(&facts.values[state][*field])] as u64,
            ));
        }
        per_state.push(Value::Array(row));
    }
    let mut root = Map::new();
    root.insert(
        "fields".to_string(),
        Value::Array(
            facts
                .fields
                .iter()
                .map(|field| Value::String(field.clone()))
                .collect(),
        ),
    );
    root.insert(
        "constant_fields".to_string(),
        Value::Array(
            constant_fields
                .iter()
                .map(|field| Value::String(facts.fields[*field].clone()))
                .collect(),
        ),
    );
    root.insert(
        "varying_fields".to_string(),
        Value::Array(
            varying_fields
                .iter()
                .map(|field| Value::String(facts.fields[*field].clone()))
                .collect(),
        ),
    );
    root.insert("dictionaries".to_string(), Value::Object(dictionaries));
    root.insert(
        "blocks".to_string(),
        Value::Array(
            blocks
                .iter()
                .map(|block| Value::String(block.clone()))
                .collect(),
        ),
    );
    root.insert("per_block".to_string(), Value::Object(per_block));
    root.insert(
        "blocks_per_state".to_string(),
        Value::Array(
            facts
                .blocks_per_state
                .iter()
                .map(|block| Value::from(*block as u64))
                .collect(),
        ),
    );
    root.insert("per_state".to_string(), Value::Array(per_state));
    Value::Object(root)
}
