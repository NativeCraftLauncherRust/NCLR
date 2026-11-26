//! `block-behaviours.json#states`' first column: the per-state sturdy bitmask.
//!
//! `VanillaBlockBehaviourExporter.states()` writes, per registry state, an 18-bit
//! mask:
//!
//! ```text
//! for (SupportType type : {FULL, CENTER, RIGID})          // s = 0..2
//!     for (Direction d : Direction.values())              // d = 0..5 (DOWN, UP, NORTH, SOUTH, WEST, EAST)
//!         if (state.isFaceSturdy(EmptyBlockGetter.INSTANCE, BlockPos.ZERO, d, type))
//!             sturdy |= 1 << (s * 6 + d.ordinal());
//! ```
//!
//! `BlockState.isFaceSturdy` dispatches to `SupportType.isSupporting` (see the
//! `SupportType$1/2/3` classes), all of which start from
//! `getBlockSupportShape(state, EmptyBlockGetter, BlockPos.ZERO)`:
//!
//! * **FULL** (`SupportType$1`): `Block.isFaceFull(shape, d)`, i.e.
//!   `isShapeFullBlock(shape.getFaceShape(d))` 鈥?the face slice is the full cube.
//! * **CENTER** (`SupportType$2`): `!Shapes.joinIsNotEmpty(faceShape,
//!   CENTER_SUPPORT_SHAPE, BooleanOp.ONLY_SECOND)` 鈥?the face slice covers
//!   `Block.column(2, 0, 10)` = `box(7, 0, 7, 9, 10, 9)` completely.
//! * **RIGID** (`SupportType$3`): the same test against
//!   `Shapes.join(Shapes.block(), Block.column(12, 0, 16), ONLY_FIRST)`, the
//!   full-height 2/16 rim of the block.
//!
//! `getFaceShape` (`VoxelShape.calculateFace`) is modelled exactly:
//!
//! * an empty shape or `Shapes.block()` returns itself;
//! * a shape whose coordinate list on that axis is exactly `[0, 1]`
//!   (`isCubeLikeAlong`) returns itself;
//! * otherwise the **outermost cell** on that axis is sliced
//!   (`SliceShape`): empty when that cell is unoccupied, the `Shapes.block()`
//!   singleton when the slice is itself cube-like on all three axes, else the
//!   slice. The slice is normalized to `[0, 1]` on the sliced axis, so it spans
//!   the whole block along the face normal.
//!
//! `joinIsNotEmpty` is "there is a merged-grid cell where `ONLY_SECOND` holds",
//! so the two `SupportType` tests are exact set-containment checks on the merged
//! coordinate grid of the face slice and the constant support shape.
//!
//! # Support shapes
//!
//! The support shape per state comes from `shape_final::evaluate` 鈥?the final
//! integration of the shape engines (its seventh source is `shape_support`,
//! which models the ten classes that override `getBlockSupportShape`). That
//! evaluator deliberately leaves the 199 `dynamicShape()` states `None`
//! (the prepared collision/support tables store `-1` for them), while
//! `block-behaviours.json` *does* carry sturdy bits for them: `isFaceSturdy`
//! computes the shape instead of reading the cache. This module fills those
//! states from their bytecode shape rules:
//!
//! * `ShulkerBoxBlock` 鈥?`getBlockSupportShape` is `Shapes.block()` when
//!   `level.getBlockEntity(pos)` is not an open shulker box; over
//!   `EmptyBlockGetter` it is `null`, so **all 102 shulker states** are
//!   `Shapes.block()` (mask `0x3ffff`).
//! * `PowderSnowBlock` 鈥?`getCollisionShape` over
//!   `CollisionContext.empty()` falls through to `Shapes.empty()` (`= block
//!   support shape`), so mask 0.
//! * `BambooStalkBlock` 鈥?support = `SHAPE_COLLISION.move(state.getOffset(pos))`;
//!   the result touches none of the three support shapes (mask 0).
//! * `MovingPistonBlock` 鈥?its block-entity-driven shape is empty over
//!   `EmptyBlockGetter`; mask 0.
//! * `ScaffoldingBlock` 鈥?support = its collision shape over the empty context:
//!   `box(0, 14, 0, 16, 16, 16)` (top plate), mask `8322`.
//! * `SpeleothemBlock` (`PointedDripstoneBlock`, `SulfurSpikeBlock`) 鈥?support =
//!   `getShape` = the static `THICKNESS`/`TIP_DIRECTION` column table, mask
//!   `192/64/128/1472` depending on the five thicknesses.
//!
//! # Validation (measured against `tmp/ship-extract`)
//!
//! `tests/behaviour_sturdy.rs` compares all 35,723 rows against
//! `block-behaviours.json#states`:
//!
//! ```text
//! sturdy: 35,723 of 35,723 exact, 35,723 derived, 199 dynamic filled, 0 gaps
//! ```
//!
//! That includes every bit of the three `SupportType` masks, every direction,
//! and the dynamic-shape states (shulker boxes, scaffolding, speleothems,
//! bamboo, moving piston, powder snow) whose support shapes the shape tables
//! leave uncached.

