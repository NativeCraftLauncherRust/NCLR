//! Static-map and constructor-built shape builders for the blocks whose shape
//! vanilla computes in a class initialiser or a constructor-created `Function`.
//!
//! `shape_families.rs` covers the families whose shape is selected from the
//! state's properties by a `switch` over pre-built shapes. The blocks here are
//! the remaining *static map* builders: their shapes come from `Map`s /
//! `VoxelShape[]`s / captured `Function`s that the class builds once
//! (`<clinit>` or the constructor), so a shape selector has to reproduce those
//! builders exactly:
//!
//! | class | source | tables |
//! | --- | --- | --- |
//! | `CeilingHangingSignBlock` | `<clinit>`: `SHAPE_DEFAULT = Block.column(10, 0, 16)`, `SHAPES = rotateHorizontal(Block.column(14, 2, 0, 10))` re-keyed by `RotationSegment` (N/S/E/W -> 0/4/8/12), `getShape = SHAPES.getOrDefault(rotation, SHAPE_DEFAULT)` | collision `noCollision()` -> empty; occlusion `getShape` |
//! | `FlowerBedBlock` | constructor: `SegmentableBlock.getShapeCalculator(FACING, AMOUNT)` with `getShapeHeight() == 3` (both registered blocks pass `3` to the constructor) | collision `noCollision()` -> empty; occlusion the calculator |
//! | `LeafLitterBlock` | same calculator, default `getShapeHeight() == 1` | collision empty; occlusion the calculator |
//! | `ComposterBlock` | `<clinit>`: `SHAPES = Block.boxes(8, i -> Shapes.join(Shapes.block(), Block.column(12, clamp(1 + 2i, 2, 16), 16), ONLY_FIRST))` then `SHAPES[8] = SHAPES[7]`; `getShape = SHAPES[level]`, `getCollisionShape = SHAPES[0]` | both |
//! | `DecoratedPotBlock` | `<clinit>`: `SHAPE = Block.column(14, 0, 16)` | both `getShape` |
//! | `HopperBlock` | constructor: `base = Block.column(12, 11, 16)`, `shapes = makeShapes(base)`; `makeShapes` builds `or(column(16, 10, 16), column(8, 4, 10))`, `ONLY_FIRST` against `base`, and a `rotateAll(boxZ(4, 4, 8, 0, 8), Vec3(8, 6, 8).scale(1/16))` spout map; `getShape` ORs `and(spout, Shapes.block())` onto the body | both `getShape` |
//! | `MossyCarpetBlock` | constructor: `makeShapes()` builds `rotateHorizontal(boxZ(16, 0, 10, 0, 1))` (low plates) and `rotateAll(boxZ(16, 0, 1))` (tall plates); the captured `Function` starts from the `DOWN` tall plate when `bottom` and ORs one plate per `low`/`tall` side, falling back to `Shapes.block()` when empty; `getCollisionShape = bottom ? shape(defaultState) : empty` | both |
//!
//! # Shape model
//!
//! `shape_families::VoxelShape` (`src/voxelshape.rs`) is the model the family
//! selectors build on, but it documents two deliberate approximations this
//! module cannot use: its `optimize()` treats a uniform coordinate grid as a
//! `CubeVoxelShape` and it has no `BooleanOp.ONLY_FIRST`. The prepared tables
//! are produced by `Shapes.join(a, b, op).optimize()` exactly as the jar
//! implements it, where `optimize()` folds `Shapes.joinUnoptimized` over
//! `forAllBoxes` and the per-axis `CubePointRange` bits come from
//! `Shapes.createIndexMerger`:
//!
//! * both merged lists are `CubePointRange`s and `cells_so_far * lcm(a, b) <= 256`
//!   -> `DiscreteCubeMerger` (coords are the full `CubePointRange`, bit set);
//! * disjoint lists -> `NonOverlappingMerger` (array list, bit clear);
//! * equal size and equal list object -> `IdenticalMerger` (keeps the first
//!   list's bit; not reproducible from values, see [`merge`]);
//! * otherwise `IndirectMerger` (array list, bit clear).
//!
//! That detail matters: `ComposterBlock`'s nine shapes are only byte-identical
//! when the third axis falls out of `DiscreteCubeMerger` because
//! `64 * lcm(1, 8) > 256` (the prepared tables store `cube_axes == 3` for the
//! level-0 composter, not `7`). [`Shape`] therefore carries the same information
//! a `VoxelShape` object does (coords, occupancy, class, per-axis range bits)
//! and implements the four operations these builders need: `Shapes.create`
//! (`box`/`column`/`boxZ`), `Shapes.join` + `optimize` (`or`/`and`/`onlyFirst`),
//! `Shapes.rotate` (including a non-block-centre `Vec3`) and
//! `VoxelShape.singleEncompassing`.
//!
//! A [`Shape`] converts to `shape_families::EncodedShape`, so callers can treat
//! the two modules the same way.
//!
//! # Coverage
//!
//! `family_for_class` names the classes handled; `shape_for_class` returns
//! `None` for anything else, keeping the gap reports of the other engines
//! honest. The builder-side table derivations (which block uses which class) are
//! not repeated here: `block::registry` has the class per block.

