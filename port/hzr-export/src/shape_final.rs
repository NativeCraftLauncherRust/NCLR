//! Final shape-table integration: every shape engine in one evaluator.
//!
//! The prepared geometry bundle is three NBT tables plus one JSON array:
//!
//! * `cached-collision.nbt` — `getCollisionShape(state, EmptyBlockGetter,
//!   BlockPos.ZERO, CollisionContext.empty())`, one entry per registry state;
//! * `occlusion.nbt` — `getOcclusionShape(state)`;
//! * `worldgen/blocks/cached-support.nbt` — `getBlockSupportShape(state,
//!   EmptyBlockGetter, BlockPos.ZERO)`, `Kind = "block_support"`;
//! * `structure-blocks.json#full_block` —
//!   `!block.hasDynamicShape() && state.isCollisionShapeFullBlock(...)`.
//!
//! This module is the integration point of the engines the previous steps built;
//! each one is included by path and owns its own slice of the registry:
//!
//! | order | source | what it contributes |
//! | --- | --- | --- |
//! | 1 | `geometry` default path | classes whose chain overrides no shape method: exactly `hasCollision ? Shapes.block() : Shapes.empty()` for collision and `getShape` for occlusion. It must come **first**: `hasCollision` only reaches the shape through `BlockBehaviour.getCollisionShape`'s default implementation, so a class that overrides the method (e.g. `WallHangingSignBlock`) bypasses the flag and the default path reports `None` for it |
//! | 2 | `shape_gate` | fence gates (`getShape`/`getCollisionShape`/`getOcclusionShape`/support maps) and cave vines |
//! | 3 | `shape_stateholder` | the multiface occlusion family (`GlowLichenBlock`, `MultifaceBlock`, `SculkVeinBlock`) |
//! | 4 | `shape_maps` | the static-map builders: ceiling hanging signs, flower beds, leaf litter, composter, decorated pot, hopper, mossy carpet |
//! | 5 | `shape_families` | the big geometric families (stairs, slabs, walls, fences, panes, doors, trapdoors, candles, vines, plants, carpets, snow, ladders, chains, lanterns, ...) |
//! | 6 | `shape_offset`'s VM (`shape_static`) | everything left: the block classes' own bytecode, with the `getOffset` hook patched to `Vec3.ZERO` so the position-dependent shapes are evaluated at `BlockPos.ZERO`, plus the offset the vanilla `getOffset` would add |
//! | 7 | `shape_support` | per-face support: pass-through of the final collision shape, plus the ten classes that override `getBlockSupportShape` |
//!
//! `shape_offset` (the offset wrapper around the step-2b VM) is used directly as
//! the interpreter, so the outline/collision/occlusion/support queries each see
//! the same patched `BlockStateBase.getOffset`; the module also applies the
//! vanilla offset at `BlockPos.ZERO` to the resulting shape.
//!
//! # The shape model
//!
//! The prepared dictionary stores the *construction history* of the vanilla
//! `VoxelShape` (`canonical`, `cube`, and the per-axis `CubePointRange` bits),
//! not just the point set. The engines' selectors (`shape_families`,
//! `shape_maps`, `shape_gate`) already encode that; the VM needs a factory with
//! the same semantics, which [`ExactFactory`] provides:
//!
//! * `create` = `Shapes.create` (`findBits` 0..=3, `CubeVoxelShape` /
//!   `ArrayVoxelShape` / `Shapes.block()` selection);
//! * `join` = `Shapes.joinUnoptimized(...).optimize()`, where the per-axis merger
//!   follows `Shapes.createIndexMerger` (both lists `CubePointRange` and
//!   `cells_so_far * lcm(a, b) <= 256` -> `DiscreteCubeMerger`, which keeps the
//!   `CubePointRange` bit; otherwise `NonOverlapping`/`Indirect` -> array bit
//!   clear), `CUBE = all three mergers discrete`, and `optimize` folds
//!   `Shapes.joinUnoptimized` over `BitSetDiscreteVoxelShape.forAllBoxes`
//!   re-created through `Shapes.box`;
//! * `rotate` = `Shapes.rotate(shape, group, center)` (identity returns the
//!   shape, a cube about the block centre stays a cube, everything else becomes
//!   an array with permuted/mirrored coordinate lists);
//! * `translate` = `VoxelShape.move` (an array over shifted coordinate lists, or
//!   the same object for a zero offset).
//!
//! # The per-class registration profile
//!
//! `hasDynamicShape` and `hasCollision` are per-registration facts built from
//! `Properties.dynamicShape()` / `Properties.noCollision()`. The property chain
//! can live in the initialiser, in a `Blocks` helper method
//! (`shulkerBoxProperties`), or in the property lambda of a `ColorCollection`
//! family (banners, shulker boxes), so [`class_profiles`] analyses the class's
//! method bodies (walking `invokedynamic` bootstraps to the lambda bodies) and
//! records the flags on the class each registration constructs. This matters for
//! exactness twice:
//!
//! * `dynamic` drives the uncached entries. `BlockStateBase.initCache()` only
//!   builds the shape cache when `!hasDynamicShape()`; the collision and support
//!   oracles read that cache and write `-1`, which is exactly the 199 states of
//!   the 23 dynamic blocks (7 classes: bamboo, moving piston, pointed dripstone,
//!   powder snow, scaffolding, sulfur spike, shulker boxes). `full_block` then
//!   follows the exporter's `!hasDynamicShape() && ...` rule.
//! * `no_collision`/`colliding` correct the family-scan `hasCollision` flag where
//!   the properties are built inside a lambda: the scan mis-reads the colour
//!   families, and the profile puts `BannerBlock`/`WallBannerBlock` back on the
//!   non-colliding side (320 states).
//!
//! # Validation (measured against `tmp/ship-extract`)
//!
//! `tests/shape_final.rs` drives the evaluator over all 35,723 registry states:
//!
//! ```text
//! cached-collision: 35,520 exact + 4 point-set equal, 0 gaps  (199 uncached matched)
//! occlusion:        35,687 exact + 4 point-set equal, 32 gaps (scaffolding)
//! cached-support:   35,520 exact + 4 point-set equal, 0 gaps  (199 uncached matched)
//! full_block:       35,723 / 35,723 exact
//! ```
//!
//! The four repartitioned states are `StrawBedBlock` (`getShape` uses a
//! `Direction.BY_3D_DATA` lookup the VM models geometrically but with a different
//! coordinate partition). The single remaining gap class is
//! `ScaffoldingBlock`'s occlusion: its `getShape` feeds
//! `CollisionContext`/`hasOffsetFunction` and `Shapes.or` over a list the VM
//! cannot materialise (`null check on an opaque value; not a shape (opaque
//! Shapes.or) in Block.asItem`), so those 32 states stay `None` rather than
//! guessed. Its collision and support shapes are exact (the offset wrapper
//! resolves `getOffset`).
//!
//! The test compares every entry byte for byte where the engine reproduces the
//! construction history and asserts point-set equality otherwise, checks the
//! uncached `-1` pattern exactly, and re-reads the written bundle with
//! `geometry::read_table`. No shipped table value is read: the evaluator only
//! opens `mc263/client.jar`, and `tmp/ship-extract` is used by the test as the
//! oracle.

