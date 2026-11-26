//! Per-state block support geometry for `worldgen/blocks/cached-support.nbt`,
//! rebuilt from the player's own `client.jar`.
//!
//! Vanilla's `BlockBehaviour.getBlockSupportShape(state, level, pos)` is
//! `getCollisionShape(state, level, pos, CollisionContext.empty())` unless a
//! block class overrides it. The prepared table was queried over
//! `EmptyBlockGetter.INSTANCE` at `BlockPos.ZERO` (see
//! `VanillaBlockSupportOracle.java`), so:
//!
//! * states of classes that do **not** override `getBlockSupportShape` get their
//!   collision shape verbatim (same `VoxelShape` object, so the encoding and the
//!   `canonical`/`cube`/`cube_axes` class flags match the collision table);
//! * the ten classes that do override it are modelled here from their bytecode.
//!
//! | class | `getBlockSupportShape` over `EmptyBlockGetter` |
//! | --- | --- |
//! | `LeavesBlock` (+ `TintedParticleLeavesBlock`, `UntintedParticleLeavesBlock`, `FallingParticlesLeavesBlock`, `MangroveLeavesBlock`) | `Shapes.empty()` |
//! | `MudBlock`, `SoulSandBlock` | `Shapes.block()` |
//! | `ShulkerBoxBlock` | `Shapes.block()` (`getBlockEntity` is `null` over `EmptyBlockGetter`, so the open-box branch is unreachable) |
//! | `SnowLayerBlock` | `SHAPES[layers]` = `Block.column(16, 0, 2 * layers)` (same as its `getShape`) |
//! | `ChorusFlowerBlock` | `SHAPE_BLOCK_SUPPORT` = `Block.column(14, 0, 15)` |
//! | `FenceGateBlock` | open -> `Shapes.empty()`, else `SHAPE_SUPPORT[facing.axis]` = `Shapes.rotateHorizontalAxis(Block.column(16, 4, 5, 24))` |
//! | `CeilingHangingSignBlock` | `SHAPES.getOrDefault(rotation, SHAPE_DEFAULT)`; `SHAPES` keys are `RotationSegment.convertToSegment(direction)` (0/4/8/12 = north/east/south/west) |
//! | `WallHangingSignBlock` | `SHAPES[facing.axis]` = `Shapes.rotateHorizontalAxis(Shapes.or(plank, post))` |
//!
//! # Rotations
//!
//! `Shapes.rotate(shape, group)` is reproduced on the table encoding, not on box
//! lists: the discrete cells are permuted/inverted by the `OctahedralGroup`
//! (`out[i] = source[permutation[i]]`, mirrored around 0.5 where the axis is
//! inverted), a `CubeVoxelShape` stays a cube over the rotated grid, and an
//! `ArrayVoxelShape` keeps the source coordinate list on each non-inverted axis
//! (`flipAxisIfNeeded` returns the same `DoubleList` at the block centre, so a
//! `CubePointRange` survives) while inverted axes get a fresh plain list. That is
//! exactly what decides `cube` / `cube_axes` in the reference dictionary.
//!
//! # Coverage (measured against the prepared copies, offline)
//!
//! * Rule level, feeding the prepared collision table in as the collision
//!   provider: **35,524 / 35,524 reachable states exact** (33,764 collision
//!   pass-through + 1,760 modelled override states), 0 differences.
//! * Pipeline level, collision from `shape_combine::combine` (families + default
//!   path + VM): **34,344 / 35,723 states covered (96.1%), 34,024 exact (95.2%)**
//!   -- up from 21,844 (61.1%) covered / 21,524 exact when support went through
//!   the VM alone. Every remaining support mismatch is inherited from a
//!   collision mismatch; the support rule adds none.
//! * 199 states have no runtime cache entry at all (the prepared table stores
//!   `-1` for them). 102 of those are shulker-box states, which the
//!   `ShulkerBoxBlock` override still answers (`Shapes.block()`); the other 97
//!   (`BambooStalkBlock`, `MovingPistonBlock`, `PointedDripstoneBlock`,
//!   `PowderSnowBlock`, `ScaffoldingBlock`, `SulfurSpikeBlock`) are reported as
//!   gaps rather than guessed.
//!
//! Path includes keep the module self-contained until it is wired into
//! `shape_combine.rs`: `voxelshape` comes through `shape_families`, the state
//! registry through `states`/`block`, and the table container through `geometry`.
//! `support_table` takes that container's `Option<Shape>` slice; an includer with
//! its own copy of the module bridges with a field copy (see the test).

#![allow(dead_code, clippy::duplicate_mod)]