#![allow(dead_code, clippy::duplicate_mod)]

use std::collections::BTreeMap;
use std::path::Path;

#[path = "shape_final.rs"]
mod shape_final;

use shape_final::block;
use shape_final::geometry;
use shape_final::states;

/// The table this module reproduces.
pub const BEHAVIOUR_TABLE: &str = "block-behaviours.json";
pub const DATA_VERSION: i32 = 5023;

/// `SupportType.values()` order in the exporter.
pub const SUPPORTS: [&str; 3] = ["FULL", "CENTER", "RIGID"];
/// `Direction.values()` order in the exporter; the mask bit is `s * 6 + d`.
pub const DIRECTIONS: [&str; 6] = ["DOWN", "UP", "NORTH", "SOUTH", "WEST", "EAST"];

// ---------------------------------------------------------------------------
// VoxelShape helpers (the `geometry::Shape` table representation)
// ---------------------------------------------------------------------------

fn coords(shape: &geometry::Shape, axis: usize) -> &[f64] {
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

/// `VoxelShape.findIndex(axis, value)`:
/// `Mth.binarySearch(0, cells + 1, i -> value < coords[i]) - 1`.
fn find_index(shape: &geometry::Shape, axis: usize, value: f64) -> i64 {
    let coords = coords(shape, axis);
    for (index, coord) in coords.iter().enumerate() {
        if value < *coord {
            return index as i64 - 1;
        }
    }
    coords.len() as i64 - 1
}

fn occupied(shape: &geometry::Shape, cell: [usize; 3]) -> bool {
    let sizes = cells(shape);
    if cell[0] >= sizes[0] || cell[1] >= sizes[1] || cell[2] >= sizes[2] {
        return false;
    }
    shape.occupied[(cell[0] * sizes[1] + cell[1]) * sizes[2] + cell[2]] != 0
}

/// Point membership (`VoxelShape.getCoords`/occupancy lookup at a point).
fn contains(shape: &geometry::Shape, point: [f64; 3]) -> bool {
    if shape.canonical == 1 {
        return false;
    }
    if shape.canonical == 2 {
        return true;
    }
    let sizes = cells(shape);
    let mut cell = [0usize; 3];
    for axis in 0..3 {
        let index = find_index(shape, axis, point[axis]);
        if index < 0 || index as usize >= sizes[axis] {
            return false;
        }
        cell[axis] = index as usize;
    }
    occupied(shape, cell)
}

/// `VoxelShape.isCubeLikeAlong(axis)`: `getCoords(axis)` is exactly `[0, 1]`.
fn is_cube_like_along(shape: &geometry::Shape, axis: usize) -> bool {
    let coords = coords(shape, axis);
    coords.len() == 2 && (coords[0] - 0.0).abs() < 1e-7 && (coords[1] - 1.0).abs() < 1e-7
}

/// The block-covering test `Block.isShapeFullBlock`: the shape fills the unit
/// cube (the prepared `Shapes.block()` singleton included).
fn is_full_block(shape: &geometry::Shape) -> bool {
    if shape.canonical == 2 {
        return true;
    }
    if shape.canonical == 1 {
        return false;
    }
    for axis in 0..3 {
        let coords = coords(shape, axis);
        if (coords[0] - 0.0).abs() >= 1e-7 || (coords[coords.len() - 1] - 1.0).abs() >= 1e-7 {
            return false;
        }
    }
    shape.occupied.iter().all(|value| *value != 0)
}

/// `VoxelShape.getFaceShape(direction)`, as a point-set predicate plus the
/// coordinate bounds the tests intersect it with.
#[derive(Clone, Copy)]
enum Face<'a> {
    Empty,
    /// The shape itself (empty/block/`isCubeLikeAlong` cases).
    Whole(&'a geometry::Shape),
    /// A sliced face (`SliceShape`): the axis is normalized to `[0, 1]` and the
    /// cells are the ones at `cell` on that axis.
    Slice {
        shape: &'a geometry::Shape,
        axis: usize,
        cell: usize,
    },
    /// The slice turned into the `Shapes.block()` singleton by `isCubeLike`.
    Block,
}

fn face_shape<'a>(shape: &'a geometry::Shape, direction: usize) -> Face<'a> {
    if shape.canonical == 2 {
        return Face::Whole(shape);
    }
    if shape.canonical == 1 {
        return Face::Empty;
    }
    // axis and positive side of Direction.values() = DOWN, UP, NORTH, SOUTH,
    // WEST, EAST.
    let axis = [1usize, 1, 2, 2, 0, 0][direction];
    let positive = [false, true, false, true, false, true][direction];
    if is_cube_like_along(shape, axis) {
        return Face::Whole(shape);
    }
    let value = if positive { 0.9999999 } else { 1e-7 };
    let index = find_index(shape, axis, value);
    if index < 0 {
        return Face::Empty;
    }
    let sizes = cells(shape);
    let index = index as usize;
    if index >= sizes[axis] {
        return Face::Empty;
    }
    // Does the outermost cell contain anything?
    let mut any = false;
    let other_axes: Vec<usize> = (0..3).filter(|other| *other != axis).collect();
    let mut cell = [0usize; 3];
    cell[axis] = index;
    for a in 0..sizes[other_axes[0]] {
        for b in 0..sizes[other_axes[1]] {
            cell[other_axes[0]] = a;
            cell[other_axes[1]] = b;
            if occupied(shape, cell) {
                any = true;
            }
        }
    }
    if !any {
        return Face::Empty;
    }
    // `SliceShape.isCubeLike`: the slice's non-normalized axes are `[0, 1]`.
    let slice_is_block = other_axes
        .iter()
        .all(|axis| is_cube_like_along(shape, *axis));
    if slice_is_block {
        Face::Block
    } else {
        Face::Slice {
            shape,
            axis,
            cell: index,
        }
    }
}

