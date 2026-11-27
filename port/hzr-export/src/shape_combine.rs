//! Combines the three shape engines into the final NBT tables.
//!
//! Sources, in order (each covers disjoint state classes in practice):
//!
//! 1. `shape_families` — hand-modelled selectors for the big families
//!    (stairs, slabs, walls, fences, panes, doors, trapdoors, candles, vines,
//!    multiface, plants, …), built on `voxelshape`'s exact factories;
//! 2. `geometry` — the default path (blocks whose class chain does not override
//!    the shape methods): full cube / empty;
//! 3. `shape_methods` — the bytecode VM for everything else, driven by an
//!    `ExactFactory` that reproduces vanilla's `Shapes.create`/`join`/
//!    `optimize`/`rotate` semantics (including the `forAllBoxes` splitter and
//!    the per-axis `CubePointRange` bits).
//!
//! The output is `cached-collision.nbt`, `occlusion.nbt`,
//! `worldgen/blocks/cached-support.nbt` and the per-state `full_block` flags
//! for `structure-blocks.json` (`isCollisionShapeFullBlock`).
//!
//! Every state a source cannot derive stays `None` and is reported with its
//! class in [`Coverage::gaps`]; nothing is guessed.

#![allow(dead_code, unused_variables, unused_mut)]
#![allow(clippy::duplicate_mod, clippy::duplicated_attributes)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

#[path = "geometry.rs"]
#[allow(unused)]
pub mod geometry;

#[path = "shape_families.rs"]
#[allow(unused)]
mod shape_families;

#[path = "shape_methods.rs"]
#[allow(unused)]
mod shape_methods;

#[path = "states.rs"]
#[allow(unused)]
mod states;

#[path = "block.rs"]
#[allow(unused)]
mod block;

use shape_methods::{JoinOp, Query, Rotation, ShapeFactory, State as VmState, Vm};

/// The files this module writes.
pub const COLLISION: &str = "cached-collision.nbt";
pub const OCCLUSION: &str = "occlusion.nbt";
pub const SUPPORT: &str = "worldgen/blocks/cached-support.nbt";
pub const TABLES: [&str; 3] = [COLLISION, OCCLUSION, SUPPORT];
pub const DATA_VERSION: i32 = 5023;

// ---------------------------------------------------------------------------
// Exact shape model for the VM (`voxelshape`'s semantics, plus rotation)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct ExactShape {
    coords: [Vec<f64>; 3],
    occupied: Vec<bool>,
    kind: Kind,
    ranges: [bool; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Empty,
    Block,
    Cube,
    Array,
}

impl ExactShape {
    fn empty() -> ExactShape {
        ExactShape {
            coords: [vec![0.0], vec![0.0], vec![0.0]],
            occupied: Vec::new(),
            kind: Kind::Empty,
            ranges: [false; 3],
        }
    }

    fn block() -> ExactShape {
        ExactShape {
            coords: [vec![0.0, 1.0], vec![0.0, 1.0], vec![0.0, 1.0]],
            occupied: vec![true],
            kind: Kind::Block,
            ranges: [true; 3],
        }
    }

