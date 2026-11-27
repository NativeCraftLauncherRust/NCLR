//! Horizon's block geometry tables, rebuilt from the player's own `client.jar`.
//!
//! Three files in the bundle carry a shape dictionary plus one index per state:
//!
//! ```text
//! cached-collision.nbt         {cached_shapes, shapes, DataVersion}
//! occlusion.nbt                {cached_shapes, shapes, DataVersion}
//! worldgen/blocks/cached-support.nbt
//!                              {cached_shapes, shapes, Kind:"block_support",
//!                               DataVersion}
//! ```
//!
//! Each dictionary entry is a `VoxelShape` encoded from the vanilla runtime:
//!
//! | key | type | meaning |
//! | --- | --- | --- |
//! | `x`,`y`,`z` | long array | `VoxelShape.getCoords(axis)` as raw `double` bits |
//! | `canonical` | byte | 1 = `Shapes.empty()`, 2 = `Shapes.block()`, else 0 |
//! | `cube` | byte | 1 if the shape is a `CubeVoxelShape` |
//! | `cube_axes` | byte | per-axis bit: `getCoords(axis) instanceof CubePointRange` |
//! | `discrete_min`/`discrete_max` | int array | `DiscreteVoxelShape.firstFull/lastFull` |
//! | `occupied` | byte array | the discrete occupancy, x-major then y then z |
//!
//! `cached_shapes` holds one index per state in registry order.
//!
//! # Geometry derivation
//!
//! Vanilla computes these shapes in block code over a live level:
//! `getCollisionShape`/`getShape` for collision, `getFaceOcclusionShape`/
//! `getOcclusionShape` for occlusion, `isFaceSturdy` for support. This module
//! derives the default path from the jar and reports the rest as gaps:
//!
//! * a block whose class chain up to `BlockBehaviour` defines none of the
//!   shape methods gets `Shapes.block()` when its `Properties` has collision
//!   and `Shapes.empty()` when the registration chain calls `noCollision()`;
//! * anything with an override, an unusual shape method or a dynamic shape is
//!   left out and counted (see [`Coverage`]).
//!
//! The `full_block` field of `structure-blocks.json` follows the same rule: a
//! state is full-block exactly when the derived collision shape is
//! `Shapes.block()`.

#![allow(dead_code, unused, clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "block.rs"]
#[allow(unused)]
mod block;

#[path = "states.rs"]
#[allow(unused)]
mod states;

use classfile::ClassFile;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;

/// The files this engine knows.
pub const COLLISION: &str = "cached-collision.nbt";
pub const OCCLUSION: &str = "occlusion.nbt";
pub const SUPPORT: &str = "worldgen/blocks/cached-support.nbt";
pub const TABLES: [&str; 3] = [COLLISION, OCCLUSION, SUPPORT];

pub const DATA_VERSION: i32 = 5023;
pub const SUPPORT_KIND: &str = "block_support";

// ---------------------------------------------------------------------------
// Voxel shapes and the table container
// ---------------------------------------------------------------------------

/// A `VoxelShape`: the per-axis coordinate partition plus the occupancy grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
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

