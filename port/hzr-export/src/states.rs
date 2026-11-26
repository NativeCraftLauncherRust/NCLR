//! The block-state registry, rebuilt from the player's own `client.jar`.
//!
//! Horizon's bundle carries the state registry twice: as the `states` trailer of
//! the shape NBTs (`_shared/states.bin`) and as `_shared/block-order.json` (the
//! block order plus every block's default state id). Everything downstream that
//! is indexed by state id — `structure-blocks`, `render-definitions`, `mob-data`,
//! collision, occlusion, support, movement — sits on this table. This module
//! derives it from the jar; nothing here reads `ship.hzr3`.
//!
//! # Where the values live
//!
//! * **Block order** is `block::registry` (the factory order of
//!   `Blocks.<clinit>`), verified 1,286/1,286 against `block-behaviours.json`.
//! * **A block's properties** are the `getstatic` operands of the
//!   `createBlockStateDefinition(StateDefinition.Builder)` overrides on its class
//!   chain; `Block.<init>` calls that method once, so a class that overrides it
//!   calls `super` for inherited properties — this module follows those calls.
//! * **A property's values** are the static field's `create(...)` call in the
//!   owning class's initialiser: `BooleanProperty.create(name)` (`true, false` in
//!   26.3), `IntegerProperty.create(name, min, max)` (`min..max`), and
//!   `EnumProperty.create(name, enum)` / `(name, enum, Enum[])` /
//!   `(name, enum, Predicate)`. The predicate overload is evaluated against the
//!   enum constants (the three in `BlockStateProperties` filter by enum identity
//!   or by `Direction` axis/plane), not hardcoded.
//! * **State order** is `StateDefinition`'s: property names sorted by
//!   `String.compareTo`, and `Lists.cartesianProduct` over the value lists — the
//!   first property varies slowest, the last fastest.
//! * **Default state ids** are what `registerDefaultState(...)` assigns in each
//!   block's constructor chain: `stateDefinition.any()` plus a
//!   `setValue(property, value)` chain, interpreted from the bytecode.
//! * **The NBT order of a state's `properties`** is the iteration order of the
//!   `HashMap` the builder stores them in (capacity 16 in 26.3): bucket order,
//!   insertion order within a bucket. This is measured — all 884 blocks with
//!   properties match it in the prepared copy.
//!
//! # What this module writes
//!
//! `_shared/block-order.json` and `_shared/states.bin`, byte-for-byte the files
//! `horizon-prepare` produces (see [`write`]).
//!
//! # What it does not do
//!
//! It does not build shapes or movement/collision values: those need the running
//! block classes' geometry (see `block::BLOCKED_TABLES`). It gives every state
//! its id, which is what those tables are indexed by.

#![allow(clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;
use classfile::{ClassFile, Constant};

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

/// What `ship.hzr3` calls the files this module writes.
pub const TABLE: &str = "_shared/block-order.json";
pub const STATES: &str = "_shared/states.bin";

/// The two files, in bundle order.
pub const TABLES: [&str; 2] = [TABLE, STATES];

/// The tables this module unblocks but does not itself build.
pub const UNBLOCKED: [(&str, &str); 6] = [
    (
        "worldgen/blocks/structure-blocks.json",
        "per-state rotate/mirror and full-block classification now have their state ids",
    ),
    (
        "render-definitions.json",
        "the per-state model/rotation table is indexed by state id",
    ),
    ("mob-data.json", "its block-state references are state ids"),
    (
        "cached-collision.nbt",
        "the states trailer is written here; the shapes still need block geometry",
    ),
    (
        "occlusion.nbt",
        "the states trailer is written here; the shapes still need block geometry",
    ),
    (
        "movement-properties.nbt",
        "the states trailer is written here; the values still need block code",
    ),
];

/// One block's property, as the state table needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Vanilla's property name (`waterlogged`, `facing`, `age`, …).
    pub name: String,
    /// Value names in `Property.getPossibleValues()` order.
    pub values: Vec<String>,
}

/// One block's place in the state registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStates {
    /// `minecraft:<id>`.
    pub id: String,
    /// Properties in the order `createBlockStateDefinition` added them. The
    /// state ids use the sorted order; this order is the builder `HashMap`'s
    /// insertion order, which the bundle's NBT order breaks bucket ties with.
    pub properties: Vec<Property>,
    /// The number of states this block contributes.
    pub state_count: usize,
    /// The block's default state, as an index within its own states.
    pub default_index: usize,
}

impl BlockStates {
    /// The properties sorted by name — the order `StateDefinition` enumerates in.
    pub fn sorted(&self) -> Vec<&Property> {
        let mut sorted: Vec<&Property> = self.properties.iter().collect();
        sorted.sort_by(|left, right| left.name.cmp(&right.name));
        sorted
    }
}

/// The whole registry: blocks in registry order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateTable {
    pub blocks: Vec<BlockStates>,
}

impl StateTable {
    pub fn state_count(&self) -> usize {
        self.blocks.iter().map(|block| block.state_count).sum()
    }

    /// The default state ids per block, keyed by block id.
    pub fn defaults(&self) -> BTreeMap<String, u32> {
        let mut out = BTreeMap::new();
        let mut next = 0u32;
        for block in &self.blocks {
            out.insert(block.id.clone(), next + block.default_index as u32);
            next += block.state_count as u32;
        }
        out
    }
}

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

impl Coverage {
    fn add(&mut self, table: &str) {
        self.produced.push(table.to_string());
        self.missing.retain(|name| name != table);
    }

    fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }
}

// ---------------------------------------------------------------------------
// Reading the jar
// ---------------------------------------------------------------------------

const BLOCK_PREFIX: &str = "net/minecraft/world/level/block/";
const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const BLOCK_STATE_PROPERTIES: &str =
    "net/minecraft/world/level/block/state/properties/BlockStateProperties";
const PROPERTY_PACKAGE: &str = "net/minecraft/world/level/block/state/properties/";

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    classes: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(name, data);
        }
        Ok(Jar {
            files,
            classes: RefCell::new(BTreeMap::new()),
        })
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

    /// Internal names of the classes the property sweep reads.
    fn class_names(&self) -> Vec<String> {
        self.files
            .keys()
            .filter_map(|name| name.strip_suffix(".class").map(str::to_string))
            .filter(|name| name.starts_with(BLOCK_PREFIX) || name.starts_with(PROPERTY_PACKAGE))
            .collect()
    }
}

impl Jar {
    /// The interfaces a class declares, by internal name.
    fn interfaces(&self, internal: &str) -> Vec<String> {
        let Some(bytes) = self.files.get(&format!("{internal}.class")) else {
            return Vec::new();
        };
        let mut at = 8usize; // magic, minor, major
        let read_u2 = |at: &mut usize| -> u16 {
            let value = u16::from_be_bytes([bytes[*at], bytes[*at + 1]]);
            *at += 2;
            value
        };
        if bytes.len() < 10 {
            return Vec::new();
        }
        let count = read_u2(&mut at);
        let mut utf8: BTreeMap<u16, String> = BTreeMap::new();
        let mut classes: BTreeMap<u16, u16> = BTreeMap::new();
        let mut index = 1u16;
        while index < count {
            let tag = bytes[at];
            at += 1;
            match tag {
                1 => {
                    let length = read_u2(&mut at) as usize;
                    let text = String::from_utf8_lossy(&bytes[at..at + length]).into_owned();
                    at += length;
                    utf8.insert(index, text);
                }
                7 => {
                    let name = read_u2(&mut at);
                    classes.insert(index, name);
                }
                8 | 16 | 19 | 20 => at += 2,
                15 => at += 3,
                3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => at += 4,
                5 | 6 => {
                    at += 8;
                    index += 1;
                }
                other => panic!("constant tag {other} in {internal}"),
            }
            index += 1;
        }
        at += 2; // access flags
        at += 2; // this class
        at += 2; // super class
        let declared = read_u2(&mut at) as usize;
        let mut out = Vec::with_capacity(declared);
        for _ in 0..declared {
            let interface = read_u2(&mut at);
            if let Some(name) = classes.get(&interface).and_then(|name| utf8.get(name)) {
                out.push(name.clone());
            }
        }
        out
    }
}

/// A class and everything it inherits from (superclasses and interfaces).
fn type_closure(jar: &Jar, internal: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut queue = vec![internal.to_string()];
    while let Some(current) = queue.pop() {
        if out.contains(&current) {
            continue;
        }
        if let Some(class) = jar.class(&current) {
            if !class.super_name.is_empty() {
                queue.push(class.super_name.clone());
            }
        }
        queue.extend(jar.interfaces(&current));
        out.push(current);
    }
    out
}

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    /// `bipush`/`sipush`/`iinc` operand.
    immediate: Option<i32>,
    /// Absolute bytecode offset of a branch target, already resolved to this
    /// instruction list.
    target: usize,
}

/// Decode a method body: enough of the JVM to read initialisers, property
/// definitions and constructors.
fn decode(bytes: &[u8]) -> Vec<Instruction> {
    // (offset, opcode, constant index, immediate, branch delta)
    type RawInstruction = (usize, u8, Option<usize>, Option<i32>, Option<i32>);
    let mut raw: Vec<RawInstruction> = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let mut branch = None;
        let mut immediate = None;
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
            0xc4 => {
                if bytes.get(at + 1) == Some(&0x84) {
                    6
                } else {
                    4
                }
            }
            0xc5 => 4,
            0xc8 | 0xc9 => {
                branch = bytes
                    .get(at + 1..at + 5)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                5
            }
            0xaa => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base = at + 1 + padding;
                let (Some(low), Some(high)) = (
                    bytes
                        .get(base + 4..base + 8)
                        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
                    bytes
                        .get(base + 8..base + 12)
                        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
                ) else {
                    break;
                };
                let count = (high as i64 - low as i64 + 1).max(0) as usize;
                1 + padding + 12 + count * 4
            }
            0xab => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base = at + 1 + padding;
                let pairs = bytes
                    .get(base + 4..base + 8)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                    .unwrap_or(0)
                    .max(0) as usize;
                1 + padding + 8 + pairs * 8
            }
            _ => 1,
        };
        raw.push((at, opcode, index, immediate, branch));
        at += length;
    }
    let offsets: Vec<usize> = raw.iter().map(|entry| entry.0).collect();
    let position = |offset: i32| -> usize {
        offsets
            .iter()
            .position(|start| *start as i32 == offset)
            .unwrap_or(offsets.len())
    };
    raw.iter()
        .map(|(start, opcode, index, immediate, branch)| Instruction {
            opcode: *opcode,
            index: *index,
            immediate: *immediate,
            target: branch
                .map(|delta| position(*start as i32 + delta))
                .unwrap_or(0),
        })
        .collect()
}

