//! `model-states.json` derived from `facts.json`.
//!
//! `VanillaBlockStateBindingOracle` exports, per block, the state container the
//! client binds blockstate files against:
//!
//! ```json
//! {"minecraft:acacia_button": {
//!    "schema": {"face": {"named": ["ceiling","floor","wall"]}, ...},
//!    "states": [{"face":"floor","facing":"north","powered":"true"}, ...]}}
//! ```
//!
//! `facts.json` already lists every state in registry order, each with its
//! properties. Grouping those by block, in order, reproduces `states`; the
//! distinct values of each property, in first-appearance order, reproduce
//! `schema`. So the file is a projection of `facts.json`, and a device that has
//! `facts.json` does not need it transferred.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// One block's accumulated states, in the order they appeared.
#[derive(Default)]
struct Block {
    states: Vec<Map<String, Value>>,
}

/// Derive the `model-states.json` document from `facts.json`'s bytes.
pub fn derive(facts: &[u8]) -> Result<Value, String> {
    let root: Value = serde_json::from_slice(facts).map_err(|e| format!("facts.json: {e}"))?;
    let states = root
        .get("states")
        .and_then(Value::as_array)
        .ok_or("facts.json has no states array")?;

    let mut blocks: Vec<(String, Block)> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (position, row) in states.iter().enumerate() {
        let row = row
            .as_array()
            .ok_or_else(|| format!("facts.json states[{position}] is not an array"))?;
        let name = row
            .first()
            .and_then(Value::as_str)
            .ok_or_else(|| format!("facts.json states[{position}] has no block name"))?;
        // Column 4 is the state's own object: `{"properties": {...}, "id": ...}`.
        let detail = row
            .get(4)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("facts.json states[{position}] has no detail object"))?;
        let properties = detail
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let slot = *index.entry(name.to_string()).or_insert_with(|| {
            blocks.push((name.to_string(), Block::default()));
            blocks.len() - 1
        });
        blocks[slot].1.states.push(properties);
    }

    let mut out = Map::new();
    for (name, block) in blocks {
        out.insert(name, json!({
            "schema": schema_of(&block.states),
            "states": block.states,
        }));
    }
    Ok(Value::Object(out))
}

/// Each property's values, sorted by name.
///
/// The oracle sorts them (`face: ceiling, floor, wall` — not the enum's
/// `FLOOR, WALL, CEILING`), so the order is not the property's own and cannot be
/// used to recover it. The states array, which does keep registry order, is
/// where a consumer gets the enumeration.
fn schema_of(states: &[Map<String, Value>]) -> Value {
    let mut values: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for state in states {
        for (property, value) in state {
            let Some(text) = value.as_str() else { continue };
            let entry = values.entry(property.as_str()).or_default();
            if !entry.contains(&text) {
                entry.push(text);
            }
        }
    }
    let mut schema = Map::new();
    for (property, list) in values {
        schema.insert(property.to_string(), kind_of(&list));
    }
    Value::Object(schema)
}

/// A property's declared kind, from the values the states show.
///
/// Vanilla has two: an enum (`{"named": [...]}`, sorted, as the oracle writes
/// it) and an integer range (`{"integer": [min, max]}`), which the states
/// enumerate in full, so a contiguous run of numbers is an integer property and
/// anything else is named.
fn kind_of(values: &[&str]) -> Value {
    let numbers: Option<Vec<i64>> = values.iter().map(|v| v.parse::<i64>().ok()).collect();
    if let Some(numbers) = numbers {
        let mut sorted = numbers.clone();
        sorted.sort_unstable();
        sorted.dedup();
        let contiguous = sorted.windows(2).all(|window| window[1] == window[0] + 1);
        if let (Some(&min), Some(&max)) = (sorted.first(), sorted.last()) {
            if contiguous && !sorted.is_empty() && max - min + 1 == sorted.len() as i64 {
                return json!({ "integer": [min, max] });
            }
        }
    }
    let mut named = values.to_vec();
    named.sort_unstable();
    json!({ "named": named })
}