#![allow(dead_code, clippy::duplicate_mod)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Debug;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

#[path = "block.rs"]
pub mod block;

#[path = "states.rs"]
pub mod states;

#[path = "geometry.rs"]
pub mod geometry;

#[path = "shape_families.rs"]
mod shape_families;

#[path = "shape_maps.rs"]
mod shape_maps;

#[path = "shape_gate.rs"]
mod shape_gate;

#[path = "shape_stateholder.rs"]
mod shape_stateholder;

#[path = "shape_support.rs"]
mod shape_support;

#[path = "shape_flags.rs"]
mod shape_flags;

#[path = "shape_offset.rs"]
mod shape_offset;

use shape_offset::vm::{
    decode_code, JarSource, JoinOp, Query, Rotation, ShapeFactory, State as VmState,
};

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

pub const COLLISION: &str = "cached-collision.nbt";
pub const OCCLUSION: &str = "occlusion.nbt";
pub const SUPPORT: &str = "worldgen/blocks/cached-support.nbt";
/// The `full_block` key of `structure-blocks.json`, written as its own fragment
/// (the rest of that file is `structure.rs`'s table).
pub const FULL_BLOCK: &str = "structure-blocks.full_block.json";
pub const DATA_VERSION: i32 = 5023;
pub const SUPPORT_KIND: &str = "block_support";

const BLOCKS_CLASS: &str = "net/minecraft/world/level/block/Blocks";
const BLOCK_ITEMS: &str = "net/minecraft/references/BlockItemIds";
const BLOCK_IDS: &str = "net/minecraft/references/BlockIds";
const PROPERTIES: &str = "net/minecraft/world/level/block/state/BlockBehaviour$Properties";

// ---------------------------------------------------------------------------
// The exact shape model (`voxelshape`'s semantics, plus the merger bookkeeping)
// ---------------------------------------------------------------------------

const EPSILON: f64 = 1e-7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Empty,
    Block,
    Cube,
    Array,
}

/// A `VoxelShape`: per-axis coordinate partition, discrete occupancy and the
/// constructor history the prepared dictionary reads back.
#[derive(Clone, Debug, PartialEq)]
struct Exact {
    coords: [Vec<f64>; 3],
    /// `occupied[(x * ny + y) * nz + z]`, over `sizes = len(coords) - 1`.
    occupied: Vec<bool>,
    kind: Kind,
    /// Per-axis `getCoords(axis) instanceof CubePointRange`.
    ranges: [bool; 3],
}

impl Exact {
    fn empty() -> Exact {
        Exact {
            coords: [vec![0.0], vec![0.0], vec![0.0]],
            occupied: Vec::new(),
            kind: Kind::Empty,
            ranges: [false; 3],
        }
    }

    fn block() -> Exact {
        Exact {
            coords: [vec![0.0, 1.0], vec![0.0, 1.0], vec![0.0, 1.0]],
            occupied: vec![true],
            kind: Kind::Block,
            ranges: [true; 3],
        }
    }