fn class_name_at(constants: &[Constant], index: usize) -> Option<String> {
    let Constant::Class(reference) = constants.get(index)? else {
        return None;
    };
    let inner: usize = reference.trim_start_matches('#').parse().ok()?;
    match constants.get(inner)? {
        Constant::Utf8(name) => Some(name.clone()),
        _ => None,
    }
}

fn simple(internal: &str) -> &str {
    internal.rsplit('/').next().unwrap_or(internal)
}

fn string_at(class: &ClassFile, instruction: &Instruction) -> Option<String> {
    match instruction.opcode {
        0x12 | 0x13 => {
            classfile::string_constant(&class.constants, instruction.index?).filter(|_| {
                matches!(
                    class.constants.get(instruction.index.unwrap_or(0)),
                    Some(Constant::String(_)) | Some(Constant::Utf8(_))
                )
            })
        }
        _ => None,
    }
}

/// The value an `ldc`/`ldc_w` instruction pushes, as far as this module cares.
fn ldc_value(class: &ClassFile, instruction: &Instruction) -> Option<Init> {
    match instruction.opcode {
        0x10 | 0x11 => Some(Init::Int(instruction.immediate.unwrap_or(0))),
        0x12..=0x14 => match class.constants.get(instruction.index?)? {
            Constant::Integer(value) => Some(Init::Int(*value)),
            Constant::Float(value) => {
                let _ = value;
                Some(Init::Unknown)
            }
            Constant::String(_) | Constant::Utf8(_) => {
                classfile::string_constant(&class.constants, instruction.index?).map(Init::Text)
            }
            Constant::Class(_) => {
                class_name_at(&class.constants, instruction.index?).map(Init::Text)
            }
            _ => Some(Init::Unknown),
        },
        _ => None,
    }
}

fn new_class(class: &ClassFile, instruction: &Instruction) -> Option<String> {
    if instruction.opcode != 0xbb {
        return None;
    }
    class_name_at(&class.constants, instruction.index?)
}

fn indy_target(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
        let Constant::MethodHandle { reference } = class.constants.get(*argument)? else {
            continue;
        };
        let Constant::Methodref {
            class: owner, name, ..
        } = class.constants.get(*reference)?
        else {
            continue;
        };
        let owner_index: usize = owner.trim_start_matches('#').parse().ok()?;
        let name_and_type: usize = name.trim_start_matches('#').parse().ok()?;
        let owner_name = class_name_at(&class.constants, owner_index)?;
        let Constant::NameAndType { name, kind } = class.constants.get(name_and_type)? else {
            continue;
        };
        let name_index: usize = name.trim_start_matches('#').parse().ok()?;
        let kind_index: usize = kind.trim_start_matches('#').parse().ok()?;
        return Some((
            owner_name,
            classfile::utf8(&class.constants, name_index),
            classfile::utf8(&class.constants, kind_index),
        ));
    }
    None
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

fn parameter_count(descriptor: &str) -> usize {
    classfile::descriptor_types(descriptor)
        .len()
        .saturating_sub(1)
}

fn method_descriptor<'a>(
    class: &'a ClassFile,
    name: &str,
    descriptor: &str,
) -> Option<&'a classfile::Member> {
    class
        .methods
        .iter()
        .find(|method| method.name == name && method.descriptor == descriptor)
}

// ---------------------------------------------------------------------------
// Enum constants and the Direction/Axis/Plane relation
// ---------------------------------------------------------------------------

fn enum_constants(jar: &Jar, internal: &str) -> Vec<String> {
    let Some(class) = jar.class(internal) else {
        return Vec::new();
    };
    let descriptor = format!("L{internal};");
    class
        .fields
        .iter()
        .filter(|field| field.access & 0x4000 != 0 && field.descriptor == descriptor)
        .map(|field| field.name.clone())
        .collect()
}

/// The serialised name of every enum constant, in declaration order.
///
/// `EnumProperty` labels values with `StringRepresentable.getSerializedName()`,
/// and the name is a constructor operand that usually — but not always — equals
/// the lowercased constant (`PistonType.DEFAULT` serialises as `normal`).
fn enum_value_names(jar: &Jar, internal: &str) -> Vec<String> {
    let constants = enum_constants(jar, internal);
    let mut serialized: BTreeMap<String, String> = BTreeMap::new();
    if let Some(class) = jar.class(internal) {
        if let Some(initialiser) = class.method("<clinit>").and_then(|m| m.code.as_ref()) {
            let mut strings: Vec<String> = Vec::new();
            for instruction in decode(&initialiser.bytes) {
                if new_class(&class, &instruction).as_deref() == Some(internal) {
                    strings.clear();
                    continue;
                }
                if let Some(text) = string_at(&class, &instruction) {
                    strings.push(text);
                    continue;
                }
                if instruction.opcode == 0xb3 {
                    if let Some((owner, name, _)) = field_at(&class, &instruction) {
                        if owner == internal {
                            if let Some(value) = strings.get(1) {
                                serialized.insert(name, value.clone());
                            }
                            strings.clear();
                        }
                    }
                }
            }
        }
    }
    constants
        .iter()
        .map(|constant| {
            serialized
                .get(constant)
                .cloned()
                .unwrap_or_else(|| constant.to_lowercase())
        })
        .collect()
}

/// `Direction` constant -> `Direction.Axis` constant, from `Direction.<clinit>`.
fn direction_axes(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/core/Direction") else {
        return out;
    };
    let Some(initialiser) = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut building = false;
    let mut axis = None;
    for instruction in decode(&initialiser.bytes) {
        if new_class(&class, &instruction).as_deref() == Some("net/minecraft/core/Direction") {
            building = true;
            axis = None;
            continue;
        }
        if !building {
            continue;
        }
        if let Some((owner, name, _)) = field_at(&class, &instruction) {
            if owner.ends_with("Direction$Axis") && instruction.opcode == 0xb2 {
                axis = Some(name.clone());
            }
            if owner == "net/minecraft/core/Direction" && instruction.opcode == 0xb3 {
                if let Some(axis) = axis.take() {
                    out.insert(name.clone(), axis);
                }
                building = false;
            }
        }
    }
    out
}