impl Shape {
    /// `Shapes.empty()`.
    pub fn empty() -> Shape {
        Shape {
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

    /// `Shapes.block()`.
    pub fn block() -> Shape {
        Shape {
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

    pub fn is_empty(&self) -> bool {
        self.canonical == 1
    }

    pub fn is_block(&self) -> bool {
        self.canonical == 2
    }
}

/// A parsed shape table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub kind: Option<String>,
    pub data_version: i32,
    pub cached: Vec<i32>,
    pub shapes: Vec<Shape>,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, String> {
        let value = *self.data.get(self.pos).ok_or("truncated")?;
        self.pos += 1;
        Ok(value)
    }

    fn i16(&mut self) -> Result<i16, String> {
        let bytes = self.data.get(self.pos..self.pos + 2).ok_or("truncated")?;
        self.pos += 2;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn i32(&mut self) -> Result<i32, String> {
        let bytes = self.data.get(self.pos..self.pos + 4).ok_or("truncated")?;
        self.pos += 4;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i64(&mut self) -> Result<i64, String> {
        let bytes = self.data.get(self.pos..self.pos + 8).ok_or("truncated")?;
        self.pos += 8;
        let mut buffer = [0u8; 8];
        buffer.copy_from_slice(bytes);
        Ok(i64::from_be_bytes(buffer))
    }

    fn string(&mut self) -> Result<String, String> {
        let length = self.i16()? as usize;
        let bytes = self
            .data
            .get(self.pos..self.pos + length)
            .ok_or("truncated")?;
        self.pos += length;
        String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string())
    }

    fn skip(&mut self, length: usize) -> Result<(), String> {
        if self.pos + length > self.data.len() {
            return Err("truncated".to_string());
        }
        self.pos += length;
        Ok(())
    }
}

/// Decode one of the three shape tables.
pub fn decode(data: &[u8]) -> Result<Table, String> {
    let mut reader = Reader { data, pos: 0 };
    if reader.u8()? != 10 {
        return Err("root is not a compound".to_string());
    }
    if !reader.string()?.is_empty() {
        return Err("unexpected root name".to_string());
    }
    let mut table = Table {
        kind: None,
        data_version: 0,
        cached: Vec::new(),
        shapes: Vec::new(),
    };
    loop {
        let tag = reader.u8()?;
        if tag == 0 {
            return Ok(table);
        }
        let key = reader.string()?;
        match (key.as_str(), tag) {
            ("DataVersion", 3) => table.data_version = reader.i32()?,
            ("Kind", 8) => table.kind = Some(reader.string()?),
            ("cached_shapes", 11) => table.cached = read_ints(&mut reader)?,
            ("shapes", 9) => {
                if reader.u8()? != 10 {
                    return Err("shapes are not compounds".to_string());
                }
                let length = reader.i32()? as usize;
                for _ in 0..length {
                    table.shapes.push(read_shape(&mut reader)?);
                }
            }
            (_, tag) => skip_value(&mut reader, tag)?,
        }
    }
}

fn read_shape(reader: &mut Reader<'_>) -> Result<Shape, String> {
    let mut shape = Shape::empty();
    loop {
        let tag = reader.u8()?;
        if tag == 0 {
            return Ok(shape);
        }
        let name = reader.string()?;
        match (name.as_str(), tag) {
            ("canonical", 1) => shape.canonical = reader.u8()?,
            ("cube", 1) => shape.cube = reader.u8()?,
            ("cube_axes", 1) => shape.cube_axes = reader.u8()?,
            ("x", 12) => shape.x = read_longs(reader)?,
            ("y", 12) => shape.y = read_longs(reader)?,
            ("z", 12) => shape.z = read_longs(reader)?,
            ("discrete_min", 11) => {
                let values = read_ints(reader)?;
                for (slot, value) in shape.discrete_min.iter_mut().zip(values) {
                    *slot = value;
                }
            }
            ("discrete_max", 11) => {
                let values = read_ints(reader)?;
                for (slot, value) in shape.discrete_max.iter_mut().zip(values) {
                    *slot = value;
                }
            }
            ("occupied", 7) => {
                let length = reader.i32()? as usize;
                let bytes = reader
                    .data
                    .get(reader.pos..reader.pos + length)
                    .ok_or("truncated")?;
                shape.occupied = bytes.to_vec();
                reader.pos += length;
            }
            (_, tag) => skip_value(reader, tag)?,
        }
    }
}

fn read_longs(reader: &mut Reader<'_>) -> Result<Vec<f64>, String> {
    let length = reader.i32()? as usize;
    let mut values = Vec::with_capacity(length);
    for _ in 0..length {
        values.push(f64::from_bits(reader.i64()? as u64));
    }
    Ok(values)
}

fn read_ints(reader: &mut Reader<'_>) -> Result<Vec<i32>, String> {
    let length = reader.i32()? as usize;
    let mut values = Vec::with_capacity(length);
    for _ in 0..length {
        values.push(reader.i32()?);
    }
    Ok(values)
}

fn skip_value(reader: &mut Reader<'_>, tag: u8) -> Result<(), String> {
    match tag {
        1 => reader.skip(1),
        2 => reader.skip(2),
        3 | 5 => reader.skip(4),
        4 | 6 => reader.skip(8),
        7 | 11 | 12 => {
            let length = reader.i32()? as usize;
            let unit = match tag {
                7 => 1,
                11 => 4,
                _ => 8,
            };
            reader.skip(length * unit)
        }
        8 => reader.string().map(|_| ()),
        9 => {
            let element = reader.u8()?;
            let length = reader.i32()? as usize;
            for _ in 0..length {
                skip_value(reader, element)?;
            }
            Ok(())
        }
        10 => loop {
            let tag = reader.u8()?;
            if tag == 0 {
                return Ok(());
            }
            reader.string()?;
            skip_value(reader, tag)?;
        },
        _ => Err(format!("unknown tag {tag}")),
    }
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn write_longs(out: &mut Vec<u8>, values: &[f64]) {
    out.extend_from_slice(&(values.len() as i32).to_be_bytes());
    for value in values {
        out.extend_from_slice(&(value.to_bits() as i64).to_be_bytes());
    }
}

fn write_ints(out: &mut Vec<u8>, values: &[i32]) {
    out.extend_from_slice(&(values.len() as i32).to_be_bytes());
    for value in values {
        out.extend_from_slice(&value.to_be_bytes());
    }
}

fn write_bytes(out: &mut Vec<u8>, values: &[u8]) {
    out.extend_from_slice(&(values.len() as i32).to_be_bytes());
    out.extend_from_slice(values);
}

fn write_shape(out: &mut Vec<u8>, shape: &Shape) {
    // The vanilla writer inserted canonical, cube, cube_axes, x, y, z,
    // discrete_min, discrete_max, occupied into a Java `HashMap`; for these
    // names the iteration order is the prepared files' order, below.
    out.push(0x0c);
    write_string(out, "x");
    write_longs(out, &shape.x);
    out.push(0x0c);
    write_string(out, "y");
    write_longs(out, &shape.y);
    out.push(0x01);
    write_string(out, "cube_axes");
    out.push(shape.cube_axes);
    out.push(0x0c);
    write_string(out, "z");
    write_longs(out, &shape.z);
    out.push(0x01);
    write_string(out, "canonical");
    out.push(shape.canonical);
    out.push(0x01);
    write_string(out, "cube");
    out.push(shape.cube);
    out.push(0x0b);
    write_string(out, "discrete_min");
    write_ints(out, &shape.discrete_min);
    out.push(0x0b);
    write_string(out, "discrete_max");
    write_ints(out, &shape.discrete_max);
    out.push(0x07);
    write_string(out, "occupied");
    write_bytes(out, &shape.occupied);
}

/// Encode a shape table exactly as the prepared copies carry it.
pub fn encode(table: &Table) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x0a);
    write_string(&mut out, "");
    out.push(0x0b);
    write_string(&mut out, "cached_shapes");
    write_ints(&mut out, &table.cached);
    out.push(0x09);
    write_string(&mut out, "shapes");
    out.push(0x0a);
    out.extend_from_slice(&(table.shapes.len() as i32).to_be_bytes());
    for shape in &table.shapes {
        write_shape(&mut out, shape);
        out.push(0x00);
    }
    if let Some(kind) = table.kind.as_deref() {
        out.push(0x08);
        write_string(&mut out, "Kind");
        write_string(&mut out, kind);
    }
    out.push(0x03);
    write_string(&mut out, "DataVersion");
    out.extend_from_slice(&table.data_version.to_be_bytes());
    out.push(0x00);
    out
}

/// Read one of the prepared tables.
pub fn read_table(path: &Path) -> Result<Table, String> {
    let data = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    decode(&data)
}

/// `(block id, state index)` for every state, from the prepared `states.bin`.
///
/// The shape tables index states in registry order, which this exposes without
/// a second copy of the state derivation.
pub fn state_blocks(path: &Path) -> Result<Vec<(String, usize)>, String> {
    let data = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut reader = Reader {
        data: &data,
        pos: 0,
    };
    if reader.u8()? != 9 {
        return Err("states.bin root is not a list".to_string());
    }
    reader.string()?;
    if reader.u8()? != 10 {
        return Err("states.bin elements are not compounds".to_string());
    }
    let count = reader.i32()? as usize;
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        if reader.u8()? != 8 {
            return Err("a state does not start with its id".to_string());
        }
        reader.string()?;
        let id = reader.string()?;
        out.push((id, index));
        skip_value(&mut reader, 10)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Deriving shapes from the jar
// ---------------------------------------------------------------------------

/// What a derivation step produced, and what it could not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// States with a derived shape.
    pub covered: usize,
    /// States left to block-code geometry.
    pub missing: usize,
    /// Blocks left out entirely, with the owning shape method.
    pub gaps: BTreeMap<String, String>,
}

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    classes: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
    names: Vec<String>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        let mut names = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            if let Some(stripped) = name.strip_suffix(".class") {
                names.push(stripped.to_string());
            }
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(name, data);
        }
        Ok(Jar {
            files,
            classes: RefCell::new(BTreeMap::new()),
            names,
        })
    }

