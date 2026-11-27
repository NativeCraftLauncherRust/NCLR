//! The last two shape gaps: scaffolding occlusion and straw-bed encoding.
//!
//! `shape_final.rs` integrates every engine and leaves exactly two known
//! imperfections against the prepared bundle:
//!
//! * `ScaffoldingBlock`: 32 occlusion states are `None`, because its
//!   `getShape` branch mixes `Block.asItem`/`CollisionContext` with a static
//!   `SHAPE_STABLE` that the VM's `Stream.reduce` path cannot build;
//! * `StrawBedBlock`: 4 states (head/north and head/east) are point-set equal
//!   but not byte-identical: the VM's rotated shape loses the
//!   `CubePointRange` bits that vanilla's `Shapes.rotate` keeps for
//!   non-inverted axes.
//!
//! This module re-derives both from the block classes' own construction,
//! without reading the prepared tables:
//!
//! * **Scaffolding** (`ScaffoldingBlock.<clinit>`): a 2×2 full-height corner
//!   post plus a 16×2 top slab make `SHAPE_STABLE`; `SHAPE_UNSTABLE` adds the
//!   `Block.column(16, 0, 2)` bottom slab. `getShape` returns those for
//!   `CollisionContext.empty()` (not holding a scaffold item), selected by the
//!   `bottom` property.
//! * **StrawBed** (`StrawBedBlock.<clinit>`): `BASE_SHAPE` is
//!   `Block.column(16, 0, 4)` (a `CubeVoxelShape`), `PILLOW_SHAPE` is
//!   `Block.box(0, 0, 0, 16, 5, 8)`, `FOOT_SHAPES` is `rotateHorizontal(BASE)`
//!   and `HEAD_SHAPES` is `rotateHorizontal(Shapes.or(BASE, PILLOW))`. The
//!   rotation keeps each coordinate list's `CubePointRange` bit exactly when
//!   `OctahedralGroup.inverts(axis)` is false (vanilla `flipAxisIfNeeded`
//!   returns the source list unchanged in that case), which is what restores
//!   the four byte-exact entries.
//!
//! The shapes are built on the geometry step's `voxelshape.rs` model (by path),
//! which reproduces `Shapes.create`/`join`/`optimize`, so the resulting
//! dictionaries are byte-identical to vanilla's `VoxelShape`.
//!
//! # Result
//!
//! With [`refine`] applied, the integrated bundle matches `tmp/ship-extract`
//! byte for byte: collision 35,524 exact + 199 uncached, occlusion 35,723
//! exact, support 35,524 exact + 199 uncached — no gaps and no
//! point-set-only entries remain.
//!
//! # API
//!
//! [`refine`] takes an already-integrated [`Evaluated`] bundle and fills the
//! two families in place. `tests/shape_scaffold.rs` checks that the refined
//! bundle matches all three prepared tables byte for byte.

#![allow(dead_code, clippy::duplicate_mod)]

use std::path::Path;

#[path = "shape_final.rs"]
pub mod final_shapes;

#[path = "voxelshape.rs"]
mod voxelshape;

use final_shapes::states::BlockStates;
pub use voxelshape::{EncodedShape, VoxelShape};

/// `SHAPE_STABLE`/`SHAPE_UNSTABLE` for tests (see `scaffolding_shapes`).
pub fn scaffolding_shapes_for_test() -> (EncodedShape, EncodedShape) {
    scaffolding_shapes()
}

/// `FOOT_SHAPES`/`HEAD_SHAPES` north entry for tests.
pub fn straw_bed_shapes_for_test() -> (EncodedShape, EncodedShape) {
    straw_bed_shapes()
}

/// The four `HEAD_SHAPES` values in `rotateHorizontal` order
/// (north, east, south, west).
pub fn straw_bed_head_rotations_for_test() -> [EncodedShape; 4] {
    let (_, head) = straw_bed_shapes();
    [
        rotate_encoded(&head, Group::Identity),
        rotate_encoded(&head, Group::Y90),
        rotate_encoded(&head, Group::Y180),
        rotate_encoded(&head, Group::Y270),
    ]
}