/// Blocks the client registers but the server does not, recovered from their
/// blockstate files.
///
/// `facts.json` comes from the server jar, so it lacks the two client-only
/// blocks (`item_frame`, `glow_item_frame`). Their blockstate files do enumerate
/// every state, so a block with exactly one property can be rebuilt from the
/// variant keys. Anything richer (multipart, several properties) is left to the
/// caller to report rather than guessed at.
pub fn synthesize_from_blockstates(
    dir: &Path,
    known: &Map<String, Value>,
) -> Result<(Map<String, Value>, Vec<String>), String> {
    let mut added = Map::new();
    let mut unsupported = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = format!("minecraft:{stem}");
        if known.contains_key(&name) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            unsupported.push(name);
            continue;
        };
        let Ok(document) = serde_json::from_slice::<Value>(&bytes) else {
            unsupported.push(name);
            continue;
        };
        let Some(variants) = document.get("variants").and_then(Value::as_object) else {
            unsupported.push(name);
            continue;
        };
        let mut properties: BTreeMap<&str, std::collections::BTreeSet<&str>> = BTreeMap::new();
        for key in variants.keys() {
            for part in key.split(',').filter(|part| !part.is_empty()) {
                let Some((property, value)) = part.split_once('=') else {
                    properties.clear();
                    break;
                };
                properties.entry(property).or_default().insert(value);
            }
        }
        if properties.len() > 1 || (properties.is_empty() && !variants.contains_key("")) {
            unsupported.push(name);
            continue;
        }
        let (schema, states) = if properties.is_empty() {
            (json!({}), vec![json!({})])
        } else {
            let Some((property, values)) = properties.iter().next() else {
                return Err(format!("{name}: the blockstate has no property"));
            };
            let ordered = ordered_values(values);
            let states = ordered
                .iter()
                .map(|value| json!({ *property: value }))
                .collect::<Vec<Value>>();
            let mut schema = Map::new();
            schema.insert(property.to_string(), kind_of(&ordered));
            (Value::Object(schema), states)
        };
        added.insert(
            name,
            json!({ "schema": schema, "states": states }),
        );
    }
    Ok((added, unsupported))
}

/// A property's values in the order vanilla enumerates them: a boolean is
/// `true` then `false` (as every button's `powered` shows), anything else sorted.
fn ordered_values<'a>(values: &std::collections::BTreeSet<&'a str>) -> Vec<&'a str> {
    if values.len() == 2 && values.contains("true") && values.contains("false") {
        return vec!["true", "false"];
    }
    values.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(rows: &[(&str, Option<Value>)]) -> Vec<u8> {
        let states: Vec<Value> = rows
            .iter()
            .map(|(name, properties)| {
                let detail = match properties {
                    Some(props) => json!({"properties": props, "id": name}),
                    None => json!({"id": name}),
                };
                json!([name, false, false, "Block{}", detail])
            })
            .collect();
        serde_json::to_vec(&json!({ "states": states })).unwrap()
    }

    #[test]
    fn states_keep_their_order_and_blocks_their_first_appearance() {
        let bytes = facts(&[
            ("minecraft:air", None),
            ("minecraft:door", Some(json!({"facing": "north", "open": "true"}))),
            ("minecraft:door", Some(json!({"facing": "north", "open": "false"}))),
            ("minecraft:door", Some(json!({"facing": "east", "open": "true"}))),
            ("minecraft:door", Some(json!({"facing": "east", "open": "false"}))),
        ]);
        let derived = derive(&bytes).unwrap();
        let door = &derived["minecraft:door"];
        assert_eq!(door["states"].as_array().unwrap().len(), 4);
        assert_eq!(door["states"][2]["facing"], "east");
        // Values sorted by name, as the oracle writes them.
        assert_eq!(door["schema"]["facing"]["named"], json!(["east", "north"]));
        assert_eq!(door["schema"]["open"]["named"], json!(["false", "true"]));
        // A block with no properties still gets an entry.
        assert_eq!(derived["minecraft:air"]["schema"], json!({}));
        assert_eq!(derived["minecraft:air"]["states"], json!([{}]));
    }

    #[test]
    fn a_contiguous_numeric_property_is_an_integer_range() {
        // `rotation` is `{"integer": [0, 15]}` in the prepared file, not a list
        // of sixteen names.
        let rows: Vec<(&str, Option<Value>)> = (0..16)
            .map(|n| ("minecraft:sign", Some(json!({"rotation": n.to_string()}))))
            .collect();
        let derived = derive(&facts(&rows)).unwrap();
        assert_eq!(
            derived["minecraft:sign"]["schema"]["rotation"],
            json!({"integer": [0, 15]})
        );
        // A gap in the run means names, not a range.
        let sparse = facts(&[
            ("minecraft:x", Some(json!({"level": "0"}))),
            ("minecraft:x", Some(json!({"level": "2"}))),
        ]);
        assert_eq!(
            derive(&sparse).unwrap()["minecraft:x"]["schema"]["level"],
            json!({"named": ["0", "2"]})
        );
    }

    #[test]
    fn a_facts_file_without_states_is_refused() {
        assert!(derive(b"{}").is_err());
        assert!(derive(b"not json").is_err());
        assert!(derive(br#"{"states": [[]]}"#).is_err());
    }

    #[test]
    fn one_block_mixed_among_others_is_grouped_once() {
        let bytes = facts(&[
            ("a", Some(json!({"p": "1"}))),
            ("b", Some(json!({"p": "1"}))),
            ("a", Some(json!({"p": "2"}))),
        ]);
        let derived = derive(&bytes).unwrap();
        let keys: Vec<&String> = derived.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(derived["a"]["states"].as_array().unwrap().len(), 2);
        assert_eq!(derived["b"]["states"].as_array().unwrap().len(), 1);
    }
}
