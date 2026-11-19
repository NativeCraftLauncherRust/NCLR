//! `facts.json` rebuilt from the block order plus `model-states.json`.
//!
//! The Java exporter is short (`VanillaHeightmapOracle.facts`): for every block
//! state in registry order it writes
//!
//! ```json
//! ["minecraft:grass_block", false, false, "Block{minecraft:grass_block}[snowy=true]",
//!  {"properties": {"snowy": "true"}, "id": "minecraft:grass_block"}]
//! ```
//!
//! of which only two things come from code rather than data: the **order** of the
//! block states, and three small predicates. Measured against a real
//! `facts.json` (`route3/AUDIT.md`):
//!
//! - every block's state list equals `model-states.json`'s, in order (1,286 of 1,286);
//! - `isAir` is exactly three blocks;
//! - `hasFluid` is "waterlogged is true, or one of seven always-fluid blocks";
//! - `toString` is the name and its properties; `id` is the block.
//!
//! So `facts.json` (10.1 MiB) is rebuildable from the block order (a list of
//! names) and the state table the model binding already needs.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// The only blocks whose every state is air.
pub const AIR: [&str; 3] = ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"];

/// The blocks whose every state carries a fluid, whatever their properties say.
pub const ALWAYS_FLUID: [&str; 7] = [
    "minecraft:water",
    "minecraft:lava",
    "minecraft:seagrass",
    "minecraft:tall_seagrass",
    "minecraft:kelp",
    "minecraft:kelp_plant",
    "minecraft:bubble_column",
];

/// Per block, the states in registry order. `order` is registry order across blocks.
pub struct Table {
    pub order: Vec<String>,
    pub states: BTreeMap<String, Vec<Map<String, Value>>>,
    /// Each block's own default state id.
    ///
    /// Not derivable: a block's class declares it (`registerDefaultState`), and
    /// 746 of 1,286 blocks default to something other than their first state 鈥?    /// a button defaults to `face=wall`. Carried as a small table.
    pub defaults: Option<BTreeMap<String, u64>>,
}

impl Table {
    /// Every state id: the block's states, in `order`, numbered from zero.
    pub fn state_count(&self) -> usize {
        self.order
            .iter()
            .map(|name| self.states.get(name).map_or(0, Vec::len))
            .sum()
    }
}

/// The block names of `facts.json`, in the order they first appear.
pub fn block_order(facts: &[u8]) -> Result<Vec<String>, String> {
    let root: Value = serde_json::from_slice(facts).map_err(|e| format!("facts.json: {e}"))?;
    let states = root
        .get("states")
        .and_then(Value::as_array)
        .ok_or("facts.json has no states array")?;
    let mut order: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (index, row) in states.iter().enumerate() {
        let name = row
            .as_array()
            .and_then(|row| row.first())
            .and_then(Value::as_str)
            .ok_or_else(|| format!("facts.json states[{index}] has no block name"))?;
        if seen.insert(name.to_string()) {
            order.push(name.to_string());
        }
    }
    Ok(order)
}

/// The `defaults` map of `facts.json`: each block's default state id.
pub fn defaults_from(facts: &[u8]) -> Result<BTreeMap<String, u64>, String> {
    let root: Value = serde_json::from_slice(facts).map_err(|e| format!("facts.json: {e}"))?;
    let defaults = root
        .get("defaults")
        .and_then(Value::as_object)
        .ok_or("facts.json has no defaults object")?;
    let mut table = BTreeMap::new();
    for (name, value) in defaults {
        let id = value
            .as_u64()
            .ok_or_else(|| format!("{name}: the default is not a state id"))?;
        table.insert(name.clone(), id);
    }
    Ok(table)
}

/// The table `model-states.json` holds: per block, its ordered states.
pub fn from_model_states(model_states: &[u8]) -> Result<BTreeMap<String, Vec<Map<String, Value>>>, String> {
    let root: Value =
        serde_json::from_slice(model_states).map_err(|e| format!("model-states.json: {e}"))?;
    let blocks = root
        .as_object()
        .ok_or("model-states.json is not an object")?;
    let mut table = BTreeMap::new();
    for (name, entry) in blocks {
        let states = entry
            .get("states")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{name}: no states"))?;
        let mut list = Vec::with_capacity(states.len());
        for state in states {
            let map = state
                .as_object()
                .ok_or_else(|| format!("{name}: a state is not an object"))?;
            list.push(map.clone());
        }
        table.insert(name.clone(), list);
    }
    Ok(table)
}