    /// Resolve a simple block class name through the `block` package tree.
    fn resolve(&self, simple: &str) -> Option<String> {
        let direct = format!("net/minecraft/world/level/block/{simple}");
        if self.names.contains(&direct) {
            return Some(direct);
        }
        self.names
            .iter()
            .find(|name| {
                name.starts_with("net/minecraft/world/level/block/")
                    && name.ends_with(&format!("/{simple}"))
            })
            .cloned()
    }

    fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.classes.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .files
            .get(&format!("{internal}.class"))
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.classes
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }
}

const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const PROPERTIES: &str = "net/minecraft/world/level/block/state/BlockBehaviour$Properties";
const BEHAVIOUR: &str = "net/minecraft/world/level/block/state/BlockBehaviour";

/// The methods whose override changes a collision shape.
const COLLISION_METHODS: [&str; 2] = ["getShape", "getCollisionShape"];
/// The methods whose override changes an occlusion shape.
const OCCLUSION_METHODS: [&str; 4] = [
    "getOcclusionShape",
    "getFaceOcclusionShape",
    "getShape",
    "getCollisionShape",
];

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
    target: usize,
}

fn decode_code(bytes: &[u8]) -> Vec<Instruction> {
    type RawInstruction = (usize, u8, Option<usize>, Option<i32>, Option<i32>);
    let mut raw: Vec<RawInstruction> = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let mut immediate = None;
        let mut branch = None;
        let length = match opcode {
            0x10 => {
                immediate = bytes.get(at + 1).map(|value| *value as i8 as i32);
                2
            }
            0x11 => {
                immediate = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| i16::from_be_bytes([pair[0], pair[1]]) as i32);
                3
            }
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]) as usize);
                3
            }
            0x12 => {
                index = bytes.get(at + 1).map(|value| *value as usize);
                2
            }
            0xb9 | 0xba => {
                index = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]) as usize);
                5
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => {
                index = bytes.get(at + 1).map(|value| *value as usize);
                2
            }
            0x84 => {
                index = bytes.get(at + 1).map(|value| *value as usize);
                immediate = bytes.get(at + 2).map(|value| *value as i8 as i32);
                3
            }
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                branch = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| i16::from_be_bytes([pair[0], pair[1]]) as i32);
                3
            }
            0xc5 => 4,
            0xc8 | 0xc9 => {
                branch = bytes
                    .get(at + 1..at + 5)
                    .map(|bytes| i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
                5
            }
            _ => 1,
        };
        raw.push((at, opcode, index, immediate, branch));
        at += length;
    }
    let offsets: Vec<usize> = raw.iter().map(|entry| entry.0).collect();
    raw.iter()
        .map(|(start, opcode, index, immediate, branch)| Instruction {
            opcode: *opcode,
            index: *index,
            immediate: *immediate,
            target: branch
                .map(|delta| {
                    let wanted = *start as i32 + delta;
                    offsets
                        .iter()
                        .position(|offset| *offset as i32 == wanted)
                        .unwrap_or(offsets.len())
                })
                .unwrap_or(0),
        })
        .collect()
}