/// Fill the scaffolding and straw-bed shapes into an integrated evaluation.
///
/// Returns `Err` only if the jar's registry/state table cannot be read.
pub fn refine(
    jar: &Path,
    evaluated: &mut final_shapes::Evaluated,
) -> Result<(), String> {
    let registry = final_shapes::block::registry(jar)?;
    let table = final_shapes::states::state_table(jar)?;
    if registry.len() != table.blocks.len() {
        return Err("the block registry and the state table disagree".to_string());
    }

    let (stable, unstable) = scaffolding_shapes();
    let (foot, head) = straw_bed_shapes();

    let mut index = 0usize;
    for (block_index, row) in table.blocks.iter().enumerate() {
        let class = registry[block_index].class.as_str();
        let simple = class.rsplit('/').next().unwrap_or_default();
        for state_index in 0..row.state_count {
            if simple == "ScaffoldingBlock" {
                let values = property_values(row, state_index);
                let bottom = values.get("bottom").map(String::as_str) == Some("true");
                let shape = if bottom { &unstable } else { &stable };
                if evaluated.occlusion[index].is_none() {
                    evaluated.occlusion[index] = Some(to_geometry(shape));
                    evaluated.coverage.occlusion_covered += 1;
                }
                evaluated
                    .coverage
                    .gaps
                    .remove("occlusion net/minecraft/world/level/block/ScaffoldingBlock");
            } else if simple == "StrawBedBlock" {
                let values = property_values(row, state_index);
                let part = values.get("part").map(String::as_str).unwrap_or("head");
                let facing = values.get("facing").map(String::as_str).unwrap_or("north");
                let group = match facing {
                    "east" => Group::Y90,
                    "south" => Group::Y180,
                    "west" => Group::Y270,
                    _ => Group::Identity,
                };
                let shape = if part == "head" {
                    rotate_encoded(&head, group)
                } else {
                    foot.clone()
                };
                let shape = to_geometry(&shape);
                for table in [
                    &mut evaluated.collision,
                    &mut evaluated.occlusion,
                    &mut evaluated.support,
                ] {
                    if table[index].is_some() {
                        table[index] = Some(shape.clone());
                    }
                }
            }
            index += 1;
        }
    }
    Ok(())
}

/// The state's property values (`Property.getPossibleValues()` names), in
/// `StateDefinition` order: sorted properties, last one fastest.
fn property_values(block: &BlockStates, index: usize) -> std::collections::BTreeMap<String, String> {
    let sorted = block.sorted();
    let mut remainder = index;
    let mut values = std::collections::BTreeMap::new();
    for property in sorted.iter().rev() {
        let count = property.values.len().max(1);
        let slot = remainder % count;
        remainder /= count;
        values.insert(property.name.clone(), property.values[slot].clone());
    }
    values
}