#![allow(dead_code)]

#[path = "shape_families.rs"]
mod shape_families;

pub use shape_families::{EncodedShape, ShapeKind, State};

const EPSILON: f64 = 1e-7;

/// `Vec3(0.5, 0.5, 0.5)`, vanilla's `Shapes.BLOCK_CENTER`.
const BLOCK_CENTER: [f64; 3] = [0.5, 0.5, 0.5];

// ---------------------------------------------------------------------------
// The shape model: `VoxelShape` fields plus the merger bookkeeping
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Empty,
    Block,
    Cube,
    Array,
}

/// A `VoxelShape`: per-axis boundary coordinates, a discrete occupancy grid and
/// the constructor history the encoder reads back (`cube`, `cube_axes`).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    coords: [Vec<f64>; 3],
    /// `occupied[(x * ny + y) * nz + z]` over `sizes = len(coords) - 1`.
    occupied: Vec<bool>,
    kind: Kind,
    /// Per-axis `getCoords(axis) instanceof CubePointRange`.
    ranges: [bool; 3],
}

impl Shape {
    /// `Shapes.empty()`.
    pub fn empty() -> Shape {
        Shape {
            coords: [vec![0.0], vec![0.0], vec![0.0]],
            occupied: Vec::new(),
            kind: Kind::Empty,
            ranges: [false; 3],
        }
    }

    /// `Shapes.block()`.
    pub fn block() -> Shape {
        Shape {
            coords: [vec![0.0, 1.0], vec![0.0, 1.0], vec![0.0, 1.0]],
            occupied: vec![true],
            kind: Kind::Block,
            ranges: [true; 3],
        }
    }

