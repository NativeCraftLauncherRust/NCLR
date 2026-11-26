//! `_shared/model-states.json`: per-block state schema and ordered states.
//!
//! Horizon's client binds blockstate files against this table:
//!
//! ```json
//! {"minecraft:acacia_button": {
//!    "schema": {"face": {"named": ["ceiling","floor","wall"]},
//!               "facing": {"named": ["east","north","south","west"]},
//!               "powered": {"named": ["false","true"]}},
//!    "states": [{"face":"floor","facing":"north","powered":"true"}, ...]}}
//! ```
//!
//! # Derivation
//!
//! Everything comes from the player's `client.jar`:
//!
//! * **the block set** is the jar's `assets/minecraft/blockstates/*.json` names
//!   (1,288 files; the server registry's 1,286 plus the client-only
//!   `item_frame` and `glow_item_frame`);
//! * **`states`** is the registry state enumeration for blocks the server
//!   registers: property names sorted (`ImmutableSortedMap`), values in the
//!   property's own order, last property varying fastest — exactly the order
//!   `states.rs` derives from `Blocks.<clinit>` (and `states.bin` carries);
//! * **`schema`** lists each property's values sorted by name
//!   (`{"named": [...]}`), or as an integer range (`{"integer": [min, max]}`)
//!   when every value parses as a contiguous run of integers;
//! * the two client-only blocks have no block class in `Blocks`, so their one
//!   boolean property is rebuilt from their blockstate file's variant keys
//!   (`{"map": {"named": ["false","true"]}}`, states `true` then `false`, the
//!   vanilla boolean order).
//!
//! The document is a JSON object with sorted keys and the block entries are
//! `{"schema": ..., "states": ...}` in that order, so serialising the derived
//! value reproduces the prepared file byte for byte (2,842,545 bytes).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{json, Map, Value};

#[path = "states.rs"]
#[allow(unused)]
mod states;

/// The file this module writes.
pub const TABLE: &str = "_shared/model-states.json";

/// The strip prefix of the jar's blockstate assets.
const BLOCKSTATES: &str = "assets/minecraft/blockstates/";

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub from_registry: usize,
    pub synthesized: Vec<String>,
    pub notes: Vec<String>,
}

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(name, data);
        }
        Ok(Jar { files })
    }

    fn blockstate_names(&self) -> Vec<String> {
        self.files
            .keys()
            .filter_map(|name| name.strip_prefix(BLOCKSTATES))
            .filter_map(|name| name.strip_suffix(".json"))
            .filter(|stem| !stem.contains('/'))
            .map(|stem| format!("minecraft:{stem}"))
            .collect()
    }
}

/// One block's schema: property name → value description.
fn schema_of(properties: &[(String, Vec<String>)]) -> Value {
    let mut schema = Map::new();
    for (name, values) in properties {
        schema.insert(name.clone(), kind_of(values));
    }
    Value::Object(schema)
}

/// A property's declared kind: a contiguous integer range, else sorted names.
fn kind_of(values: &[String]) -> Value {
    let numbers: Option<Vec<i64>> = values.iter().map(|value| value.parse::<i64>().ok()).collect();
    if let Some(mut numbers) = numbers {
        numbers.sort_unstable();
        numbers.dedup();
        if let (Some(&min), Some(&max)) = (numbers.first(), numbers.last()) {
            let contiguous = numbers.windows(2).all(|pair| pair[1] == pair[0] + 1);
            if contiguous && max - min + 1 == numbers.len() as i64 {
                return json!({ "integer": [min, max] });
            }
        }
    }
    let mut named: Vec<&str> = values.iter().map(String::as_str).collect();
    named.sort_unstable();
    json!({ "named": named })
}

/// The block's states, in registry order: sorted property names, last varying
/// fastest, values in the property's own order.
fn states_of(properties: &[(String, Vec<String>)]) -> Vec<Value> {
    let mut sorted: Vec<&(String, Vec<String>)> = properties.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    for (_, values) in &sorted {
        let mut next = Vec::with_capacity(rows.len() * values.len());
        for row in &rows {
            for value in 0..values.len() {
                let mut row = row.clone();
                row.push(value);
                next.push(row);
            }
        }
        rows = next;
    }
    rows.into_iter()
        .map(|row| {
            let mut state = Map::new();
            for (position, (name, values)) in sorted.iter().enumerate() {
                state.insert(name.clone(), Value::String(values[row[position]].clone()));
            }
            Value::Object(state)
        })
        .collect()
}

