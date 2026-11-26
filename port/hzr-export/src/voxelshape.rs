//! Vanilla `VoxelShape` semantics: the model and factories the shape families
//! build on.
//!
//! Horizon's shape tables encode a shape as the per-axis boundary coordinates
//! plus a discrete occupancy grid; this module reproduces the vanilla objects
//! that produce those fields, so the family selectors in `shape_families.rs`
//! (and the method VM) can build shapes exactly.
//!
//! # What vanilla does (from the jar)
//!
//! `Shapes.create(x1,y1,z1,x2,y2,z2)`:
//!
//! 1. an axis with `max - min < 1e-7` makes the shape `Shapes.empty()`;
//! 2. `findBits(min,max)` is the smallest `k` in 0..=3 with both
//!    `min * 2^k` and `max * 2^k` integers (within `1e-7 * 2^k`);
//! 3. if any axis has no such `k`, the box becomes an `ArrayVoxelShape`:
//!    one discrete cell, exact boundary coords, `cube = 0`, `cubeAxes = 0`;
//! 4. if all three bits are 0 the box is `0..1` on every axis, and `create`
//!    returns the `Shapes.block()` singleton (`canonical = 2`);
//! 5. otherwise it builds a `BitSetDiscreteVoxelShape` of size `2^k` per axis
//!    with the filled bounds `round(min * 2^k) .. round(max * 2^k)` (max
//!    exclusive) and wraps it in a `CubeVoxelShape`: `cube = 1`,
//!    `cube_axes = 7`, and `getCoords(axis)` is the `CubePointRange`
//!    `0, 1/2^k, ..., 1`.
//!
//! In particular **the grid is adaptive (1/8 at the finest), not a fixed 1/16
//! grid**: a box on the 1/16 lattice (e.g. farmland's `0..15/16` height) takes
//! the `ArrayVoxelShape` branch (`cube = 0`), while a half slab (`0..1/2`) is a
//! two-cell `CubeVoxelShape` (`cube = 1`, `cubeAxes = 7`).
//!
//! `Shapes.or`/`Shapes.and`/`Shapes.join` are `joinUnoptimized` followed by
//! `optimize()`; `optimize` rebuilds the shape from its merged boxes, so a union
//! ends up with the coordinate boundaries of those boxes (an `ArrayVoxelShape`
//! unless the result is a single created box or the full block). This module
//! implements that reconstruction: the union's occupancy is computed on the
//! common boundary grid, then normalised back through the box decomposition.
//!
//! # API
//!
//! `VoxelShape` is opaque; `box_` is `Shapes.box` (a Rust keyword forced the
//! trailing underscore), `column` is `Shapes.column`, `block_box` and
//! `block_column` take the `Block.*` helpers' 1/16 units. `encode()` returns the
//! exact dictionary fields (`EncodedShape`) that `geometry::Shape` carries.
//!
//! The module is standalone (std only) so other tasks can
//! `#[path = "voxelshape.rs"] mod voxelshape;`.

#![allow(dead_code)]

/// The three shape axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
}

/// The boolean ops vanilla passes to `Shapes.join` (`BooleanOp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    /// `BooleanOp.OR`.
    Or,
    /// `BooleanOp.AND`.
    And,
}

/// The encoded fields of a shape dictionary entry.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodedShape {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub z: Vec<f64>,
    pub canonical: u8,
    pub cube: u8,
    pub cube_axes: u8,
    pub discrete_min: [i32; 3],
    pub discrete_max: [i32; 3],
    pub occupied: Vec<u8>,
}

/// A `VoxelShape`: boundary coordinates plus a discrete occupancy grid.
#[derive(Debug, Clone, PartialEq)]
pub struct VoxelShape {
    coords: [Vec<f64>; 3],
    /// `occupied[(x * ny + y) * nz + z]`, over `sizes = len(coords)-1`.
    occupied: Vec<bool>,
    kind: Kind,
    /// Per-axis getCoords(axis) instanceof CubePointRange.
    ranges: [bool; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Empty,
    Block,
    Cube,
    Array,
}

const EPSILON: f64 = 1e-7;

impl VoxelShape {
    /// `Shapes.empty()`.
    pub fn empty() -> VoxelShape {
        VoxelShape {
            coords: [vec![0.0], vec![0.0], vec![0.0]],
            occupied: Vec::new(),
            kind: Kind::Empty,
            ranges: [false; 3],
        }
    }

