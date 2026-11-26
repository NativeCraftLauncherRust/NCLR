//! Exact `block-behaviours.json#states` rows, composed from the validated column
//! modules:
//!
//! * `behaviour_states` provides the base row set and the columns without a
//!   dedicated module (fluid, fluidAmount, falling, weak, strong, signalError);
//! * `behaviour_flags_fix` overwrites the flags column (sorted property order,
//!   35,723/35,723 validated);
//! * `behaviour_sturdy::sturdy_masks` overwrites the sturdy mask where it has a
//!   value (shape-engine derived);
//! * `behaviour_instrument_final` overwrites the instrument column per block.
//!
//! The test reports per-column agreement with the prepared export so a wrong
//! column cannot hide behind the others.

#![allow(dead_code, clippy::duplicate_mod, clippy::empty_line_after_doc_comments, clippy::needless_range_loop)]

use std::path::Path;

use serde_json::Value;

#[path = "behaviour_states.rs"]
#[allow(unused, clippy::all)]
mod behaviour_states;

#[path = "behaviour_flags_fix.rs"]
#[allow(unused, clippy::all)]
mod behaviour_flags_fix;

#[path = "behaviour_sturdy.rs"]
#[allow(unused, clippy::all)]
mod behaviour_sturdy;

#[path = "behaviour_instrument_final.rs"]
#[allow(unused, clippy::all)]
mod behaviour_instrument_final;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub states: usize,
    pub flags_patched: usize,
    pub sturdy_patched: usize,
    pub instruments_patched: usize,
    pub notes: Vec<String>,
}

/// Compose the rows.
pub fn rows(jar_path: &Path) -> Result<(Vec<Value>, Coverage), String> {
    let states = behaviour_states::behaviour_states(jar_path)?;
    let mut coverage = Coverage {
        states: states.rows.len(),
        ..Default::default()
    };
    let (flags, _) = behaviour_flags_fix::flags(jar_path)?;
    let (instruments, _) = behaviour_instrument_final::block_instruments(jar_path)?;
    let (sturdy, _sturdy_coverage) = behaviour_sturdy::sturdy_masks(jar_path)?;
    let block_ids = behaviour_states::state_block_ids(jar_path)?;
    let mut out = Vec::with_capacity(states.rows.len());
    for (index, row) in states.rows.iter().enumerate() {
        let mut values = vec![
            Value::from(row.sturdy),
            Value::from(row.flags),
            Value::from(row.fluid),
            Value::from(row.fluid_amount),
            Value::from(row.falling),
            Value::from(row.weak),
            Value::from(row.strong),
            Value::from(row.signal_error),
            Value::String(row.instrument.clone()),
        ];
        if let Some(value) = flags.get(index) {
            if *value != row.flags {
                coverage.flags_patched += 1;
            }
            values[1] = Value::from(*value);
        }
        if let Some(Some(mask)) = sturdy.get(index) {
            if *mask != row.sturdy {
                coverage.sturdy_patched += 1;
            }
            values[0] = Value::from(*mask);
        }
        if let Some(block) = block_ids.get(index) {
            if let Some(instrument) = instruments.get(block) {
                if *instrument != row.instrument {
                    coverage.instruments_patched += 1;
                }
                values[8] = Value::String(instrument.clone());
            }
        }
        out.push(Value::Array(values));
    }
    Ok((out, coverage))
}

/// Per-column agreement with a prepared row set: 9 counts, one per column.
pub fn column_agreement(rows: &[Value], reference: &[Value]) -> Vec<usize> {
    let mut counts = vec![0usize; 9];
    for (mine, theirs) in rows.iter().zip(reference) {
        let (Some(mine), Some(theirs)) = (mine.as_array(), theirs.as_array()) else {
            continue;
        };
        for index in 0..9 {
            if mine.get(index) == theirs.get(index) {
                counts[index] += 1;
            }
        }
    }
    counts
}