    /// `Shapes.create` / `Shapes.box`.
    fn create(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Exact {
        if x2 - x1 < EPSILON || y2 - y1 < EPSILON || z2 - z1 < EPSILON {
            return Exact::empty();
        }
        let bits = [find_bits(x1, x2), find_bits(y1, y2), find_bits(z1, z2)];
        if bits.iter().any(|bit| bit.is_none()) {
            return Exact {
                coords: [vec![x1, x2], vec![y1, y2], vec![z1, z2]],
                occupied: vec![true],
                kind: Kind::Array,
                ranges: [false; 3],
            };
        }
        let bits = [bits[0].unwrap(), bits[1].unwrap(), bits[2].unwrap()];
        if bits == [0, 0, 0] {
            return Exact::block();
        }
        let sizes = [1usize << bits[0], 1usize << bits[1], 1usize << bits[2]];
        let mut shape = Exact {
            coords: [
                grid_coords(sizes[0]),
                grid_coords(sizes[1]),
                grid_coords(sizes[2]),
            ],
            occupied: vec![false; sizes[0] * sizes[1] * sizes[2]],
            kind: Kind::Cube,
            ranges: [true; 3],
        };
        let low = [
            (x1 * sizes[0] as f64).round() as i64,
            (y1 * sizes[1] as f64).round() as i64,
            (z1 * sizes[2] as f64).round() as i64,
        ];
        let high = [
            ((x2 * sizes[0] as f64).round() as i64).min(sizes[0] as i64),
            ((y2 * sizes[1] as f64).round() as i64).min(sizes[1] as i64),
            ((z2 * sizes[2] as f64).round() as i64).min(sizes[2] as i64),
        ];
        for x in low[0].max(0)..high[0] {
            for y in low[1].max(0)..high[1] {
                for z in low[2].max(0)..high[2] {
                    shape.set(x as usize, y as usize, z as usize, true);
                }
            }
        }
        shape
    }

    fn sizes(&self) -> [usize; 3] {
        [
            self.coords[0].len().saturating_sub(1),
            self.coords[1].len().saturating_sub(1),
            self.coords[2].len().saturating_sub(1),
        ]
    }

    fn key(x: usize, y: usize, z: usize, sizes: &[usize; 3]) -> usize {
        (x * sizes[1] + y) * sizes[2] + z
    }

    fn set(&mut self, x: usize, y: usize, z: usize, value: bool) {
        let sizes = self.sizes();
        if x < sizes[0] && y < sizes[1] && z < sizes[2] {
            let index = Exact::key(x, y, z, &sizes);
            self.occupied[index] = value;
        }
    }

    fn contains(&self, point: [f64; 3]) -> bool {
        let sizes = self.sizes();
        if sizes.iter().any(|size| *size == 0) {
            return false;
        }
        let mut cell = [0usize; 3];
        for axis in 0..3 {
            let coords = &self.coords[axis];
            if point[axis] < coords[0] || point[axis] > coords[coords.len() - 1] {
                return false;
            }
            let after = coords.partition_point(|value| *value <= point[axis]);
            if after == 0 || after > coords.len() - 1 {
                return false;
            }
            cell[axis] = after - 1;
        }
        self.occupied[Exact::key(cell[0], cell[1], cell[2], &sizes)]
    }

    fn is_empty(&self) -> bool {
        self.kind == Kind::Empty || !self.occupied.iter().any(|value| *value)
    }

    fn canonical(&self) -> u8 {
        match self.kind {
            Kind::Empty => 1,
            Kind::Block => 2,
            _ => 0,
        }
    }

    fn cube(&self) -> bool {
        matches!(self.kind, Kind::Cube | Kind::Block)
    }

    fn cube_axes(&self) -> u8 {
        if self.cube() {
            return 7;
        }
        let mut bits = 0u8;
        for (axis, ranged) in self.ranges.iter().enumerate() {
            if *ranged {
                bits |= 1 << axis;
            }
        }
        bits
    }

    fn discrete_min(&self) -> [i32; 3] {
        let sizes = self.sizes();
        let mut out = [0i32; 3];
        for axis in 0..3 {
            let mut found = 0;
            for cell in 0..sizes[axis] {
                if (0..sizes[(axis + 1) % 3]).any(|b| {
                    (0..sizes[(axis + 2) % 3]).any(|c| {
                        let mut slot = [0usize; 3];
                        slot[axis] = cell;
                        slot[(axis + 1) % 3] = b;
                        slot[(axis + 2) % 3] = c;
                        self.occupied[Exact::key(slot[0], slot[1], slot[2], &sizes)]
                    })
                }) {
                    found = cell as i32;
                    break;
                }
            }
            out[axis] = found;
        }
        out
    }

    fn discrete_max(&self) -> [i32; 3] {
        let sizes = self.sizes();
        let mut out = [0i32; 3];
        for axis in 0..3 {
            let mut found = 0;
            for cell in (0..sizes[axis]).rev() {
                let hit = (0..sizes[(axis + 1) % 3]).any(|b| {
                    (0..sizes[(axis + 2) % 3]).any(|c| {
                        let mut slot = [0usize; 3];
                        slot[axis] = cell;
                        slot[(axis + 1) % 3] = b;
                        slot[(axis + 2) % 3] = c;
                        self.occupied[Exact::key(slot[0], slot[1], slot[2], &sizes)]
                    })
                });
                if hit {
                    found = cell as i32 + 1;
                    break;
                }
            }
            out[axis] = found;
        }
        out
    }

    /// `VoxelShape.optimize()`: fold `Shapes.joinUnoptimized` over the maximal
    /// box list re-created through `Shapes.box`.
    fn optimize(self) -> Exact {
        if self.is_empty() {
            return Exact::empty();
        }
        let mut result = Exact::empty();
        for cells in self.boxes() {
            let boxed = Exact::create(
                self.coords[0][cells[0]],
                self.coords[1][cells[1]],
                self.coords[2][cells[2]],
                self.coords[0][cells[3]],
                self.coords[1][cells[4]],
                self.coords[2][cells[5]],
            );
            result = merge(&result, &boxed, JoinOp::Or);
        }
        result
    }

    /// `BitSetDiscreteVoxelShape.forAllBoxes` (cell indices, y/x/z scan).
    fn boxes(&self) -> Vec<[usize; 6]> {
        let sizes = self.sizes();
        if sizes.iter().any(|size| *size == 0) {
            return Vec::new();
        }
        let mut copy = self.occupied.clone();
        let mut out = Vec::new();
        let key = |x: usize, y: usize, z: usize| (x * sizes[1] + y) * sizes[2] + z;
        for y in 0..=sizes[1] {
            for x in 0..=sizes[0] {
                let mut z_start: Option<usize> = None;
                for z in 0..=sizes[2] {
                    let full = x < sizes[0] && y < sizes[1] && z < sizes[2] && copy[key(x, y, z)];
                    if full {
                        if z_start.is_none() {
                            z_start = Some(z);
                        }
                    } else if let Some(start) = z_start {
                        let mut x_end = x;
                        let mut y_end = y;
                        for zz in start..z {
                            copy[key(x_end, y_end, zz)] = false;
                        }
                        while is_strip(&copy, &sizes, start, z, x_end + 1, y_end) {
                            x_end += 1;
                            for zz in start..z {
                                copy[key(x_end, y_end, zz)] = false;
                            }
                        }
                        loop {
                            let grows = (x..=x_end)
                                .all(|xx| is_strip(&copy, &sizes, start, z, xx, y_end + 1));
                            if !grows {
                                break;
                            }
                            for xx in x..=x_end {
                                for zz in start..z {
                                    copy[key(xx, y_end + 1, zz)] = false;
                                }
                            }
                            y_end += 1;
                        }
                        out.push([x, y, start, x_end + 1, y_end + 1, z]);
                        z_start = None;
                    }
                }
            }
        }
        out
    }

    /// The exact prepared-table dictionary entry.
    fn encode(&self) -> geometry::Shape {
        if self.is_empty() {
            return geometry::Shape::empty();
        }
        let sizes = self.sizes();
        let mut occupied = Vec::with_capacity(sizes[0] * sizes[1] * sizes[2]);
        for x in 0..sizes[0] {
            for y in 0..sizes[1] {
                for z in 0..sizes[2] {
                    occupied.push(self.occupied[Exact::key(x, y, z, &sizes)] as u8);
                }
            }
        }
        geometry::Shape {
            x: self.coords[0].clone(),
            y: self.coords[1].clone(),
            z: self.coords[2].clone(),
            canonical: self.canonical(),
            cube: self.cube() as u8,
            cube_axes: self.cube_axes(),
            discrete_min: self.discrete_min(),
            discrete_max: self.discrete_max(),
            occupied,
        }
    }
}

fn is_strip(copy: &[bool], sizes: &[usize; 3], z1: usize, z2: usize, x: usize, y: usize) -> bool {
    x < sizes[0] && y < sizes[1] && (z1..z2).all(|z| copy[(x * sizes[1] + y) * sizes[2] + z])
}

fn find_bits(min: f64, max: f64) -> Option<u32> {
    if min < -EPSILON || max > 1.0 + EPSILON {
        return None;
    }
    for bits in 0..=3u32 {
        let scale = (1u32 << bits) as f64;
        let low = min * scale;
        let high = max * scale;
        if (low - low.round()).abs() < EPSILON * scale
            && (high - high.round()).abs() < EPSILON * scale
        {
            return Some(bits);
        }
    }
    None
}

fn grid_coords(size: usize) -> Vec<f64> {
    (0..=size).map(|i| i as f64 / size as f64).collect()
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn lcm(a: usize, b: usize) -> usize {
    if a == 0 || b == 0 {
        0
    } else {
        a / gcd(a, b) * b
    }
}

/// `Shapes.joinUnoptimized` with `Shapes.createIndexMerger`'s per-axis outcome.
///
/// `a == b` (object identity) and `IdenticalMerger` need the *list objects*: the
/// builders never join a shape with itself, and two distinct coordinate lists
/// with equal values are never `Objects.equals`, so the equal-value case falls
/// through to `IndirectMerger` — which is what the prepared dictionary shows.
fn merge(first: &Exact, second: &Exact, op: JoinOp) -> Exact {
    let first_only = op.apply(true, false);
    let second_only = op.apply(false, true);
    if first.is_empty() {
        return if second_only {
            second.clone()
        } else {
            Exact::empty()
        };
    }
    if second.is_empty() {
        return if first_only {
            first.clone()
        } else {
            Exact::empty()
        };
    }
    let first_sizes = first.sizes();
    let second_sizes = second.sizes();
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut discrete = [false; 3];
    let mut cells_so_far = 1usize;
    for axis in 0..3 {
        let mut range = false;
        if first.ranges[axis] && second.ranges[axis] {
            let merged = lcm(first_sizes[axis], second_sizes[axis]);
            // `createIndexMerger`'s guard: `cells_so_far * lcm <= 256`.
            if (cells_so_far as u64) * (merged as u64) <= 256 {
                coords[axis] = grid_coords(merged);
                range = true;
            }
        }
        if !range {
            let mut values: Vec<f64> = first.coords[axis]
                .iter()
                .chain(second.coords[axis].iter())
                .copied()
                .collect();
            values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            values.dedup_by(|a, b| (*a - *b).abs() < EPSILON);
            coords[axis] = values;
        }
        discrete[axis] = range;
        cells_so_far = cells_so_far.saturating_mul(coords[axis].len().saturating_sub(1));
    }
    let sizes = [
        coords[0].len().saturating_sub(1),
        coords[1].len().saturating_sub(1),
        coords[2].len().saturating_sub(1),
    ];
    let mut occupied = vec![false; sizes[0] * sizes[1] * sizes[2]];
    for x in 0..sizes[0] {
        for y in 0..sizes[1] {
            for z in 0..sizes[2] {
                let point = [
                    (coords[0][x] + coords[0][x + 1]) / 2.0,
                    (coords[1][y] + coords[1][y + 1]) / 2.0,
                    (coords[2][z] + coords[2][z + 1]) / 2.0,
                ];
                occupied[Exact::key(x, y, z, &sizes)] =
                    op.apply(first.contains(point), second.contains(point));
            }
        }
    }
    Exact {
        coords,
        occupied,
        kind: if discrete == [true; 3] {
            Kind::Cube
        } else {
            Kind::Array
        },
        ranges: discrete,
    }
}

/// `Shapes.rotate(shape, group, center)`.
fn rotate(shape: &Exact, rotation: &Rotation, center: [f64; 3]) -> Exact {
    if rotation.is_identity() {
        return shape.clone();
    }
    let sizes = shape.sizes();
    let new_sizes = [
        sizes[rotation.perm[0]],
        sizes[rotation.perm[1]],
        sizes[rotation.perm[2]],
    ];
    let cube = matches!(shape.kind, Kind::Cube | Kind::Block);
    let block_center = center == [0.5, 0.5, 0.5];
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for axis in 0..3 {
        if cube && block_center {
            coords[axis] = grid_coords(new_sizes[axis]);
            continue;
        }
        let source = &shape.coords[rotation.perm[axis]];
        let factor = center[axis] + center[rotation.perm[axis]];
        let mut values = Vec::with_capacity(source.len());
        if rotation.inverted[axis] {
            for index in (0..source.len()).rev() {
                values.push(factor - source[index]);
            }
        } else {
            for value in source {
                values.push(*value - center[rotation.perm[axis]] + center[axis]);
            }
        }
        coords[axis] = values;
    }
    let mut occupied = vec![false; new_sizes[0] * new_sizes[1] * new_sizes[2]];
    for x in 0..new_sizes[0] {
        for y in 0..new_sizes[1] {
            for z in 0..new_sizes[2] {
                let index = [x, y, z];
                let mut source = [0usize; 3];
                for axis in 0..3 {
                    let from = rotation.perm[axis];
                    source[from] = if rotation.inverted[axis] {
                        sizes[from] - 1 - index[axis]
                    } else {
                        index[axis]
                    };
                }
                occupied[Exact::key(x, y, z, &new_sizes)] =
                    shape.occupied[Exact::key(source[0], source[1], source[2], &sizes)];
            }
        }
    }
    let (kind, ranges) = if cube && block_center {
        (Kind::Cube, [true; 3])
    } else {
        (Kind::Array, [false; 3])
    };
    Exact {
        coords,
        occupied,
        kind,
        ranges,
    }
}

/// `VoxelShape.move(x, y, z)`: an array over the shifted coordinate lists (the
/// same shape for a zero offset).
fn translate(shape: &Exact, delta: [f64; 3]) -> Exact {
    if shape.is_empty() || delta == [0.0, 0.0, 0.0] {
        return shape.clone();
    }
    let mut coords = shape.coords.clone();
    for axis in 0..3 {
        for value in coords[axis].iter_mut() {
            *value += delta[axis];
        }
    }
    Exact {
        coords,
        occupied: shape.occupied.clone(),
        kind: Kind::Array,
        ranges: [false; 3],
    }
}

// ---------------------------------------------------------------------------
// The VM factory
// ---------------------------------------------------------------------------

/// `ShapeFactory` on top of [`Exact`] for the step-2b/offset interpreter.
struct ExactFactory;

impl ShapeFactory for ExactFactory {
    type Shape = Exact;

    fn empty(&mut self) -> Exact {
        Exact::empty()
    }

    fn block(&mut self) -> Exact {
        Exact::block()
    }

    fn create(&mut self, x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Exact {
        Exact::create(x1, y1, z1, x2, y2, z2)
    }

    fn or(&mut self, first: &Exact, second: &Exact) -> Exact {
        merge(first, second, JoinOp::Or).optimize()
    }

    fn join(&mut self, first: &Exact, second: &Exact, operation: JoinOp) -> Exact {
        merge(first, second, operation).optimize()
    }

    fn rotate(&mut self, shape: &Exact, rotation: &Rotation, center: [f64; 3]) -> Exact {
        rotate(shape, rotation, center)
    }

    fn translate(&mut self, shape: &Exact, delta: [f64; 3]) -> Exact {
        translate(shape, delta)
    }

    fn is_empty(&mut self, shape: &Exact) -> bool {
        shape.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Engine adapters
// ---------------------------------------------------------------------------

/// Each path-included engine carries its own copy of the shape modules, so the
/// `EncodedShape` types differ; these bridges copy the value field by field.
fn from_families(encoded: shape_families::EncodedShape) -> geometry::Shape {
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

fn from_maps(encoded: shape_maps::EncodedShape) -> geometry::Shape {
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

fn from_gate(encoded: shape_gate::EncodedShape) -> geometry::Shape {
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

fn from_stateholder(shape: &shape_stateholder::geometry::Shape) -> geometry::Shape {
    geometry::Shape {
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

fn to_support(shape: &geometry::Shape) -> shape_support::geometry::Shape {
    shape_support::geometry::Shape {
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

fn from_support(shape: &shape_support::geometry::Shape) -> geometry::Shape {
    geometry::Shape {
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

/// One state's property values, as `(name, value)` pairs.
fn property_values(block: &states::BlockStates, index: usize) -> Vec<(String, String)> {
    let sorted = block.sorted();
    let mut remainder = index;
    let mut values: BTreeMap<String, String> = BTreeMap::new();
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

fn resolve_class(names: &[String], simple: &str) -> Option<String> {
    let direct = format!("net/minecraft/world/level/block/{simple}");
    if names.iter().any(|name| *name == direct) {
        return Some(direct);
    }
    let suffix = format!("/{simple}");
    names
        .iter()
        .find(|name| {
            name.starts_with("net/minecraft/world/level/block/") && name.ends_with(&suffix)
        })
        .cloned()
}

/// Classes whose `getBlockSupportShape` is modelled by `shape_support` as a
/// constant, so the support table can be built from the collision table.
fn read_class(jar: &Path, internal: &str) -> Result<Option<Vec<u8>>, String> {
    let file = fs::File::open(jar).map_err(|error| format!("{}: {error}", jar.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", jar.display()))?;
    let wanted = format!("{internal}.class");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.name().map_err(|error| error.to_string())? != wanted {
            continue;
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| format!("{wanted}: {error}"))?;
        return Ok(Some(data));
    }
    Ok(None)
}

/// `Block.hasDynamicShape()` per block id, from `Blocks.<clinit>`.
/// `Block.hasDynamicShape()` per registered block, from `Blocks.<clinit>`.
/// The block classes registered with `hasDynamicShape() == true`.
///
/// A block is dynamic when its registration's `Properties` chain calls
/// `Properties.dynamicShape()` — directly in the initialiser, through a `Blocks`
/// helper (`shulkerBoxProperties`), or through the property lambda of a
/// `ColorCollection` family (`DYED_SHULKER_BOX`). The analysis runs over the
/// class's method bodies:
///
/// 1. a method is *dynamic* when its code calls `Properties.dynamicShape()`;
/// 2. close the set over `Blocks` methods that call a dynamic method;
/// 3. walk the initialiser: an `invokedynamic` whose lambda method is dynamic, or
///    a direct `dynamicShape()` call, marks the next registration; the class the
///    registration's factory lambda creates (`new <Block>`) is then dynamic.
///
/// `Block.hasDynamicShape()` is what decides whether `initCache()` builds the
/// shape cache, which is why the collision and support oracles store `-1` for
/// exactly these blocks' states. In this jar no class mixes dynamic and
/// Per-class `Properties` facts derived from `Blocks.<clinit>`.
///
/// `hasCollision` and `hasDynamicShape` are instance facts of each registration:
/// `Properties.noCollision()` / `Properties.dynamicShape()` sit in the property
/// chain, which can live in the initialiser, in a `Blocks` helper method
/// (`shulkerBoxProperties`), or in the property lambda of a `ColorCollection`
/// family (banners, shulker boxes). The analysis therefore works on the class's
/// method bodies:
///
/// 1. a method *sets* a flag when its code calls that `Properties` method;
/// 2. close each set over `Blocks` methods that call a setter method;
/// 3. walk the initialiser: an `invokedynamic` whose lambda is in a set, or a
///    direct `Properties` call, marks the next registration; the class the
///    registration's `invokedynamic`s construct is recorded with those flags.
///
/// `dynamic` drives the uncached (`-1`) entries: `BlockStateBase.initCache()`
/// only builds the shape cache when `!hasDynamicShape()`, and the collision and
/// support oracles read that cache. `no_collision`/`colliding` give the
/// authoritative `hasCollision` per class, used to correct the family-scan flag
/// where registrations build their properties inside a lambda. A class
/// registered both ways stays out of both sets (the caller falls back).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClassProfile {
    pub dynamic: BTreeSet<String>,
    pub no_collision: BTreeSet<String>,
    pub colliding: BTreeSet<String>,
}

pub fn class_profiles(jar: &Path) -> Result<ClassProfile, String> {
    let bytes =
        read_class(jar, BLOCKS_CLASS)?.ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let parsed = classfile::parse(&bytes).map_err(|error| format!("Blocks: {error}"))?;
    let setters = |property: &str| -> BTreeSet<String> {
        let mut methods: BTreeSet<String> = BTreeSet::new();
        for method in &parsed.methods {
            let Some(code) = method.code.as_ref() else {
                continue;
            };
            let hit = decode_code(&code.bytes).iter().any(|ins| {
                ins.opcode == 0xb6
                    && matches!(
                        classfile::method_ref(&parsed.constants, ins.index),
                        Some((owner, name, _)) if owner == PROPERTIES && name == property
                    )
            });
            if hit {
                methods.insert(method.name.clone());
            }
        }
        loop {
            let mut grew = false;
            for method in &parsed.methods {
                if methods.contains(&method.name) {
                    continue;
                }
                let Some(code) = method.code.as_ref() else {
                    continue;
                };
                let calls = decode_code(&code.bytes).iter().any(|ins| {
                    if ins.opcode != 0xb8 && ins.opcode != 0xb6 {
                        return false;
                    }
                    match classfile::method_ref(&parsed.constants, ins.index) {
                        Some((owner, name, _)) => owner == BLOCKS_CLASS && methods.contains(&name),
                        None => false,
                    }
                });
                if calls {
                    methods.insert(method.name.clone());
                    grew = true;
                }
            }
            if !grew {
                return methods;
            }
        }
    };
    let dynamic_methods = setters("dynamicShape");
    let no_collision_methods = setters("noCollision");
    let calls = |index: usize, methods: &BTreeSet<String>| -> bool {
        match classfile::method_ref(&parsed.constants, index) {
            Some((owner, name, _)) => owner == BLOCKS_CLASS && methods.contains(&name),
            None => false,
        }
    };
    let initializer = parsed
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let instructions = decode_code(&initializer.bytes);
    let mut profile = ClassProfile::default();
    let mut dynamic_seen: BTreeSet<String> = BTreeSet::new();
    let mut collision_seen: BTreeSet<String> = BTreeSet::new();
    let mut pending_dynamic = false;
    let mut pending_no_collision = false;
    let mut indys: Vec<usize> = Vec::new();
    for instruction in &instructions {
        match instruction.opcode {
            0xb2 => {
                let Some((owner, _field, _)) =
                    classfile::field_ref(&parsed.constants, instruction.index)
                else {
                    continue;
                };
                if owner == BLOCK_ITEMS || owner == BLOCK_IDS {
                    pending_dynamic = false;
                    pending_no_collision = false;
                    indys.clear();
                }
            }
            0xb6 => {
                if let Some((owner, name, _)) =
                    classfile::method_ref(&parsed.constants, instruction.index)
                {
                    if owner == PROPERTIES {
                        if name == "dynamicShape" {
                            pending_dynamic = true;
                        } else if name == "noCollision" {
                            pending_no_collision = true;
                        }
                    }
                }
            }
            0xba => {
                indys.push(instruction.index);
                if let Some((owner, name)) = lambda_target(&parsed, instruction.index) {
                    if owner == BLOCKS_CLASS {
                        if dynamic_methods.contains(&name) {
                            pending_dynamic = true;
                        }
                        if no_collision_methods.contains(&name) {
                            pending_no_collision = true;
                        }
                    }
                }
            }
            0xb8 => {
                let Some((owner, name, _)) =
                    classfile::method_ref(&parsed.constants, instruction.index)
                else {
                    continue;
                };
                if owner == BLOCKS_CLASS && dynamic_methods.contains(&name) {
                    pending_dynamic = true;
                    continue;
                }
                if owner == BLOCKS_CLASS && no_collision_methods.contains(&name) {
                    pending_no_collision = true;
                    continue;
                }
                let single = owner == BLOCKS_CLASS
                    && matches!(
                        name.as_str(),
                        "register"
                            | "registerStair"
                            | "registerSlab"
                            | "registerWall"
                            | "registerLegacyStair"
                    );
                // `zipMap` hands the derived colour collection to the same
                // registration helper as `registerBlocks` (see `block::registry`).
                let family = owner.ends_with("ColorCollection")
                    && (name == "registerBlocks" || name == "zipMap")
                    || owner.ends_with("WeatheringCopperCollection") && name == "registerBlocks";
                if !single && !family {
                    continue;
                }
                let mut class: Option<String> = None;
                for index in &indys {
                    if let Some(found) = created_class(&parsed, *index) {
                        class = Some(found);
                        break;
                    }
                }
                if let Some(class) = class {
                    if pending_dynamic {
                        profile.dynamic.insert(class.clone());
                    }
                    if pending_no_collision {
                        profile.no_collision.insert(class.clone());
                    } else {
                        profile.colliding.insert(class.clone());
                    }
                    let _ = (&dynamic_seen, &collision_seen);
                }
                let _ = calls(instruction.index, &dynamic_methods);
                pending_dynamic = false;
                pending_no_collision = false;
                indys.clear();
            }
            _ => {}
        }
    }
    // A class registered both ways is not safe to classify from one sample.
    let both: Vec<String> = profile
        .no_collision
        .intersection(&profile.colliding)
        .cloned()
        .collect();
    for class in both {
        profile.no_collision.remove(&class);
        profile.colliding.remove(&class);
    }
    Ok(profile)
}

/// The block class a registration's `invokedynamic` constructs.
///
/// A registration's factory is a method reference to the block constructor
/// (`MovingPistonBlock::new` -> a `<init>` handle whose owner is the block
/// class) or a synthetic `Blocks.lambda$...` whose body starts with `new
/// <Block>`. Colour families carry the constructor in one of their lambdas, so
/// every `invokedynamic` of the registration is tried.
fn created_class(class: &classfile::ClassFile, indy_index: usize) -> Option<String> {
    let (owner, name) = lambda_target(class, indy_index)?;
    let block_prefix = "net/minecraft/world/level/block/";
    if name == "<init>" && owner.starts_with(block_prefix) {
        return Some(simple_class(&owner));
    }
    if owner != "net/minecraft/world/level/block/Blocks" {
        return None;
    }
    let method = class.method(&name)?;
    let code = method.code.as_ref()?;
    for instruction in decode_code(&code.bytes) {
        if instruction.opcode != 0xbb {
            continue;
        }
        if !matches!(
            class.constants.get(instruction.index),
            Some(classfile::Constant::Class(_))
        ) {
            continue;
        }
        let target = match class_constant_name(&class.constants, instruction.index) {
            Some(target) => target,
            None => continue,
        };
        if target.starts_with(block_prefix) {
            return Some(simple_class(&target));
        }
    }
    None
}

/// The internal name behind a `Constant::Class` (`#<utf8 index>`).
fn class_constant_name(constants: &[classfile::Constant], index: usize) -> Option<String> {
    let classfile::Constant::Class(reference) = constants.get(index)? else {
        return None;
    };
    let inner: usize = reference.trim_start_matches('#').parse().ok()?;
    match constants.get(inner)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

fn simple_class(internal: &str) -> String {
    let last = internal.rsplit('/').next().unwrap_or(internal);
    last.rsplit('$').next().unwrap_or(last).to_string()
}

/// The implementation method an `invokedynamic` bootstrap points at: the first
/// method handle argument (`LambdaMetafactory`'s `implMethod`, or the second
/// argument for the constant-bootstrap forms).
fn lambda_target(class: &classfile::ClassFile, constant_index: usize) -> Option<(String, String)> {
    let classfile::Constant::InvokeDynamic { bootstrap, .. } =
        class.constants.get(constant_index)?
    else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
        let Some(classfile::Constant::MethodHandle { reference }) = class.constants.get(*argument)
        else {
            continue;
        };
        if let Some((owner, name, _)) = classfile::method_ref(&class.constants, *reference) {
            return Some((owner, name));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

/// What the integration produced, and every remaining gap class.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub states: usize,
    /// States of `dynamicShape()` blocks: uncached (`-1`) in collision/support.
    pub dynamic_states: usize,
    pub collision_covered: usize,
    pub occlusion_covered: usize,
    pub support_covered: usize,
    pub full_block_covered: usize,
    /// `"<table> <class>"` -> why the state could not be derived (first reason
    /// plus the count).
    pub gaps: BTreeMap<String, String>,
}

impl Coverage {
    fn note(
        &mut self,
        table: &str,
        class: &str,
        reason: &str,
        counts: &mut BTreeMap<String, usize>,
    ) {
        let key = format!("{table} {class}");
        *counts.entry(key.clone()).or_insert(0) += 1;
        let entry = self.gaps.entry(key).or_default();
        if entry.is_empty() {
            entry.push_str(reason);
        }
    }

    fn finish_gaps(&mut self, counts: &BTreeMap<String, usize>) {
        for (key, count) in counts {
            if let Some(reason) = self.gaps.get_mut(key) {
                if !reason.contains("states") {
                    reason.push_str(&format!(" [{count} states]"));
                }
            }
        }
    }
}

/// The three tables plus `full_block`, one entry per registry state.
pub struct Evaluated {
    pub collision: Vec<Option<geometry::Shape>>,
    pub occlusion: Vec<Option<geometry::Shape>>,
    pub support: Vec<Option<geometry::Shape>>,
    pub full_block: Vec<Option<bool>>,
    pub coverage: Coverage,
}

/// Evaluate the three tables and `full_block` from the jar.
pub fn evaluate(jar_path: &Path) -> Result<Evaluated, String> {
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    if registry.len() != state_table.blocks.len() {
        return Err("the block registry and the state table disagree".to_string());
    }
    let has_collision = shape_flags::has_collision(jar_path)?;
    if has_collision.len() != registry.len() {
        return Err(format!(
            "hasCollision flags {} != registry {}",
            has_collision.len(),
            registry.len()
        ));
    }
    let profile = class_profiles(jar_path)?;
    let dynamic = &profile.dynamic;
    let (default_collision, _) = geometry::collision_shapes(jar_path)?;
    let (default_occlusion, _) = geometry::occlusion_shapes(jar_path)?;
    let multiface_occlusion: Vec<Option<geometry::Shape>> =
        shape_stateholder::multiface_occlusion_table(jar_path)?
            .0
            .into_iter()
            .map(|shape| shape.map(|shape| from_stateholder(&shape)))
            .collect();
    let names: Vec<String> = JarSource::open(jar_path)?
        .names()
        .map(str::to_string)
        .collect();
    let mut vm = shape_offset::OffsetVm::new(jar_path, ExactFactory)?;

    let total = state_table.state_count();
    if default_collision.len() != total || default_occlusion.len() != total {
        return Err("the default-path tables do not cover every state".to_string());
    }
    let mut collision: Vec<Option<geometry::Shape>> = vec![None; total];
    let mut occlusion: Vec<Option<geometry::Shape>> = vec![None; total];
    let mut coverage = Coverage {
        states: total,
        ..Default::default()
    };
    let mut gap_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut index = 0usize;
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        let class_simple = &registry[block_index].class;
        let internal = resolve_class(&names, class_simple)
            .ok_or_else(|| format!("{}: class {class_simple} not in the jar", row.id))?;
        let is_dynamic = dynamic.contains(&registry[block_index].class);
        let class_collides = if profile.no_collision.contains(&registry[block_index].class) {
            false
        } else if profile.colliding.contains(&registry[block_index].class) {
            true
        } else {
            has_collision[block_index]
        };
        for state_index in 0..row.state_count {
            let pairs = property_values(row, state_index);
            let pair_refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            let values: BTreeMap<String, String> = pairs.iter().cloned().collect();
            if is_dynamic {
                coverage.dynamic_states += 1;
            }

            // ---- collision ----
            // The default-path entry is the *whole* answer for a class whose
            // chain does not override `getCollisionShape`: it is exactly
            // `hasCollision ? getShape (Shapes.block()) : Shapes.empty()`.
            // Selectors only cover classes that override the method, so they
            // come after it.
            let mut collision_shape: Option<geometry::Shape> = None;
            let mut collision_reason = String::new();
            if !is_dynamic {
                if let Some(shape) = &default_collision[index] {
                    collision_shape = Some(shape.clone());
                } else if let Some(shape) = shape_gate::shape_for_class(
                    class_simple,
                    &shape_gate::State::new(&pair_refs),
                    shape_gate::Method::Collision,
                ) {
                    collision_shape = Some(from_gate(shape.encode()));
                } else if let Some(shape) = shape_maps::shape_for_class(
                    class_simple,
                    &shape_maps::State::new(&pair_refs),
                    shape_maps::ShapeKind::Collision,
                ) {
                    collision_shape = Some(from_maps(shape.encode()));
                } else if let Some(shape) = shape_families::shape_for_class(
                    class_simple,
                    &shape_families::State::new(&pair_refs),
                    shape_families::ShapeKind::Collision,
                ) {
                    collision_shape = Some(from_families(shape.encode()));
                } else {
                    let vm_state = VmState {
                        class: internal.clone(),
                        block: row.id.clone(),
                        values: values.clone(),
                        has_collision: class_collides,
                    };
                    let result = vm.query(&vm_state, Query::Collision);
                    if result.warnings.is_empty() {
                        collision_shape = result.shape.map(|shape| shape.encode());
                        if collision_shape.is_none() {
                            collision_reason = "no shape returned".to_string();
                        }
                    } else {
                        collision_reason = result.warnings.join("; ");
                    }
                }
            }

            // ---- occlusion ----
            let mut occlusion_shape: Option<geometry::Shape> = None;
            let mut occlusion_reason = String::new();
            if let Some(shape) = &default_occlusion[index] {
                occlusion_shape = Some(shape.clone());
            } else if let Some(shape) = shape_gate::shape_for_class(
                class_simple,
                &shape_gate::State::new(&pair_refs),
                shape_gate::Method::Occlusion,
            ) {
                occlusion_shape = Some(from_gate(shape.encode()));
            } else if let Some(shape) = &multiface_occlusion[index] {
                occlusion_shape = Some(shape.clone());
            } else if let Some(shape) = shape_maps::shape_for_class(
                class_simple,
                &shape_maps::State::new(&pair_refs),
                shape_maps::ShapeKind::Occlusion,
            ) {
                occlusion_shape = Some(from_maps(shape.encode()));
            } else if let Some(shape) = shape_families::shape_for_class(
                class_simple,
                &shape_families::State::new(&pair_refs),
                shape_families::ShapeKind::Occlusion,
            ) {
                occlusion_shape = Some(from_families(shape.encode()));
            } else {
                let vm_state = VmState {
                    class: internal.clone(),
                    block: row.id.clone(),
                    values: values.clone(),
                    has_collision: class_collides,
                };
                let result = vm.query(&vm_state, Query::Occlusion);
                if result.warnings.is_empty() {
                    occlusion_shape = result.shape.map(|shape| shape.encode());
                    if occlusion_shape.is_none() {
                        occlusion_reason = "no shape returned".to_string();
                    }
                } else {
                    occlusion_reason = result.warnings.join("; ");
                }
            }

            if collision_shape.is_some() {
                coverage.collision_covered += 1;
            } else if !is_dynamic {
                coverage.note(
                    "collision",
                    class_simple,
                    &collision_reason,
                    &mut gap_counts,
                );
            }
            if occlusion_shape.is_some() {
                coverage.occlusion_covered += 1;
            } else {
                coverage.note(
                    "occlusion",
                    class_simple,
                    &occlusion_reason,
                    &mut gap_counts,
                );
            }
            collision[index] = collision_shape;
            occlusion[index] = occlusion_shape;
            index += 1;
        }
    }

    // ---- support ----
    let support_input: Vec<Option<shape_support::geometry::Shape>> = collision
        .iter()
        .map(|shape| shape.as_ref().map(to_support))
        .collect();
    let (support_raw, support_coverage) = shape_support::support_table(jar_path, &support_input)?;
    let mut support: Vec<Option<geometry::Shape>> = support_raw
        .into_iter()
        .map(|shape| shape.map(|shape| from_support(&shape)))
        .collect();
    if support.len() != total {
        return Err("the support table does not cover every state".to_string());
    }
    for (key, reason) in &support_coverage.gaps {
        let key = format!("support {key}");
        gap_counts.insert(key.clone(), 1);
        coverage.gaps.entry(key).or_insert_with(|| reason.clone());
    }
    // The support oracle skips the dynamic-shape blocks exactly like the
    // collision one; `support_table` cannot see that, so clear them here.
    let mut index = 0usize;
    let mut dynamic_classes: BTreeSet<String> = BTreeSet::new();
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        let is_dynamic = dynamic.contains(&registry[block_index].class);
        if is_dynamic {
            dynamic_classes.insert(registry[block_index].class.clone());
        }
        for _ in 0..row.state_count {
            if is_dynamic {
                support[index] = None;
            }
            index += 1;
        }
    }
    // ... and their missing collision shape is the design, not a gap.
    for class in dynamic_classes {
        coverage.gaps.remove(&format!("support {class}"));
        gap_counts.remove(&format!("support {class}"));
    }

    // ---- full_block ----
    let mut full_block: Vec<Option<bool>> = Vec::with_capacity(total);
    let mut index = 0usize;
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        let is_dynamic = dynamic.contains(&registry[block_index].class);
        let class_collides = if profile.no_collision.contains(&registry[block_index].class) {
            false
        } else if profile.colliding.contains(&registry[block_index].class) {
            true
        } else {
            has_collision[block_index]
        };
        for _ in 0..row.state_count {
            let value = if is_dynamic {
                // `!hasDynamicShape() && ...` is `false` for a dynamic block.
                Some(false)
            } else {
                collision[index].as_ref().map(|shape| shape.canonical == 2)
            };
            if value.is_some() {
                coverage.full_block_covered += 1;
            }
            full_block.push(value);
            index += 1;
        }
    }

    coverage.support_covered = support.iter().filter(|shape| shape.is_some()).count();
    for (key, reason) in &support_coverage.gaps {
        let _ = (key, reason);
    }
    coverage.finish_gaps(&gap_counts);
    Ok(Evaluated {
        collision,
        occlusion,
        support,
        full_block,
        coverage,
    })
}

// ---------------------------------------------------------------------------
// Writing the bundle
// ---------------------------------------------------------------------------

fn shape_key(shape: &geometry::Shape) -> Vec<u8> {
    let mut key = Vec::with_capacity(64 + shape.occupied.len());
    key.push(shape.canonical);
    key.push(shape.cube);
    key.push(shape.cube_axes);
    for axis in [&shape.x, &shape.y, &shape.z] {
        key.extend_from_slice(&(axis.len() as u32).to_le_bytes());
        for value in axis {
            key.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    for value in shape.discrete_min.iter().chain(shape.discrete_max.iter()) {
        key.extend_from_slice(&value.to_le_bytes());
    }
    key.extend_from_slice(&shape.occupied);
    key
}

/// Deduplicate a state list into a table dictionary, keeping the uncached `-1`
/// entries the oracles write for dynamic-shape blocks.
pub fn table_of(shapes: &[Option<geometry::Shape>], kind: Option<&str>) -> geometry::Table {
    let mut dictionary: Vec<geometry::Shape> = Vec::new();
    let mut index: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut cached = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let Some(shape) = shape else {
            cached.push(-1);
            continue;
        };
        let key = shape_key(shape);
        let id = match index.get(&key) {
            Some(id) => *id,
            None => {
                let id = dictionary.len();
                dictionary.push(shape.clone());
                index.insert(key, id);
                id
            }
        };
        cached.push(id as i32);
    }
    geometry::Table {
        kind: kind.map(str::to_string),
        data_version: DATA_VERSION,
        cached,
        shapes: dictionary,
    }
}

/// The `structure-blocks.json#full_block` array as JSON.
pub fn full_block_json(full_block: &[Option<bool>]) -> Value {
    Value::Array(
        full_block
            .iter()
            .map(|value| match value {
                Some(value) => Value::Bool(*value),
                None => Value::Null,
            })
            .collect(),
    )
}

/// Write the three NBT tables plus the `full_block` fragment below `out`.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let evaluated = evaluate(jar_path)?;
    write_bundle(&evaluated, out)?;
    Ok(evaluated.coverage.clone())
}

/// Write an already-evaluated bundle (the tables and the `full_block` fragment).
pub fn write_bundle(evaluated: &Evaluated, out: &Path) -> Result<(), String> {
    let tables: [(&str, &[Option<geometry::Shape>], Option<&str>); 3] = [
        (COLLISION, &evaluated.collision, None),
        (OCCLUSION, &evaluated.occlusion, None),
        (SUPPORT, &evaluated.support, Some(SUPPORT_KIND)),
    ];
    for (name, shapes, kind) in tables {
        let table = table_of(shapes, kind);
        let bytes = geometry::encode(&table);
        let path = out.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        fs::write(&path, &bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let mut root = Map::new();
    root.insert(
        "full_block".to_string(),
        full_block_json(&evaluated.full_block),
    );
    let text = serde_json::to_string(&Value::Object(root)).map_err(|error| error.to_string())?;
    let path = out.join(FULL_BLOCK);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(())
}