    /// `Shapes.block()`.
    pub fn block() -> VoxelShape {
        VoxelShape {
            coords: [vec![0.0, 1.0], vec![0.0, 1.0], vec![0.0, 1.0]],
            occupied: vec![true],
            kind: Kind::Block,
            ranges: [true; 3],
        }
    }

    /// `Shapes.box(x1,y1,z1,x2,y2,z2)` (`Shapes.create`).
    pub fn box_(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> VoxelShape {
        // `Shapes.box` validates `min <= max` and `Shapes.create` rejects
        // degenerate axes with the empty shape.
        if x2 - x1 < EPSILON || y2 - y1 < EPSILON || z2 - z1 < EPSILON {
            return VoxelShape::empty();
        }
        let bits = [find_bits(x1, x2), find_bits(y1, y2), find_bits(z1, z2)];
        if bits.iter().any(|bit| bit.is_none()) {
            return VoxelShape {
                coords: [vec![x1, x2], vec![y1, y2], vec![z1, z2]],
                occupied: vec![true],
                kind: Kind::Array,
                ranges: [false; 3],
            };
        }
        let bits: [u32; 3] = [bits[0].unwrap(), bits[1].unwrap(), bits[2].unwrap()];
        if bits == [0, 0, 0] {
            return VoxelShape::block();
        }
        let sizes = [1usize << bits[0], 1usize << bits[1], 1usize << bits[2]];
        let coords = [
            grid_coords(sizes[0]),
            grid_coords(sizes[1]),
            grid_coords(sizes[2]),
        ];
        let min = [
            (x1 * sizes[0] as f64).round() as i64,
            (y1 * sizes[1] as f64).round() as i64,
            (z1 * sizes[2] as f64).round() as i64,
        ];
        let max = [
            (x2 * sizes[0] as f64).round() as i64,
            (y2 * sizes[1] as f64).round() as i64,
            (z2 * sizes[2] as f64).round() as i64,
        ];
        let mut shape = VoxelShape {
            coords,
            occupied: vec![false; sizes[0] * sizes[1] * sizes[2]],
            kind: Kind::Cube,
            ranges: [true; 3],
        };
        for x in min[0]..max[0] {
            for y in min[1]..max[1] {
                for z in min[2]..max[2] {
                    shape.set(x as usize, y as usize, z as usize, true);
                }
            }
        }
        shape
    }

    /// `Block.box(x1,y1,z1,x2,y2,z2)`: the same box in 1/16 units.
    pub fn block_box(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> VoxelShape {
        VoxelShape::box_(
            x1 / 16.0,
            y1 / 16.0,
            z1 / 16.0,
            x2 / 16.0,
            y2 / 16.0,
            z2 / 16.0,
        )
    }

    /// `Shapes.column(radius, height)`: a centred column.
    pub fn column(radius: f64, height: f64) -> VoxelShape {
        VoxelShape::box_(
            0.5 - radius,
            0.0,
            0.5 - radius,
            0.5 + radius,
            height,
            0.5 + radius,
        )
    }

    /// `Block.column(radius, height)` in 1/16 units.
    pub fn block_column(radius: f64, height: f64) -> VoxelShape {
        VoxelShape::column(radius / 16.0, height / 16.0)
    }

    /// The union of the boxes (`Shapes.or` over the list).
    pub fn from_boxes(boxes: &[[f64; 6]]) -> VoxelShape {
        let mut shape = VoxelShape::empty();
        for b in boxes {
            shape = shape.or(VoxelShape::box_(b[0], b[1], b[2], b[3], b[4], b[5]));
        }
        shape
    }

    /// `Shapes.or`.
    pub fn or(self, other: VoxelShape) -> VoxelShape {
        self.join(other, BooleanOp::Or)
    }

    /// `Shapes.and`.
    pub fn and(self, other: VoxelShape) -> VoxelShape {
        self.join(other, BooleanOp::And)
    }

    /// `Shapes.joinUnoptimized`: merge both coordinate lists, then sample the
    /// boolean op on the merged grid.
    fn join_unoptimized(a: &VoxelShape, b: &VoxelShape, op: BooleanOp) -> VoxelShape {
        if a.is_empty() {
            return match op {
                BooleanOp::Or => b.clone(),
                BooleanOp::And => VoxelShape::empty(),
            };
        }
        if b.is_empty() {
            return match op {
                BooleanOp::Or => a.clone(),
                BooleanOp::And => VoxelShape::empty(),
            };
        }
        // `createIndexMerger`: the sorted, de-duplicated merge of the operands'
        // coordinate lists (a CubeVoxelShape keeps its full-range grid).
        let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for axis in 0..3 {
            let mut values = a.coords[axis].clone();
            values.extend(b.coords[axis].iter().copied());
            values.sort_by(|left, right| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            });
            values.dedup_by(|left, right| (*left - *right).abs() < EPSILON);
            coords[axis] = values;
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
                    let left = a.contains(point);
                    let right = b.contains(point);
                    occupied[(x * sizes[1] + y) * sizes[2] + z] = match op {
                        BooleanOp::Or => left || right,
                        BooleanOp::And => left && right,
                    };
                }
            }
        }
        VoxelShape {
            coords,
            occupied,
            kind: Kind::Array,
            ranges: [0, 1, 2].map(|axis| {
                // `createIndexMerger` has four outcomes; two matter for the
                // `CubePointRange` bit:
                //  * both lists are ranges -> `DiscreteCubeMerger` (new range);
                //  * equal lists -> `IdenticalMerger(a)`, which returns `a`'s
                //    list, so the LEFT operand's bit wins (an array list equal to
                //    a range keeps the range);
                //  * otherwise `IndirectMerger`/`NonOverlappingMerger` -> array.
                if a.ranges[axis] && b.ranges[axis] {
                    true
                } else if a.coords[axis].len() == b.coords[axis].len()
                    && a.coords[axis] == b.coords[axis]
                {
                    a.ranges[axis]
                } else {
                    false
                }
            }),
        }
    }