impl Face<'_> {
    fn contains(&self, point: [f64; 3]) -> bool {
        match self {
            Face::Empty => false,
            Face::Block => true,
            Face::Whole(shape) => contains(shape, point),
            Face::Slice { shape, axis, cell } => {
                let coord = coords(shape, *axis);
                let middle = (coord[*cell] + coord[*cell + 1]) / 2.0;
                let mut point = point;
                point[*axis] = middle;
                contains(shape, point)
            }
        }
    }

    /// The coordinate bounds used to merge this face's grid with a constant
    /// support shape. The sliced axis is normalized to `[0, 1]`.
    fn bounds(&self) -> Option<[[f64; 2]; 3]> {
        match self {
            Face::Empty => None,
            Face::Block => Some([[0.0, 1.0]; 3]),
            Face::Whole(shape) => Some([
                [shape.x[0], shape.x[shape.x.len() - 1]],
                [shape.y[0], shape.y[shape.y.len() - 1]],
                [shape.z[0], shape.z[shape.z.len() - 1]],
            ]),
            Face::Slice { shape, axis, .. } => {
                let mut bounds = [
                    [
                        shape.x[0],
                        shape.x[shape.x.len() - 1],
                    ],
                    [
                        shape.y[0],
                        shape.y[shape.y.len() - 1],
                    ],
                    [
                        shape.z[0],
                        shape.z[shape.z.len() - 1],
                    ],
                ];
                bounds[*axis] = [0.0, 1.0];
                Some(bounds)
            }
        }
    }

    fn is_full(&self) -> bool {
        match self {
            Face::Block => true,
            Face::Whole(shape) => is_full_block(shape),
            // `Block.isShapeFullBlock` is "the point set equals the unit cube",
            // so a slice whose two in-plane axes span 0..1 and whose cells are
            // all occupied is full (its normal axis is normalized to 0..1).
            Face::Slice { shape, axis, cell } => {
                let sizes = cells(shape);
                for other in 0..3 {
                    if other == *axis {
                        continue;
                    }
                    let coords = coords(shape, other);
                    if (coords[0] - 0.0).abs() >= 1e-7
                        || (coords[coords.len() - 1] - 1.0).abs() >= 1e-7
                    {
                        return false;
                    }
                }
                let other_axes: Vec<usize> = (0..3).filter(|other| *other != *axis).collect();
                let mut cell_corner = [0usize; 3];
                cell_corner[*axis] = *cell;
                for a in 0..sizes[other_axes[0]] {
                    for b in 0..sizes[other_axes[1]] {
                        cell_corner[other_axes[0]] = a;
                        cell_corner[other_axes[1]] = b;
                        if !occupied(shape, cell_corner) {
                            return false;
                        }
                    }
                }
                true
            }
            Face::Empty => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The constants used by SupportType.CENTER/RIGID
// ---------------------------------------------------------------------------

/// `SupportType.CENTER`'s `CENTER_SUPPORT_SHAPE = Block.column(2, 0, 10)`.
fn center_shape() -> geometry::Shape {
    geometry::Shape {
        x: vec![7.0 / 16.0, 9.0 / 16.0],
        y: vec![0.0, 10.0 / 16.0],
        z: vec![7.0 / 16.0, 9.0 / 16.0],
        canonical: 0,
        cube: 0,
        cube_axes: 0,
        discrete_min: [0, 0, 0],
        discrete_max: [1, 1, 1],
        occupied: vec![1],
    }
}

/// `SupportType.RIGID`'s `RIGID_SUPPORT_SHAPE = Shapes.join(Shapes.block(),
/// Block.column(12, 0, 16), ONLY_FIRST)`: the full-height 2/16 rim.
fn rigid_shape() -> geometry::Shape {
    let grid: Vec<f64> = (0..=8).map(|index| index as f64 / 8.0).collect();
    let mut occupied = Vec::with_capacity(8 * 8);
    for x in 0..8 {
        for z in 0..8 {
            // Column(12, 0, 16) occupies cells 1..=6 on both horizontal axes.
            let inside = (1..=6).contains(&x) && (1..=6).contains(&z);
            occupied.push(if inside { 0 } else { 1 });
        }
    }
    geometry::Shape {
        x: grid.clone(),
        y: vec![0.0, 1.0],
        z: grid,
        canonical: 0,
        cube: 1,
        cube_axes: 7,
        discrete_min: [0, 0, 0],
        discrete_max: [8, 1, 8],
        occupied,
    }
}

/// Merge two coordinate lists into the union grid.
fn merged_coords(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut out: Vec<f64> = a.iter().chain(b.iter()).copied().collect();
    out.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|left, right| (*left - *right).abs() < 1e-9);
    out
}

/// `!Shapes.joinIsNotEmpty(face, target, ONLY_SECOND)`: the face covers every
/// point of `target` (`target(p) && !face(p)` is never true on the merged grid).
fn face_covers(face: &Face, target_coords: [&[f64]; 3], target: &dyn Fn([f64; 3]) -> bool) -> bool {
    let Some(bounds) = face.bounds() else {
        return false;
    };
    let mut xs = merged_coords(&[bounds[0][0], bounds[0][1]], target_coords[0]);
    let mut ys = merged_coords(&[bounds[1][0], bounds[1][1]], target_coords[1]);
    let mut zs = merged_coords(&[bounds[2][0], bounds[2][1]], target_coords[2]);
    // Use the shapes' full coordinate partitions, not just the outer bounds.
    xs = face_grid(face, 0, &xs);
    ys = face_grid(face, 1, &ys);
    zs = face_grid(face, 2, &zs);
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            for z in zs.windows(2) {
                let point = [
                    (x[0] + x[1]) / 2.0,
                    (y[0] + y[1]) / 2.0,
                    (z[0] + z[1]) / 2.0,
                ];
                if target(point) && !face.contains(point) {
                    return false;
                }
            }
        }
    }
    true
}

