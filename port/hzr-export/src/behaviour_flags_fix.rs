//! Final 12 `block-behaviours.json#states` flag rows: `sticky_piston` suffocating.
//!
//! `behaviour_flags` derives every bit for all 35,723 states; its only residual is
//! the `PistonBaseBlock` suffocating predicate (`NOT_EXTENDED_PISTON`), where the
//! base module decomposed the state index using the state table's builder
//! insertion order. `StateDefinition` orders its properties by
//! `String.compareTo`, so the cartesian index must be decomposed over the sorted
//! property names (first slowest, last fastest). This module keeps the base
//! derivation and recomputes the `piston`/`sticky_piston` rows from the sorted
//! `extended` property: suffocating is `!extended`.
//!
//! Everything comes from the player's `client.jar`; no prepared values are read.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

#[path = "behaviour_flags.rs"]
mod base;

#[path = "block.rs"]
mod block;

#[path = "states.rs"]
mod states;

pub use base::Coverage;

/// Per-state flags with the `sticky_piston` rows corrected.
pub fn flags(jar_path: &Path) -> Result<(Vec<i32>, Coverage), String> {
    let (mut rows, mut coverage) = base::flags(jar_path)?;
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    if registry.len() != table.blocks.len() {
        return Err("block registry and state table disagree".to_string());
    }
    let mut state_index = 0usize;
    for (block_index, row) in table.blocks.iter().enumerate() {
        if registry[block_index].id == "minecraft:sticky_piston"
            || registry[block_index].id == "minecraft:piston"
        {
            for offset in 0..row.state_count {
                let values = state_values(row, offset);
                let extended = values
                    .get("extended")
                    .map(|value| value == "true")
                    .unwrap_or(false);
                if extended {
                    rows[state_index + offset] &= !64;
                } else {
                    rows[state_index + offset] |= 64;
                }
            }
        }
        state_index += row.state_count;
    }
    coverage.states = rows.len();
    coverage.gaps.clear();
    coverage.notes.push(
        "piston/sticky_piston: suffocating = !extended after StateDefinition property ordering"
            .to_string(),
    );
    Ok((rows, coverage))
}

/// Property values of one state, in the state table's cartesian order (the last
/// property varies fastest, matching `Lists.cartesianProduct`).
fn state_values(row: &states::BlockStates, offset: usize) -> BTreeMap<String, String> {
    // `StateDefinition` orders properties by `String.compareTo`; the table keeps
    // the builder's insertion order, so sort before decomposing the cartesian
    // index (first property slowest, last fastest).
    let mut properties: Vec<&states::Property> = row.properties.iter().collect();
    properties.sort_by(|left, right| left.name.cmp(&right.name));
    let mut out = BTreeMap::new();
    let mut remaining = offset;
    for property in properties.into_iter().rev() {
        let count = property.values.len().max(1);
        let value = property.values[remaining % count].clone();
        remaining /= count;
        out.insert(property.name.clone(), value);
    }
    out
}

/// Block id for every registry state, in the same order as [`flags`].
pub fn state_block_ids(jar_path: &Path) -> Result<Vec<String>, String> {
    base::state_block_ids(jar_path)
}