    /// `Shapes.join` = `joinUnoptimized` followed by `optimize()`.
    pub fn join(self, other: VoxelShape, op: BooleanOp) -> VoxelShape {
        VoxelShape::join_unoptimized(&self, &other, op).optimize()
    }

    /// `VoxelShape.optimize()`: rebuild from the merged boxes, re-joining each
    /// through `Shapes.box` (which keeps a recreated box's full cube grid).
    fn optimize(self) -> VoxelShape {
        if self.is_empty() {
            return VoxelShape::empty();
        }
        let mut result = VoxelShape::empty();
        for b in self.boxes() {
            let boxed = VoxelShape::box_(b[0], b[1], b[2], b[3], b[4], b[5]);
            result = VoxelShape::join_unoptimized(&result, &boxed, BooleanOp::Or);
        }
        if result.is_empty() {
            return VoxelShape::empty();
        }
        if result.is_full_block() {
            return VoxelShape::block();
        }
        // A shape whose every axis is a uniform 0..1 grid is a CubeVoxelShape in
        // vanilla (a join of aligned cube ranges keeps that class).
        let uniform = result.coords.iter().all(|coords| {
            coords.len() >= 2 && coords[0] == 0.0 && coords[coords.len() - 1] == 1.0 && {
                let size = coords.len() - 1;
                coords
                    .iter()
                    .enumerate()
                    .all(|(index, value)| (value - index as f64 / size as f64).abs() < EPSILON)
            }
        });
        if uniform {
            result.kind = Kind::Cube;
            result.ranges = [true; 3];
        }
        result
    }
    /// The maximal merged box list (`BitSetDiscreteVoxelShape.forAllBoxes`).
    ///
    /// Vanilla scans y outer, x middle, z inner; when a full z strip starts it
    /// extends it along x, then along y as an x×z rectangle, clears the cells and
    /// emits the box. Replacing this scan with a different greedy order changes
    /// the box extents (and therefore `optimize`'s recreated cube grids).
    fn boxes(&self) -> Vec<[f64; 6]> {
        let sizes = self.cells();
        if sizes[0] == 0 || sizes[1] == 0 || sizes[2] == 0 {
            return Vec::new();
        }
        let mut copy = self.occupied.clone();
        let mut out: Vec<[usize; 6]> = Vec::new();
        for y in 0..=sizes[1] {
            for x in 0..=sizes[0] {
                let mut z_start: Option<usize> = None;
                for z in 0..=sizes[2] {
                    let full = x < sizes[0] && y < sizes[1] && z < sizes[2] && {
                        copy[(x * sizes[1] + y) * sizes[2] + z]
                    };
                    if full {
                        if z_start.is_none() {
                            z_start = Some(z);
                        }
                    } else if let Some(start) = z_start {
                        let mut x_end = x;
                        let mut y_end = y;
                        clear_strip(&mut copy, &sizes, start, z, x_end, y_end);
                        while is_strip_full(&copy, &sizes, start, z, x_end + 1, y_end) {
                            x_end += 1;
                            clear_strip(&mut copy, &sizes, start, z, x_end, y_end);
                        }
                        loop {
                            let grows = (x..=x_end)
                                .all(|xx| is_strip_full(&copy, &sizes, start, z, xx, y_end + 1));
                            if !grows {
                                break;
                            }
                            for xx in x..=x_end {
                                clear_strip(&mut copy, &sizes, start, z, xx, y_end + 1);
                            }
                            y_end += 1;
                        }
                        out.push([x, y, start, x_end + 1, y_end + 1, z]);
                        z_start = None;
                    }
                }
            }
        }
        out.into_iter()
            .map(|b| {
                [
                    self.coords[0][b[0]],
                    self.coords[1][b[1]],
                    self.coords[2][b[2]],
                    self.coords[0][b[3]],
                    self.coords[1][b[4]],
                    self.coords[2][b[5]],
                ]
            })
            .collect()
    }
    fn cells(&self) -> [usize; 3] {
        [
            self.coords[0].len().saturating_sub(1),
            self.coords[1].len().saturating_sub(1),
            self.coords[2].len().saturating_sub(1),
        ]
    }