fn method_at(class: &ClassFile, instruction: &Instruction) -> Option<(String, String, String)> {
    if !(0xb6..=0xb9).contains(&instruction.opcode) {
        return None;
    }
    classfile::method_ref(&class.constants, instruction.index?)
}

fn field_at(class: &ClassFile, instruction: &Instruction) -> Option<(String, String, String)> {
    if !(0xb2..=0xb5).contains(&instruction.opcode) {
        return None;
    }
    classfile::field_ref(&class.constants, instruction.index?)
}

/// `noCollision()`/`noOcclusion()` per registration site, in registration order.
fn property_flags(jar: &Jar) -> Result<(Vec<bool>, Vec<bool>), String> {
    let class = jar
        .class(BLOCKS)
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let code = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let instructions = decode_code(&code.bytes);
    let mut no_collision = Vec::new();
    let mut no_occlusion = Vec::new();
    let mut pending_collision = false;
    let mut pending_occlusion = false;
    for instruction in &instructions {
        if instruction.opcode == 0xb6 {
            let (owner, name, _) = method_at(&class, instruction).unwrap_or_default();
            if owner.ends_with("Properties") {
                match name.as_str() {
                    "noCollision" => pending_collision = true,
                    "noOcclusion" => pending_occlusion = true,
                    _ => {}
                }
            }
        }
        if instruction.opcode == 0xb8 {
            let (owner, name, _) = method_at(&class, instruction).unwrap_or_default();
            let family =
                owner.ends_with("ColorCollection") || owner.ends_with("WeatheringCopperCollection");
            let single = owner == BLOCKS
                && matches!(
                    name.as_str(),
                    "register"
                        | "registerStair"
                        | "registerSlab"
                        | "registerWall"
                        | "registerLegacyStair"
                );
            if single {
                no_collision.push(pending_collision);
                no_occlusion.push(pending_occlusion);
                pending_collision = false;
                pending_occlusion = false;
            } else if family {
                let count = if owner.ends_with("WeatheringCopperCollection") {
                    8
                } else {
                    16
                };
                for _ in 0..count {
                    no_collision.push(pending_collision);
                    no_occlusion.push(pending_occlusion);
                }
                pending_collision = false;
                pending_occlusion = false;
            }
        }
    }
    Ok((no_collision, no_occlusion))
}

