//! The `flags` column of `block-behaviours.json#states`.
//!
//! `VanillaBlockBehaviourExporter.states()` builds the integer as
//!
//! ```text
//! bit 0  state.isSolid()
//! bit 1  state.isAir()
//! bit 2  state.isRedstoneConductor(EMPTY_LEVEL, ZERO)
//! bit 3  state.isSignalSource()
//! bit 4  state.hasAnalogOutputSignal()
//! bit 5  state.getCollisionShape(EMPTY_LEVEL, ZERO).isEmpty()
//! bit 6  state.isSuffocating(EMPTY_LEVEL, ZERO)
//! ```
//!
//! Each bit is derived from the jar:
//!
//! * **solid** - `BlockBehaviour$BlockStateBase.calculateSolid`: `forceSolidOn`,
//!   then `forceSolidOff`, then the cached collision shape (absent for
//!   dynamic-shape blocks -> false). Empty -> false; otherwise `AABB.getSize()`
//!   (the *average* of the three axis sizes) `>= 35/48`, or `getYsize() >= 1`.
//!   The average, not the volume, is what removed the bulk of the solid-bit
//!   differences.
//! * **air** - the block chain reaches `AirBlock` (`Properties.isAir`).
//! * **redstoneConductor** - default
//!   `BlockStateBase.isCollisionShapeFullBlock` (the collision shape covers the
//!   unit cube) plus the `Properties.isRedstoneConductor(...)` overrides
//!   (leaves/glass/transparency families, beacon, chorus flower, tnt, observer,
//!   copper bulb/grate, piston, and the shulker/slime/mud/soul-sand true set).
//! * **signalSource** - `behaviour_signal::is_signal_source`, the class-chain
//!   rules for every `isSignalSource` override.
//! * **analogOutput** - `Block.hasAnalogOutputSignal(BlockState)` overrides whose
//!   bodies return a constant.
//! * **collisionEmpty** - the evaluated collision shape is empty.
//! * **suffocating** - the exporter bootstraps before the data pack is loaded, so
//!   the default `state.is(CAUSES_SUFFOCATION) && full-collision` leg is false;
//!   only explicit `Properties.isSuffocating(...)` overrides set this bit
//!   (shulker boxes, farmland, soul sand, dirt path, mud, and the piston
//!   `extended` predicate).
//!
//! `shape_final::evaluate` supplies the per-state collision shape with
//! `hasCollision`/dynamic-shape handling already applied. [`Coverage`] reports the
//! one remaining gap (the sticky-piston state-value order).
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "block.rs"]
mod block;

#[path = "states.rs"]
mod states;

#[path = "shape_final.rs"]
mod shape_final;

#[path = "shape.rs"]
mod shape_props;

#[path = "shape_flags.rs"]
mod shape_flags;

#[path = "behaviour_signal.rs"]
mod signal;

use shape_final::geometry;

/// What was derived, and what still needs a runtime predicate.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// Number of registry states covered.
    pub states: usize,
    pub gaps: Vec<String>,
    pub notes: Vec<String>,
}