    fn set(&mut self, x: usize, y: usize, z: usize, value: bool) {
        let sizes = self.cells();
        if x < sizes[0] && y < sizes[1] && z < sizes[2] {
            self.occupied[(x * sizes[1] + y) * sizes[2] + z] = value;
        }
    }

    /// Whether a point is inside the shape.
    pub fn contains(&self, point: [f64; 3]) -> bool {
        let sizes = self.cells();
        if sizes[0] == 0 || sizes[1] == 0 || sizes[2] == 0 {
            return false;
        }
        let index = |axis: usize| -> Option<usize> {
            let coords = &self.coords[axis];
            if point[axis] < coords[0] || point[axis] > coords[coords.len() - 1] {
                return None;
            }
            (0..coords.len() - 1)
                .find(|cell| point[axis] >= coords[*cell] && point[axis] < coords[cell + 1])
        };
        let (Some(x), Some(y), Some(z)) = (index(0), index(1), index(2)) else {
            return false;
        };
        self.occupied[(x * sizes[1] + y) * sizes[2] + z]
    }

    /// `VoxelShape.isEmpty()`.
    pub fn is_empty(&self) -> bool {
        self.kind == Kind::Empty || !self.occupied.iter().any(|value| *value)
    }

    /// `VoxelShape.isFullBlock()` / `Shapes.block()` geometry.
    pub fn is_full_block(&self) -> bool {
        self.kind == Kind::Block
            || (self.coords[0] == [0.0, 1.0]
                && self.coords[1] == [0.0, 1.0]
                && self.coords[2] == [0.0, 1.0]
                && self.occupied == vec![true])
    }

    /// The per-axis boundary coordinates (`getCoords(axis)`).
    pub fn coords(&self, axis: Axis) -> &[f64] {
        &self.coords[axis.index()]
    }

    /// The discrete grid sizes.
    pub fn sizes(&self) -> [usize; 3] {
        self.cells()
    }

    /// Whether a discrete cell is full.
    pub fn occupied(&self, x: usize, y: usize, z: usize) -> bool {
        let sizes = self.cells();
        if x >= sizes[0] || y >= sizes[1] || z >= sizes[2] {
            return false;
        }
        self.occupied[(x * sizes[1] + y) * sizes[2] + z]
    }

    /// `DiscreteVoxelShape.firstFull(axis)`.
    pub fn discrete_min(&self) -> [i32; 3] {
        let sizes = self.cells();
        let mut out = [0i32; 3];
        for axis in 0..3 {
            let mut found = None;
            for cell in 0..sizes[axis] {
                if (0..sizes[(axis + 1) % 3]).any(|b| {
                    (0..sizes[(axis + 2) % 3]).any(|c| {
                        let mut slot = [0usize; 3];
                        slot[axis] = cell;
                        slot[(axis + 1) % 3] = b;
                        slot[(axis + 2) % 3] = c;
                        self.occupied(slot[0], slot[1], slot[2])
                    })
                }) {
                    found = Some(cell as i32);
                    break;
                }
            }
            out[axis] = found.unwrap_or(0);
        }
        out
    }

