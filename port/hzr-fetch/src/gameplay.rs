//! `gameplay-facts.json` re-encoded losslessly.
//!
//! The exporter writes 35,723 states, each with 27 fields. Measured on a real
//! file (`route3/AUDIT.md`):
//!
//! - 16 of the 27 fields never vary inside a block (hardness, map colour, sounds,
//!   tool requirement, ...): they are per-block values repeated for every state;
//! - the other 11 do vary, but over tiny domains — `emission` has 16 values,
//!   `dampening` 3 — except the geometry: `outline` and `outline_edges` have
//!   **866 distinct values across 35,723 states**.
//!
//! So the file is almost entirely repetition. This module stores one dictionary
//! per field, the constant fields once per block, and the varying ones as an
//! index per state: the same information, without the repeats.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// One field's values, per state, with the block each state belongs to.
#[derive(Debug)]
pub struct Source {
    pub fields: Vec<String>,
    /// `[state][field]`, in the source's field order.
    pub values: Vec<Vec<Value>>,
    /// The block of each state, in state-id order.
    pub blocks: Vec<String>,
}

/// Read a `gameplay-facts.json` against the block of each state.
pub fn read(gameplay: &[u8], block_of_state: &[String]) -> Result<Source, String> {
    let root: Value =
        serde_json::from_slice(gameplay).map_err(|e| format!("gameplay-facts.json: {e}"))?;
    let states = root
        .get("states")
        .and_then(Value::as_array)
        .ok_or("gameplay-facts.json has no states array")?;
    if states.len() != block_of_state.len() {
        return Err(format!(
            "gameplay-facts.json has {} states, the block order {} — not the same version",
            states.len(),
            block_of_state.len()
        ));
    }
    let mut fields: Vec<String> = Vec::new();
    for (index, state) in states.iter().enumerate() {
        let object = state
            .as_object()
            .ok_or_else(|| format!("state {index} is not an object"))?;
        if fields.is_empty() {
            fields = object.keys().filter(|key| *key != "id").cloned().collect();
            fields.sort();
            if fields.is_empty() {
                return Err("state 0 has no fields".into());
            }
        }
        // Every state must carry every field, or the encoding would silently
        // drop what one of them has.
        for field in &fields {
            if !object.contains_key(field) {
                return Err(format!("state {index} has no {field}"));
            }
        }
    }
    let mut values = Vec::with_capacity(states.len());
    for state in states {
        let object = state
            .as_object()
            .ok_or("state is not an object, though the first pass said it was")?;
        values.push(
            fields
                .iter()
                .map(|field| object.get(field).cloned().unwrap_or(Value::Null))
                .collect(),
        );
    }
    Ok(Source {
        fields,
        values,
        blocks: block_of_state.to_vec(),
    })
}

fn key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// The compact document: dictionaries, per-block constants, per-state indices.
pub fn compress(source: &Source) -> Result<Value, String> {
    // Which fields never vary inside a block.
    let mut constant = vec![true; source.fields.len()];
    let mut seen: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for (index, block) in source.blocks.iter().enumerate() {
        let row = seen.entry(block.as_str()).or_default();
        if row.is_empty() {
            for field in 0..source.fields.len() {
                row.push(&source.values[index][field]);
            }
        } else {
            for field in 0..source.fields.len() {
                if key(row[field]) != key(&source.values[index][field]) {
                    constant[field] = false;
                }
            }
        }
    }

    // The block order, by first appearance, and each block's first state.
    let mut blocks: Vec<String> = Vec::new();
    let mut position: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_state: BTreeMap<String, usize> = BTreeMap::new();
    for (state, block) in source.blocks.iter().enumerate() {
        first_state.entry(block.clone()).or_insert(state);
        if !position.contains_key(block) {
            position.insert(block.clone(), blocks.len());
            blocks.push(block.clone());
        }
    }

    let mut dictionaries: Map<String, Value> = Map::new();
    let mut index_of: Vec<BTreeMap<String, usize>> = Vec::with_capacity(source.fields.len());
    for field in 0..source.fields.len() {
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
                walk(&source.values[first_state[block]][field]);
            }
        } else {
            for state in 0..source.values.len() {
                walk(&source.values[state][field]);
            }
        }
        // The dictionary itself, parsed once per entry.
        let mut list = Vec::with_capacity(order.len());
        for text in &order {
            list.push(serde_json::from_str::<Value>(text).unwrap_or(Value::Null));
        }
        dictionaries.insert(source.fields[field].clone(), Value::Array(list));
        index_of.push(map);
    }

    // Per-block indices for the constant fields.
    let constant_fields: Vec<usize> = (0..source.fields.len()).filter(|f| constant[*f]).collect();
    let mut per_block = Map::new();
    for block in &blocks {
        let state = first_state[block];
        let mut row = Vec::with_capacity(constant_fields.len());
        for field in &constant_fields {
            let text = key(&source.values[state][*field]);
            row.push(json!(index_of[*field][&text]));
        }
        per_block.insert(block.clone(), Value::Array(row));
    }

    // Per-state indices for the fields that vary.
    let varying_fields: Vec<usize> = (0..source.fields.len()).filter(|f| !constant[*f]).collect();
    let mut per_state = Vec::with_capacity(source.values.len());
    for state in 0..source.values.len() {
        let mut row = Vec::with_capacity(varying_fields.len());
        for field in &varying_fields {
            let text = key(&source.values[state][*field]);
            row.push(json!(index_of[*field][&text]));
        }
        per_state.push(Value::Array(row));
    }

    Ok(json!({
        "fields": source.fields,
        "constant_fields": constant_fields.iter().map(|f| &source.fields[*f]).collect::<Vec<_>>(),
        "varying_fields": varying_fields.iter().map(|f| &source.fields[*f]).collect::<Vec<_>>(),
        "dictionaries": dictionaries,
        "blocks": blocks,
        "per_block": per_block,
        "blocks_per_state": source.blocks.iter().map(|block| json!(position[block])).collect::<Vec<_>>(),
        "per_state": per_state,
    }))
}