fn is_air(name: &str) -> bool {
    AIR.contains(&name)
}

fn has_fluid(name: &str, state: &Map<String, Value>) -> bool {
    if ALWAYS_FLUID.contains(&name) {
        return true;
    }
    // Waterlogged blocks carry water in the states that say so.
    state.get("waterlogged").and_then(Value::as_str) == Some("true")
}

/// `Block{minecraft:oak_stairs}[facing=north,half=top,...]`.
fn described(name: &str, state: &Map<String, Value>) -> String {
    if state.is_empty() {
        return format!("Block{{{name}}}");
    }
    let pairs: Vec<String> = state
        .iter()
        .map(|(property, value)| {
            format!(
                "{property}={}",
                value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())
            )
        })
        .collect();
    format!("Block{{{name}}}[{}]", pairs.join(","))
}

/// The `facts.json` document for `table`.
pub fn build(table: &Table) -> Result<Value, String> {
    let mut states = Vec::with_capacity(table.state_count());
    let mut defaults = Map::new();
    let mut next_id = 0u64;
    for name in &table.order {
        let list = table
            .states
            .get(name)
            .ok_or_else(|| format!("{name}: no states in the table"))?;
        let default = match &table.defaults {
            Some(defaults) => defaults
                .get(name)
                .copied()
                .ok_or_else(|| format!("{name}: no default state in the table"))?,
            None => next_id,
        };
        defaults.insert(name.clone(), json!(default));
        for state in list {
            let detail = if state.is_empty() {
                json!({ "id": name })
            } else {
                json!({ "properties": state, "id": name })
            };
            states.push(json!([
                name,
                is_air(name),
                has_fluid(name, state),
                described(name, state),
                detail
            ]));
            next_id += 1;
        }
    }
    Ok(json!({ "states": states, "defaults": defaults }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    fn table() -> Table {
        let mut states = BTreeMap::new();
        states.insert("minecraft:air".to_string(), vec![Map::new()]);
        states.insert(
            "minecraft:oak_stairs".to_string(),
            vec![
                state(&[("facing", "north"), ("waterlogged", "true")]),
                state(&[("facing", "north"), ("waterlogged", "false")]),
            ],
        );
        states.insert("minecraft:water".to_string(), vec![state(&[("level", "0")])]);
        Table {
            order: vec![
                "minecraft:air".into(),
                "minecraft:oak_stairs".into(),
                "minecraft:water".into(),
            ],
            states,
            defaults: None,
        }
    }

    #[test]
    fn the_three_predicates_follow_the_measured_rules() {
        let facts = build(&table()).unwrap();
        let rows = facts["states"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        // air: isAir true, no fluid
        assert_eq!(rows[0][1], true);
        assert_eq!(rows[0][2], false);
        // waterlogged state carries a fluid, the other does not
        assert_eq!(rows[1][1], false);
        assert_eq!(rows[1][2], true);
        assert_eq!(rows[2][2], false);
        // water is always fluid
        assert_eq!(rows[3][2], true);
    }

    #[test]
    fn ids_run_across_blocks_in_order_and_defaults_name_the_first_state() {
        let facts = build(&table()).unwrap();
        assert_eq!(facts["defaults"]["minecraft:air"], 0);
        assert_eq!(facts["defaults"]["minecraft:oak_stairs"], 1);
        assert_eq!(facts["defaults"]["minecraft:water"], 3);
        let rows = facts["states"].as_array().unwrap();
        assert_eq!(rows[3][4]["id"], "minecraft:water");
    }

    #[test]
    fn the_description_lists_properties_and_nothing_when_there_are_none() {
        let facts = build(&table()).unwrap();
        let rows = facts["states"].as_array().unwrap();
        assert_eq!(rows[0][3], "Block{minecraft:air}");
        assert_eq!(
            rows[1][3],
            "Block{minecraft:oak_stairs}[facing=north,waterlogged=true]"
        );
        assert_eq!(rows[0][4].get("properties"), None);
    }

    #[test]
    fn the_order_is_the_first_appearance_order_of_the_states() {
        let facts = br#"{"states":[["a"],["b"],["a"],["c"],["b"]]}"#;
        assert_eq!(block_order(facts).unwrap(), vec!["a", "b", "c"]);
        assert!(block_order(b"{}").is_err());
    }
}