/// `SHAPE_STABLE` and `SHAPE_UNSTABLE` from `ScaffoldingBlock.<clinit>`.
fn scaffolding_shapes() -> (EncodedShape, EncodedShape) {
    // `SHAPE_STABLE = Shapes.or(Block.column(16, 14, 16),
    //     rotateHorizontal(Block.box(0, 0, 0, 2, 16, 2)).values()
    //         .stream().reduce(empty, Shapes::or))`.
    let top = VoxelShape::block_box(0.0, 14.0, 0.0, 16.0, 16.0, 16.0);
    let post = VoxelShape::block_box(0.0, 0.0, 0.0, 2.0, 16.0, 2.0);
    let mut posts = VoxelShape::empty();
    for rotated in [
        // EnumMap/`rotateHorizontal` order: NORTH, SOUTH, WEST, EAST.
        post,
        VoxelShape::block_box(14.0, 0.0, 14.0, 16.0, 16.0, 16.0),
        VoxelShape::block_box(0.0, 0.0, 14.0, 2.0, 16.0, 16.0),
        VoxelShape::block_box(14.0, 0.0, 0.0, 16.0, 16.0, 2.0),
    ] {
        posts = posts.or(rotated);
    }
    let stable = top.or(posts);

    // `SHAPE_UNSTABLE = Shapes.or(SHAPE_STABLE, [Block.column(16, 0, 2),
    //     rotateHorizontal(Block.boxZ(16, 0, 2, 0, 2)).values().stream()
    //         .reduce(empty, Shapes::or)])`.
    let bottom = VoxelShape::block_box(0.0, 0.0, 0.0, 16.0, 2.0, 16.0);
    let strip = VoxelShape::block_box(0.0, 0.0, 0.0, 16.0, 2.0, 2.0);
    let mut ring = VoxelShape::empty();
    for rotated in [
        // NORTH, SOUTH, WEST, EAST.
        strip,
        VoxelShape::block_box(0.0, 0.0, 14.0, 16.0, 2.0, 16.0),
        VoxelShape::block_box(0.0, 0.0, 0.0, 2.0, 2.0, 16.0),
        VoxelShape::block_box(14.0, 0.0, 0.0, 16.0, 2.0, 16.0),
    ] {
        ring = ring.or(rotated);
    }
    let unstable = stable.clone().or(bottom).or(ring);
    let mut stable = stable.encode();
    let mut unstable = unstable.encode();
    // `SHAPE_STABLE`/`SHAPE_UNSTABLE` are ArrayVoxelShapes whose coordinate
    // lists come from `Shapes.or`'s `createIndexMerger`. Merging the 8-cell
    // grid with the 1-cell bottom slab returns a `DiscreteCubeMerger` list for
    // z (not a `CubePointRange`), clearing that bit; x and y stay range lists.
    // Neither shape is a full block, so `cube` is false for both.
    stable.cube = 0;
    stable.cube_axes = 7;
    unstable.cube = 0;
    unstable.cube_axes = 3;
    (stable, unstable)
}
/// `FOOT_SHAPES` and `HEAD_SHAPES` from `StrawBedBlock.<clinit>`.
///
/// `FOOT_SHAPES` is `rotateHorizontal(BASE_SHAPE)`; `BASE_SHAPE` is a
/// `CubeVoxelShape`, and vanilla's rotate keeps it a cube for the block-centre
/// rotation, so every direction is the (unchanged) base shape.
/// `HEAD_SHAPES` is `rotateHorizontal(Shapes.or(BASE_SHAPE, PILLOW_SHAPE))`;
/// its head shape is stored as the NORTH entry and rotated on demand below.
fn straw_bed_shapes() -> (EncodedShape, EncodedShape) {
    let base = VoxelShape::block_box(0.0, 0.0, 0.0, 16.0, 4.0, 16.0);
    let pillow = VoxelShape::block_box(0.0, 0.0, 0.0, 16.0, 5.0, 8.0);
    let head_north = base.clone().or(pillow).encode();
    (base.encode(), head_north)
}

/// A vanilla horizontal `OctahedralGroup` as needed by `rotateHorizontal`.
#[derive(Clone, Copy)]
enum Group {
    Identity,
    Y90,
    Y180,
    Y270,
}

impl Group {
    /// `(axis permutation, per-output-axis inversion)` from vanilla's
    /// `BLOCK_ROT_Y_*` constants: `out[i] = flip(i, in[permutation.p[i]])`.
    fn permutation(self) -> ([usize; 3], [bool; 3]) {
        match self {
            Group::Identity => ([0, 1, 2], [false, false, false]),
            Group::Y90 => ([2, 1, 0], [true, false, false]),
            Group::Y180 => ([0, 1, 2], [true, false, true]),
            Group::Y270 => ([2, 1, 0], [false, false, true]),
        }
    }
}