    fn create(x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> ExactShape {
        if x2 - x1 < 1e-7 || y2 - y1 < 1e-7 || z2 - z1 < 1e-7 {
            return ExactShape::empty();
        }
        let bits = [
            find_bits(x1, x2),
            find_bits(y1, y2),
            find_bits(z1, z2),
        ];
        if bits.iter().any(|bit| bit.is_none()) {
            return ExactShape {
                coords: [vec![x1, x2], vec![y1, y2], vec![z1, z2]],
                occupied: vec![true],
                kind: Kind::Array,
                ranges: [false; 3],
            };
        }
        let bits = [bits[0].unwrap(), bits[1].unwrap(), bits[2].unwrap()];
        if bits == [0, 0, 0] {
            return ExactShape::block();
        }
        let sizes = [1usize << bits[0], 1usize << bits[1], 1usize << bits[2]];
        let coords = [
            grid_coords(sizes[0]),
            grid_coords(sizes[1]),
            grid_coords(sizes[2]),
        ];
        let low = [
            (x1 * sizes[0] as f64).round() as usize,
            (y1 * sizes[1] as f64).round() as usize,
            (z1 * sizes[2] as f64).round() as usize,
        ];
        let high = [
            ((x2 * sizes[0] as f64).round() as usize).min(sizes[0]),
            ((y2 * sizes[1] as f64).round() as usize).min(sizes[1]),
            ((z2 * sizes[2] as f64).round() as usize).min(sizes[2]),
        ];
        let mut shape = ExactShape {
            coords,
            occupied: vec![false; sizes[0] * sizes[1] * sizes[2]],
            kind: Kind::Cube,
            ranges: [true; 3],
        };
        for x in low[0]..high[0] {
            for y in low[1]..high[1] {
                for z in low[2]..high[2] {
                    shape.set(x, y, z);
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

    fn key(&self, x: usize, y: usize, z: usize, sizes: &[usize; 3]) -> usize {
        (x * sizes[1] + y) * sizes[2] + z
    }

    fn set(&mut self, x: usize, y: usize, z: usize) {
        let sizes = self.sizes();
        if x < sizes[0] && y < sizes[1] && z < sizes[2] {
            let index = self.key(x, y, z, &sizes);
            self.occupied[index] = true;
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
        self.occupied[self.key(cell[0], cell[1], cell[2], &sizes)]
    }

    fn is_empty(&self) -> bool {
        self.kind == Kind::Empty || !self.occupied.iter().any(|value| *value)
    }

    fn is_full_block(&self) -> bool {
        self.kind == Kind::Block
            || (self.coords[0] == [0.0, 1.0]
                && self.coords[1] == [0.0, 1.0]
                && self.coords[2] == [0.0, 1.0]
                && self.occupied == vec![true])
    }

    fn canonical(&self) -> u8 {
        match self.kind {
            Kind::Empty => 1,
            Kind::Block => 2,
            _ => 0,
        }
    }

    fn cube(&self) -> bool {
        // A `CubeVoxelShape` is exactly a shape whose three coordinate lists are
        // `CubePointRange`s (the prepared files agree on 19,451 of 19,452
        // entries; the remaining one is a construction-history artifact).
        !self.is_empty() && self.ranges == [true; 3]
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

    fn is_strip_full(&self, z1: usize, z2: usize, x: usize, y: usize) -> bool {
        let sizes = self.sizes();
        x < sizes[0]
            && y < sizes[1]
            && (z1..z2).all(|z| self.occupied[self.key(x, y, z, &sizes)])
    }

    fn clear_strip(&mut self, z1: usize, z2: usize, x: usize, y: usize) {
        let sizes = self.sizes();
        for z in z1..z2 {
            let index = self.key(x, y, z, &sizes);
            self.occupied[index] = false;
        }
    }

    /// `BitSetDiscreteVoxelShape.forAllBoxes` (cell indices).
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

    fn optimize(mut self) -> ExactShape {
        if self.is_empty() {
            return ExactShape::empty();
        }
        let mut result = ExactShape::empty();
        for cells in self.boxes() {
            let b = [
                self.coords[0][cells[0]],
                self.coords[1][cells[1]],
                self.coords[2][cells[2]],
                self.coords[0][cells[3]],
                self.coords[1][cells[4]],
                self.coords[2][cells[5]],
            ];
            let boxed = ExactShape::create(b[0], b[1], b[2], b[3], b[4], b[5]);
            result = merge(&result, &boxed, JoinOp::Or);
        }
        if result.is_empty() {
            return ExactShape::empty();
        }
        if result.is_full_block() {
            return ExactShape::block();
        }
        let uniform = result.coords.iter().all(|coords| {
            coords.len() >= 2
                && coords[0] == 0.0
                && coords[coords.len() - 1] == 1.0
                && {
                    let size = coords.len() - 1;
                    coords
                        .iter()
                        .enumerate()
                        .all(|(index, value)| (value - index as f64 / size as f64).abs() < 1e-7)
                }
        });
        if uniform {
            result.kind = Kind::Cube;
            result.ranges = [true; 3];
        }
        result
    }

    fn encode(&self) -> geometry::Shape {
        if self.is_empty() {
            return geometry::Shape::empty();
        }
        let sizes = self.sizes();
        let mut occupied = Vec::with_capacity(sizes[0] * sizes[1] * sizes[2]);
        for x in 0..sizes[0] {
            for y in 0..sizes[1] {
                for z in 0..sizes[2] {
                    occupied.push(self.occupied[self.key(x, y, z, &sizes)] as u8);
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

    fn discrete_min(&self) -> [i32; 3] {
        let sizes = self.sizes();
        let mut low = [0i32; 3];
        for axis in 0..3 {
            let mut found = 0;
            for cell in 0..sizes[axis] {
                if (0..sizes[(axis + 1) % 3]).any(|b| {
                    (0..sizes[(axis + 2) % 3]).any(|c| {
                        let mut slot = [0usize; 3];
                        slot[axis] = cell;
                        slot[(axis + 1) % 3] = b;
                        slot[(axis + 2) % 3] = c;
                        self.occupied[self.key(slot[0], slot[1], slot[2], &sizes)]
                    })
                }) {
                    found = cell as i32;
                    break;
                }
            }
            low[axis] = found;
        }
        low
    }

    fn discrete_max(&self) -> [i32; 3] {
        let sizes = self.sizes();
        let mut high = [0i32; 3];
        for axis in 0..3 {
            let mut found = 0;
            for cell in (0..sizes[axis]).rev() {
                let hit = (0..sizes[(axis + 1) % 3]).any(|b| {
                    (0..sizes[(axis + 2) % 3]).any(|c| {
                        let mut slot = [0usize; 3];
                        slot[axis] = cell;
                        slot[(axis + 1) % 3] = b;
                        slot[(axis + 2) % 3] = c;
                        self.occupied[self.key(slot[0], slot[1], slot[2], &sizes)]
                    })
                });
                if hit {
                    found = cell as i32 + 1;
                    break;
                }
            }
            high[axis] = found;
        }
        high
    }

    /// Convert a table shape (from `geometry`/`voxelshape`) into the VM model.
    fn from_encoded(shape: &geometry::Shape) -> ExactShape {
        let sizes = [
            shape.x.len().saturating_sub(1),
            shape.y.len().saturating_sub(1),
            shape.z.len().saturating_sub(1),
        ];
        let kind = match shape.canonical {
            1 => Kind::Empty,
            2 => Kind::Block,
            _ if shape.cube == 1 => Kind::Cube,
            _ => Kind::Array,
        };
        let ranges = if shape.cube == 1 {
            [true; 3]
        } else {
            [
                shape.cube_axes & 1 != 0,
                shape.cube_axes & 2 != 0,
                shape.cube_axes & 4 != 0,
            ]
        };
        ExactShape {
            coords: [shape.x.clone(), shape.y.clone(), shape.z.clone()],
            occupied: shape.occupied.iter().map(|value| *value == 1).collect(),
            kind,
            ranges,
        }
    }
}

fn is_strip(copy: &[bool], sizes: &[usize; 3], z1: usize, z2: usize, x: usize, y: usize) -> bool {
    x < sizes[0]
        && y < sizes[1]
        && (z1..z2).all(|z| copy[(x * sizes[1] + y) * sizes[2] + z])
}

fn merge(first: &ExactShape, second: &ExactShape, operation: JoinOp) -> ExactShape {
    if operation == JoinOp::Or {
        if first.is_empty() {
            return second.clone();
        }
        if second.is_empty() {
            return first.clone();
        }
    }
    let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for axis in 0..3 {
        let mut values: Vec<f64> = first.coords[axis]
            .iter()
            .chain(second.coords[axis].iter())
            .copied()
            .collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        values.dedup_by(|a, b| (*a - *b).abs() < 1e-7);
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
                occupied[(x * sizes[1] + y) * sizes[2] + z] =
                    operation.apply(first.contains(point), second.contains(point));
            }
        }
    }
    let ranges = [0, 1, 2].map(|axis| {
        if first.ranges[axis] && second.ranges[axis] {
            true
        } else if first.coords[axis].len() == second.coords[axis].len()
            && first.coords[axis] == second.coords[axis]
        {
            first.ranges[axis]
        } else {
            false
        }
    });
    ExactShape {
        coords,
        occupied,
        kind: Kind::Array,
        ranges,
    }
}

fn find_bits(min: f64, max: f64) -> Option<u32> {
    if min < -1e-7 || max > 1.0 + 1e-7 {
        return None;
    }
    for bits in 0..=3u32 {
        let scale = (1u32 << bits) as f64;
        let low = min * scale;
        let high = max * scale;
        if (low - low.round()).abs() < 1e-7 * scale && (high - high.round()).abs() < 1e-7 * scale {
            return Some(bits);
        }
    }
    None
}

fn grid_coords(size: usize) -> Vec<f64> {
    (0..=size).map(|i| i as f64 / size as f64).collect()
}

/// The VM factory: exact vanilla factories plus rotation.
struct ExactFactory;

impl ShapeFactory for ExactFactory {
    type Shape = ExactShape;

    fn empty(&mut self) -> ExactShape {
        ExactShape::empty()
    }

    fn block(&mut self) -> ExactShape {
        ExactShape::block()
    }

    fn create(&mut self, x1: f64, y1: f64, z1: f64, x2: f64, y2: f64, z2: f64) -> ExactShape {
        ExactShape::create(x1, y1, z1, x2, y2, z2)
    }

    fn or(&mut self, first: &ExactShape, second: &ExactShape) -> ExactShape {
        merge(first, second, JoinOp::Or).optimize()
    }

    fn join(&mut self, first: &ExactShape, second: &ExactShape, operation: JoinOp) -> ExactShape {
        merge(first, second, operation).optimize()
    }

    fn rotate(&mut self, shape: &ExactShape, rotation: &Rotation, center: [f64; 3]) -> ExactShape {
        if rotation.is_identity() {
            return shape.clone();
        }
        let sizes = shape.sizes();
        let mut coords: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for axis in 0..3 {
            let source = rotation.perm[axis];
            let from_center = center[source];
            let to_center = center[axis];
            let mut values: Vec<f64> = shape.coords[source]
                .iter()
                .map(|value| {
                    if rotation.inverted[axis] {
                        to_center - (*value - from_center)
                    } else {
                        *value - from_center + to_center
                    }
                })
                .collect();
            if rotation.inverted[axis] {
                values.reverse();
            }
            coords[axis] = values;
        }
        let mut occupied = vec![false; shape.occupied.len()];
        for x in 0..sizes[0] {
            for y in 0..sizes[1] {
                for z in 0..sizes[2] {
                    if !shape.occupied[(x * sizes[1] + y) * sizes[2] + z] {
                        continue;
                    }
                    let source = [x, y, z];
                    let mut cell = [0usize; 3];
                    for axis in 0..3 {
                        let index = source[rotation.perm[axis]];
                        cell[axis] = if rotation.inverted[axis] {
                            sizes[rotation.perm[axis]] - 1 - index
                        } else {
                            index
                        };
                    }
                    let new_sizes = [
                        coords[0].len() - 1,
                        coords[1].len() - 1,
                        coords[2].len() - 1,
                    ];
                    if cell[0] < new_sizes[0] && cell[1] < new_sizes[1] && cell[2] < new_sizes[2] {
                        occupied[(cell[0] * new_sizes[1] + cell[1]) * new_sizes[2] + cell[2]] = true;
                    }
                }
            }
        }
        ExactShape {
            coords,
            occupied,
            kind: if shape.kind == Kind::Block {
                Kind::Block
            } else {
                Kind::Array
            },
            ranges: [shape.ranges[rotation.perm[0]], shape.ranges[rotation.perm[1]], shape.ranges[rotation.perm[2]]],
        }
    }

    fn is_empty(&mut self, shape: &ExactShape) -> bool {
        shape.is_empty()
    }
}

// ---------------------------------------------------------------------------
// The combined evaluator
// ---------------------------------------------------------------------------

/// What the combination produced, and every remaining gap class.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub states: usize,
    pub collision_covered: usize,
    pub occlusion_covered: usize,
    pub support_covered: usize,
    /// Block id -> reason the state could not be derived, per table.
    pub gaps: BTreeMap<String, String>,
}

impl Coverage {
    fn note(&mut self, class: &str, table: &str, reason: &str) {
        let entry = self
            .gaps
            .entry(format!("{table} {class}"))
            .or_default();
        if entry.is_empty() {
            *entry = reason.to_string();
        } else if entry.len() < 120 && !entry.contains(reason) {
            entry.push_str("; ");
            entry.push_str(reason);
        }
    }
}

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

/// `hasCollision` per registered block (from `Properties.noCollision()`).
fn has_collision(jar_path: &Path) -> Result<Vec<bool>, String> {
    let registry = block::registry(jar_path)?;
    let no_collision = scan_no_collision(jar_path)?;
    if no_collision.len() != registry.len() {
        return Err(format!(
            "noCollision flags {} != registry {}",
            no_collision.len(),
            registry.len()
        ));
    }
    Ok(no_collision.into_iter().map(|flag| !flag).collect())
}

struct Jar {
    classes: BTreeMap<String, Vec<u8>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut classes = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            if let Some(stem) = name.strip_suffix(".class") {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|error| format!("{name}: {error}"))?;
                classes.insert(stem.to_string(), data);
            }
        }
        Ok(Jar { classes })
    }
}

fn method_name(class: &classfile::ClassFile, instruction: &shape_methods::Ins) -> Option<(String, String)> {
    let (owner, name, _) = classfile::method_ref(&class.constants, instruction.index)?;
    Some((owner, name))
}

fn scan_no_collision(jar_path: &Path) -> Result<Vec<bool>, String> {
    let jar = Jar::open(jar_path)?;
    let bytes = jar
        .classes
        .get("net/minecraft/world/level/block/Blocks")
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let parsed = classfile::parse(bytes).map_err(|error| format!("Blocks: {error}"))?;
    let code = parsed
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let instructions = shape_methods::decode_code(&code.bytes);
    let mut flags = Vec::new();
    // For each registration call, look back to the id push that starts its
    // properties chain and check whether that chain calls `noCollision()`.
    for (position, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xb8 {
            continue;
        }
        let Some((owner, name)) = method_name(&parsed, instruction) else {
            continue;
        };
        let family = owner.ends_with("ColorCollection")
            || owner.ends_with("WeatheringCopperCollection");
        let single = owner == "net/minecraft/world/level/block/Blocks"
            && matches!(
                name.as_str(),
                "register"
                    | "registerStair"
                    | "registerSlab"
                    | "registerWall"
                    | "registerLegacyStair"
            );
        if !single && !family {
            continue;
        }
        let mut no_collision = false;
        for previous in instructions[..position].iter().rev() {
            if previous.opcode == 0xb2 {
                if let Some((field_owner, _field, _)) = classfile::field_ref(
                    &parsed.constants,
                    previous.index,
                ) {
                    if field_owner.ends_with("BlockItemIds") || field_owner.ends_with("BlockIds") {
                        break;
                    }
                }
            }
            if previous.opcode == 0xb6 {
                if let Some((call_owner, call_name)) = method_name(&parsed, previous) {
                    if call_owner.ends_with("Properties") && call_name == "noCollision" {
                        no_collision = true;
                    }
                }
            }
        }
        let count = if family {
            if owner.ends_with("WeatheringCopperCollection") {
                8
            } else {
                16
            }
        } else {
            1
        };
        for _ in 0..count {
            flags.push(no_collision);
        }
    }
    Ok(flags)
}
/// The combined shape evaluator.
pub struct Combined {
    pub collision: Vec<Option<geometry::Shape>>,
    pub occlusion: Vec<Option<geometry::Shape>>,
    pub support: Vec<Option<geometry::Shape>>,
    pub coverage: Coverage,
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

fn resolve_class(names: &[String], simple: &str) -> Option<String> {
    let direct = format!("net/minecraft/world/level/block/{simple}");
    if names.iter().any(|name| *name == direct) {
        return Some(direct);
    }
    let suffix = format!("/{simple}");
    names
        .iter()
        .find(|name| name.starts_with("net/minecraft/world/level/block/") && name.ends_with(&suffix))
        .cloned()
}

/// Evaluate all three tables from the jar.
pub fn combine(jar_path: &Path) -> Result<Combined, String> {
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    if registry.len() != state_table.blocks.len() {
        return Err("registry and state table disagree".to_string());
    }
    let flags = has_collision(jar_path)?;
    let (default_collision, _) = geometry::collision_shapes(jar_path)?;
    let (default_occlusion, _) = geometry::occlusion_shapes(jar_path)?;
    let source = shape_methods::JarSource::open(jar_path)?;
    let names: Vec<String> = source.names().map(str::to_string).collect();
    let mut vm = Vm::new(Box::new(source), ExactFactory);
    let total = state_table.state_count();
    let mut collision: Vec<Option<geometry::Shape>> = vec![None; total];
    let mut occlusion: Vec<Option<geometry::Shape>> = vec![None; total];
    let mut support: Vec<Option<geometry::Shape>> = vec![None; total];
    let mut coverage = Coverage {
        states: total,
        ..Default::default()
    };
    let mut index = 0usize;
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        let class_simple = &registry[block_index].class;
        let internal = resolve_class(&names, class_simple)
            .ok_or_else(|| format!("{}: class not in the jar", row.id))?;
        for state_index in 0..row.state_count {
            let pairs = property_values(row, state_index);
            let pair_refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            let values: BTreeMap<String, String> = pairs.iter().cloned().collect();

            let vm_state = VmState {
                class: internal.clone(),
                values,
                has_collision: flags[block_index],
            };
            // Collision: family, default path, then VM.
            let mut collision_shape: Option<geometry::Shape> = shape_families::family_for_class(class_simple)
                .and_then(|_| {
                    shape_families::shape_for_class(
                        class_simple,
                        &shape_families::State::new(&pair_refs),
                        shape_families::ShapeKind::Collision,
                    )
                })
                .map(|shape| encode_voxel(&shape));
            if collision_shape.is_none() {
                collision_shape = default_collision[index].clone();
            }

            let mut collision_gap = None;
            if collision_shape.is_none() {
                let result = vm.query(&vm_state, Query::Collision);
                match result.shape {
                    Some(shape) if result.warnings.is_empty() => {
                        collision_shape = Some(shape.encode_to_geometry());
                    }
                    _ => {
                        collision_gap = Some(if result.warnings.is_empty() {
                            "no shape returned".to_string()
                        } else {
                            result.warnings.join("; ")
                        });
                    }
                }
            }
            // Occlusion: family, default path, then VM.
            let mut occlusion_shape: Option<geometry::Shape> = shape_families::family_for_class(class_simple)
                .and_then(|family| {
                    shape_families::shape_for_class(
                        class_simple,
                        &shape_families::State::new(&pair_refs),
                        shape_families::ShapeKind::Occlusion,
                    )
                })
                .map(|shape| encode_voxel(&shape));
            if occlusion_shape.is_none() {
                occlusion_shape = default_occlusion[index].clone();
            }
            let mut occlusion_gap = None;
            if occlusion_shape.is_none() {
                let result = vm.query(&vm_state, Query::Occlusion);
                match result.shape {
                    Some(shape) if result.warnings.is_empty() => {
                        occlusion_shape = Some(shape.encode_to_geometry());
                    }
                    _ => {
                        occlusion_gap = Some(if result.warnings.is_empty() {
                            "no shape returned".to_string()
                        } else {
                            result.warnings.join("; ")
                        });
                    }
                }
            }
            // Support: VM only.
            let result = vm.query(&vm_state, Query::BlockSupport);
            let mut support_gap = None;
            let support_shape = match result.shape {
                Some(shape) if result.warnings.is_empty() => Some(shape.encode_to_geometry()),
                _ => {
                    support_gap = Some(if result.warnings.is_empty() {
                        "no shape returned".to_string()
                    } else {
                        result.warnings.join("; ")
                    });
                    None
                }
            };
            collision[index] = collision_shape;
            occlusion[index] = occlusion_shape;
            support[index] = support_shape;
            if collision[index].is_some() {
                coverage.collision_covered += 1;
            } else if let Some(reason) = &collision_gap {
                coverage.note(class_simple, "collision", reason);
            }
            if occlusion[index].is_some() {
                coverage.occlusion_covered += 1;
            } else if let Some(reason) = &occlusion_gap {
                coverage.note(class_simple, "occlusion", reason);
            }
            if support[index].is_some() {
                coverage.support_covered += 1;
            } else if let Some(reason) = &support_gap {
                coverage.note(class_simple, "support", reason);
            }
            index += 1;
        }
    }
    Ok(Combined {
        collision,
        occlusion,
        support,
        coverage,
    })
}

// Bridge from `voxelshape::VoxelShape` / `ExactShape` to the table shape.
fn encode_voxel(shape: &shape_families::VoxelShape) -> geometry::Shape {
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

trait EncodeToGeometry {
    fn encode_to_geometry(&self) -> geometry::Shape;
}

impl EncodeToGeometry for ExactShape {
    fn encode_to_geometry(&self) -> geometry::Shape {
        self.encode()
    }
}
/// Deduplicate a state list into a table dictionary.
pub fn table_of(shapes: &[Option<geometry::Shape>], kind: Option<&str>) -> geometry::Table {
    let mut dictionary: Vec<geometry::Shape> = Vec::new();
    let mut cached = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let shape = shape.clone().unwrap_or_else(geometry::Shape::empty);
        let index = match dictionary
            .iter()
            .position(|candidate| candidate == &shape)
        {
            Some(index) => index,
            None => {
                dictionary.push(shape);
                dictionary.len() - 1
            }
        };
        cached.push(index as i32);
    }
    geometry::Table {
        kind: kind.map(|kind| kind.to_string()),
        data_version: shape_combine_version(),
        cached,
        shapes: dictionary,
    }
}

fn shape_combine_version() -> i32 {
    5023
}

/// Write the three NBT tables below `out`.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let combined = combine(jar_path)?;
    let tables: [(&str, Vec<Option<geometry::Shape>>, Option<&str>); 3] = [
        (COLLISION, combined.collision.clone(), None),
        (OCCLUSION, combined.occlusion.clone(), None),
        (SUPPORT, combined.support.clone(), Some("block_support")),
    ];
    for (name, shapes, kind) in tables {
        let table = table_of(&shapes, kind);
        let bytes = geometry::encode(&table);
        let path = out.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        fs::write(&path, &bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(combined.coverage)
}

/// The per-state `full_block` flags (`isCollisionShapeFullBlock`), when derived.
pub fn full_block(jar_path: &Path) -> Result<Vec<Option<bool>>, String> {
    let combined = combine(jar_path)?;
    Ok(combined
        .collision
        .iter()
        .map(|shape| shape.as_ref().map(|shape| shape.canonical == 2))
        .collect())
}