    /// `Shapes.box(x1, y1, z1, x2, y2, z2)` in block units (`Shapes.create`).
    pub fn create(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Shape {
        if x2 - x1 < EPSILON || y2 - y1 < EPSILON || z2 - z1 < EPSILON {
            return Shape::empty();
        }
        let bits = [find_bits(x1, x2), find_bits(y1, y2), find_bits(z1, z2)];
        if bits.iter().any(|bit| bit.is_none()) {
            // `new ArrayVoxelShape(BLOCK.shape, exact coords)`: one cell.
            return Shape {
                coords: [vec![x1, x2], vec![y1, y2], vec![z1, z2]],
                occupied: vec![true],
                kind: Kind::Array,
                ranges: [false; 3],
            };
        }
        let bits = [bits[0].unwrap(), bits[1].unwrap(), bits[2].unwrap()];
        if bits == [0, 0, 0] {
            return Shape::block();
        }
        let sizes = [1usize << bits[0], 1usize << bits[1], 1usize << bits[2]];
        let coords = [
            grid_coords(sizes[0]),
            grid_coords(sizes[1]),
            grid_coords(sizes[2]),
        ];
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
        let mut shape = Shape {
            coords,
            occupied: vec![false; sizes[0] * sizes[1] * sizes[2]],
            kind: Kind::Cube,
            ranges: [true; 3],
        };
        for x in low[0]..high[0] {
            for y in low[1]..high[1] {
                for z in low[2]..high[2] {
                    shape.set(x as usize, y as usize, z as usize, true);
                }
            }
        }
        shape
    }

    /// `Block.box(x1, y1, z1, x2, y2, z2)`: `Shapes.create` in 1/16 units.
    pub fn block_box(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> Shape {
        Shape::create(
            x1 / 16.0,
            y1 / 16.0,
            z1 / 16.0,
            x2 / 16.0,
            y2 / 16.0,
            z2 / 16.0,
        )
    }

    /// `Block.column(size, y1, y2)` in 1/16 units.
    pub fn block_column(size: f64, y1: f64, y2: f64) -> Shape {
        Shape::block_column4(size, size, y1, y2)
    }

    /// `Block.column(sizeX, sizeZ, y1, y2)` in 1/16 units.
    pub fn block_column4(size_x: f64, size_z: f64, y1: f64, y2: f64) -> Shape {
        Shape::block_box(
            8.0 - size_x / 2.0,
            y1,
            8.0 - size_z / 2.0,
            8.0 + size_x / 2.0,
            y2,
            8.0 + size_z / 2.0,
        )
    }

    /// `Block.boxZ(sizeX, y1, y2, z1, z2)` in 1/16 units.
    pub fn block_box_z(size_x: f64, y1: f64, y2: f64, z1: f64, z2: f64) -> Shape {
        Shape::block_box(8.0 - size_x / 2.0, y1, z1, 8.0 + size_x / 2.0, y2, z2)
    }

    /// `Block.boxZ(size, z1, z2)` in 1/16 units.
    pub fn block_box_z_square(size: f64, z1: f64, z2: f64) -> Shape {
        Shape::block_box_z(size, 8.0 - size / 2.0, 8.0 + size / 2.0, z1, z2)
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
            let index = Shape::key(x, y, z, &sizes);
            self.occupied[index] = value;
        }
    }

    /// Whether a point is inside the shape (`VoxelShape` cell lookup).
    pub fn contains(&self, point: [f64; 3]) -> bool {
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
        self.occupied[Shape::key(cell[0], cell[1], cell[2], &sizes)]
    }

    /// `VoxelShape.isEmpty()`.
    pub fn is_empty(&self) -> bool {
        self.kind == Kind::Empty || !self.occupied.iter().any(|value| *value)
    }

    /// `VoxelShape.isFullBlock()`.
    pub fn is_full_block(&self) -> bool {
        self.kind == Kind::Block
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

    /// `DiscreteVoxelShape.firstFull(axis)`.
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
                        self.occupied[Shape::key(slot[0], slot[1], slot[2], &sizes)]
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

    /// `DiscreteVoxelShape.lastFull(axis)`: one past the last full cell.
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
                        self.occupied[Shape::key(slot[0], slot[1], slot[2], &sizes)]
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

    /// `VoxelShape.singleEncompassing()`: the bounding box as one shape.
    pub fn single_encompassing(&self) -> Shape {
        if self.is_empty() {
            return Shape::empty();
        }
        let low = self.discrete_min();
        let high = self.discrete_max();
        let mut min = [0.0f64; 3];
        let mut max = [1.0f64; 3];
        for axis in 0..3 {
            let coords = &self.coords[axis];
            min[axis] = coords[(low[axis] as usize).min(coords.len() - 1)];
            max[axis] = coords[(high[axis] as usize).min(coords.len() - 1)];
        }
        Shape::create(min[0], min[1], min[2], max[0], max[1], max[2])
    }

    /// `Shapes.or`.
    pub fn or(&self, other: &Shape) -> Shape {
        merge(self, other, Op::Or).optimize()
    }

    /// `Shapes.and`.
    pub fn and(&self, other: &Shape) -> Shape {
        merge(self, other, Op::And).optimize()
    }

    /// `Shapes.join(self, other, BooleanOp.ONLY_FIRST)`.
    pub fn only_first(&self, other: &Shape) -> Shape {
        merge(self, other, Op::OnlyFirst).optimize()
    }

    /// `VoxelShape.optimize()`: fold `Shapes.joinUnoptimized` over the maximal
    /// box list (`forAllBoxes`) re-created through `Shapes.box`.
    fn optimize(self) -> Shape {
        if self.is_empty() {
            return Shape::empty();
        }
        let mut result = Shape::empty();
        for cells in self.boxes() {
            let boxed = Shape::create(
                self.coords[0][cells[0]],
                self.coords[1][cells[1]],
                self.coords[2][cells[2]],
                self.coords[0][cells[3]],
                self.coords[1][cells[4]],
                self.coords[2][cells[5]],
            );
            result = merge(&result, &boxed, Op::Or);
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

    /// The exact `EncodedShape` of the prepared tables.
    pub fn encode(&self) -> EncodedShape {
        if self.is_empty() {
            return EncodedShape {
                x: vec![0.0],
                y: vec![0.0],
                z: vec![0.0],
                canonical: 1,
                cube: 0,
                cube_axes: 0,
                discrete_min: [0, 0, 0],
                discrete_max: [0, 0, 0],
                occupied: Vec::new(),
            };
        }
        let sizes = self.sizes();
        let mut occupied = Vec::with_capacity(sizes[0] * sizes[1] * sizes[2]);
        for x in 0..sizes[0] {
            for y in 0..sizes[1] {
                for z in 0..sizes[2] {
                    occupied.push(self.occupied[Shape::key(x, y, z, &sizes)] as u8);
                }
            }
        }
        EncodedShape {
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

/// `Shapes.findBits`: the smallest `k` in 0..=3 that makes both scaled values
/// integral (within `1e-7 * 2^k`), or `None`.
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

/// The `CubePointRange` coordinates `0, 1/size, ..., 1`.
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

// ---------------------------------------------------------------------------
// The boolean ops (`Shapes.joinUnoptimized`)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Or,
    And,
    OnlyFirst,
}

impl Op {
    fn apply(self, first: bool, second: bool) -> bool {
        match self {
            Op::Or => first || second,
            Op::And => first && second,
            Op::OnlyFirst => first && !second,
        }
    }
}

/// `Shapes.joinUnoptimized` (no `optimize()`).
///
/// The `a == b` early return needs object identity and cannot be reached from
/// values (the builders never join a shape with itself), and `IdenticalMerger`
/// needs `Objects.equals` on the *list objects*: two distinct `DoubleList`s with
/// equal values are never identical, so the equal-values case falls through to
/// `IndirectMerger` here. That is the behaviour the prepared tables show.
fn merge(first: &Shape, second: &Shape, op: Op) -> Shape {
    let first_only = op.apply(true, false);
    let second_only = op.apply(false, true);
    if first.is_empty() {
        return if second_only {
            second.clone()
        } else {
            Shape::empty()
        };
    }
    if second.is_empty() {
        return if first_only {
            first.clone()
        } else {
            Shape::empty()
        };
    }
    let first_sizes = first.sizes();
    let second_sizes = second.sizes();
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut ranges = [false; 3];
    let mut all_discrete = true;
    let mut cells_so_far = 1usize;
    for axis in 0..3 {
        let size_first = first_sizes[axis];
        let size_second = second_sizes[axis];
        let mut discrete = false;
        if first.ranges[axis] && second.ranges[axis] {
            let merged = lcm(size_first, size_second);
            // `createIndexMerger`'s guard: `cells * lcm <= 256`.
            if (cells_so_far as u64) * (merged as u64) <= 256 {
                discrete = true;
                coords[axis] = grid_coords(merged);
            }
        }
        if !discrete {
            all_discrete = false;
            let a = &first.coords[axis];
            let b = &second.coords[axis];
            let mut values = a.clone();
            values.extend(b.iter().copied());
            values.sort_by(|left, right| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            });
            values.dedup_by(|left, right| (*left - *right).abs() < EPSILON);
            coords[axis] = values;
        }
        ranges[axis] = discrete;
        // `createIndexMerger` receives the number of cells produced by the
        // preceding axes (`1`, `sx`, `sx * sy`).
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
                occupied[Shape::key(x, y, z, &sizes)] =
                    op.apply(first.contains(point), second.contains(point));
            }
        }
    }
    Shape {
        coords,
        occupied,
        kind: if all_discrete {
            Kind::Cube
        } else {
            Kind::Array
        },
        ranges,
    }
}

// ---------------------------------------------------------------------------
// Rotations (`Shapes.rotate`)
// ---------------------------------------------------------------------------

/// A vanilla `OctahedralGroup`: output axis `i` reads source axis `p[i]` and
/// mirrors it around the rotation centre when `inv[i]` is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rotation {
    p: [usize; 3],
    inv: [bool; 3],
}

impl Rotation {
    pub const IDENTITY: Rotation = Rotation {
        p: [0, 1, 2],
        inv: [false, false, false],
    };
    /// `BLOCK_ROT_Y_90` (`ROT_90_Y_NEG`): `(x, y, z) -> (1 - z, y, x)`.
    pub const Y90: Rotation = Rotation {
        p: [2, 1, 0],
        inv: [true, false, false],
    };
    /// `BLOCK_ROT_Y_180` (`ROT_180_FACE_XZ`): `(x, y, z) -> (1 - x, y, 1 - z)`.
    pub const Y180: Rotation = Rotation {
        p: [0, 1, 2],
        inv: [true, false, true],
    };
    /// `BLOCK_ROT_Y_270` (`ROT_90_Y_POS`): `(x, y, z) -> (z, y, 1 - x)`.
    pub const Y270: Rotation = Rotation {
        p: [2, 1, 0],
        inv: [false, false, true],
    };
    /// `BLOCK_ROT_X_90` (`ROT_90_X_NEG`): `(x, y, z) -> (x, z, 1 - y)`.
    pub const X90: Rotation = Rotation {
        p: [0, 2, 1],
        inv: [false, false, true],
    };
    /// `BLOCK_ROT_X_270` (`ROT_90_X_POS`): `(x, y, z) -> (x, 1 - z, y)`.
    pub const X270: Rotation = Rotation {
        p: [0, 2, 1],
        inv: [false, true, false],
    };

    fn is_identity(self) -> bool {
        self == Rotation::IDENTITY
    }
}

/// `Shapes.rotate(shape, group, center)`.
///
/// `IDENTITY` returns the shape itself; a `CubeVoxelShape` rotated around the
/// block centre stays a `CubeVoxelShape`; everything else becomes an
/// `ArrayVoxelShape` with the permuted/mirrored coordinate lists.
pub fn rotate(shape: &Shape, rotation: Rotation, center: [f64; 3]) -> Shape {
    if rotation.is_identity() {
        return shape.clone();
    }
    let sizes = shape.sizes();
    let new_sizes = [
        sizes[rotation.p[0]],
        sizes[rotation.p[1]],
        sizes[rotation.p[2]],
    ];
    let cube = matches!(shape.kind, Kind::Cube | Kind::Block);
    let block_center = center == BLOCK_CENTER;
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut occupied = vec![false; new_sizes[0] * new_sizes[1] * new_sizes[2]];
    for axis in 0..3 {
        if cube && block_center {
            // The rotated `CubePointRange` is the range of the new size.
            coords[axis] = grid_coords(new_sizes[axis]);
        } else {
            let source = &shape.coords[rotation.p[axis]];
            let factor = center[axis] + center[rotation.p[axis]];
            let mut values = Vec::with_capacity(source.len());
            if rotation.inv[axis] {
                for index in (0..source.len()).rev() {
                    values.push(factor - source[index]);
                }
            } else {
                for value in source {
                    values.push(*value - center[rotation.p[axis]] + center[axis]);
                }
            }
            coords[axis] = values;
        }
    }
    for x in 0..new_sizes[0] {
        for y in 0..new_sizes[1] {
            for z in 0..new_sizes[2] {
                let index = [x, y, z];
                let mut source = [0usize; 3];
                for axis in 0..3 {
                    let from = rotation.p[axis];
                    source[from] = if rotation.inv[axis] {
                        sizes[from] - 1 - index[axis]
                    } else {
                        index[axis]
                    };
                }
                let value = shape.occupied[Shape::key(source[0], source[1], source[2], &sizes)];
                occupied[Shape::key(x, y, z, &new_sizes)] = value;
            }
        }
    }
    let (kind, ranges) = if cube && block_center {
        (Kind::Cube, [true; 3])
    } else {
        (Kind::Array, [false; 3])
    };
    Shape {
        coords,
        occupied,
        kind,
        ranges,
    }
}

/// `Shapes.rotateHorizontal(shape)`: `NORTH`, `EAST`, `SOUTH`, `WEST`.
pub fn rotate_horizontal(shape: &Shape) -> [Shape; 4] {
    [
        rotate(shape, Rotation::IDENTITY, BLOCK_CENTER),
        rotate(shape, Rotation::Y90, BLOCK_CENTER),
        rotate(shape, Rotation::Y180, BLOCK_CENTER),
        rotate(shape, Rotation::Y270, BLOCK_CENTER),
    ]
}

/// `Shapes.rotateAll(shape, center)`: `NORTH`, `EAST`, `SOUTH`, `WEST`, `UP`,
/// `DOWN`.
pub fn rotate_all(shape: &Shape, center: [f64; 3]) -> [Shape; 6] {
    [
        rotate(shape, Rotation::IDENTITY, center),
        rotate(shape, Rotation::Y90, center),
        rotate(shape, Rotation::Y180, center),
        rotate(shape, Rotation::Y270, center),
        rotate(shape, Rotation::X270, center),
        rotate(shape, Rotation::X90, center),
    ]
}

/// `Direction.getCounterClockWise` for the four horizontal directions.
fn counter_clockwise(name: &str) -> &'static str {
    match name {
        "north" => "west",
        "west" => "south",
        "south" => "east",
        _ => "north",
    }
}

/// The index of a horizontal direction in `rotate_horizontal`/`rotate_all`.
fn horizontal_index(name: &str) -> Option<usize> {
    match name {
        "north" => Some(0),
        "east" => Some(1),
        "south" => Some(2),
        "west" => Some(3),
        _ => None,
    }
}

/// The index of any direction in `rotate_all`.
fn direction_index(name: &str) -> Option<usize> {
    match name {
        "up" => Some(4),
        "down" => Some(5),
        _ => horizontal_index(name),
    }
}

// ---------------------------------------------------------------------------
// Families
// ---------------------------------------------------------------------------

/// The classes whose shapes this module derives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapFamily {
    /// `CeilingHangingSignBlock`: `SHAPES` keyed by the 16-segment rotation.
    CeilingHangingSign,
    /// `FlowerBedBlock`: the `SegmentableBlock` shape calculator, height 3/16.
    FlowerBed,
    /// `LeafLitterBlock`: the same calculator, height 1/16.
    LeafLitter,
    /// `ComposterBlock.SHAPES` (`ONLY_FIRST` against a growing column).
    Composter,
    /// `DecoratedPotBlock.SHAPE`.
    DecoratedPot,
    /// `HopperBlock`: body, `ONLY_FIRST` lip and the `rotateAll` spout.
    Hopper,
    /// `MossyCarpetBlock`: base plate plus `LOW`/`TALL` side plates.
    MossyCarpet,
}