/// Rebuild `gameplay-facts.json` from the compact document.
pub fn expand(compact: &[u8]) -> Result<Value, String> {
    let root: Value = serde_json::from_slice(compact).map_err(|e| format!("compact: {e}"))?;
    let strings = |value: Option<&Value>, what: &str| -> Result<Vec<String>, String> {
        value
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{what} is not an array"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{what} holds a non-string"))
            })
            .collect()
    };
    let fields = strings(root.get("fields"), "fields")?;
    let constant_fields = strings(root.get("constant_fields"), "constant_fields")?;
    let varying_fields = strings(root.get("varying_fields"), "varying_fields")?;
    let blocks = strings(root.get("blocks"), "blocks")?;
    let dictionaries = root
        .get("dictionaries")
        .and_then(Value::as_object)
        .ok_or("dictionaries is not an object")?;
    let per_block = root
        .get("per_block")
        .and_then(Value::as_object)
        .ok_or("per_block is not an object")?;
    let per_state = root
        .get("per_state")
        .and_then(Value::as_array)
        .ok_or("per_state is not an array")?;

    let lookup = |field: &str, index: usize| -> Result<Value, String> {
        dictionaries
            .get(field)
            .and_then(Value::as_array)
            .and_then(|list| list.get(index))
            .cloned()
            .ok_or_else(|| format!("{field}[{index}] is out of range"))
    };
    let index_at = |value: Option<&Value>| -> Result<usize, String> {
        value
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .ok_or_else(|| "an index is not a number".to_string())
    };

    // Which block each state belongs to: the order file's blocks, each repeated
    // for its state count, is not in the compact document, so it is carried in
    // `blocks_per_state`.
    let per_state_blocks = root
        .get("blocks_per_state")
        .and_then(Value::as_array)
        .ok_or("blocks_per_state is missing")?;

    let mut out = Vec::with_capacity(per_state.len());
    for (state, row) in per_state.iter().enumerate() {
        let row = row.as_array().ok_or("a per_state row is not an array")?;
        let block_index = index_at(per_state_blocks.get(state))?;
        let block = blocks
            .get(block_index)
            .ok_or("a block index is out of range")?;
        let constants = per_block
            .get(block)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{block}: no per_block row"))?;
        let mut object = Map::new();
        object.insert("id".into(), json!(state));
        let mut constant_position = 0;
        let mut varying_position = 0;
        for field in &fields {
            let value = if constant_fields.iter().any(|name| name == field) {
                let index = index_at(constants.get(constant_position))?;
                constant_position += 1;
                lookup(field, index)?
            } else {
                let index = index_at(row.get(varying_position))?;
                varying_position += 1;
                lookup(field, index)?
            };
            object.insert(field.clone(), value);
        }
        if varying_position != varying_fields.len() {
            return Err(format!("state {state}: varying fields do not line up"));
        }
        out.push(Value::Object(object));
    }
    Ok(json!({ "states": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        // Two blocks: one whose fields are constant, one whose emission varies.
        let blocks = vec![
            "a".to_string(),
            "a".to_string(),
            "b".to_string(),
            "b".to_string(),
        ];
        let values = vec![
            vec![json!(0), json!("x"), json!(1.0)],
            vec![json!(0), json!("x"), json!(1.0)],
            vec![json!(3), json!("x"), json!(2.0)],
            vec![json!(7), json!("y"), json!(2.0)],
        ];
        Source {
            fields: vec!["emission".into(), "sound".into(), "hardness".into()],
            values,
            blocks,
        }
    }

    #[test]
    fn a_round_trip_returns_every_value() {
        let source = source();
        let compact = compress(&source).unwrap();
        assert_eq!(compact["blocks_per_state"], json!([0, 0, 1, 1]));
        let bytes = serde_json::to_vec(&compact).unwrap();
        let expanded = expand(&bytes).unwrap();
        let states = expanded["states"].as_array().unwrap();
        assert_eq!(states.len(), 4);
        for (index, state) in states.iter().enumerate() {
            assert_eq!(state["id"], index);
            for (field, expected) in source.fields.iter().zip(&source.values[index]) {
                assert_eq!(&state[field], expected, "state {index} {field}");
            }
        }
    }

    #[test]
    fn constant_fields_are_stored_once_per_block() {
        let source = source();
        let compact = compress(&source).unwrap();
        // hardness is 1.0 for both of a's states and 2.0 for both of b's, so it
        // is per-block; emission and sound vary inside a block.
        assert_eq!(compact["constant_fields"], json!(["hardness"]));
        assert_eq!(compact["varying_fields"], json!(["emission", "sound"]));
        assert_eq!(compact["per_block"].as_object().unwrap().len(), 2);
        assert_eq!(compact["per_state"].as_array().unwrap().len(), 4);
        assert_eq!(compact["per_state"][0].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_state_count_that_does_not_match_is_refused() {
        let gameplay = br#"{"states":[{"id":0,"a":1},{"id":1,"a":2}]}"#;
        let error = read(gameplay, &["x".to_string()]).unwrap_err();
        assert!(error.contains("not the same version"), "{error}");
    }

    #[test]
    fn a_state_missing_a_field_is_refused() {
        let gameplay = br#"{"states":[{"id":0,"a":1},{"id":1,"b":2}]}"#;
        let error = read(gameplay, &["x".to_string(), "x".to_string()]).unwrap_err();
        assert!(error.contains("has no"), "{error}");
    }
}