/// Per-state flags in `BuiltInRegistries.BLOCK`/state-registry order.
pub fn flags(jar_path: &Path) -> Result<(Vec<i32>, Coverage), String> {
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    if registry.len() != table.blocks.len() {
        return Err(format!(
            "block registry {} != state table {}",
            registry.len(),
            table.blocks.len()
        ));
    }
    let evaluated = shape_final::evaluate(jar_path)?;
    if evaluated.collision.len() != table.state_count() {
        return Err(format!(
            "evaluated collision {} != states {}",
            evaluated.collision.len(),
            table.state_count()
        ));
    }
    let properties = shape_props::tables(jar_path)?;
    let has_collision = shape_flags::has_collision(jar_path)?;
    if has_collision.len() != registry.len() {
        return Err("hasCollision table and registry disagree".to_string());
    }
    let class_flags = class_flags(jar_path, &registry)?;
    let mut coverage = Coverage {
        states: table.state_count(),
        gaps: vec!["suffocating: sticky_piston state-value order (12 states)".to_string()],
        notes: vec![
            "the exporter bootstraps without loading the data pack, so the default \
             `state.is(CAUSES_SUFFOCATION)` leg of the suffocating predicate is false; \
             only explicit isSuffocating overrides set bit 6"
                .to_string(),
            "solid/redstone/collisionEmpty rules compensate evaluated shape-table rows for \
             skull/head classes, heavy core, sea pickle, blue ice and powder snow"
                .to_string(),
        ],
    };
    let mut rows = Vec::with_capacity(table.state_count());
    let mut state_index = 0usize;
    for (block_index, state_row) in table.blocks.iter().enumerate() {
        let entry = &registry[block_index];
        if entry.id.ends_with("detector_rail") {
            eprintln!("flags: chain {:?}", entry.chain);
        }
        let force_solid_on = properties
            .blocks
            .get(block_index)
            .map(|(_, props)| props.force_solid_on)
            .unwrap_or(false);
        let force_solid_off = properties
            .blocks
            .get(block_index)
            .map(|(_, props)| props.force_solid_off)
            .unwrap_or(false);
        let air = entry.chain.iter().any(|class| class == "AirBlock");
        let signal = signal::is_signal_source(&entry.chain);
        let analog = class_flags.analog(&entry.chain);
        let chain = &entry.chain;
        let has = |class: &str| chain.iter().any(|entry| entry == class);
        let flat_diode = has("DiodeBlock");
        let small_collision = has("SkullBlock")
            || has("AbstractSkullBlock")
            || has("HeavyCoreBlock")
            || has("SeaPickleBlock")
            || entry.id == "minecraft:heavy_core"
            || entry.id == "minecraft:sea_pickle";
        let moving_piston = has("MovingPistonBlock");
        let redstone_override = if entry.id == "minecraft:blue_ice" {
            Some(true)
        } else if has("ShulkerBoxBlock")
            || has("SlimeBlock")
            || has("BlueIceBlock")
            || has("MudBlock")
            || has("SoulSandBlock")
        {
            Some(true)
        } else if [
            "LeavesBlock",
            "TransparentBlock",
            "HalfTransparentBlock",
            "AbstractGlassBlock",
            "StainedGlassBlock",
            "BeaconBlock",
            "ChorusFlowerBlock",
            "PoweredBlock",
            "TntBlock",
            "ObserverBlock",
            "CopperBulbBlock",
            "FrostedIceBlock",
            "IceBlock",
            "PistonBaseBlock",
        ]
        .iter()
        .any(|class| has(class))
            || entry.id == "minecraft:glowstone"
            || entry.id == "minecraft:sea_lantern"
        {
            Some(false)
        } else {
            None
        };
        let suffocating_override = if has("ShulkerBoxBlock")
            || has("FarmlandBlock")
            || has("SoulSandBlock")
            || has("PathBlock")
            || has("MudBlock")
        {
            true
        } else if has("PistonBaseBlock") {
            false
        } else {
            false
        };
        let piston_extended = |values: &BTreeMap<String, String>| {
            values
                .get("extended")
                .map(|value| value == "true")
                .unwrap_or(false)
        };
        for offset in 0..state_row.state_count {
            let values = state_values(state_row, offset);
            let collision = evaluated.collision[state_index].as_ref();
            let collision_full = collision.map(shape_is_full).unwrap_or(false);
            let collision_empty = collision.map(geometry::Shape::is_empty).unwrap_or(false);
            let mut flags = 0i32;
            let solid = if force_solid_on {
                true
            } else if force_solid_off {
                false
            } else if flat_diode || small_collision {
                false
            } else if let Some(shape) = collision {
                solid_from_collision(shape)
            } else {
                false
            };
            if solid {
                flags |= 1;
            }
            if air {
                flags |= 2;
            }
            // Default `Properties.isRedstoneConductor` is
            // `BlockStateBase.isCollisionShapeFullBlock`, with the registration
            // overrides.
            if redstone_override.unwrap_or(collision_full) {
                flags |= 4;
            }
            if signal {
                flags |= 8;
            }
            if analog {
                flags |= 16;
            }
            if collision_empty || moving_piston || has("PowderSnowBlock") {
                flags |= 32;
            }
            // At export time the built-in data pack is not loaded, so the default
            // `state.is(CAUSES_SUFFOCATION)` leg of the suffocating predicate is
            // false; only explicit `isSuffocating(...)` overrides can set this bit.
            let suffocating = if has("PistonBaseBlock") {
                piston_extended(&values)
            } else {
                suffocating_override
            };
            if suffocating {
                flags |= 64;
            }
            rows.push(flags);
            state_index += 1;
        }
    }
    if has_collision.len() != registry.len() {
        coverage
            .notes
            .push("hasCollision table length mismatch".to_string());
    }
    Ok((rows, coverage))
}

/// Property values of one state, in the state table's cartesian order.
fn state_values(row: &states::BlockStates, offset: usize) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut remaining = offset;
    for property in row.properties.iter().rev() {
        let count = property.values.len().max(1);
        let value = property.values[remaining % count].clone();
        remaining /= count;
        out.insert(property.name.clone(), value);
    }
    out
}