/// Every class name this module understands, with its family.
pub const CLASSES: &[(&str, MapFamily)] = &[
    ("CeilingHangingSignBlock", MapFamily::CeilingHangingSign),
    ("FlowerBedBlock", MapFamily::FlowerBed),
    ("LeafLitterBlock", MapFamily::LeafLitter),
    ("ComposterBlock", MapFamily::Composter),
    ("DecoratedPotBlock", MapFamily::DecoratedPot),
    ("HopperBlock", MapFamily::Hopper),
    ("MossyCarpetBlock", MapFamily::MossyCarpet),
];

/// The family of a block class, or `None` when this module does not cover it.
pub fn family_for_class(class: &str) -> Option<MapFamily> {
    CLASSES
        .iter()
        .find(|(name, _)| *name == class)
        .map(|(_, family)| *family)
}

/// The shape of one state, or `None` when the class is not covered.
pub fn shape_for_class(class: &str, state: &State, kind: ShapeKind) -> Option<Shape> {
    let family = family_for_class(class)?;
    Some(shape_for_family(family, state, kind))
}

/// The shape one state of a family contributes to `kind`'s table.
pub fn shape_for_family(family: MapFamily, state: &State, kind: ShapeKind) -> Shape {
    match family {
        MapFamily::CeilingHangingSign => ceiling_hanging_sign(state, kind),
        MapFamily::FlowerBed => flower_bed(state, kind, 3.0, "flower_amount"),
        MapFamily::LeafLitter => flower_bed(state, kind, 1.0, "segment_amount"),
        MapFamily::Composter => composter(state, kind),
        MapFamily::DecoratedPot => decorated_pot(),
        MapFamily::Hopper => hopper(state),
        MapFamily::MossyCarpet => mossy_carpet(state, kind),
    }
}