fn face_grid(face: &Face, axis: usize, bounds: &[f64]) -> Vec<f64> {
    let mut values = bounds.to_vec();
    if let Face::Slice { shape, axis: sliced, .. } = face {
        // `SliceShape.getCoords` normalizes the sliced axis to `CubePointRange(1)`
        // = `[0, 1]`; the other axes keep the delegate's coordinate list.
        if *sliced != axis {
            values.extend(shape_coords(shape, axis).iter().copied());
        }
    } else if let Face::Whole(shape) = face {
        values.extend(shape_coords(shape, axis).iter().copied());
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    values.dedup_by(|left, right| (*left - *right).abs() < 1e-9);
    values
}

fn shape_coords(shape: &geometry::Shape, axis: usize) -> &[f64] {
    coords(shape, axis)
}

/// The 18-bit mask for one support shape.
pub fn sturdy_mask(shape: &geometry::Shape) -> i64 {
    sturdy_mask_with(shape, &center_shape(), &rigid_shape())
}

fn sturdy_mask_with(shape: &geometry::Shape, center: &geometry::Shape, rigid: &geometry::Shape) -> i64 {
    let mut mask = 0i64;
    let center_coords: [&[f64]; 3] = [&center.x, &center.y, &center.z];
    let rigid_coords: [&[f64]; 3] = [&rigid.x, &rigid.y, &rigid.z];
    for direction in 0..6 {
        let face = face_shape(shape, direction);
        // FULL: the face slice is the full cube.
        if face.is_full() {
            mask |= 1 << direction;
        }
        // CENTER: the face slice covers Block.column(2, 0, 10).
        let center_contains = |point: [f64; 3]| contains(center, point);
        if face_covers(&face, center_coords, &center_contains) {
            mask |= 1 << (6 + direction);
        }
        // RIGID: the face slice covers the full-height 2/16 rim (`Shapes.block()
        // minus Block.column(12, 0, 16)`).
        let rigid_contains = |point: [f64; 3]| contains(rigid, point);
        if face_covers(&face, rigid_coords, &rigid_contains) {
            mask |= 1 << (12 + direction);
        }
    }
    mask
}

// ---------------------------------------------------------------------------
// The support shape for the 199 dynamic states
// ---------------------------------------------------------------------------

/// `direction` names as the VM's property values use them.
fn prop_bool(state: &BTreeMap<String, String>, name: &str) -> Option<bool> {
    state.get(name).map(|value| value == "true")
}

fn prop_text<'a>(state: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    state.get(name).map(String::as_str)
}