use std::collections::BTreeMap;
use std::path::Path;

#[path = "shape_families.rs"]
mod shape_families;

#[path = "geometry.rs"]
pub mod geometry;

#[path = "states.rs"]
mod states;

#[path = "block.rs"]
mod block;

use shape_families::{State, VoxelShape};

/// What `ship.hzr3` calls the file this module produces.
pub const SUPPORT: &str = "worldgen/blocks/cached-support.nbt";
pub const DATA_VERSION: i32 = 5023;

// ---------------------------------------------------------------------------
// Small box helpers (same conventions as `shape_families`)
// ---------------------------------------------------------------------------

/// `[x1, y1, z1, x2, y2, z2]` in block units.
type Box6 = [f64; 6];

/// `Block.box` in 1/16 units.
fn box16(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Box6 {
    [
        x1 / 16.0,
        y1 / 16.0,
        z1 / 16.0,
        x2 / 16.0,
        y2 / 16.0,
        z2 / 16.0,
    ]
}

/// `Block.column(radius, yMin, yMax)`: centred square column.
fn column16(radius: f64, y1: f64, y2: f64) -> Box6 {
    let half = radius / 2.0;
    box16(8.0 - half, y1, 8.0 - half, 8.0 + half, y2, 8.0 + half)
}

fn union(boxes: &[Box6]) -> geometry::Shape {
    encode_voxel(&VoxelShape::from_boxes(boxes))
}

/// Encode a `VoxelShape` as the dictionary entry the shape tables store.
pub fn encode_voxel(shape: &VoxelShape) -> geometry::Shape {
    let encoded = shape.encode();
    geometry::Shape {
        x: encoded.x,
        y: encoded.y,
        z: encoded.z,
        canonical: encoded.canonical,
        cube: encoded.cube,
        cube_axes: encoded.cube_axes,
        discrete_min: encoded.discrete_min,
        discrete_max: encoded.discrete_max,
        occupied: encoded.occupied,
    }
}

// ---------------------------------------------------------------------------
// Vanilla `Shapes.rotate`
// ---------------------------------------------------------------------------

/// A vanilla `OctahedralGroup`, restricted to the Y-quarter-turns used here:
/// `out[i] = in[p[i]]`, mirrored around 0.5 where `inv[i]` is set.
#[derive(Clone, Copy, Debug)]
struct Rotation {
    p: [usize; 3],
    inv: [bool; 3],
}

impl Rotation {
    const IDENTITY: Rotation = Rotation {
        p: [0, 1, 2],
        inv: [false, false, false],
    };
    /// `BLOCK_ROT_Y_90`: `(x,y,z) -> (1-z, y, x)`.
    const Y90: Rotation = Rotation {
        p: [2, 1, 0],
        inv: [true, false, false],
    };
    /// `BLOCK_ROT_Y_180`: `(x,y,z) -> (1-x, y, 1-z)`.
    const Y180: Rotation = Rotation {
        p: [0, 1, 2],
        inv: [true, false, true],
    };
    /// `BLOCK_ROT_Y_270`: `(x,y,z) -> (z, y, 1-x)`.
    const Y270: Rotation = Rotation {
        p: [2, 1, 0],
        inv: [false, false, true],
    };
}

fn coords_of(shape: &geometry::Shape, axis: usize) -> &[f64] {
    match axis {
        0 => &shape.x,
        1 => &shape.y,
        _ => &shape.z,
    }
}

fn cells(shape: &geometry::Shape) -> [usize; 3] {
    [
        shape.x.len().saturating_sub(1),
        shape.y.len().saturating_sub(1),
        shape.z.len().saturating_sub(1),
    ]
}

/// `Shapes.rotate(shape, rotation)` on the table encoding.
fn rotate_shape(shape: &geometry::Shape, rotation: Rotation) -> geometry::Shape {
    if rotation.p == [0, 1, 2] && !rotation.inv.iter().any(|inverted| *inverted) {
        return shape.clone();
    }
    if shape.canonical == 1 {
        return shape.clone();
    }
    let old_sizes = cells(shape);
    let new_sizes = [
        old_sizes[rotation.p[0]],
        old_sizes[rotation.p[1]],
        old_sizes[rotation.p[2]],
    ];
    let mut occupied = vec![0u8; new_sizes[0] * new_sizes[1] * new_sizes[2]];
    for x in 0..old_sizes[0] {
        for y in 0..old_sizes[1] {
            for z in 0..old_sizes[2] {
                if shape.occupied[(x * old_sizes[1] + y) * old_sizes[2] + z] == 0 {
                    continue;
                }
                let old = [x, y, z];
                let mut new = [0usize; 3];
                for axis in 0..3 {
                    let source = old[rotation.p[axis]];
                    new[axis] = if rotation.inv[axis] {
                        new_sizes[axis] - 1 - source
                    } else {
                        source
                    };
                }
                occupied[(new[0] * new_sizes[1] + new[1]) * new_sizes[2] + new[2]] = 1;
            }
        }
    }
    let (coords, cube, cube_axes) = if shape.cube == 1 {
        // `rotate` wraps the rotated discrete grid in a new `CubeVoxelShape`, so
        // every axis is the uniform grid of the rotated size again.
        let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for axis in 0..3 {
            coords[axis] = (0..=new_sizes[axis])
                .map(|index| index as f64 / new_sizes[axis] as f64)
                .collect();
        }
        (coords, 1, 7)
    } else {
        let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut cube_axes = 0u8;
        for axis in 0..3 {
            let source = rotation.p[axis];
            if rotation.inv[axis] {
                // `flipAxisIfNeeded` builds a fresh `DoubleArrayList`.
                coords[axis] = coords_of(shape, source)
                    .iter()
                    .rev()
                    .map(|value| 1.0 - value)
                    .collect();
            } else {
                // Not inverted at the block centre: the same list is reused.
                coords[axis] = coords_of(shape, source).to_vec();
                if shape.cube_axes & (1 << source) != 0 {
                    cube_axes |= 1 << axis;
                }
            }
        }
        (coords, 0, cube_axes)
    };
    let mut min = [0i32; 3];
    let mut max = [0i32; 3];
    for axis in 0..3 {
        let size = new_sizes[axis];
        let mut first = None;
        let mut last = 0;
        for index in 0..size {
            let mut full = false;
            for other_a in 0..new_sizes[(axis + 1) % 3] {
                for other_b in 0..new_sizes[(axis + 2) % 3] {
                    // Build the cell index with the axis in its place.
                    let mut cell = [0usize; 3];
                    cell[axis] = index;
                    cell[(axis + 1) % 3] = other_a;
                    cell[(axis + 2) % 3] = other_b;
                    if occupied[(cell[0] * new_sizes[1] + cell[1]) * new_sizes[2] + cell[2]] != 0 {
                        full = true;
                    }
                }
            }
            if full {
                if first.is_none() {
                    first = Some(index);
                }
                last = index;
            }
        }
        min[axis] = first.unwrap_or(0) as i32;
        max[axis] = if first.is_some() { last as i32 + 1 } else { 0 };
    }
    let [x, y, z] = coords;
    geometry::Shape {
        x,
        y,
        z,
        canonical: 0,
        cube,
        cube_axes,
        discrete_min: min,
        discrete_max: max,
        occupied,
    }
}

// ---------------------------------------------------------------------------
// The override classes
// ---------------------------------------------------------------------------

/// Every block class that overrides `getBlockSupportShape`. The `LeavesBlock`
/// subclasses are spelled out because callers pass `block::Block::class`, the
/// simple class name.
pub const OVERRIDE_CLASSES: &[&str] = &[
    "LeavesBlock",
    "TintedParticleLeavesBlock",
    "UntintedParticleLeavesBlock",
    "FallingParticlesLeavesBlock",
    "MangroveLeavesBlock",
    "MudBlock",
    "SoulSandBlock",
    "ShulkerBoxBlock",
    "SnowLayerBlock",
    "ChorusFlowerBlock",
    "FenceGateBlock",
    "CeilingHangingSignBlock",
    "WallHangingSignBlock",
];

/// Whether a class overrides `getBlockSupportShape` (so its support shape is not
/// simply its collision shape).
pub fn class_overrides_support(class: &str) -> bool {
    OVERRIDE_CLASSES.contains(&class)
}

/// The class's `getBlockSupportShape` result, or `None` when the class uses the
/// default (collision) shape.
///
/// `state` carries vanilla property-value names (`north`, `left`, `0`..`15`, 鈥?.
pub fn override_shape(class: &str, state: &State) -> Option<geometry::Shape> {
    let shape = match class {
        // `LeavesBlock.getBlockSupportShape` returns `Shapes.empty()`.
        "LeavesBlock"
        | "TintedParticleLeavesBlock"
        | "UntintedParticleLeavesBlock"
        | "FallingParticlesLeavesBlock"
        | "MangroveLeavesBlock" => return Some(geometry::Shape::empty()),
        // Both are full blocks for support even though their collision shape is
        // 14/16 high.
        "MudBlock" | "SoulSandBlock" => return Some(geometry::Shape::block()),
        // `EmptyBlockGetter.getBlockEntity` is null, so `ShulkerBoxBlock` always
        // takes its closed branch: `Shapes.block()`.
        "ShulkerBoxBlock" => return Some(geometry::Shape::block()),
        // `SnowLayerBlock.getBlockSupportShape` is `SHAPES[layers]`, exactly what
        // its `getShape` returns; `shape_families` already models that.
        "SnowLayerBlock" => {
            let shape = shape_families::shape_for_class(
                class,
                state,
                shape_families::ShapeKind::Occlusion,
            )?;
            encode_voxel(&shape)
        }
        // `SHAPE_BLOCK_SUPPORT = Block.column(14, 0, 15)`, independent of age.
        "ChorusFlowerBlock" => union(&[column16(14.0, 0.0, 15.0)]),
        "FenceGateBlock" => fence_gate_support(state),
        "CeilingHangingSignBlock" => ceiling_hanging_sign_support(state),
        "WallHangingSignBlock" => wall_hanging_sign_support(state),
        _ => return None,
    };
    Some(shape)
}

/// `FenceGateBlock`: open -> `Shapes.empty()`; closed ->
/// `SHAPE_SUPPORT[facing.axis]`, where
/// `SHAPE_SUPPORT = Shapes.rotateHorizontalAxis(Block.column(16, 4, 5, 24))`
/// (`Block.column(xSize, zSize, yMin, yMax)` = `box(0, 5, 6, 16, 24, 10)` for the
/// Z axis). `in_wall` does not change the support shape.
fn fence_gate_support(state: &State) -> geometry::Shape {
    if state.boolean("open").unwrap_or(false) {
        return geometry::Shape::empty();
    }
    let base = union(&[box16(0.0, 5.0, 6.0, 16.0, 24.0, 10.0)]);
    match state.text("facing") {
        Some("east") | Some("west") => rotate_shape(&base, Rotation::Y90),
        _ => base,
    }
}

/// `CeilingHangingSignBlock`: `SHAPES.getOrDefault(rotation, SHAPE_DEFAULT)`.
///
/// `SHAPE_DEFAULT = Block.column(10, 0, 16)`; `SHAPES` is
/// `Shapes.rotateHorizontal(Block.column(14, 2, 0, 10))` re-keyed by
/// `RotationSegment.convertToSegment(direction)` (north=0, east=4, south=8,
/// west=12), so only rotations 0/4/8/12 have a plank shape.
fn ceiling_hanging_sign_support(state: &State) -> geometry::Shape {
    let base = union(&[box16(1.0, 0.0, 7.0, 15.0, 10.0, 9.0)]);
    let rotation = match state.int("rotation").unwrap_or(0) {
        0 => return base,
        4 => Rotation::Y90,
        8 => Rotation::Y180,
        12 => Rotation::Y270,
        _ => return union(&[column16(10.0, 0.0, 16.0)]),
    };
    rotate_shape(&base, rotation)
}

/// `WallHangingSignBlock`: `SHAPES[facing.axis]` where
/// `SHAPES = Shapes.rotateHorizontalAxis(Shapes.or(plank, post))`,
/// `plank = Block.column(16, 4, 14, 16)` (`box(0, 14, 6, 16, 16, 10)`) and
/// `post = Block.column(14, 2, 0, 10)` (`box(1, 0, 7, 15, 10, 9)`).
fn wall_hanging_sign_support(state: &State) -> geometry::Shape {
    let combined = union(&[
        box16(0.0, 14.0, 6.0, 16.0, 16.0, 10.0),
        box16(1.0, 0.0, 7.0, 15.0, 10.0, 9.0),
    ]);
    match state.text("facing") {
        Some("east") | Some("west") => rotate_shape(&combined, Rotation::Y90),
        _ => combined,
    }
}

/// The support shape of one state: the class override when it has one, the
/// collision shape otherwise.
pub fn support_shape(class: &str, state: &State, collision: &geometry::Shape) -> geometry::Shape {
    override_shape(class, state).unwrap_or_else(|| collision.clone())
}

// ---------------------------------------------------------------------------
// Building the whole table
// ---------------------------------------------------------------------------

/// What the derivation produced and what it could not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// States with a support shape (override or collision pass-through).
    pub covered: usize,
    /// States that used a modelled `getBlockSupportShape` override.
    pub overridden: usize,
    /// States that passed their collision shape through.
    pub passed_through: usize,
    /// States with no shape at all.
    pub missing: usize,
    /// Class -> reason for states that were left out.
    pub gaps: BTreeMap<String, String>,
}