    /// `DiscreteVoxelShape.lastFull(axis)`: one past the last full cell.
    pub fn discrete_max(&self) -> [i32; 3] {
        let sizes = self.cells();
        let mut out = [0i32; 3];
        for axis in 0..3 {
            let mut found = None;
            for cell in (0..sizes[axis]).rev() {
                let hit = (0..sizes[(axis + 1) % 3]).any(|b| {
                    (0..sizes[(axis + 2) % 3]).any(|c| {
                        let mut slot = [0usize; 3];
                        slot[axis] = cell;
                        slot[(axis + 1) % 3] = b;
                        slot[(axis + 2) % 3] = c;
                        self.occupied(slot[0], slot[1], slot[2])
                    })
                });
                if hit {
                    found = Some(cell as i32 + 1);
                    break;
                }
            }
            out[axis] = found.unwrap_or(0);
        }
        out
    }

    /// `canonical`: 1 for `Shapes.empty()`, 2 for `Shapes.block()`, else 0.
    pub fn canonical(&self) -> u8 {
        match self.kind {
            Kind::Empty => 1,
            Kind::Block => 2,
            _ => 0,
        }
    }

    /// `shape instanceof CubeVoxelShape`.
    pub fn cube(&self) -> bool {
        matches!(self.kind, Kind::Cube | Kind::Block)
    }