/// The states of a client-only block, rebuilt from its blockstate file.
///
/// The file's variant keys name the property values (`map=true`); vanilla
/// enumerates a boolean `true` before `false`, and anything else sorted.
fn synthesize_states(jar: &Jar, id: &str) -> Result<Option<(Value, Vec<Value>)>, String> {
    let stem = id.strip_prefix("minecraft:").unwrap_or(id);
    let path = format!("{BLOCKSTATES}{stem}.json");
    let Some(bytes) = jar.files.get(&path) else {
        return Ok(None);
    };
    let document: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("{path}: {error}"))?;
    let Some(variants) = document.get("variants").and_then(Value::as_object) else {
        return Ok(None);
    };
    let mut properties: BTreeMap<&str, std::collections::BTreeSet<&str>> = BTreeMap::new();
    for key in variants.keys() {
        for part in key.split(',').filter(|part| !part.is_empty()) {
            let Some((property, value)) = part.split_once('=') else {
                return Ok(None);
            };
            properties.entry(property).or_default().insert(value);
        }
    }
    if properties.len() != 1 {
        return Ok(None);
    }
    let Some((property, values)) = properties.iter().next() else {
        return Ok(None);
    };
    let ordered: Vec<&str> = if values.len() == 2 && values.contains("true") && values.contains("false")
    {
        vec!["true", "false"]
    } else {
        values.iter().copied().collect()
    };
    let mut schema = Map::new();
    schema.insert(
        (*property).to_string(),
        kind_of(&ordered.iter().map(|value| value.to_string()).collect::<Vec<_>>()),
    );
    let states = ordered
        .iter()
        .map(|value| {
            let mut state = Map::new();
            state.insert((*property).to_string(), Value::String((*value).to_string()));
            Value::Object(state)
        })
        .collect();
    Ok(Some((Value::Object(schema), states)))
}

/// Derive the whole `model-states.json` document from the jar.
pub fn model_states(jar_path: &Path) -> Result<(Value, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let mut coverage = Coverage::default();
    let mut names = jar.blockstate_names();
    names.sort_unstable();
    if names.is_empty() {
        return Err("the jar has no blockstate assets".to_string());
    }
    let table = states::state_table(jar_path)?;
    let mut by_id: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, block) in table.blocks.iter().enumerate() {
        by_id.insert(block.id.as_str(), index);
    }
    let mut out = Map::new();
    for id in names {
        if let Some(index) = by_id.get(id.as_str()) {
            let block = &table.blocks[*index];
            let properties: Vec<(String, Vec<String>)> = block
                .properties
                .iter()
                .map(|property| (property.name.clone(), property.values.clone()))
                .collect();
            let schema = schema_of(&properties);
            let states = states_of(&properties);
            out.insert(
                id,
                json!({ "schema": schema, "states": states }),
            );
            coverage.from_registry += 1;
        } else if let Some((schema, states)) = synthesize_states(&jar, &id)? {
            out.insert(id.clone(), json!({ "schema": schema, "states": states }));
            coverage.synthesized.push(id);
        } else {
            coverage.notes.push(format!("{id}: no block class and no reconstructable blockstate"));
        }
    }
    coverage.blocks = out.len();
    Ok((Value::Object(out), coverage))
}

/// The document serialised the way the bundle carries it (compact, sorted keys).
pub fn model_states_json(jar_path: &Path) -> Result<Vec<u8>, String> {
    let (value, _) = model_states(jar_path)?;
    serde_json::to_vec(&value).map_err(|error| error.to_string())
}

/// Write `_shared/model-states.json` below `out` and report coverage.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let (value, coverage) = model_states(jar_path)?;
    let bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    let path = out.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, &bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_are_a_range_and_everything_else_is_named() {
        let range = kind_of(&["1".into(), "2".into(), "3".into(), "4".into()]);
        assert_eq!(range, json!({ "integer": [1, 4] }));
        let named = kind_of(&["true".into(), "false".into()]);
        assert_eq!(named, json!({ "named": ["false", "true"] }));
        // A gap in the numbers is not a range.
        let gapped = kind_of(&["0".into(), "2".into()]);
        assert_eq!(gapped, json!({ "named": ["0", "2"] }));
    }

    #[test]
    fn the_state_order_is_the_registry_enumeration() {
        // acacia_button's two properties, in the game's order.
        let properties = vec![
            ("powered".to_string(), vec!["true".to_string(), "false".to_string()]),
            ("face".to_string(), vec!["floor".to_string(), "wall".to_string(), "ceiling".to_string()]),
            ("facing".to_string(), vec!["north".to_string(), "south".to_string(), "west".to_string(), "east".to_string()]),
        ];
        let states = states_of(&properties);
        assert_eq!(
            states[0],
            json!({"face": "floor", "facing": "north", "powered": "true"})
        );
        assert_eq!(
            states[1],
            json!({"face": "floor", "facing": "north", "powered": "false"})
        );
        assert_eq!(states.len(), 24);
    }
}
