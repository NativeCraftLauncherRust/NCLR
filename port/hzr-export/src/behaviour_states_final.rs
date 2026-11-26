//! Final `block-behaviours.json#states` rows, exact on every column.
//!
//! `behaviour_states_exact` already composes a row set whose sturdy, flags,
//! fluidAmount, falling, signalError and instrument columns agree with the
//! prepared export for all 35,723 states; its only remaining errors are the
//! redstone-signal columns (`weak`, `strong` — 2,115 and 2,069 rows) and one
//! fluid id. This module extends that composition **by path include** and
//! replaces exactly those outputs:
//!
//! * `weak`/`strong` and the flags source bit (bit 8) come from
//!   `behaviour_signal::signal_row(chain, values)`, which decodes
//!   `isSignalSource`/`getSignal`/`getDirectSignal` from the block classes over
//!   the exporter's `EmptyBlockGetter` and is validated 35,723/35,723 on its
//!   own (`tests/behaviour_signal.rs`);
//! * the one fluid id the base module gets wrong: a `level=0` liquid is the
//!   **source** fluid (`FluidState.getType()` of the source, e.g.
//!   `Fluids.WATER`) while the base `fluid_row` only special-cases lava and
//!   returns the flowing fluid for a water source as well. That is one state
//!   (`minecraft:water[level=0]`); every other fluid id/amount/falling value
//!   already agrees.
//!
//! The state values (property name -> value) are rebuilt from the state table
//! in the same sorted/reverse-radix order the exporter enumerates, and the block
//! chains come from the block registry, so no value is read from the prepared
//! copy (that file is the oracle in the test only).

#![allow(
    dead_code,
    clippy::all,
    clippy::duplicate_mod,
    clippy::empty_line_after_doc_comments
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

#[path = "behaviour_states_exact.rs"]
#[allow(unused, clippy::all)]
mod behaviour_states_exact;

#[path = "behaviour_signal.rs"]
#[allow(unused, clippy::all)]
mod behaviour_signal;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "block.rs"]
#[allow(unused, clippy::all)]
mod block;

#[path = "states.rs"]
#[allow(unused, clippy::all)]
mod states;

/// Exporter columns (fixed order).
pub const COLUMN_STURDY: usize = 0;
pub const COLUMN_FLAGS: usize = 1;
pub const COLUMN_FLUID: usize = 2;
pub const COLUMN_WEAK: usize = 5;
pub const COLUMN_STRONG: usize = 6;
pub const COLUMN_SIGNAL_ERROR: usize = 7;

/// `state.isSignalSource()` inside the flags mask.
pub const FLAG_SIGNAL_SOURCE: i64 = 1 << 3;

/// What the signal/fluid patch did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub states: usize,
    /// States whose signal columns (source bit/weak/strong/error) changed.
    pub signals_patched: usize,
    /// States whose fluid id changed (the source-fluid rule below).
    pub fluid_fixed: usize,
    /// States where the base signalError already agreed (all of them expected).
    pub signal_error_kept: usize,
    /// State indices the source-fluid rule matched, for the report.
    pub fluid_fixed_states: Vec<usize>,
    pub notes: Vec<String>,
}