/// Whether the block's class chain (up to `BlockBehaviour`) overrides one of
/// `methods`; names the owning class and method when it does.
fn shape_override(jar: &Jar, class: &str, methods: &[&str]) -> Option<String> {
    let mut current = Some(class.to_string());
    let mut depth = 0;
    while let Some(internal) = current {
        if internal == BEHAVIOUR {
            return None;
        }
        let parsed = jar.class(&internal)?;
        // Interfaces are not walked: the shape methods all live on the class
        // hierarchy, and a class's own methods cover the vanilla patterns.
        for method in &parsed.methods {
            if methods.contains(&method.name.as_str()) {
                return Some(format!("{internal}#{}", method.name));
            }
        }
        current = if parsed.super_name.is_empty() {
            None
        } else {
            Some(parsed.super_name.clone())
        };
        depth += 1;
        if depth > 32 {
            return None;
        }
    }
    None
}

/// The state's derived collision shape when the default path applies.
pub fn collision_shapes(jar_path: &Path) -> Result<(Vec<Option<Shape>>, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    let (no_collision, _) = property_flags(&jar)?;
    if no_collision.len() != registry.len() {
        return Err(format!(
            "properties flags {} != registry {}",
            no_collision.len(),
            registry.len()
        ));
    }
    let mut coverage = Coverage::default();
    let mut shapes = Vec::with_capacity(table.state_count());
    for (index, row) in table.blocks.iter().enumerate() {
        let block_row = &registry[index];
        let internal = jar
            .resolve(&block_row.class)
            .ok_or_else(|| format!("{}: class not in the jar", block_row.id))?;
        let gap = shape_override(&jar, &internal, &COLLISION_METHODS);
        let shape = if gap.is_none() {
            Some(if no_collision[index] {
                Shape::empty()
            } else {
                Shape::block()
            })
        } else {
            coverage
                .gaps
                .insert(row.id.clone(), gap.unwrap_or_default());
            None
        };
        for _ in 0..row.state_count {
            if shape.is_some() {
                coverage.covered += 1;
            } else {
                coverage.missing += 1;
            }
            shapes.push(shape.clone());
        }
    }
    Ok((shapes, coverage))
}