/// `CeilingHangingSignBlock`: `SHAPE_DEFAULT` (the pole) for the twelve
/// rotations a board does not cover, and the `rotateHorizontal` board otherwise.
/// The class is registered without collision.
fn ceiling_hanging_sign(state: &State, kind: ShapeKind) -> Shape {
    if kind == ShapeKind::Collision {
        return Shape::empty();
    }
    // `SHAPES = Shapes.rotateHorizontal(Block.column(14, 2, 0, 10))` re-keyed by
    // `RotationSegment.convertToSegment`: north/east/south/west -> 0/4/8/12.
    let board = Shape::block_column4(14.0, 2.0, 0.0, 10.0);
    match state.int("rotation").unwrap_or(0) {
        0 => rotate(&board, Rotation::IDENTITY, BLOCK_CENTER),
        4 => rotate(&board, Rotation::Y90, BLOCK_CENTER),
        8 => rotate(&board, Rotation::Y180, BLOCK_CENTER),
        12 => rotate(&board, Rotation::Y270, BLOCK_CENTER),
        _ => Shape::block_column(10.0, 0.0, 16.0),
    }
}

/// `SegmentableBlock.getShapeCalculator(facing, amount)`:
/// `Block.box(0, 0, 0, 8, height, 8)` rotated to each horizontal direction, then
/// one quarter is ORed per `amount` while walking counter-clockwise from
/// `facing`, and the result is replaced by its `singleEncompassing()` box.
fn flower_bed(state: &State, kind: ShapeKind, height: f64, amount_property: &str) -> Shape {
    if kind == ShapeKind::Collision {
        return Shape::empty();
    }
    let base = Shape::block_box(0.0, 0.0, 0.0, 8.0, height, 8.0);
    let plates = rotate_horizontal(&base);
    let amount = state.int(amount_property).unwrap_or(1).max(0);
    let mut direction = state.text("facing").unwrap_or("north");
    let mut shape = Shape::empty();
    for _ in 0..amount {
        if let Some(index) = horizontal_index(direction) {
            shape = shape.or(&plates[index]);
        }
        direction = counter_clockwise(direction);
    }
    shape.single_encompassing()
}