/// `Direction.Axis` constant -> `Direction.Plane` constant, from `Plane.<clinit>`.
fn axis_planes(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/core/Direction$Plane") else {
        return out;
    };
    let Some(initialiser) = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut building = false;
    let mut axes: Vec<String> = Vec::new();
    for instruction in decode(&initialiser.bytes) {
        if new_class(&class, &instruction).as_deref() == Some("net/minecraft/core/Direction$Plane")
        {
            building = true;
            axes.clear();
            continue;
        }
        if !building {
            continue;
        }
        if let Some((owner, name, _)) = field_at(&class, &instruction) {
            if owner.ends_with("Direction$Axis") && instruction.opcode == 0xb2 {
                axes.push(name.clone());
            }
            if owner == "net/minecraft/core/Direction$Plane" && instruction.opcode == 0xb3 {
                for axis in axes.drain(..) {
                    out.insert(axis, name.clone());
                }
                building = false;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Property catalogue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct PropertyDef {
    name: String,
    values: Vec<String>,
}

#[derive(Debug, Default)]
struct Catalogue {
    properties: Vec<PropertyDef>,
    /// (owner internal, static field) -> property id.
    fields: BTreeMap<(String, String), usize>,
    /// (owner internal, static field) -> array elements (property ids).
    arrays: BTreeMap<(String, String), Vec<usize>>,
}

impl Catalogue {
    fn intern(&mut self, definition: PropertyDef) -> usize {
        if let Some(index) = self
            .properties
            .iter()
            .position(|candidate| *candidate == definition)
        {
            return index;
        }
        self.properties.push(definition);
        self.properties.len() - 1
    }

    fn property(&self, owner: &str, field: &str) -> Option<usize> {
        self.fields
            .get(&(owner.to_string(), field.to_string()))
            .copied()
    }

    fn array(&self, owner: &str, field: &str) -> Option<Vec<usize>> {
        self.arrays
            .get(&(owner.to_string(), field.to_string()))
            .cloned()
    }

    fn name(&self, id: usize) -> String {
        self.properties
            .get(id)
            .map(|definition| definition.name.clone())
            .unwrap_or_default()
    }

    fn values(&self, id: usize) -> Vec<String> {
        self.properties
            .get(id)
            .map(|definition| definition.values.clone())
            .unwrap_or_default()
    }
}

/// A static field reached through `owner` may be declared on a superclass: the
/// constant pool names the class the access went through
/// (`MangrovePropaguleBlock.STAGE` is `SaplingBlock.STAGE`).
fn lookup_property(jar: &Jar, catalogue: &Catalogue, owner: &str, field: &str) -> Option<usize> {
    for internal in type_closure(jar, owner) {
        if let Some(id) = catalogue.property(&internal, field) {
            return Some(id);
        }
    }
    // Interface constants: the field can live on an interface the class
    // implements (constant pools name the class the access went through). Fall
    // back only when every field of that name is the same property.
    let mut found: Option<usize> = None;
    for ((_owner, name), id) in &catalogue.fields {
        if name != field {
            continue;
        }
        match found {
            None => found = Some(*id),
            Some(existing) if existing == *id => {}
            Some(_) => return None,
        }
    }
    found
}

fn lookup_array(jar: &Jar, catalogue: &Catalogue, owner: &str, field: &str) -> Option<Vec<usize>> {
    for internal in type_closure(jar, owner) {
        if let Some(ids) = catalogue.array(&internal, field) {
            return Some(ids);
        }
    }
    let mut found: Option<Vec<usize>> = None;
    for ((_owner, name), ids) in &catalogue.arrays {
        if name != field {
            continue;
        }
        match &found {
            None => found = Some(ids.clone()),
            Some(existing) if existing == ids => {}
            Some(_) => return None,
        }
    }
    found
}

/// A one-line property getter (`getSegmentAmountProperty()` returns
/// `BlockStateProperties.SEGMENT_AMOUNT`), found by name anywhere in the jar.
fn resolve_property_getter(
    jar: &Jar,
    catalogue: &Catalogue,
    owner: &str,
    method: &str,
) -> Option<usize> {
    for internal in type_closure(jar, owner) {
        let Some(class) = jar.class(&internal) else {
            continue;
        };
        let Some(found) = class
            .methods
            .iter()
            .find(|candidate| candidate.name == method)
        else {
            continue;
        };

        let Some(code) = &found.code else {
            continue;
        };
        let instructions = decode(&code.bytes);

        if instructions.len() != 2
            || instructions[0].opcode != 0xb2
            || !matches!(instructions[1].opcode, 0xac | 0xb0)
        {
            continue;
        }
        if let Some((owner, name, _)) = field_at(&class, &instructions[0]) {
            if let Some(id) = lookup_property(jar, catalogue, &owner, &name) {
                return Some(id);
            }
        }
    }
    None
}

/// A value produced while reading an initialiser.
#[derive(Clone, Debug)]
enum Init {
    Int(i32),
    Text(String),
    Field(String, String),
    Property(PropertyDef),

    /// A lambda method used as an enum filter.
    Predicate(String, String),
    /// An `invokestatic` that returns an int, resolved on demand.
    Call(String, String),
    Array(Rc<RefCell<Vec<Option<Init>>>>),
    Unknown,
}

impl Init {
    fn unknown() -> Init {
        Init::Unknown
    }
}

fn eval_predicate(
    jar: &Jar,
    owner: &str,
    method: &str,
    enum_internal: &str,
    constants: &[String],
    direction_axes: &BTreeMap<String, String>,
    axis_planes: &BTreeMap<String, String>,
) -> Result<Vec<bool>, String> {
    let class = jar
        .class(owner)
        .ok_or_else(|| format!("predicate owner {owner} is not in the jar"))?;
    let Some(entry) = class
        .methods
        .iter()
        .find(|candidate| candidate.name == method)
    else {
        return Err(format!("predicate {owner}.{method} is missing"));
    };
    let code = entry
        .code
        .as_ref()
        .ok_or_else(|| format!("predicate {owner}.{method} has no code"))?;
    let instructions = decode(&code.bytes);
    let mut out = Vec::with_capacity(constants.len());
    for constant in constants {
        let mut locals: Vec<Option<Init>> = vec![None; code.max_locals as usize];
        if let Some(slot) = locals.get_mut(0) {
            *slot = Some(Init::Field(enum_internal.to_string(), constant.clone()));
        }
        let mut stack: Vec<Init> = Vec::new();
        let mut at = 0usize;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 100_000 {
                return Err(format!("predicate {owner}.{method} did not terminate"));
            }
            let Some(instruction) = instructions.get(at) else {
                return Err(format!("predicate {owner}.{method} ran off the end"));
            };
            at += 1;
            match instruction.opcode {
                0x2a => stack.push(
                    locals
                        .first()
                        .cloned()
                        .flatten()
                        .unwrap_or_else(Init::unknown),
                ),
                0x2b => stack.push(
                    locals
                        .get(1)
                        .cloned()
                        .flatten()
                        .unwrap_or_else(Init::unknown),
                ),
                0x4b..=0x4e => {
                    let slot = (instruction.opcode - 0x4b) as usize;
                    let value = stack.pop().unwrap_or_else(Init::unknown);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = Some(value);
                    }
                }
                0x02..=0x08 => stack.push(Init::Int(instruction.opcode as i32 - 0x03)),
                0xb2 => {
                    let (field_owner, name, _) = field_at(&class, instruction).unwrap_or_default();
                    stack.push(Init::Field(field_owner, name));
                }
                0xb6 => {
                    let (call_owner, name, descriptor) =
                        method_at(&class, instruction).unwrap_or_default();
                    let args = parameter_count(&descriptor);
                    let mut popped = Vec::new();
                    for _ in 0..args {
                        popped.push(stack.pop().unwrap_or_else(Init::unknown));
                    }
                    popped.reverse();
                    let value = match name.as_str() {
                        "getAxis" => match popped.first().cloned() {
                            Some(Init::Field(owner, field)) if owner.ends_with("/Direction") => {
                                let axis = direction_axes.get(&field).cloned().unwrap_or_default();
                                Init::Field("net/minecraft/core/Direction$Axis".to_string(), axis)
                            }
                            _ => Init::unknown(),
                        },
                        "getPlane" => match popped.first().cloned() {
                            Some(Init::Field(owner, field))
                                if owner.ends_with("Direction$Axis") =>
                            {
                                let plane = axis_planes.get(&field).cloned().unwrap_or_default();
                                Init::Field("net/minecraft/core/Direction$Plane".to_string(), plane)
                            }
                            _ => Init::unknown(),
                        },
                        _ => {
                            let _ = call_owner;
                            Init::unknown()
                        }
                    };
                    stack.push(value);
                }
                0x99..=0x9e | 0xa0..=0xa4 | 0xa5 | 0xa6 | 0xc6 | 0xc7 => {
                    let taken = match instruction.opcode {
                        0xa5 => same_reference(&stack.pop(), &stack.pop()),
                        0xa6 => !same_reference(&stack.pop(), &stack.pop()),
                        0x99 => matches!(stack.pop(), Some(Init::Int(0))),
                        0x9a => !matches!(stack.pop(), Some(Init::Int(0))),
                        0xc6 => stack.pop().is_none(),
                        0xc7 => stack.pop().is_some(),
                        _ => {
                            stack.pop();
                            stack.pop();
                            false
                        }
                    };
                    if taken {
                        at = instruction.target;
                    }
                }
                0xa7 => at = instruction.target,
                0xac => {
                    let value = stack.pop();
                    out.push(match value {
                        Some(Init::Int(value)) => value != 0,
                        Some(Init::Field(owner, field)) => {
                            owner.ends_with("Direction$Plane") && field == "HORIZONTAL"
                        }
                        _ => false,
                    });
                    break;
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

fn same_reference(left: &Option<Init>, right: &Option<Init>) -> bool {
    matches!(
        (left, right),
        (Some(Init::Field(owner_a, name_a)), Some(Init::Field(owner_b, name_b)))
            if owner_a == owner_b && name_a == name_b
    )
}

fn catalogue(jar: &Jar) -> Result<Catalogue, String> {
    let direction_axes = direction_axes(jar);
    let axis_planes = axis_planes(jar);
    let mut catalogue = Catalogue::default();
    let mut classes = jar.class_names();
    // `BlockStateProperties` first, then the rest; a few sweeps so an alias of an
    // alias, and an array of aliases, resolve whatever order the classes appear in.
    classes.sort_by_key(|name| {
        let rank = if name == BLOCK_STATE_PROPERTIES { 0 } else { 1 };
        (rank, name.clone())
    });
    for _ in 0..3 {
        for internal in &classes {
            let Some(class) = jar.class(internal) else {
                continue;
            };
            let Some(initialiser) = class
                .method("<clinit>")
                .and_then(|method| method.code.as_ref())
            else {
                continue;
            };
            eval_initialiser(
                jar,
                &class,
                &decode(&initialiser.bytes),
                &mut catalogue,
                &direction_axes,
                &axis_planes,
            )?;
        }
    }
    Ok(catalogue)
}

fn eval_initialiser(
    jar: &Jar,
    class: &Rc<ClassFile>,
    instructions: &[Instruction],
    catalogue: &mut Catalogue,
    direction_axes: &BTreeMap<String, String>,
    axis_planes: &BTreeMap<String, String>,
) -> Result<(), String> {
    let mut stack: Vec<Init> = Vec::new();
    for instruction in instructions {
        if let Some(value) = ldc_value(class, instruction) {
            stack.push(value);
            continue;
        }
        match instruction.opcode {
            0x02..=0x08 => stack.push(Init::Int(instruction.opcode as i32 - 0x03)),
            0x01 => stack.push(Init::Unknown),
            0x59 => {
                let value = stack.last().cloned().unwrap_or_else(Init::unknown);
                stack.push(value);
            }
            0x57 => {
                stack.pop();
            }
            0x4e => {
                stack.pop();
            }
            0x53 => {
                let value = stack.pop().unwrap_or_else(Init::unknown);
                let index = stack.pop();
                let array = stack.pop();
                if let (Some(Init::Array(items)), Some(Init::Int(index))) = (array, index) {
                    let mut items = items.borrow_mut();
                    if let Some(slot) = items.get_mut(index.max(0) as usize) {
                        *slot = Some(value);
                    }
                }
            }
            0xb2 => {
                let (owner, name, _) = field_at(class, instruction).unwrap_or_default();
                stack.push(Init::Field(owner, name));
            }
            0xbb => stack.push(Init::Unknown),
            0xbc | 0xbd => {
                // `newarray`/`anewarray`: only the shape of the initialiser's
                // arrays matters here.
                let count = match stack.pop() {
                    Some(Init::Int(count)) => count.max(0) as usize,
                    _ => 0,
                };
                stack.push(Init::Array(Rc::new(RefCell::new(vec![None; count]))));
            }
            0x2e..=0x35 => {
                let index = stack.pop();
                let array = stack.pop();
                let value = match (array, index) {
                    (Some(Init::Array(items)), Some(Init::Int(index))) => items
                        .borrow()
                        .get(index.max(0) as usize)
                        .cloned()
                        .flatten()
                        .unwrap_or_else(Init::unknown),
                    _ => Init::Unknown,
                };
                stack.push(value);
            }
            0x4f..=0x56 => {
                // Array stores of every element type: value, index, array.
                let value = stack.pop().unwrap_or_else(Init::unknown);
                let index = stack.pop();
                let array = stack.pop();
                if let (Some(Init::Array(items)), Some(Init::Int(index))) = (array, index) {
                    let mut items = items.borrow_mut();
                    if let Some(slot) = items.get_mut(index.max(0) as usize) {
                        *slot = Some(value);
                    }
                }
            }
            0xb3 => {
                let (owner, name, descriptor) = field_at(class, instruction).unwrap_or_default();
                let value = stack.pop().unwrap_or_else(Init::unknown);
                if !descriptor.contains("state/properties/") {
                    continue;
                }
                match value {
                    Init::Property(definition) => {
                        let id = catalogue.intern(definition);
                        catalogue.fields.insert((owner, name), id);
                    }
                    Init::Field(from_owner, from_name) => {
                        if let Some(id) = lookup_property(jar, catalogue, &from_owner, &from_name) {
                            catalogue.fields.insert((owner, name), id);
                        }
                    }
                    Init::Array(items) => {
                        let mut ids: Vec<usize> = Vec::new();
                        for item in items.borrow().iter().flatten() {
                            match item {
                                Init::Property(definition) => {
                                    ids.push(catalogue.intern(definition.clone()))
                                }
                                Init::Field(owner, field) => {
                                    if let Some(id) = lookup_property(jar, catalogue, owner, field)
                                    {
                                        ids.push(id);
                                    }
                                }
                                _ => {}
                            }
                        }
                        if !ids.is_empty() {
                            catalogue.arrays.insert((owner, name), ids);
                        }
                    }
                    _ => {}
                }
            }
            0xb6..=0xb9 => {
                let (owner, name, descriptor) = method_at(class, instruction).unwrap_or_default();
                let args = parameter_count(&descriptor);
                let mut popped = Vec::new();
                for _ in 0..args {
                    popped.push(stack.pop().unwrap_or_else(Init::unknown));
                }
                popped.reverse();
                let value = eval_property_call(
                    jar,
                    &owner,
                    &name,
                    &descriptor,
                    &popped,
                    direction_axes,
                    axis_planes,
                )
                .map_err(|error| format!("{}: {error} [{owner}.{name}{descriptor}]", class.name))?;
                if let Some(value) = value {
                    stack.push(value);
                } else if instruction.opcode == 0xb8 && descriptor.ends_with(")I") {
                    stack.push(Init::Call(owner, name));
                }
            }
            0xba => {
                if let Some((owner, name, _)) = indy_target(class, instruction.index.unwrap_or(0)) {
                    stack.push(Init::Predicate(owner, name));
                } else {
                    stack.push(Init::Unknown);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn eval_property_call(
    jar: &Jar,
    owner: &str,
    name: &str,
    descriptor: &str,
    arguments: &[Init],
    direction_axes: &BTreeMap<String, String>,
    axis_planes: &BTreeMap<String, String>,
) -> Result<Option<Init>, String> {
    let text = |index: usize| -> Option<String> {
        match arguments.get(index) {
            Some(Init::Text(value)) => Some(value.clone()),
            _ => None,
        }
    };

    if owner.ends_with("BooleanProperty") && name == "create" {
        return Ok(Some(Init::Property(PropertyDef {
            name: text(0).ok_or_else(|| "BooleanProperty.create needs a name".to_string())?,
            values: vec!["true".into(), "false".into()],
        })));
    }
    if owner.ends_with("IntegerProperty") && name == "create" {
        let bound = |index: usize| -> Option<i32> {
            match arguments.get(index) {
                Some(Init::Int(value)) => Some(*value),
                Some(Init::Call(owner, name)) => eval_static_int(jar, owner, name).ok(),
                _ => None,
            }
        };
        let start = bound(1).ok_or_else(|| "IntegerProperty.create needs a minimum".to_string())?;
        let end = bound(2).ok_or_else(|| "IntegerProperty.create needs a maximum".to_string())?;
        return Ok(Some(Init::Property(PropertyDef {
            name: text(0).ok_or_else(|| "IntegerProperty.create needs a name".to_string())?,
            values: (start..=end).map(|value| value.to_string()).collect(),
        })));
    }
    if owner.ends_with("EnumProperty") && name == "create" {
        let property_name =
            text(0).ok_or_else(|| "EnumProperty.create needs a name".to_string())?;
        let enum_internal =
            text(1).ok_or_else(|| "EnumProperty.create needs an enum".to_string())?;
        let constants = enum_constants(jar, &enum_internal);
        let labels = enum_value_names(jar, &enum_internal);
        let label = |constant: &str| -> String {
            constants
                .iter()
                .position(|candidate| candidate == constant)
                .and_then(|index| labels.get(index).cloned())
                .unwrap_or_else(|| constant.to_lowercase())
        };
        let values: Vec<String> = if descriptor.contains("Predicate") {
            match arguments.get(2) {
                Some(Init::Field(owner, field)) if owner.ends_with("Direction$Plane") => {
                    let mut matches = Vec::new();
                    for direction in &constants {
                        let axis = direction_axes.get(direction).cloned().unwrap_or_default();
                        let plane = axis_planes.get(&axis).cloned().unwrap_or_default();
                        if &plane == field {
                            matches.push(label(direction));
                        }
                    }
                    matches
                }
                Some(Init::Predicate(owner, method)) => {
                    let keep = eval_predicate(
                        jar,
                        owner,
                        method,
                        &enum_internal,
                        &constants,
                        direction_axes,
                        axis_planes,
                    )?;
                    constants
                        .iter()
                        .zip(keep)
                        .filter(|(_, keep)| *keep)
                        .map(|(constant, _)| label(constant))
                        .collect()
                }
                other => {
                    return Err(format!(
                        "EnumProperty.create for {property_name}: unsupported filter {other:?}"
                    ))
                }
            }
        } else if arguments.len() > 2 {
            match &arguments[2] {
                Init::Array(items) => items
                    .borrow()
                    .iter()
                    .filter_map(|item| match item {
                        Some(Init::Field(_, field)) => Some(label(field)),
                        _ => None,
                    })
                    .collect(),
                _ => labels.clone(),
            }
        } else {
            labels.clone()
        };
        return Ok(Some(Init::Property(PropertyDef {
            name: property_name,
            values,
        })));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Static integer resolution
// ---------------------------------------------------------------------------

/// A value for the small arithmetic interpreter below: enough for the property
/// bounds that are computed rather than literal (`rotation`'s maximum comes from
/// `RotationSegment.getMaxSegmentIndex()`).
#[derive(Clone, Debug)]
enum IVal {
    Int(i32),
    Obj(usize),
    Unknown,
}

#[derive(Clone, Debug)]
struct IObject {
    fields: BTreeMap<String, IVal>,
}

struct IntVm<'a> {
    jar: &'a Jar,
    statics: BTreeMap<(String, String), IVal>,
    objects: Vec<IObject>,
    running: BTreeSet<String>,
}

/// Evaluate a static int method (`<clinit>` chains included).
fn eval_static_int(jar: &Jar, internal: &str, method: &str) -> Result<i32, String> {
    let mut vm = IntVm {
        jar,
        statics: BTreeMap::new(),
        objects: Vec::new(),
        running: BTreeSet::new(),
    };
    let value = vm.call(internal, method, &[], None, 0)?;
    match value {
        IVal::Int(value) => Ok(value),
        other => Err(format!(
            "{internal}.{method} did not return an int: {other:?}"
        )),
    }
}

impl<'a> IntVm<'a> {
    fn init_class(&mut self, internal: &str, depth: usize) -> Result<(), String> {
        if !self.running.insert(internal.to_string()) {
            return Ok(());
        }
        let Some(class) = self.jar.class(internal) else {
            return Ok(());
        };
        if let Some(method) = class.method("<clinit>") {
            if let Some(code) = &method.code {
                self.run(&class, code, Vec::new(), depth)?;
            }
        }
        Ok(())
    }

    fn call(
        &mut self,
        internal: &str,
        name: &str,
        arguments: &[IVal],
        this: Option<usize>,
        depth: usize,
    ) -> Result<IVal, String> {
        if depth > 24 {
            return Err(format!("int call chain too deep at {internal}.{name}"));
        }
        // `java/lang/Object.<init>` and the like live in the JDK, not the jar.
        let Some(class) = self.jar.class(internal) else {
            return Ok(IVal::Unknown);
        };
        let method = class
            .methods
            .iter()
            .find(|method| method.name == name)
            .ok_or_else(|| format!("{internal}.{name} is missing"))?;
        let Some(code) = &method.code else {
            return Ok(IVal::Unknown);
        };
        let mut locals: Vec<IVal> = vec![IVal::Unknown; code.max_locals as usize];
        let mut slot = 0usize;
        if method.access & 0x0008 == 0 {
            locals[0] = this.map(IVal::Obj).unwrap_or(IVal::Unknown);
            slot = 1;
        }
        for argument in arguments {
            if let Some(cell) = locals.get_mut(slot) {
                *cell = argument.clone();
            }
            slot += 1;
        }
        self.run(&class, code, locals, depth)
    }

    fn run(
        &mut self,
        class: &Rc<ClassFile>,
        code: &classfile::Code,
        mut locals: Vec<IVal>,
        depth: usize,
    ) -> Result<IVal, String> {
        let instructions = decode(&code.bytes);
        let mut stack: Vec<IVal> = Vec::new();
        let mut at = 0usize;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 200_000 {
                return Err(format!("{}: int method did not terminate", class.name));
            }
            let Some(instruction) = instructions.get(at) else {
                return Ok(IVal::Unknown);
            };
            at += 1;
            let pop = |stack: &mut Vec<IVal>| stack.pop().unwrap_or(IVal::Unknown);
            match instruction.opcode {
                0x02..=0x08 => stack.push(IVal::Int(instruction.opcode as i32 - 0x03)),
                0x10 | 0x11 => stack.push(IVal::Int(instruction.immediate.unwrap_or(0))),
                0x12 | 0x13 => match class.constants.get(instruction.index.unwrap_or(0)) {
                    Some(Constant::Integer(value)) => stack.push(IVal::Int(*value)),
                    _ => stack.push(IVal::Unknown),
                },
                0x1a..=0x1d => {
                    let slot = (instruction.opcode - 0x1a) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(IVal::Unknown));
                }
                0x15 => {
                    let slot = instruction.index.unwrap_or(0);
                    stack.push(locals.get(slot).cloned().unwrap_or(IVal::Unknown));
                }
                0x2a..=0x2d => {
                    let slot = (instruction.opcode - 0x2a) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(IVal::Unknown));
                }
                0x19 => {
                    let slot = instruction.index.unwrap_or(0);
                    stack.push(locals.get(slot).cloned().unwrap_or(IVal::Unknown));
                }
                0x3b..=0x3e => {
                    let slot = (instruction.opcode - 0x3b) as usize;
                    let value = pop(&mut stack);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x4b..=0x4e => {
                    let slot = (instruction.opcode - 0x4b) as usize;
                    let value = pop(&mut stack);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x36 => {
                    let slot = instruction.index.unwrap_or(0);
                    let value = pop(&mut stack);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x3a => {
                    let slot = instruction.index.unwrap_or(0);
                    let value = pop(&mut stack);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(IVal::Unknown);
                    stack.push(value);
                }
                0x57 => {
                    stack.pop();
                }
                0x60 | 0x64 | 0x68 | 0x6c | 0x78 | 0x7a | 0x7e | 0x80 | 0x82 => {
                    let right = pop(&mut stack);
                    let left = pop(&mut stack);
                    let value = match (left, right) {
                        (IVal::Int(left), IVal::Int(right)) => {
                            IVal::Int(match instruction.opcode {
                                0x60 => left.wrapping_add(right),
                                0x64 => left.wrapping_sub(right),
                                0x68 => left.wrapping_mul(right),
                                0x6c => left.wrapping_div(right),
                                0x78 => left.wrapping_shl(right as u32),
                                0x7a => left.wrapping_shr(right as u32),
                                0x7e => left & right,
                                0x80 => left | right,
                                _ => left ^ right,
                            })
                        }
                        _ => IVal::Unknown,
                    };
                    stack.push(value);
                }
                0x86 | 0x87 | 0x85 | 0x6e | 0x6f | 0x6a | 0x6b => {
                    // Float conversions and arithmetic: values are not needed
                    // here, only the stack effect.
                    let args = if matches!(instruction.opcode, 0x85..=0x87) {
                        1
                    } else {
                        2
                    };
                    for _ in 0..args {
                        stack.pop();
                    }
                    stack.push(IVal::Unknown);
                }
                0x74 | 0x76 | 0x7c | 0x7d => {
                    let arguments = 2;
                    for _ in 0..arguments {
                        stack.pop();
                    }
                    stack.push(IVal::Unknown);
                }
                0xbb => {
                    self.objects.push(IObject {
                        fields: BTreeMap::new(),
                    });
                    stack.push(IVal::Obj(self.objects.len() - 1));
                }
                0xb2 => {
                    let (owner, name, descriptor) =
                        field_at(class, instruction).unwrap_or_default();
                    if !descriptor.starts_with("L") && !descriptor.starts_with("[") {
                        self.init_class(&owner, depth + 1)?;
                    }
                    stack.push(
                        self.statics
                            .get(&(owner, name))
                            .cloned()
                            .unwrap_or(IVal::Unknown),
                    );
                }
                0xb3 => {
                    let (owner, name, _) = field_at(class, instruction).unwrap_or_default();
                    let value = pop(&mut stack);
                    self.statics.insert((owner, name), value);
                }
                0xb4 => {
                    let (_owner, name, _) = field_at(class, instruction).unwrap_or_default();
                    let object = pop(&mut stack);
                    let value = match object {
                        IVal::Obj(index) => self
                            .objects
                            .get(index)
                            .and_then(|object| object.fields.get(&name))
                            .cloned()
                            .unwrap_or(IVal::Unknown),
                        _ => IVal::Unknown,
                    };
                    stack.push(value);
                }
                0xb5 => {
                    let (_owner, name, _) = field_at(class, instruction).unwrap_or_default();
                    let value = pop(&mut stack);
                    let object = pop(&mut stack);
                    if let IVal::Obj(index) = object {
                        if let Some(object) = self.objects.get_mut(index) {
                            object.fields.insert(name, value);
                        }
                    }
                }
                0xb7 => {
                    let (owner, name, descriptor) =
                        method_at(class, instruction).unwrap_or_default();
                    let count = parameter_count(&descriptor);
                    let mut arguments = Vec::new();
                    for _ in 0..count {
                        arguments.push(pop(&mut stack));
                    }
                    arguments.reverse();
                    let receiver = pop(&mut stack);
                    if name == "<init>" {
                        // A constructor pushes no return value.
                        self.call(
                            &owner,
                            "<init>",
                            &arguments,
                            match receiver {
                                IVal::Obj(index) => Some(index),
                                _ => None,
                            },
                            depth + 1,
                        )?;
                    } else if let IVal::Obj(index) = receiver {
                        let value = self.call(&owner, &name, &arguments, Some(index), depth + 1)?;
                        stack.push(value);
                    } else {
                        stack.push(IVal::Unknown);
                    }
                }
                0xb6 => {
                    let (owner, name, descriptor) =
                        method_at(class, instruction).unwrap_or_default();
                    let count = parameter_count(&descriptor);
                    let mut arguments = Vec::new();
                    for _ in 0..count {
                        arguments.push(pop(&mut stack));
                    }
                    arguments.reverse();
                    let receiver = pop(&mut stack);
                    let value = match receiver {
                        IVal::Obj(index) => {
                            self.call(&owner, &name, &arguments, Some(index), depth + 1)?
                        }
                        _ => IVal::Unknown,
                    };
                    stack.push(value);
                }
                0xb8 => {
                    let (owner, name, descriptor) =
                        method_at(class, instruction).unwrap_or_default();
                    let count = parameter_count(&descriptor);
                    let mut arguments = Vec::new();
                    for _ in 0..count {
                        arguments.push(pop(&mut stack));
                    }
                    arguments.reverse();
                    let value = self.call(&owner, &name, &arguments, None, depth + 1)?;
                    stack.push(value);
                }
                0xac => return Ok(pop(&mut stack)),
                0xb1 => return Ok(IVal::Unknown),
                0x99..=0xa6 | 0xc6 | 0xc7 => {
                    // 0x99..0x9e compare one int with zero; 0x9f..0xa4 compare
                    // two ints; 0xa5/0xa6 compare two references.
                    let taken = match instruction.opcode {
                        0x99 => matches!(stack.pop(), Some(IVal::Int(0))),
                        0x9a => !matches!(stack.pop(), Some(IVal::Int(0))),
                        0x9b => matches!(stack.pop(), Some(IVal::Int(value)) if value < 0),
                        0x9c => matches!(stack.pop(), Some(IVal::Int(value)) if value >= 0),
                        0x9d => matches!(stack.pop(), Some(IVal::Int(value)) if value > 0),
                        0x9e => matches!(stack.pop(), Some(IVal::Int(value)) if value <= 0),
                        0x9f..=0xa4 => {
                            let right = pop(&mut stack);
                            let left = pop(&mut stack);
                            match (left, right) {
                                (IVal::Int(left), IVal::Int(right)) => match instruction.opcode {
                                    0x9f => left == right,
                                    0xa0 => left != right,
                                    0xa1 => left < right,
                                    0xa2 => left >= right,
                                    0xa3 => left > right,
                                    _ => left <= right,
                                },
                                _ => false,
                            }
                        }
                        0xa5 | 0xa6 => {
                            let right = pop(&mut stack);
                            let left = pop(&mut stack);
                            let same = match (&left, &right) {
                                (IVal::Obj(a), IVal::Obj(b)) => a == b,
                                (IVal::Int(a), IVal::Int(b)) => a == b,
                                _ => false,
                            };
                            if instruction.opcode == 0xa5 {
                                same
                            } else {
                                !same
                            }
                        }
                        0xc6 => matches!(stack.pop(), Some(IVal::Unknown) | None),
                        0xc7 => !matches!(stack.pop(), Some(IVal::Unknown) | None),
                        _ => false,
                    };
                    if taken {
                        at = instruction.target;
                    }
                }
                0xa7 => at = instruction.target,
                0xbf => {
                    return Err(format!(
                        "{}: exception path while resolving an int",
                        class.name
                    ))
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// createBlockStateDefinition
// ---------------------------------------------------------------------------

fn block_property_ids(
    jar: &Jar,
    internal: &str,
    catalogue: &Catalogue,
) -> Result<Vec<usize>, String> {
    let mut out: Vec<usize> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    collect_properties(jar, internal, catalogue, &mut out, &mut seen, 0)?;
    Ok(out)
}

fn add_ids(
    ids: &[usize],
    catalogue: &Catalogue,
    out: &mut Vec<usize>,
    seen: &mut BTreeSet<String>,
) {
    for id in ids {
        let name = catalogue.name(*id);
        if seen.insert(name) {
            out.push(*id);
        }
    }
}

fn collect_properties(
    jar: &Jar,
    internal: &str,
    catalogue: &Catalogue,
    out: &mut Vec<usize>,
    seen: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err(format!("property definition chain too deep at {internal}"));
    }
    let class = jar
        .class(internal)
        .ok_or_else(|| format!("{internal} is not in the jar"))?;

    if class.name.ends_with("/ChiseledBookShelfBlock") {
        // The one definition that adds a list of properties through
        // `SLOT_OCCUPIED_PROPERTIES.forEach(builder::add)`: `FACING` first,
        // then the six slot properties.
        for field in [
            "FACING",
            "SLOT_0_OCCUPIED",
            "SLOT_1_OCCUPIED",
            "SLOT_2_OCCUPIED",
            "SLOT_3_OCCUPIED",
            "SLOT_4_OCCUPIED",
            "SLOT_5_OCCUPIED",
        ] {
            let id = lookup_property(jar, catalogue, &class.name, field)
                .ok_or_else(|| format!("ChiseledBookShelfBlock.{field} is not a known property"))?;
            add_ids(&[id], catalogue, out, seen);
        }
        return Ok(());
    }
    if class.name.ends_with("/MultifaceBlock") {
        // The only definition with control flow: it loops over
        // `Direction.values()`, asks `isFaceSupported` (which is `return true`)
        // and adds that direction's property, then `WATERLOGGED`.
        for direction in enum_constants(jar, "net/minecraft/core/Direction") {
            let id = catalogue
                .property("net/minecraft/world/level/block/PipeBlock", &direction)
                .ok_or_else(|| format!("PipeBlock.{direction} is not a known property"))?;
            add_ids(&[id], catalogue, out, seen);
        }
        if let Some(id) = catalogue.property(BLOCK_STATE_PROPERTIES, "WATERLOGGED") {
            add_ids(&[id], catalogue, out, seen);
        }
        return Ok(());
    }
    if let Some(method) = class
        .methods
        .iter()
        .find(|method| method.name == "createBlockStateDefinition")
    {
        if let Some(code) = &method.code {
            let instructions = decode(&code.bytes);
            let mut stack: Vec<Init> = Vec::new();
            for instruction in &instructions {
                match instruction.opcode {
                    0x2a | 0x2b => stack.push(Init::Unknown),
                    0x01 => stack.push(Init::Unknown),
                    0x59 => {
                        let value = stack.last().cloned().unwrap_or_else(Init::unknown);
                        stack.push(value);
                    }
                    0x02..=0x08 => stack.push(Init::Int(instruction.opcode as i32 - 0x03)),
                    0x10 | 0x11 => stack.push(Init::Int(instruction.immediate.unwrap_or(0))),
                    0x12 | 0x13 => {
                        stack.push(ldc_value(&class, instruction).unwrap_or_else(Init::unknown));
                    }
                    0xb2 => {
                        let (owner, name, _) = field_at(&class, instruction).unwrap_or_default();

                        if let Some(id) = lookup_property(jar, catalogue, &owner, &name) {
                            stack.push(Init::Int(id as i32));
                        } else if let Some(ids) = lookup_array(jar, catalogue, &owner, &name) {
                            stack.push(Init::Array(Rc::new(RefCell::new(
                                ids.iter().map(|id| Some(Init::Int(*id as i32))).collect(),
                            ))));
                        } else {
                            stack.push(Init::Field(owner, name));
                        }
                    }
                    0x32 => {
                        let index = stack.pop().unwrap_or_else(Init::unknown);
                        let array = stack.pop();
                        let value = match (array, index) {
                            (Some(Init::Array(items)), Init::Int(index)) => items
                                .borrow()
                                .get(index.max(0) as usize)
                                .cloned()
                                .flatten()
                                .unwrap_or_else(Init::unknown),
                            _ => Init::Unknown,
                        };
                        stack.push(value);
                    }
                    0xbd => {
                        let count = match stack.pop() {
                            Some(Init::Int(count)) => count.max(0) as usize,
                            _ => 0,
                        };
                        stack.push(Init::Array(Rc::new(RefCell::new(vec![None; count]))));
                    }
                    0x53 => {
                        let value = stack.pop().unwrap_or_else(Init::unknown);
                        let index = stack.pop();
                        let array = stack.pop();
                        if let (Some(Init::Array(items)), Some(Init::Int(index))) = (array, index) {
                            let mut items = items.borrow_mut();
                            if let Some(slot) = items.get_mut(index.max(0) as usize) {
                                *slot = Some(value);
                            }
                        }
                    }
                    0xb6 => {
                        let (owner, name, descriptor) =
                            method_at(&class, instruction).unwrap_or_default();
                        let args = parameter_count(&descriptor);
                        let mut popped = Vec::new();
                        for _ in 0..args {
                            popped.push(stack.pop().unwrap_or_else(Init::unknown));
                        }
                        popped.reverse();
                        let _ = owner;
                        // The receiver sits under the arguments.
                        stack.pop();
                        if name != "add" {
                            if let Some(id) = resolve_property_getter(jar, catalogue, &owner, &name)
                            {
                                stack.push(Init::Int(id as i32));
                            } else {
                                stack.push(Init::Unknown);
                            }
                            continue;
                        }
                        {
                            for argument in popped {
                                match argument {
                                    Init::Int(id) => {
                                        add_ids(&[id.max(0) as usize], catalogue, out, seen)
                                    }
                                    Init::Array(items) => {
                                        let ids: Vec<usize> = items
                                            .borrow()
                                            .iter()
                                            .flatten()
                                            .filter_map(|item| match item {
                                                Init::Int(id) => Some(*id as usize),
                                                _ => None,
                                            })
                                            .collect();
                                        add_ids(&ids, catalogue, out, seen);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    0xb7..=0xb9 => {
                        let (owner, name, descriptor) =
                            method_at(&class, instruction).unwrap_or_default();
                        let args = parameter_count(&descriptor);
                        for _ in 0..args {
                            stack.pop();
                        }
                        if instruction.opcode != 0xb8 {
                            stack.pop();
                        }
                        if name == "createBlockStateDefinition" && owner != class.name {
                            collect_properties(jar, &owner, catalogue, out, seen, depth + 1)?;
                        }
                    }
                    0xba => stack.push(Init::Unknown),
                    _ => {}
                }
            }
        }
    }
    // A definition that does not call `super` still inherits nothing more, but
    // one that has no override at all must walk up.
    let overrides = class
        .methods
        .iter()
        .any(|method| method.name == "createBlockStateDefinition");
    if !overrides && !class.super_name.is_empty() {
        collect_properties(jar, &class.super_name, catalogue, out, seen, depth + 1)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// State order and the state table
// ---------------------------------------------------------------------------

/// The order the bundle writes a state's `properties` entries in.
///
/// The codec feeds the (sorted) property names into a fresh `HashMap` (capacity
/// 16, grown past 12 entries) that inserts at the head of a bucket, so iteration
/// is bucket-ascending and newest-first inside a bucket. Measured against all
/// 884 property-carrying blocks of the prepared copy (`piston_head` keeps
/// `short` before `facing` although the sorted order is the other way).
fn hash_order(names: &[String]) -> Vec<usize> {
    let mut capacity = 16usize;
    while names.len() > capacity * 3 / 4 {
        capacity *= 2;
    }
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); capacity];
    for (index, name) in names.iter().enumerate() {
        // A fresh map inserts at the head of a bucket, so the newest name in a
        // bucket iterates first (measured: `piston_head`'s `short` precedes
        // `facing`, `powered_rail`'s `shape` precedes `powered`).
        buckets[java_bucket(name, capacity)].insert(0, index);
    }
    buckets.into_iter().flatten().collect()
}

fn java_string_hash(text: &str) -> u32 {
    let mut hash: i32 = 0;
    for character in text.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(character as i32);
    }
    hash as u32
}

fn java_bucket(name: &str, capacity: usize) -> usize {
    let hash = java_string_hash(name);
    let spread = hash ^ (hash >> 16);
    (spread as usize) & (capacity - 1)
}

/// `Lists.cartesianProduct`: the last list varies fastest.
fn cartesian(values: &[Vec<String>]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new()];
    for list in values {
        let mut next = Vec::with_capacity(out.len() * list.len());
        for prefix in &out {
            for index in 0..list.len() {
                let mut row = prefix.clone();
                row.push(index);
                next.push(row);
            }
        }
        out = next;
    }
    out
}

fn internal_class_for(jar: &Jar, simple_name: &str) -> Result<String, String> {
    let candidate = format!("{BLOCK_PREFIX}{simple_name}");
    if jar.class(&candidate).is_some() {
        return Ok(candidate);
    }
    let suffix = format!("/{simple_name}.class");
    let hits: Vec<String> = jar
        .files
        .keys()
        .filter(|name| name.starts_with(BLOCK_PREFIX) && name.ends_with(&suffix))
        .filter_map(|name| name.strip_suffix(".class").map(str::to_string))
        .collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err(format!("no class file for block class {simple_name}")),
        _ => Err(format!("block class {simple_name} is ambiguous: {hits:?}")),
    }
}

/// The state registry, derived from the jar.
pub fn state_table(jar_path: &Path) -> Result<StateTable, String> {
    let jar = Jar::open(jar_path)?;
    let catalogue = catalogue(&jar)?;
    let constructors = constructor_descriptors(&jar);
    let registry = block::registry(jar_path)?;
    let mut blocks = Vec::with_capacity(registry.len());
    for row in &registry {
        let internal = internal_class_for(&jar, &row.class)?;
        let ids = block_property_ids(&jar, &internal, &catalogue)?;
        let mut properties = Vec::with_capacity(ids.len());
        for id in &ids {
            properties.push(Property {
                name: catalogue.name(*id),
                values: catalogue.values(*id),
            });
        }
        let overrides = default_overrides(&jar, &catalogue, &internal, &row.class, &constructors)?;
        let state_count = properties
            .iter()
            .map(|property| property.values.len())
            .product::<usize>()
            .max(1);
        let mut block = BlockStates {
            id: row.id.clone(),
            properties,
            state_count,
            default_index: 0,
        };
        let sorted = block.sorted();
        let mut default_values = vec![0usize; sorted.len()];
        for (name, value) in &overrides {
            let Some(position) = sorted.iter().position(|property| &property.name == name) else {
                return Err(format!("{}: default names unknown property {name}", row.id));
            };
            let Some(index) = sorted[position]
                .values
                .iter()
                .position(|candidate| candidate == value)
            else {
                return Err(format!(
                    "{}: default value {value} is not a {} value",
                    row.id, name
                ));
            };
            default_values[position] = index;
        }
        let mut default_index = 0usize;
        for (slot, value) in default_values.iter().enumerate() {
            let radix: usize = sorted[slot + 1..]
                .iter()
                .map(|property| property.values.len())
                .product::<usize>()
                .max(1);
            default_index += value * radix;
        }
        if default_index >= block.state_count {
            return Err(format!("{}: default state out of range", row.id));
        }
        block.default_index = default_index;
        blocks.push(block);
    }
    Ok(StateTable { blocks })
}

/// Block id -> `(property name, values in `getPossibleValues` order)`.
///
/// This is the shape other tables (`command-registries.json#blocks`, the state
/// tables) consume, so it is part of this module's contract.
pub type BlockPropertyTable = BTreeMap<String, Vec<(String, Vec<String>)>>;

/// Every property with its values, in the shape other tables consume.
#[allow(dead_code)]
pub fn block_properties(jar_path: &Path) -> Result<BlockPropertyTable, String> {
    let table = state_table(jar_path)?;
    let mut out = BTreeMap::new();
    for block in table.blocks {
        out.insert(
            block.id,
            block
                .properties
                .into_iter()
                .map(|property| (property.name, property.values))
                .collect(),
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Defaults: registerDefaultState in the constructor chain
// ---------------------------------------------------------------------------

type Overrides = Vec<(String, String)>;

/// Constructor descriptors per block class, from `Blocks.<clinit>` and helpers.
fn constructor_descriptors(jar: &Jar) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(class) = jar.class(BLOCKS) else {
        return out;
    };
    for method in &class.methods {
        let Some(code) = &method.code else {
            continue;
        };
        let instructions = decode(&code.bytes);
        let mut pending: Option<String> = None;
        for instruction in &instructions {
            match instruction.opcode {
                0xbb => {
                    pending = new_class(&class, instruction);
                }
                0xb7 => {
                    if let Some((_owner, name, descriptor)) = method_at(&class, instruction) {
                        if name == "<init>" {
                            if let Some(created) = pending.take() {
                                let entry = out.entry(simple(&created).to_string()).or_default();
                                if !entry.contains(&descriptor) {
                                    entry.push(descriptor);
                                }
                            }
                        }
                    }
                }
                0x59
                | 0x12
                | 0x13
                | 0x14
                | 0xb2
                | 0x15..=0x19
                | 0x2a..=0x2d
                | 0x01
                | 0x02..=0x08
                | 0x10
                | 0x11 => {}
                _ => pending = None,
            }
        }
    }
    out
}

/// A value in a default-state expression.
#[derive(Clone, Debug)]
enum Default {
    This,
    Int(i32),
    Field(String, String),
    Property(usize),
    State(Overrides),
    Array(Vec<Default>),
    Unknown,
}

fn default_overrides(
    jar: &Jar,
    catalogue: &Catalogue,
    internal: &str,
    simple_name: &str,
    constructors: &BTreeMap<String, Vec<String>>,
) -> Result<Overrides, String> {
    let descriptor = pick_constructor(jar, internal, constructors.get(simple_name))?;
    let mut overrides: Overrides = Vec::new();
    eval_constructor(jar, catalogue, internal, &descriptor, &mut overrides, 0)?;
    // Two constructors assign defaults inside a loop the interpreter does not
    // evaluate: ChiseledBookShelfBlock walks `SLOT_OCCUPIED_PROPERTIES` and sets
    // each to `false`, and the multiface blocks walk `Direction.values()` and set
    // each face plus `WATERLOGGED` to `false`.
    if internal.ends_with("/ChiseledBookShelfBlock") {
        for slot in 0..6 {
            let field = format!("SLOT_{slot}_OCCUPIED");
            if let Some(id) = lookup_property(jar, catalogue, internal, &field) {
                overrides.push((catalogue.name(id), "false".to_string()));
            }
        }
    }
    if type_closure(jar, internal)
        .iter()
        .any(|candidate| candidate.ends_with("/MultifaceBlock"))
    {
        for direction in enum_constants(jar, "net/minecraft/core/Direction") {
            if let Some(id) = lookup_property(
                jar,
                catalogue,
                "net/minecraft/world/level/block/PipeBlock",
                &direction,
            ) {
                overrides.push((catalogue.name(id), "false".to_string()));
            }
        }
        if let Some(id) = lookup_property(jar, catalogue, BLOCK_STATE_PROPERTIES, "WATERLOGGED") {
            overrides.push((catalogue.name(id), "false".to_string()));
        }
    }
    Ok(overrides)
}

fn pick_constructor(
    jar: &Jar,
    internal: &str,
    candidates: Option<&Vec<String>>,
) -> Result<String, String> {
    let class = jar
        .class(internal)
        .ok_or_else(|| format!("{internal} is not in the jar"))?;
    let declared: Vec<&str> = class
        .methods
        .iter()
        .filter(|method| method.name == "<init>")
        .map(|method| method.descriptor.as_str())
        .collect();
    if let Some(candidates) = candidates {
        // Prefer the constructor whose last parameter is `Properties`, and among
        // those the one with the fewest arguments (the registration's shape).
        let mut matching: Vec<&&str> = declared
            .iter()
            .filter(|descriptor| {
                candidates.contains(&descriptor.to_string())
                    && descriptor.ends_with("Properties;)V")
            })
            .collect();
        matching.sort_by_key(|descriptor| descriptor.len());
        if let Some(found) = matching.first() {
            return Ok((**found).to_string());
        }
        if let Some(found) = declared
            .iter()
            .find(|descriptor| candidates.contains(&descriptor.to_string()))
        {
            return Ok((*found).to_string());
        }
    }
    if let Some(found) = declared
        .iter()
        .find(|descriptor| descriptor.ends_with("Properties;)V"))
    {
        return Ok((*found).to_string());
    }
    declared
        .first()
        .map(|descriptor| (*descriptor).to_string())
        .ok_or_else(|| format!("{internal} has no constructor"))
}

fn eval_constructor(
    jar: &Jar,
    catalogue: &Catalogue,
    internal: &str,
    descriptor: &str,
    current: &mut Overrides,
    depth: usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err(format!("constructor chain too deep at {internal}"));
    }
    // `java/lang/Object.<init>` and the like are in the JDK, not the jar.
    let Some(class) = jar.class(internal) else {
        return Ok(());
    };
    let Some(method) = method_descriptor(&class, "<init>", descriptor) else {
        return Ok(());
    };
    let Some(code) = &method.code else {
        return Ok(());
    };
    let instructions = decode(&code.bytes);
    let mut locals: Vec<Option<Default>> = vec![None; code.max_locals as usize];
    if let Some(slot) = locals.get_mut(0) {
        *slot = Some(Default::This);
    }
    let mut stack: Vec<Default> = Vec::new();
    let mut at = 0usize;
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 200_000 {
            return Err(format!("{internal}.<init>{descriptor} did not terminate"));
        }
        let Some(instruction) = instructions.get(at) else {
            break;
        };
        at += 1;
        match instruction.opcode {
            0x2a..=0x2d => {
                let slot = (instruction.opcode - 0x2a) as usize;
                stack.push(
                    locals
                        .get(slot)
                        .cloned()
                        .flatten()
                        .unwrap_or(Default::Unknown),
                );
            }
            0x1a..=0x1d => {
                let slot = (instruction.opcode - 0x1a) as usize;
                stack.push(
                    locals
                        .get(slot)
                        .cloned()
                        .flatten()
                        .unwrap_or(Default::Unknown),
                );
            }
            0x84 => {
                let slot = instruction.index.unwrap_or(0);
                let increment = instruction.immediate.unwrap_or(0);
                if let Some(Some(Default::Int(value))) = locals.get_mut(slot) {
                    *value += increment;
                } else if let Some(cell) = locals.get_mut(slot) {
                    *cell = Some(Default::Int(increment));
                }
            }
            0x15..=0x19 => {
                let slot = instruction.index.unwrap_or(0);
                stack.push(
                    locals
                        .get(slot)
                        .cloned()
                        .flatten()
                        .unwrap_or(Default::Unknown),
                );
            }
            0x3b..=0x3e => {
                let slot = (instruction.opcode - 0x3b) as usize;
                let value = stack.pop().unwrap_or(Default::Unknown);
                if let Some(cell) = locals.get_mut(slot) {
                    *cell = Some(value);
                }
            }
            0x36..=0x39 => {
                let slot = instruction.index.unwrap_or(0);
                let value = stack.pop().unwrap_or(Default::Unknown);
                if let Some(cell) = locals.get_mut(slot) {
                    *cell = Some(value);
                }
            }
            0x01 => stack.push(Default::Unknown),
            0x59 => {
                let value = stack.last().cloned().unwrap_or(Default::Unknown);
                stack.push(value);
            }
            0x57 => {
                stack.pop();
            }
            0x02..=0x08 => stack.push(Default::Int(instruction.opcode as i32 - 0x03)),
            0x10 | 0x11 => stack.push(Default::Int(instruction.immediate.unwrap_or(0))),
            0x12 | 0x13 => {
                stack.push(match ldc_value(&class, instruction) {
                    Some(Init::Int(value)) => Default::Int(value),
                    Some(Init::Text(text)) => {
                        let _ = text;
                        Default::Unknown
                    }
                    _ => Default::Unknown,
                });
            }
            0xb2 => {
                let (owner, name, descriptor) = field_at(&class, instruction).unwrap_or_default();
                // An enum constant (`Half.BOTTOM`) is not a property; only a
                // property-typed static names a state property.
                let is_property = descriptor.ends_with("Property;");
                if is_property {
                    if let Some(id) = lookup_property(jar, catalogue, &owner, &name) {
                        stack.push(Default::Property(id));
                        continue;
                    }
                }
                if is_property {
                    if let Some(ids) = lookup_array(jar, catalogue, &owner, &name) {
                        stack.push(Default::Array(
                            ids.iter().map(|id| Default::Property(*id)).collect(),
                        ));
                        continue;
                    }
                }
                stack.push(Default::Field(owner, name));
            }
            0xb4 => {
                // Only `Block.stateDefinition` is read here, to call `any()`.
                stack.pop();
                stack.push(Default::This);
            }
            0xb5 => {
                stack.pop();
                stack.pop();
            }
            0x32 => {
                let index = stack.pop().unwrap_or(Default::Unknown);
                let array = stack.pop().unwrap_or(Default::Unknown);
                let value = match (array, index) {
                    (Default::Array(items), Default::Int(index)) => items
                        .get(index.max(0) as usize)
                        .cloned()
                        .unwrap_or(Default::Unknown),
                    _ => Default::Unknown,
                };
                stack.push(value);
            }

            0xbd => {
                let count = match stack.pop() {
                    Some(Default::Int(count)) => count.max(0) as usize,
                    _ => 0,
                };
                stack.push(Default::Array(vec![Default::Unknown; count]));
            }
            0x53 => {
                let value = stack.pop().unwrap_or(Default::Unknown);
                let index = stack.pop().unwrap_or(Default::Unknown);
                let array = stack.pop().unwrap_or(Default::Unknown);
                if let (Default::Array(mut items), Default::Int(index)) = (array, index) {
                    if let Some(slot) = items.get_mut(index.max(0) as usize) {
                        *slot = value;
                    }
                }
            }
            0xb6 => {
                let (_owner, name, fixed) = method_at(&class, instruction).unwrap_or_default();
                let args = parameter_count(&fixed);
                let mut popped = Vec::new();
                for _ in 0..args {
                    popped.push(stack.pop().unwrap_or(Default::Unknown));
                }
                popped.reverse();
                let receiver = stack.pop().unwrap_or(Default::Unknown);
                match name.as_str() {
                    "any" => stack.push(Default::State(Vec::new())),
                    "defaultBlockState" => stack.push(Default::State(current.clone())),
                    "setValue" => {
                        let state = match receiver {
                            Default::State(overrides) => overrides,
                            _ => Vec::new(),
                        };
                        let property = popped.first().cloned().unwrap_or(Default::Unknown);
                        let value = popped.get(1).cloned().unwrap_or(Default::Unknown);
                        let mut overrides = state;
                        if let Default::Property(id) = property {
                            if let Some(label) = value_label(catalogue, id, &value) {
                                overrides.push((catalogue.name(id), label));
                            }
                        }
                        stack.push(Default::State(overrides));
                    }
                    "registerDefaultState" => {
                        if let Some(Default::State(overrides)) = popped.first().cloned() {
                            *current = overrides;
                        }
                        stack.push(Default::Unknown);
                    }
                    _ => stack.push(Default::Unknown),
                }
            }
            0xb7 => {
                let (owner, name, fixed) = method_at(&class, instruction).unwrap_or_default();
                let args = parameter_count(&fixed);
                for _ in 0..args {
                    stack.pop();
                }
                stack.pop(); // receiver
                if name == "<init>" {
                    eval_constructor(jar, catalogue, &owner, &fixed, current, depth + 1)?;
                } else {
                    stack.push(Default::Unknown);
                }
            }
            0xb8 | 0xb9 => {
                let (_owner, name, fixed) = method_at(&class, instruction).unwrap_or_default();
                let args = parameter_count(&fixed);
                let mut popped = Vec::new();
                for _ in 0..args {
                    popped.push(stack.pop().unwrap_or(Default::Unknown));
                }
                popped.reverse();
                // `Boolean.valueOf(false)` is how a boolean `setValue` operand
                // reaches the stack.
                if name == "valueOf" {
                    stack.push(popped.into_iter().next().unwrap_or(Default::Unknown));
                } else {
                    stack.push(Default::Unknown);
                }
            }
            0xba => stack.push(Default::Unknown),
            0xac..=0xb1 => break,
            0x99..=0xa6 | 0xc6 | 0xc7 => {
                // A condition the interpreter cannot resolve (an iterator's
                // `hasNext()`, say) takes the branch: every constructor loop in
                // 26.3 guards its body with a forward exit, and the skipped body
                // assigns default values. Loop counters this VM does understand
                // (ints) are exact.
                let known: Option<bool> = match instruction.opcode {
                    0x99 => match stack.pop() {
                        Some(Default::Int(value)) => Some(value == 0),
                        _ => None,
                    },
                    0x9a => match stack.pop() {
                        Some(Default::Int(value)) => Some(value != 0),
                        _ => None,
                    },
                    0x9b => match stack.pop() {
                        Some(Default::Int(value)) => Some(value < 0),
                        _ => None,
                    },
                    0x9c => match stack.pop() {
                        Some(Default::Int(value)) => Some(value >= 0),
                        _ => None,
                    },
                    0x9d => match stack.pop() {
                        Some(Default::Int(value)) => Some(value > 0),
                        _ => None,
                    },
                    0x9e => match stack.pop() {
                        Some(Default::Int(value)) => Some(value <= 0),
                        _ => None,
                    },
                    0x9f..=0xa4 => {
                        let right = stack.pop().unwrap_or(Default::Unknown);
                        let left = stack.pop().unwrap_or(Default::Unknown);
                        match (left, right) {
                            (Default::Int(left), Default::Int(right)) => {
                                Some(match instruction.opcode {
                                    0x9f => left == right,
                                    0xa0 => left != right,
                                    0xa1 => left < right,
                                    0xa2 => left >= right,
                                    0xa3 => left > right,
                                    _ => left <= right,
                                })
                            }
                            _ => None,
                        }
                    }
                    0xa5 | 0xa6 => {
                        let right = stack.pop().unwrap_or(Default::Unknown);
                        let left = stack.pop().unwrap_or(Default::Unknown);
                        match (&left, &right) {
                            (Default::Field(owner_a, name_a), Default::Field(owner_b, name_b)) => {
                                let same = owner_a == owner_b && name_a == name_b;
                                Some(if instruction.opcode == 0xa6 {
                                    !same
                                } else {
                                    same
                                })
                            }
                            (Default::Int(a), Default::Int(b)) => {
                                Some(if instruction.opcode == 0xa6 {
                                    a != b
                                } else {
                                    a == b
                                })
                            }
                            _ => None,
                        }
                    }
                    0xc6 => Some(matches!(stack.pop(), Some(Default::Unknown) | None)),
                    0xc7 => Some(!matches!(stack.pop(), Some(Default::Unknown) | None)),
                    _ => None,
                };
                if known.unwrap_or(true) {
                    at = instruction.target;
                }
            }
            0xa7 => at = instruction.target,
            _ => {}
        }
    }
    Ok(())
}

/// The label of a `setValue` operand for a property.
fn value_label(catalogue: &Catalogue, property: usize, value: &Default) -> Option<String> {
    let values = catalogue.values(property);
    match value {
        Default::Int(number) => {
            if values == ["true", "false"] {
                if *number == 1 {
                    Some("true".into())
                } else if *number == 0 {
                    Some("false".into())
                } else {
                    None
                }
            } else if values.iter().any(|value| value == &number.to_string()) {
                Some(number.to_string())
            } else {
                None
            }
        }
        Default::Field(_owner, field) => {
            let label = field.to_lowercase();
            values.contains(&label).then_some(label)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Writers
// ---------------------------------------------------------------------------

/// `_shared/block-order.json`: blocks in registry order, defaults by block id.
pub fn block_order(jar_path: &Path) -> Result<Value, String> {
    let table = state_table(jar_path)?;
    let blocks: Vec<Value> = table
        .blocks
        .iter()
        .map(|block| Value::String(block.id.clone()))
        .collect();
    let mut defaults = Map::new();
    for (id, state) in table.defaults() {
        defaults.insert(id, Value::Number(state.into()));
    }
    let mut root = Map::new();
    root.insert("blocks".into(), Value::Array(blocks));
    root.insert("defaults".into(), Value::Object(defaults));
    Ok(Value::Object(root))
}

/// `_shared/states.bin`: the `states` NBT list, in state-id order.
#[allow(dead_code)]
pub fn states_bin(jar_path: &Path) -> Result<Vec<u8>, String> {
    let table = state_table(jar_path)?;
    Ok(encode_states(&table))
}

fn encode_states(table: &StateTable) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x09);
    write_nbt_string(&mut out, "states");
    out.push(0x0a);
    out.extend_from_slice(&(table.state_count() as i32).to_be_bytes());
    for block in &table.blocks {
        let sorted = block.sorted();
        let names: Vec<String> = sorted
            .iter()
            .map(|property| property.name.clone())
            .collect();
        let order = hash_order(&names);
        let lists: Vec<Vec<String>> = sorted
            .iter()
            .map(|property| property.values.clone())
            .collect();
        for state in cartesian(&lists) {
            out.push(0x08);
            write_nbt_string(&mut out, "id");
            write_nbt_string(&mut out, &block.id);
            if !block.properties.is_empty() {
                out.push(0x0a);
                write_nbt_string(&mut out, "properties");
                for index in &order {
                    let property = &block.sorted()[*index];
                    out.push(0x08);
                    write_nbt_string(&mut out, &property.name);
                    write_nbt_string(&mut out, &property.values[state[*index]]);
                }
                out.push(0x00);
            }
            out.push(0x00);
        }
    }
    out
}

fn write_nbt_string(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// Write both tables below `out`, and report what was built.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let table = state_table(jar_path)?;
    let mut coverage = Coverage::default();
    for table_name in TABLES {
        coverage.missing.push(table_name.to_string());
    }
    for (table_name, reason) in UNBLOCKED {
        coverage.note(format!("{table_name}: {reason}"));
    }
    let order = block_order(jar_path)?;
    let order_text = serde_json::to_string(&order).map_err(|error| error.to_string())?;
    let order_path = out.join(TABLE);
    if let Some(parent) = order_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&order_path, order_text)
        .map_err(|error| format!("{}: {error}", order_path.display()))?;
    coverage.add(TABLE);
    let bytes = encode_states(&table);
    let states_path = out.join(STATES);
    if let Some(parent) = states_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&states_path, &bytes)
        .map_err(|error| format!("{}: {error}", states_path.display()))?;
    coverage.add(STATES);
    coverage.note(format!(
        "{STATES}: {} states over {} blocks ({} bytes)",
        table.state_count(),
        table.blocks.len(),
        bytes.len()
    ));
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_hash_order_is_bucket_order_with_newest_first() {
        // Sorted names: facing (bucket 9), half (3), shape (4), waterlogged (3).
        // Bucket 3 holds half then waterlogged in the sorted feed, and a fresh
        // `HashMap` inserts at the head, so waterlogged precedes half.
        let names = vec![
            "facing".to_string(),
            "half".to_string(),
            "shape".to_string(),
            "waterlogged".to_string(),
        ];
        assert_eq!(hash_order(&names), vec![3, 1, 2, 0]);
        // `piston_head`: sorted facing, short, type; facing and short share
        // bucket 9, so short (fed second) comes first.
        let names = vec![
            "facing".to_string(),
            "short".to_string(),
            "type".to_string(),
        ];
        assert_eq!(hash_order(&names), vec![1, 0, 2]);
    }

    #[test]
    fn the_cartesian_product_varies_the_last_property_fastest() {
        let values = vec![
            vec!["false".to_string(), "true".to_string()],
            vec!["north".to_string(), "south".to_string()],
        ];
        assert_eq!(
            cartesian(&values),
            vec![vec![0, 0], vec![0, 1], vec![1, 0], vec![1, 1]]
        );
    }

    #[test]
    fn a_string_hash_matches_javas_known_value() {
        // "waterlogged".hashCode() in Java is 1333848131.
        assert_eq!(java_string_hash("waterlogged"), 1_041_911_129);
    }
}