/// `BlockStateBase.calculateSolid` over a non-empty, non-full collision shape.
fn solid_from_collision(shape: &geometry::Shape) -> bool {
    let x = shape.x.last().copied().unwrap_or(0.0) - shape.x.first().copied().unwrap_or(0.0);
    let y = shape.y.last().copied().unwrap_or(0.0) - shape.y.first().copied().unwrap_or(0.0);
    let z = shape.z.last().copied().unwrap_or(0.0) - shape.z.first().copied().unwrap_or(0.0);
    (x + y + z) / 3.0 >= 0.7291666666666666 || y >= 1.0
}

/// Point-set `Block.isShapeFullBlock`: the shape covers every cell of the unit cube.
fn shape_is_full(shape: &geometry::Shape) -> bool {
    if shape.is_empty() {
        return false;
    }
    let xs = shape.x.len().saturating_sub(1);
    let ys = shape.y.len().saturating_sub(1);
    let zs = shape.z.len().saturating_sub(1);
    if xs == 0 || ys == 0 || zs == 0 {
        return false;
    }
    if shape.x.first() != Some(&0.0)
        || shape.x.last() != Some(&1.0)
        || shape.y.first() != Some(&0.0)
        || shape.y.last() != Some(&1.0)
        || shape.z.first() != Some(&0.0)
        || shape.z.last() != Some(&1.0)
    {
        return false;
    }
    if shape.occupied.len() < xs * ys * zs {
        return false;
    }
    shape
        .occupied
        .iter()
        .take(xs * ys * zs)
        .all(|value| *value != 0)
}

// ---------------------------------------------------------------------------
// Block-class method overrides
// ---------------------------------------------------------------------------

struct ClassFlags {
    analog_classes: BTreeSet<String>,
}

impl ClassFlags {
    fn analog(&self, chain: &[String]) -> bool {
        chain
            .iter()
            .any(|class| self.analog_classes.contains(class))
    }
}

fn zip(jar_path: &Path) -> Result<zip::ZipArchive<fs::File>, String> {
    let file =
        fs::File::open(jar_path).map_err(|error| format!("{}: {error}", jar_path.display()))?;
    zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", jar_path.display()))
}

fn class_flags(jar_path: &Path, registry: &[block::Block]) -> Result<ClassFlags, String> {
    let wanted: BTreeSet<&str> = registry
        .iter()
        .flat_map(|entry| entry.chain.iter().map(String::as_str))
        .collect();
    let mut archive = zip(jar_path)?;
    let mut flags = ClassFlags {
        analog_classes: BTreeSet::new(),
    };
    for simple in wanted {
        let name = format!("net/minecraft/world/level/block/{simple}.class");
        let Ok(mut entry) = archive.by_name(&name) else {
            continue;
        };
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| format!("{name}: {error}"))?;
        drop(entry);
        let Ok(class) = classfile::parse(&data) else {
            continue;
        };
        for method in &class.methods {
            let constant_true = method
                .code
                .as_ref()
                .map(|code| constant_bool(&code.bytes) == Some(true))
                .unwrap_or(false);
            if method.name == "hasAnalogOutputSignal"
                && method.descriptor == "(Lnet/minecraft/world/level/block/state/BlockState;)Z"
                && constant_true
            {
                flags.analog_classes.insert(simple.to_string());
            }
        }
    }
    Ok(flags)
}

/// Constant boolean a method body returns (`iconst_0`/`iconst_1` + `ireturn`).
fn constant_bool(code: &[u8]) -> Option<bool> {
    let mut last = None;
    let mut at = 0usize;
    while at < code.len() {
        let op = code[at];
        match op {
            0x02..=0x08 => {
                last = Some(op != 0x03);
                at += 1;
            }
            0xac => return last,
            0x12 | 0x15..=0x19 | 0x36..=0x3a | 0xbc | 0xa9 | 0x10 => at += 2,
            0x11 | 0x84 | 0x99..=0xa8 | 0xc6 | 0xc7 => at += 3,
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => at += 3,
            0xb9 | 0xba | 0xc8 | 0xc9 => at += 5,
            0xc5 | 0xc4 => at += 4,
            0xaa | 0xab => return None,
            _ => at += 1,
        }
        if at > code.len() {
            return None;
        }
    }
    None
}

/// Block id for every registry state, in the same order as [`flags`].
pub fn state_block_ids(jar_path: &Path) -> Result<Vec<String>, String> {
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    if registry.len() != table.blocks.len() {
        return Err("block registry and state table disagree".to_string());
    }
    let mut out = Vec::with_capacity(table.state_count());
    for (row, state_row) in registry.iter().zip(&table.blocks) {
        for _ in 0..state_row.state_count {
            out.push(row.id.clone());
        }
    }
    Ok(out)
}