/// `ComposterBlock.SHAPES[i]`: `Shapes.block()` with the inner
/// `Block.column(12, clamp(1 + 2i, 2, 16), 16)` removed. `SHAPES[8]` is
/// overwritten with `SHAPES[7]` in `<clinit>`; `getCollisionShape` is
/// hard-coded to `SHAPES[0]` and `getShape` indexes by the compost level.
fn composter(state: &State, kind: ShapeKind) -> Shape {
    let index = match kind {
        ShapeKind::Collision => 0,
        ShapeKind::Occlusion => state.int("level").unwrap_or(0),
    };
    composter_shape(index)
}

fn composter_shape(index: i32) -> Shape {
    let index = index.clamp(0, 7);
    let height = (1 + 2 * index).clamp(2, 16) as f64;
    Shape::block().only_first(&Shape::block_column(12.0, height, 16.0))
}

/// `DecoratedPotBlock.SHAPE = Block.column(14, 0, 16)`.
fn decorated_pot() -> Shape {
    Shape::block_column(14.0, 0.0, 16.0)
}

/// `HopperBlock.getShape`: the bowl (`column(16, 10, 16)`) and neck
/// (`column(8, 4, 10)`) minus the funnel opening (`column(12, 11, 16)`, joined
/// with `ONLY_FIRST`), ORed with the spout plate rotated to the hopper's facing
/// about `Vec3(8, 6, 8) / 16` and clipped to the block.
fn hopper(state: &State) -> Shape {
    let opening = Shape::block_column(12.0, 11.0, 16.0);
    let bowl = Shape::block_column(16.0, 10.0, 16.0);
    let neck = Shape::block_column(8.0, 4.0, 10.0);
    let body = bowl.or(&neck).only_first(&opening);
    // `Shapes.rotateAll(Block.boxZ(4, 4, 8, 0, 8), Vec3(8, 6, 8).scale(1/16))`
    let spout = Shape::block_box_z(4.0, 4.0, 8.0, 0.0, 8.0);
    let spouts = rotate_all(&spout, [0.5, 0.375, 0.5]);
    let facing = state.text("facing").unwrap_or("down");
    let piece = match direction_index(facing) {
        Some(index) => spouts[index].and(&Shape::block()),
        None => Shape::empty(),
    };
    body.or(&piece)
}

