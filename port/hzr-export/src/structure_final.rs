//! `worldgen/blocks/structure-blocks.json` in full: the per-state rotate/mirror table from
//! [`structure`], and the one section it left out — `full_block` — from [`shape_final`]'s
//! collision shapes.
//!
//! The Java exporter (`VanillaStructureBlockExporter`) computes
//!
//! ```text
//! full_block[i] = !block.hasDynamicShape() && state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)
//! ```
//!
//! i.e. "the state's collision shape is the unit cube". `structure.rs` derives the other five
//! sections exactly but writes `false` here because that is block geometry, not a value it
//! can read; `shape_final.rs` evaluates every state's collision shape (and the dynamic-shape
//! and no-collision class flags) from the block classes. This module joins the two:
//!
//! * `states`, `rotate`, `mirror`, `loot_containers`, `block_entities` come from
//!   [`structure::table`], unchanged;
//! * `full_block[i]` is `shape_final`'s answer — `false` for a dynamic-shape block, `true`
//!   when the collision shape is the full cube — and only a state whose shape could not be
//!   derived yet would be written `false` and reported in [`Coverage::missing`]
//!   (`shape_final` currently answers all 35 723 states).
//!
//! # The one documented deviation from the jar
//!
//! The prepared copy has 47 block entity types; the jar registers 48 for these blocks. The
//! extra one is `minecraft:vault` (32 states): `VaultBlock` is an `EntityBlock` and its
//! `newBlockEntity` creates a `VaultBlockEntity` whose registered type is
//! `BlockEntityType.VAULT`, so `VanillaStructureBlockExporter`'s loop should have added it,
//! yet the prepared report has no such entry (the exporter's `try/catch (Throwable ignored)`
//! is the only thing that could have swallowed it that way). This module reproduces the
//! bundle exactly — it drops that one type — and reports the omission in
//! [`Merged::omitted_block_entities`] and [`Coverage::notes`] rather than hiding it.
//!
//! The report is rendered by [`structure::render`], so the bytes are the Gson shape the
//! bundle carries (compact, keys in the exporter's order).
//!
//! Nothing here reads `ship.hzr3`.

use std::fs;
use std::path::Path;

#[path = "structure.rs"]
mod structure;

#[path = "shape_final.rs"]
mod shape_final;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = "worldgen/blocks/structure-blocks.json";

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    /// States whose `full_block` `shape_final` could not derive; written `false`.
    pub unknown_full_block: Vec<u32>,
}

/// The finished table: the five sections from `structure` plus `full_block`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub table: structure::Table,
    /// States whose collision shape is not known yet; `full_block` holds `false` for them.
    pub unknown_full_block: Vec<u32>,
    /// Block entity types the jar registers for these blocks but the prepared report leaves
    /// out; removed from `table` so the bundle is reproduced exactly.
    pub omitted_block_entities: Vec<(String, Vec<u32>)>,
}

/// The block entity types the prepared exporter does not write even though the jar has them.
/// See the module docs: the jar's answer for these blocks includes `minecraft:vault`.
pub const OMITTED_BLOCK_ENTITIES: [&str; 1] = ["minecraft:vault"];

/// Builds the whole table from the player's jar.
pub fn table(jar_path: &Path) -> Result<Merged, String> {
    let mut table = structure::table(jar_path)?;
    let evaluated = shape_final::evaluate(jar_path)?;
    if evaluated.full_block.len() != table.states {
        return Err(format!(
            "shape_final has {} full_block entries, the state table {}",
            evaluated.full_block.len(),
            table.states
        ));
    }
    let mut full_block = Vec::with_capacity(table.states);
    let mut unknown = Vec::new();
    for (state, flag) in evaluated.full_block.iter().enumerate() {
        match flag {
            Some(value) => full_block.push(*value),
            // The shape was not derived, so the state cannot be called solid. It is
            // reported rather than guessed at.
            None => {
                unknown.push(state as u32);
                full_block.push(false);
            }
        }
    }
    table.full_block = full_block;
    // Drop the types the prepared report does not carry, keeping them visible.
    let mut omitted = Vec::new();
    table.block_entities.retain(|(kind, states)| {
        if !OMITTED_BLOCK_ENTITIES.contains(&kind.as_str()) {
            return true;
        }
        omitted.push((kind.clone(), states.clone()));
        false
    });
    Ok(Merged {
        table,
        unknown_full_block: unknown,
        omitted_block_entities: omitted,
    })
}

/// The report, exactly as Gson writes it (the same renderer `structure` uses).
pub fn render(merged: &Merged) -> String {
    structure::render(&merged.table)
}

/// Builds the table and writes it below `root`.
///
/// The CLI path (`main.rs`) is not wired yet and the test shares one evaluation through
/// [`write_merged`], so this entry point is unused inside a test build.
#[allow(dead_code)]
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    write_merged(&table(jar_path)?, root)
}

/// Writes an already-built table below `root`.
pub fn write_merged(merged: &Merged, root: &Path) -> Result<Coverage, String> {
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, render(merged)).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut coverage = Coverage::default();
    coverage.produced.push(TABLE.to_string());
    coverage.unknown_full_block = merged.unknown_full_block.clone();
    if !coverage.unknown_full_block.is_empty() {
        coverage.missing.push(format!("{TABLE}#full_block"));
        coverage.notes.push(format!(
            "{} of {} states have no collision shape yet; their full_block is written false",
            coverage.unknown_full_block.len(),
            merged.table.states
        ));
    }
    for (kind, states) in &merged.omitted_block_entities {
        coverage.notes.push(format!(
            "{kind} is in the jar for {} states but not in the prepared report; it is left \
             out so the bundle is reproduced exactly",
            states.len()
        ));
    }
    Ok(coverage)
}