/// `Block.box` in 1/16 units to the table shape.
fn box16(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> geometry::Shape {
    // Let the engines' factory produce the exact partition/flags: build a
    // `VoxelShape` via the family factory would duplicate it, so create the
    // shape directly with `Shapes.create` semantics.
    let x1 = x1 / 16.0;
    let y1 = y1 / 16.0;
    let z1 = z1 / 16.0;
    let x2 = x2 / 16.0;
    let y2 = y2 / 16.0;
    let z2 = z2 / 16.0;
    if x2 - x1 < 1e-7 || y2 - y1 < 1e-7 || z2 - z1 < 1e-7 {
        return empty_shape();
    }
    let bits = [find_bits(x1, x2), find_bits(y1, y2), find_bits(z1, z2)];
    if bits.iter().any(|bit| bit.is_none()) {
        return geometry::Shape {
            x: vec![x1, x2],
            y: vec![y1, y2],
            z: vec![z1, z2],
            canonical: 0,
            cube: 0,
            cube_axes: 0,
            discrete_min: [0, 0, 0],
            discrete_max: [1, 1, 1],
            occupied: vec![1],
        };
    }
    let bits = [bits[0].unwrap(), bits[1].unwrap(), bits[2].unwrap()];
    if bits == [0, 0, 0] {
        return block_shape();
    }
    let sizes = [1usize << bits[0], 1usize << bits[1], 1usize << bits[2]];
    let coords: [Vec<f64>; 3] = [
        (0..=sizes[0]).map(|i| i as f64 / sizes[0] as f64).collect(),
        (0..=sizes[1]).map(|i| i as f64 / sizes[1] as f64).collect(),
        (0..=sizes[2]).map(|i| i as f64 / sizes[2] as f64).collect(),
    ];
    let min = [
        (x1 * sizes[0] as f64).round() as usize,
        (y1 * sizes[1] as f64).round() as usize,
        (z1 * sizes[2] as f64).round() as usize,
    ];
    let max = [
        (x2 * sizes[0] as f64).round() as usize,
        (y2 * sizes[1] as f64).round() as usize,
        (z2 * sizes[2] as f64).round() as usize,
    ];
    let mut occupied = vec![0u8; sizes[0] * sizes[1] * sizes[2]];
    for x in min[0]..max[0] {
        for y in min[1]..max[1] {
            for z in min[2]..max[2] {
                occupied[(x * sizes[1] + y) * sizes[2] + z] = 1;
            }
        }
    }
    geometry::Shape {
        x: coords[0].clone(),
        y: coords[1].clone(),
        z: coords[2].clone(),
        canonical: 0,
        cube: 1,
        cube_axes: 7,
        discrete_min: min.map(|value| value as i32),
        discrete_max: max.map(|value| value as i32),
        occupied,
    }
}

fn find_bits(min: f64, max: f64) -> Option<u32> {
    if min < -1e-7 || max > 1.0 + 1e-7 {
        return None;
    }
    for k in 0..=3u32 {
        let scale = (1u32 << k) as f64;
        let a = min * scale;
        let b = max * scale;
        if (a - a.round()).abs() < 1e-7 * scale && (b - b.round()).abs() < 1e-7 * scale {
            return Some(k);
        }
    }
    None
}

fn empty_shape() -> geometry::Shape {
    geometry::Shape {
        x: vec![0.0],
        y: vec![0.0],
        z: vec![0.0],
        canonical: 1,
        cube: 0,
        cube_axes: 0,
        discrete_min: [0, 0, 0],
        discrete_max: [0, 0, 0],
        occupied: Vec::new(),
    }
}

fn block_shape() -> geometry::Shape {
    geometry::Shape {
        x: vec![0.0, 1.0],
        y: vec![0.0, 1.0],
        z: vec![0.0, 1.0],
        canonical: 2,
        cube: 1,
        cube_axes: 7,
        discrete_min: [0, 0, 0],
        discrete_max: [1, 1, 1],
        occupied: vec![1],
    }
}

/// `Block.column(radius, yMin, yMax)`.
fn column16(radius: f64, y1: f64, y2: f64) -> geometry::Shape {
    let half = radius / 2.0;
    box16(8.0 - half, y1, 8.0 - half, 8.0 + half, y2, 8.0 + half)
}

/// The support shape of one of the dynamic-shape states, where
/// `shape_final::evaluate` leaves the state `None`.
fn dynamic_support_shape(class: &str, values: &BTreeMap<String, String>) -> Option<geometry::Shape> {
    let shape = match class {
        // Over `EmptyBlockGetter.getBlockEntity` is `null`, so the closed branch
        // always applies: `Shapes.block()`.
        "ShulkerBoxBlock" => block_shape(),
        // `PointedDripstoneBlock`/`SulfurSpikeBlock` inherit `SpeleothemBlock`'s
        // `getShape` column table; `getBlockSupportShape` defaults to the
        // collision shape, which is that table.
        "SpeleothemBlock" | "PointedDripstoneBlock" | "SulfurSpikeBlock" => {
            speleothem_shape(values)?
        }
        // The top plate over `CollisionContext.empty()`.
        "ScaffoldingBlock" => box16(0.0, 14.0, 0.0, 16.0, 16.0, 16.0),
        // `SHAPE_COLLISION.move(getOffset(ZERO))`: `Block.column(3, 0, 16)` moved
        // by the `XZ` offset. `Mth.getSeed(0,0,0) == 0`, the default horizontal
        // maximum is 0.25, so the offset is (-0.25, 0, -0.25).
        "BambooStalkBlock" => move_shape(&column16(3.0, 0.0, 16.0), [-0.25, 0.0, -0.25]),
        // The moving-piston shape comes from its block entity; over the empty
        // getter there is none, and the default shape supports nothing.
        "MovingPistonBlock" => box16(0.0, 0.0, 0.0, 16.0, 0.0, 16.0),
        // `CollisionContext.empty()` is neither a placement nor an entity
        // context; powder snow's collision shape is `Shapes.empty()` there.
        "PowderSnowBlock" => empty_shape(),
        _ => return None,
    };
    Some(shape)
}

/// `SpeleothemBlock.getShape`: `THICKNESS` selects the column; the `TIP`
/// thickness splits on `TIP_DIRECTION` (`BlockStateProperties.VERTICAL_DIRECTION`
/// lists `UP` then `DOWN`).
fn speleothem_shape(values: &BTreeMap<String, String>) -> Option<geometry::Shape> {
    let thickness = prop_text(values, "thickness")?;
    let shape = match thickness {
        "tip_merge" => column16(6.0, 0.0, 16.0),
        "tip" => match prop_text(values, "vertical_direction") {
            Some("down") => column16(6.0, 5.0, 16.0),
            _ => column16(6.0, 0.0, 11.0),
        },
        "frustum" => column16(8.0, 0.0, 16.0),
        "middle" => column16(10.0, 0.0, 16.0),
        "base" => column16(12.0, 0.0, 16.0),
        _ => return None,
    };
    // `getShape` ends with `shape.move(state.getOffset(pos))`.
    // `SpeleothemBlock.getMaxHorizontalOffset()` is `SHAPE_BASE`'s 2/16, the
    // offset type is `XZ`, and `Mth.getSeed(0,0,0) == 0`, so the offset is
    // (-((0.5) * 0.5) clamped to 2/16) = (-2/16, 0, -2/16).
    Some(move_shape(&shape, [-2.0 / 16.0, 0.0, -2.0 / 16.0]))
}

/// `VoxelShape.move(Vec3)`: shift the coordinate partition; the discrete
/// occupancy is unchanged (vanilla wraps the shifted lists in an
/// `ArrayVoxelShape`).
fn move_shape(shape: &geometry::Shape, offset: [f64; 3]) -> geometry::Shape {
    if offset == [0.0, 0.0, 0.0] {
        return shape.clone();
    }
    let mut moved = shape.clone();
    for (coords, delta) in [
        (&mut moved.x, offset[0]),
        (&mut moved.y, offset[1]),
        (&mut moved.z, offset[2]),
    ] {
        for value in coords.iter_mut() {
            *value += delta;
        }
    }
    // `Shapes.create`-fine coordinates cannot stay a cube, and the move path is
    // an `ArrayVoxelShape`.
    moved.cube = 0;
    moved.cube_axes = 0;
    moved.canonical = 0;
    moved
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub states: usize,
    pub derived: usize,
    /// States of `dynamicShape()` blocks filled from their bytecode rules.
    pub dynamic_filled: usize,
    pub missing: usize,
    pub gaps: BTreeMap<String, String>,
}

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

/// The sturdy mask of every registry state.
pub fn sturdy_masks(jar_path: &Path) -> Result<(Vec<Option<i64>>, Coverage), String> {
    let evaluated = shape_final::evaluate(jar_path)?;
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    if registry.len() != state_table.blocks.len() {
        return Err("the block registry and the state table disagree".to_string());
    }
    let total = state_table.state_count();
    if evaluated.support.len() != total {
        return Err("the shape evaluator did not cover every state".to_string());
    }
    let mut masks: Vec<Option<i64>> = vec![None; total];
    let mut coverage = Coverage {
        states: total,
        ..Default::default()
    };
    let mut index = 0usize;
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        let class = &registry[block_index].class;
        for state_index in 0..row.state_count {
            let shape = match &evaluated.support[index] {
                Some(shape) => Some(shape.clone()),
                None => {
                    let pairs = property_values(row, state_index);
                    let values: BTreeMap<String, String> = pairs.into_iter().collect();
                    let shape = dynamic_support_shape(class, &values);
                    if shape.is_some() {
                        coverage.dynamic_filled += 1;
                    }
                    shape
                }
            };
            match shape {
                Some(shape) => {
                    masks[index] = Some(sturdy_mask(&shape));
                    coverage.derived += 1;
                }
                None => {
                    coverage
                        .gaps
                        .insert(class.clone(), "no support shape for the dynamic state".to_string());
                }
            }
            index += 1;
        }
    }
    coverage.missing = total - coverage.derived;
    Ok((masks, coverage))
}