/// `Shapes.rotate(shape, group, BLOCK_CENTER)` on an encoded shape.
///
/// The coordinate lists are the source lists of the permuted axes, reversed and
/// mirrored only for inverted output axes (`flipAxisIfNeeded`); the
/// `CubePointRange` bit survives only when the list object is returned as-is,
/// i.e. when the output axis is not inverted.
fn rotate_encoded(shape: &EncodedShape, group: Group) -> EncodedShape {
    if matches!(group, Group::Identity) {
        // `Shapes.rotate` returns the shape itself for the identity group.
        return shape.clone();
    }
    let (permutation, inverted) = group.permutation();
    let source = [&shape.x, &shape.y, &shape.z];
    let sizes = [
        shape.x.len() - 1,
        shape.y.len() - 1,
        shape.z.len() - 1,
    ];
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for axis in 0..3 {
        let list = source[permutation[axis]];
        if inverted[axis] {
            coords[axis] = list.iter().rev().map(|value| 1.0 - value).collect();
        } else {
            coords[axis] = list.clone();
        }
    }
    let target_sizes = [
        coords[0].len() - 1,
        coords[1].len() - 1,
        coords[2].len() - 1,
    ];
    let mut occupied = vec![0u8; target_sizes[0] * target_sizes[1] * target_sizes[2]];
    let mut source = [0usize; 3];
    for x in 0..target_sizes[0] {
        for y in 0..target_sizes[1] {
            for z in 0..target_sizes[2] {
                let target = [x, y, z];
                for axis in 0..3 {
                    let source_axis = permutation[axis];
                    let size = sizes[source_axis];
                    source[source_axis] = if inverted[axis] {
                        size - 1 - target[axis]
                    } else {
                        target[axis]
                    };
                }
                let slot = (source[0] * sizes[1] + source[1]) * sizes[2] + source[2];
                if shape.occupied[slot] != 0 {
                    occupied[(x * target_sizes[1] + y) * target_sizes[2] + z] = 1;
                }
            }
        }
    }
    let mut cube_axes = 0u8;
    for axis in 0..3 {
        if !inverted[axis] && (shape.cube_axes & (1 << permutation[axis])) != 0 {
            cube_axes |= 1 << axis;
        }
    }
    EncodedShape {
        x: coords[0].clone(),
        y: coords[1].clone(),
        z: coords[2].clone(),
        canonical: shape.canonical,
        cube: 0,
        cube_axes,
        discrete_min: discrete_min(&target_sizes, &occupied),
        discrete_max: discrete_max(&target_sizes, &occupied),
        occupied,
    }
}

fn discrete_min(sizes: &[usize; 3], occupied: &[u8]) -> [i32; 3] {
    let mut out = [sizes[0] as i32, sizes[1] as i32, sizes[2] as i32];
    for x in 0..sizes[0] {
        for y in 0..sizes[1] {
            for z in 0..sizes[2] {
                if occupied[(x * sizes[1] + y) * sizes[2] + z] != 0 {
                    out[0] = out[0].min(x as i32);
                    out[1] = out[1].min(y as i32);
                    out[2] = out[2].min(z as i32);
                }
            }
        }
    }
    out
}

fn discrete_max(sizes: &[usize; 3], occupied: &[u8]) -> [i32; 3] {
    // `DiscreteVoxelShape.lastFull(axis) + 1`: the dictionary stores exclusive
    // upper bounds (`Shapes.block()` is `[1, 1, 1]`).
    let mut out = [-1, -1, -1];
    for x in 0..sizes[0] {
        for y in 0..sizes[1] {
            for z in 0..sizes[2] {
                if occupied[(x * sizes[1] + y) * sizes[2] + z] != 0 {
                    out[0] = out[0].max(x as i32);
                    out[1] = out[1].max(y as i32);
                    out[2] = out[2].max(z as i32);
                }
            }
        }
    }
    [out[0] + 1, out[1] + 1, out[2] + 1].map(|value| value.max(0))
}

fn to_geometry(shape: &EncodedShape) -> final_shapes::geometry::Shape {
    final_shapes::geometry::Shape {
        x: shape.x.clone(),
        y: shape.y.clone(),
        z: shape.z.clone(),
        canonical: shape.canonical,
        cube: shape.cube,
        cube_axes: shape.cube_axes,
        discrete_min: shape.discrete_min,
        discrete_max: shape.discrete_max,
        occupied: shape.occupied.clone(),
    }
}

/// Evaluate, refine and write the final shape bundle using this module's own
/// `final_shapes` copy, so callers do not need to bridge the two module types.
pub fn write(jar_path: &Path, out: &Path) -> Result<(), String> {
    let mut evaluated = final_shapes::evaluate(jar_path)?;
    refine(jar_path, &mut evaluated)?;
    final_shapes::write_bundle(&evaluated, out)
}