/// `MossyCarpetBlock.makeShapes`: the `DOWN` tall plate when `bottom`, plus one
/// plate per side (`LOW` = `boxZ(16, 0, 10, 0, 1)`, `TALL` = `boxZ(16, 0, 1)`),
/// falling back to `Shapes.block()` when nothing is set. The collision shape is
/// the *default state's* shape (all sides `none`) when `bottom`, else empty.
fn mossy_carpet(state: &State, kind: ShapeKind) -> Shape {
    let tall = rotate_all(&Shape::block_box_z_square(16.0, 0.0, 1.0), BLOCK_CENTER);
    let low = rotate_horizontal(&Shape::block_box_z(16.0, 0.0, 10.0, 0.0, 1.0));
    let base = state.boolean("bottom").unwrap_or(false);
    if kind == ShapeKind::Collision {
        return if base {
            tall[5].clone()
        } else {
            Shape::empty()
        };
    }
    let mut shape = if base {
        tall[5].clone()
    } else {
        Shape::empty()
    };
    for direction in ["north", "east", "south", "west"] {
        let Some(side) = state.text(direction) else {
            continue;
        };
        let Some(index) = horizontal_index(direction) else {
            continue;
        };
        match side {
            "low" => shape = shape.or(&low[index]),
            "tall" => shape = shape.or(&tall[index]),
            _ => {}
        }
    }
    if shape.is_empty() {
        Shape::block()
    } else {
        shape
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_is_a_cube_shape_on_its_axes() {
        // `Block.column(12, 2, 16)`: 2/16 = 1/8 and 16/16 are both on the
        // eight-cell grid, so every axis is a full `CubePointRange`.
        let shape = Shape::block_column(12.0, 2.0, 16.0).encode();
        assert_eq!(
            shape.y,
            vec![0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0]
        );
        assert_eq!((shape.cube, shape.cube_axes), (1, 7));
        assert_eq!(shape.occupied.len(), 8 * 8 * 8);
        assert_eq!(
            shape.occupied.iter().filter(|cell| **cell == 1).count(),
            6 * 7 * 6
        );
        assert_eq!(shape.discrete_min, [1, 1, 1]);
        assert_eq!(shape.discrete_max, [7, 8, 7]);
        // A column whose height is not on the 1/8 grid is an array box, as in
        // `ComposterBlock.SHAPES[1]`.
        let odd = Shape::block_column(12.0, 3.0, 16.0).encode();
        assert_eq!(odd.x, vec![0.125, 0.875]);
        assert_eq!(odd.y, vec![0.1875, 1.0]);
        assert_eq!((odd.cube, odd.cube_axes), (0, 0));
    }

    #[test]
    fn only_first_removes_the_inner_column() {
        // Level 0: the column starts at 2/16, so the difference is a bowl.
        let level0 = composter_shape(0).encode();
        assert_eq!(level0.canonical, 0);
        assert_eq!(level0.cube, 0);
        // `cube_axes == 3`: x/y are `DiscreteCubeMerger`s and z falls outside the
        // 256-cell guard, exactly as the prepared table records.
        assert_eq!(level0.cube_axes, 3);
        assert_eq!(level0.y.len(), 9);
        // Level 7/8: the hole is 15/16 deep, so y keeps only two coordinates.
        let level7 = composter_shape(7).encode();
        assert_eq!(level7.y, vec![0.0, 0.9375, 1.0]);
        assert_eq!(level7.cube_axes, 0);
    }

    #[test]
    fn the_hopper_spout_rotates_about_an_off_centre_point() {
        let down = [("enabled", "true"), ("facing", "down")];
        let shape = hopper(&State::new(&down));
        let encoded = shape.encode();
        assert_eq!(encoded.canonical, 0);
        // The down spout reaches y = 0 and is clipped there.
        assert_eq!(encoded.discrete_min[1], 0);
        // ... and it is not symmetric with the north spout.
        let north = [("enabled", "true"), ("facing", "north")];
        assert_ne!(encoded, hopper(&State::new(&north)).encode());
    }

    #[test]
    fn a_ceiling_hanging_sign_has_no_collision() {
        let board = [
            ("rotation", "0"),
            ("attached", "false"),
            ("waterlogged", "false"),
        ];
        let state = State::new(&board);
        assert!(ceiling_hanging_sign(&state, ShapeKind::Collision).is_empty());
        let board_shape = ceiling_hanging_sign(&state, ShapeKind::Occlusion).encode();
        assert_eq!(board_shape.x, vec![0.0625, 0.9375]);
        assert_eq!(board_shape.y, vec![0.0, 0.625]);
        assert_eq!(board_shape.z, vec![0.4375, 0.5625]);
        // The twelve unclaimed rotations fall back to the pole shape.
        let pole_state = [
            ("rotation", "3"),
            ("attached", "true"),
            ("waterlogged", "false"),
        ];
        let pole = ceiling_hanging_sign(&State::new(&pole_state), ShapeKind::Occlusion).encode();
        assert_eq!(pole.x, vec![0.1875, 0.8125]);
        assert_eq!(pole.y, vec![0.0, 1.0]);
    }

    #[test]
    fn flower_beds_accumulate_counter_clockwise_quarters() {
        let one = [("facing", "north"), ("flower_amount", "1")];
        let two = [("facing", "north"), ("flower_amount", "2")];
        let three = [("facing", "north"), ("flower_amount", "3")];
        let four = [("facing", "north"), ("flower_amount", "4")];
        let shape = |values: &[(&str, &str)]| {
            flower_bed(
                &State::new(values),
                ShapeKind::Occlusion,
                3.0,
                "flower_amount",
            )
            .encode()
        };
        assert_eq!(shape(&one).x, vec![0.0, 0.5]);
        assert_eq!(shape(&one).z, vec![0.0, 0.5]);
        assert_eq!(shape(&two).z, vec![0.0, 1.0]);
        assert_eq!(shape(&three).x, vec![0.0, 1.0]);
        assert_eq!(shape(&three).z, vec![0.0, 1.0]);
        assert_eq!(shape(&four).y, vec![0.0, 0.1875]);
        assert!(flower_bed(
            &State::new(&four),
            ShapeKind::Collision,
            3.0,
            "flower_amount"
        )
        .is_empty());
    }

    #[test]
    fn the_mossy_carpet_collision_is_the_down_plate_for_every_bottom_state() {
        let bottom = [
            ("bottom", "true"),
            ("east", "tall"),
            ("north", "none"),
            ("south", "none"),
            ("west", "none"),
        ];
        let plate = mossy_carpet(&State::new(&bottom), ShapeKind::Collision).encode();
        assert_eq!(plate.y, vec![0.0, 0.0625]);
        assert_eq!(plate.x, vec![0.0, 1.0]);
        let hollow = [
            ("bottom", "false"),
            ("east", "tall"),
            ("north", "none"),
            ("south", "none"),
            ("west", "none"),
        ];
        assert!(mossy_carpet(&State::new(&hollow), ShapeKind::Collision).is_empty());
    }
}