/// Whether the mask has the FULL bit for `direction` (0..5).
pub fn full(mask: i64, direction: usize) -> bool {
    mask & (1 << direction) != 0
}

/// Whether the mask has the CENTER bit for `direction` (0..5).
pub fn center(mask: i64, direction: usize) -> bool {
    mask & (1 << (6 + direction)) != 0
}

/// Whether the mask has the RIGID bit for `direction` (0..5).
pub fn rigid(mask: i64, direction: usize) -> bool {
    mask & (1 << (12 + direction)) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constant_shapes_have_the_expected_masks() {
        assert_eq!(sturdy_mask(&block_shape()), 0x3ffff);
        assert_eq!(sturdy_mask(&empty_shape()), 0);
    }

    #[test]
    fn a_full_width_bottom_slab_supports_only_down() {
        // snow layer 1: column(16, 0, 2).
        let shape = column16(16.0, 0.0, 2.0);
        let mask = sturdy_mask(&shape);
        assert!(full(mask, 0) && center(mask, 0) && rigid(mask, 0));
        assert_eq!(mask, 1 + 64 + 4096);
    }

    #[test]
    fn a_centered_column_supports_up_and_down_centers() {
        // Speleothem TIP_MERGE: column(6, 0, 16).
        let mask = sturdy_mask(&column16(6.0, 0.0, 16.0));
        assert!(center(mask, 0) && center(mask, 1));
        assert!(!full(mask, 0) && !full(mask, 1));
        assert_eq!(mask, 64 + 128);
    }

    #[test]
    fn the_direction_bits_are_in_the_exporter_order() {
        for direction in 0..6 {
            let mask = 1i64 << (6 + direction);
            assert!(center(mask, direction));
            assert!(!center(mask, (direction + 1) % 6));
        }
    }
}