/// Compose the final rows: `behaviour_states_exact` + signal + fluid fix.
pub fn rows(jar_path: &Path) -> Result<(Vec<Value>, Coverage), String> {
    let (base, _) = behaviour_states_exact::rows(jar_path)?;
    let mut coverage = Coverage {
        states: base.len(),
        ..Default::default()
    };
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    let fluid_ids = fluid_ids(jar_path)?;
    let water_source = *fluid_ids.get("water").unwrap_or(&2);

    let mut out = Vec::with_capacity(base.len());
    let mut state_index = 0usize;
    for (block_index, block) in registry.iter().enumerate() {
        let Some(states) = table.blocks.get(block_index) else {
            return Err(format!("state table is missing {}", block.id));
        };
        for offset in 0..states.state_count {
            let row = base
                .get(state_index)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("row {state_index} is not an array"))?;
            let mut values = row.clone();
            let property_values = state_values(states, offset);

            // ---- signal columns (behaviour_signal) ----
            let signal = behaviour_signal::signal_row(&block.chain, &property_values);
            let flags = values
                .get(COLUMN_FLAGS)
                .and_then(Value::as_i64)
                .ok_or_else(|| format!("row {state_index}: flags"))?;
            let patched_flags =
                (flags & !FLAG_SIGNAL_SOURCE) | if signal.source { FLAG_SIGNAL_SOURCE } else { 0 };
            let base_error = values
                .get(COLUMN_SIGNAL_ERROR)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if base_error == signal.error {
                coverage.signal_error_kept += 1;
            }
            let changed = patched_flags != flags
                || values.get(COLUMN_WEAK).and_then(Value::as_i64) != Some(signal.weak)
                || values.get(COLUMN_STRONG).and_then(Value::as_i64) != Some(signal.strong)
                || base_error != signal.error;
            if changed {
                coverage.signals_patched += 1;
            }
            values[COLUMN_FLAGS] = Value::from(patched_flags);
            values[COLUMN_WEAK] = Value::from(signal.weak);
            values[COLUMN_STRONG] = Value::from(signal.strong);
            values[COLUMN_SIGNAL_ERROR] = Value::from(signal.error);

            // ---- the one fluid rule the base module misses ----
            // `LiquidBlock.getFluidState`: LEVEL == 0 is the source fluid
            // (`Fluids.WATER` / `Fluids.LAVA`), not the flowing one. The base
            // module handles lava but not water.
            if block.id == "minecraft:water"
                && property_values.get("level").map(String::as_str) == Some("0")
            {
                let current = values.get(COLUMN_FLUID).and_then(Value::as_i64);
                if current != Some(water_source.into()) {
                    coverage.fluid_fixed += 1;
                    coverage.fluid_fixed_states.push(state_index);
                }
                values[COLUMN_FLUID] = Value::from(water_source);
            }

            out.push(Value::Array(values));
            state_index += 1;
        }
    }
    if out.len() != base.len() {
        return Err(format!(
            "registry/state table walked {}/{}, base has {}",
            out.len(),
            table.state_count(),
            base.len()
        ));
    }
    coverage.notes.push(format!(
        "fluid registry ids {fluid_ids:?}; water source fluid id {water_source}"
    ));
    Ok((out, coverage))
}

/// Rebuild a state's property map from the table order: properties sorted by
/// name, values in the exporter's reverse-radix order.
fn state_values(block: &states::BlockStates, offset: usize) -> BTreeMap<String, String> {
    let sorted = block.sorted();
    let mut out = BTreeMap::new();
    let mut remainder = offset;
    for property in sorted.iter().rev() {
        let count = property.values.len().max(1);
        let slot = remainder % count;
        remainder /= count;
        out.insert(property.name.clone(), property.values[slot].clone());
    }
    out
}

/// The fluid registry order, read from `Fluids.<clinit>`'s field order in the
/// jar (the same table the exporter indexes with `Fluid.getId`).
fn fluid_ids(jar_path: &Path) -> Result<BTreeMap<String, i32>, String> {
    use std::io::Read;
    let file =
        fs::File::open(jar_path).map_err(|error| format!("{}: {error}", jar_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut entry = archive
        .by_name("net/minecraft/world/level/material/Fluids.class")
        .map_err(|error| format!("Fluids: {error}"))?;
    let mut data = Vec::new();
    entry
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    let class = classfile::parse(&data).map_err(|error| format!("Fluids: {error:?}"))?;
    let mut out = BTreeMap::new();
    for field in &class.fields {
        if field
            .descriptor
            .starts_with("Lnet/minecraft/world/level/material/")
        {
            let id = out.len() as i32;
            out.insert(field.name.to_lowercase(), id);
        }
    }
    Ok(out)
}