/// The state's derived occlusion shape when the default path applies.
///
/// `BlockBehaviour.getFaceOcclusionShape`/`getOcclusionShape` fall back to the
/// block shape, and the block shape is the full cube unless `noOcclusion()` was
/// set: the prepared copy shows empty where the properties say non-occluding and
/// the full cube everywhere else on this path.
pub fn occlusion_shapes(jar_path: &Path) -> Result<(Vec<Option<Shape>>, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    let mut coverage = Coverage::default();
    let mut shapes = Vec::with_capacity(table.state_count());
    for (index, row) in table.blocks.iter().enumerate() {
        let block_row = &registry[index];
        let internal = jar
            .resolve(&block_row.class)
            .ok_or_else(|| format!("{}: class not in the jar", block_row.id))?;
        let gap = shape_override(&jar, &internal, &OCCLUSION_METHODS);
        // Default `getFaceOcclusionShape` -> `getOcclusionShape` ->
        // `state.getShape`, whose default is the full cube (it does not consult
        // `canOcclude`: leaves and mangrove roots carry `noOcclusion()` yet a
        // full cube in the prepared copy).
        let shape = if gap.is_none() {
            Some(Shape::block())
        } else {
            coverage
                .gaps
                .insert(row.id.clone(), gap.unwrap_or_default());
            None
        };
        for _ in 0..row.state_count {
            if shape.is_some() {
                coverage.covered += 1;
            } else {
                coverage.missing += 1;
            }
            shapes.push(shape.clone());
        }
    }
    Ok((shapes, coverage))
}

/// The `full_block` flag per state, when derived.
pub fn full_block(shapes: &[Option<Shape>]) -> Vec<Option<bool>> {
    shapes
        .iter()
        .map(|shape| shape.as_ref().map(|shape| shape.is_block()))
        .collect()
}

/// Encode a derived table: `Some` shapes for covered states, the empty shape for
/// gaps (the caller must check [`Coverage`] before trusting the file).
pub fn table_of(shapes: &[Option<Shape>], kind: Option<&str>) -> Table {
    let mut dictionary: Vec<Shape> = Vec::new();
    let mut index: BTreeMap<u64, usize> = BTreeMap::new();
    let mut cached = Vec::with_capacity(shapes.len());
    for shape in shapes {
        let shape = shape.clone().unwrap_or_else(Shape::empty);
        let key = shape_key(&shape);
        let id = *index.entry(key).or_insert_with(|| {
            dictionary.push(shape.clone());
            dictionary.len() - 1
        });
        cached.push(id as i32);
    }
    Table {
        kind: kind.map(|kind| kind.to_string()),
        data_version: DATA_VERSION,
        cached,
        shapes: dictionary,
    }
}

fn shape_key(shape: &Shape) -> u64 {
    // A cheap stable key: hash the piecewise content.
    let mut hash = 1469598103934665603u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(1099511628211);
    };
    mix(shape.canonical as u64);
    mix(shape.cube as u64);
    mix(shape.cube_axes as u64);
    for axis in [&shape.x, &shape.y, &shape.z] {
        mix(axis.len() as u64);
        for value in axis {
            mix(value.to_bits());
        }
    }
    for value in shape.discrete_min {
        mix(value as u32 as u64);
    }
    for value in shape.discrete_max {
        mix(value as u32 as u64);
    }
    mix(shape.occupied.len() as u64);
    for value in &shape.occupied {
        mix(*value as u64);
    }
    hash
}