    /// Per-axis `getCoords(axis) instanceof CubePointRange` bits.
    pub fn cube_axes(&self) -> u8 {
        // `getCoords(axis) instanceof CubePointRange`: CubeVoxelShape's coords are
        // CubePointRange on every axis; an ArrayVoxelShape keeps the exact
        // DoubleList it was built with, which for the vanilla factory paths is an
        // array (not a range). Rare hand-built arrays can differ; that is a
        // construction-history fact the geometric model cannot recover.
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

    /// The exact dictionary encoding of this shape.
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
        let sizes = self.cells();
        let mut occupied = Vec::with_capacity(sizes[0] * sizes[1] * sizes[2]);
        for x in 0..sizes[0] {
            for y in 0..sizes[1] {
                for z in 0..sizes[2] {
                    occupied.push(self.occupied(x, y, z) as u8);
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

/// `isZStripFull`: every cell of the strip at `(x,y)` is full.
fn is_strip_full(
    copy: &[bool],
    sizes: &[usize; 3],
    z1: usize,
    z2: usize,
    x: usize,
    y: usize,
) -> bool {
    x < sizes[0] && y < sizes[1] && (z1..z2).all(|z| copy[(x * sizes[1] + y) * sizes[2] + z])
}

/// `clearZStrip`.
fn clear_strip(copy: &mut [bool], sizes: &[usize; 3], z1: usize, z2: usize, x: usize, y: usize) {
    for z in z1..z2 {
        copy[(x * sizes[1] + y) * sizes[2] + z] = false;
    }
}

/// Sorted, de-duplicated box boundaries of two shapes (`optimize`'s rebuild).
fn boundary_coords(a: &VoxelShape, b: &VoxelShape) -> [Vec<f64>; 3] {
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for shape in [a, b] {
        for b in shape.boxes() {
            for axis in 0..3 {
                coords[axis].push(b[axis]);
                coords[axis].push(b[axis + 3]);
            }
        }
        if shape.boxes().is_empty() {
            for (axis, values) in shape.coords.iter().enumerate() {
                coords[axis].extend(values.iter().copied());
            }
        }
    }
    for axis in coords.iter_mut() {
        axis.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        axis.dedup_by(|left, right| (*left - *right).abs() < EPSILON);
        if axis.is_empty() {
            *axis = vec![0.0, 1.0];
        }
    }
    coords
}

/// `findBits`: the smallest `k` in 0..=3 with both scaled values integral.
fn find_bits(min: f64, max: f64) -> Option<u32> {
    if min < -EPSILON || max > 1.0 + EPSILON {
        return None;
    }
    for k in 0..=3u32 {
        let scale = (1u32 << k) as f64;
        let a = min * scale;
        let b = max * scale;
        if (a - a.round()).abs() < EPSILON * scale && (b - b.round()).abs() < EPSILON * scale {
            return Some(k);
        }
    }
    None
}

/// The `CubePointRange` coordinates `0, 1/size, ..., 1`.
fn grid_coords(size: usize) -> Vec<f64> {
    (0..=size).map(|i| i as f64 / size as f64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_shape_is_one_zero_per_axis() {
        let encoded = VoxelShape::empty().encode();
        assert_eq!(
            (encoded.x, encoded.y, encoded.z),
            (vec![0.0], vec![0.0], vec![0.0])
        );
        assert_eq!(encoded.canonical, 1);
        assert_eq!(encoded.occupied, Vec::<u8>::new());
        assert!(VoxelShape::empty().is_empty());
    }

    #[test]
    fn the_block_shape_is_the_full_cube() {
        let encoded = VoxelShape::block().encode();
        assert_eq!(encoded.canonical, 2);
        assert_eq!((encoded.cube, encoded.cube_axes), (1, 7));
        assert_eq!(encoded.occupied, vec![1]);
        assert!(VoxelShape::block().is_full_block());
        // `Shapes.box(0,0,0,1,1,1)` is the block singleton.
        assert_eq!(
            VoxelShape::box_(0.0, 0.0, 0.0, 1.0, 1.0, 1.0),
            VoxelShape::block()
        );
    }

    #[test]
    fn a_half_slab_is_a_two_cell_cube_shape() {
        let encoded = VoxelShape::block_box(0.0, 0.0, 0.0, 16.0, 8.0, 16.0).encode();
        assert_eq!(encoded.canonical, 0);
        assert_eq!((encoded.cube, encoded.cube_axes), (1, 7));
        assert_eq!(encoded.x, vec![0.0, 1.0]);
        assert_eq!(encoded.y, vec![0.0, 0.5, 1.0]);
        assert_eq!(encoded.z, vec![0.0, 1.0]);
        assert_eq!(encoded.occupied, vec![1, 0], "bottom slab");
        assert_eq!(encoded.discrete_min, [0, 0, 0]);
        assert_eq!(encoded.discrete_max, [1, 1, 1]);
    }

    #[test]
    fn a_one_sixteenth_box_is_an_array_shape() {
        let encoded = VoxelShape::block_box(0.0, 0.0, 0.0, 16.0, 15.0, 16.0).encode();
        assert_eq!(encoded.canonical, 0);
        assert_eq!((encoded.cube, encoded.cube_axes), (0, 0));
        assert_eq!(encoded.y, vec![0.0, 0.9375]);
        assert_eq!(encoded.occupied, vec![1]);
        assert_eq!(encoded.discrete_max, [1, 1, 1]);
    }

    #[test]
    fn a_degenerate_box_is_empty() {
        assert!(VoxelShape::box_(0.0, 0.0, 0.0, 0.0, 1.0, 1.0).is_empty());
    }

    #[test]
    fn or_of_two_halves_is_the_full_block() {
        let a = VoxelShape::block_box(0.0, 0.0, 0.0, 16.0, 8.0, 16.0);
        let b = VoxelShape::block_box(0.0, 8.0, 0.0, 16.0, 16.0, 16.0);
        assert_eq!(a.or(b), VoxelShape::block());
    }

    #[test]
    fn and_of_two_boxes_is_their_overlap() {
        let a = VoxelShape::box_(0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
        let b = VoxelShape::box_(0.0, 0.0, 0.0, 0.5, 0.5, 1.0);
        let encoded = a.and(b).encode();
        assert_eq!(encoded.x, vec![0.0, 0.5, 1.0]);
        assert_eq!(encoded.y, vec![0.0, 0.5, 1.0]);
        assert_eq!(encoded.occupied, vec![1, 0, 0, 0]);
    }

    #[test]
    fn a_union_keeps_the_merged_box_boundaries() {
        let a = VoxelShape::block_box(0.0, 0.0, 0.0, 8.0, 16.0, 16.0);
        let b = VoxelShape::block_box(8.0, 0.0, 0.0, 16.0, 16.0, 16.0);
        assert_eq!(a.or(b).encode().x, vec![0.0, 1.0]);
    }
}