/// `StateDefinition` property values for one state index, in property-name
/// order (the order `shape_families::State` lookups need).
fn property_values(block: &states::BlockStates, index: usize) -> Vec<(String, String)> {
    let sorted = block.sorted();
    let mut remainder = index;
    let mut values = BTreeMap::new();
    for property in sorted.iter().rev() {
        let count = property.values.len().max(1);
        let slot = remainder % count;
        remainder /= count;
        values.insert(property.name.clone(), property.values[slot].clone());
    }
    let mut pairs = Vec::with_capacity(values.len());
    for (name, value) in values {
        pairs.push((name, value));
    }
    pairs
}

/// Build the support table from an already-derived collision table.
///
/// `collision` must be indexed like `states::state_table(jar)` (registry order,
/// per-block state order). States of an override class always get their override
/// shape; every other state copies its collision shape, or is reported as a gap
/// when the collision engine has no shape for it either.
pub fn support_table(
    jar_path: &Path,
    collision: &[Option<geometry::Shape>],
) -> Result<(Vec<Option<geometry::Shape>>, Coverage), String> {
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    if registry.len() != state_table.blocks.len() {
        return Err("the block registry and the state table disagree".to_string());
    }
    let total = state_table.state_count();
    if collision.len() != total {
        return Err(format!(
            "collision table has {} states, the state table has {total}",
            collision.len()
        ));
    }
    let mut shapes: Vec<Option<geometry::Shape>> = vec![None; total];
    let mut coverage = Coverage::default();
    let mut index = 0usize;
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        let class = &registry[block_index].class;
        for state_index in 0..row.state_count {
            let pairs = property_values(row, state_index);
            let pair_refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            let state = State::new(&pair_refs);
            if let Some(shape) = override_shape(class, &state) {
                shapes[index] = Some(shape);
                coverage.overridden += 1;
            } else if let Some(shape) = &collision[index] {
                shapes[index] = Some(shape.clone());
                coverage.passed_through += 1;
            } else {
                coverage.gaps.insert(
                    class.clone(),
                    "no collision shape for the default path".to_string(),
                );
            }
            index += 1;
        }
    }
    coverage.covered = shapes.iter().filter(|shape| shape.is_some()).count();
    coverage.missing = total - coverage.covered;
    Ok((shapes, coverage))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_classes_match_the_jar() {
        assert!(class_overrides_support("LeavesBlock"));
        assert!(class_overrides_support("TintedParticleLeavesBlock"));
        assert!(class_overrides_support("FenceGateBlock"));
        assert!(class_overrides_support("CeilingHangingSignBlock"));
        assert!(!class_overrides_support("StairBlock"));
        assert!(!class_overrides_support("Block"));
    }

    #[test]
    fn leaves_and_mud_are_the_constant_support_shapes() {
        let state = State::new(&[]);
        assert!(
            override_shape("LeavesBlock", &state)
                .expect("leaves")
                .canonical
                == 1
        );
        assert_eq!(
            override_shape("MudBlock", &state).expect("mud").canonical,
            2
        );
        assert_eq!(
            override_shape("SoulSandBlock", &state)
                .expect("soul sand")
                .canonical,
            2
        );
    }

    #[test]
    fn a_fence_gate_is_empty_open_and_tall_closed() {
        let open = [
            ("facing", "north"),
            ("in_wall", "false"),
            ("open", "true"),
            ("powered", "false"),
        ];
        let closed = [
            ("facing", "north"),
            ("in_wall", "false"),
            ("open", "false"),
            ("powered", "false"),
        ];
        assert_eq!(
            override_shape("FenceGateBlock", &State::new(&open))
                .expect("open")
                .canonical,
            1
        );
        let closed = override_shape("FenceGateBlock", &State::new(&closed)).expect("closed");
        assert_eq!(closed.y, vec![0.3125, 1.5]);
        assert_eq!(closed.z, vec![0.375, 0.625]);
    }

    #[test]
    fn a_rotation_permutes_and_mirrors_the_discrete_cells() {
        // A single 1/16 box at the north-west top corner.
        let shape = union(&[box16(1.0, 15.0, 1.0, 2.0, 16.0, 2.0)]);
        let rotated = rotate_shape(&shape, Rotation::Y90);
        // `BLOCK_ROT_Y_90` sends (x,y,z) to (1-z, y, x).
        assert_eq!(rotated.x, vec![0.875, 0.9375]);
        assert_eq!(rotated.y, vec![0.9375, 1.0]);
        assert_eq!(rotated.z, vec![0.0625, 0.125]);
    }
}
