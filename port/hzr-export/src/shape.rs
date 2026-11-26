//! Horizon's block movement/properties tables, rebuilt from the player's own
//! `client.jar`.
//!
//! Horizon's bundle carries two small per-block tables next to `states.bin`:
//!
//! ```text
//! movement-properties.nbt          DataVersion, blocks[str -> {friction,
//!                                  speedFactor, jumpFactor, bounceRestitution,
//!                                  fallDistanceReduction, fenceGate}]
//! worldgen/blocks/block-properties.nbt
//!                                  DataVersion, blocks[str -> byte bitmask]
//! ```
//!
//! (`states` and the embedded `occlusion` were stripped into `_shared/`, so the
//! prepared copies this module reproduces are the stripped files.)
//!
//! # Where the values live
//!
//! Every value is a `BlockBehaviour.Properties` field or a class check:
//!
//! * `friction`/`speedFactor`/`jumpFactor`/`bounceRestitution`/
//!   `fallDistanceReduction` are private floats of `Properties`, defaulting to
//!   `0.6`, `1`, `1`, `0`, `0` (read from `Properties.<init>`), each set by the
//!   fluent call of the same name.
//! * `fenceGate` is `block instanceof FenceGateBlock`.
//! * The `block-properties` bitmask is `forceSolidOn | forceSolidOff<<1 |
//!   replaceable<<2 | liquid<<3 | canOcclude<<4`.
//!
//! `Blocks.<clinit>` builds one `Properties` per registration: directly as a
//! fluent chain, through the stair/slab/wall helpers (which copy a base block's
//! properties), or inside the colour/copper family factories. This module
//! replays that initialiser with a small interpreter over `Properties` values,
//! so nothing is transcribed.
//!
//! # What this module does not do
//!
//! The three shape tables (`cached-collision.nbt`, `occlusion.nbt`,
//! `cached-support.nbt`) carry per-state `VoxelShape` geometry that vanilla
//! computes in block code (`getShape`/`getCollisionShape` over a live
//! `BlockGetter`), not from data. The jar's blockstate/model assets reproduce
//! about 47% of state outlines (measured in `route3/AUDIT.md`), so they are not
//! part of this table; the shape dictionary format used by those files is
//! documented in [`shapes`] for a follow-up that owns the geometry engine.

#![allow(clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "../../hzr-bytecode/src/classfile.rs"]
#[allow(dead_code)]
mod classfile;
use classfile::{ClassFile, Constant};

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

/// What `ship.hzr3` calls the files this module writes.
pub const TABLE: &str = "worldgen/blocks/block-properties.nbt";
pub const MOVEMENT: &str = "movement-properties.nbt";

/// The stripped prepared copies this module reproduces.
pub const TABLES: [&str; 2] = [TABLE, MOVEMENT];

/// Two bytes of bitmask, so a `byte` writes back as the same byte.
pub const DATA_VERSION: i32 = 5023;

/// The shape tables this module documents but does not build.
pub const BLOCKED_TABLES: [(&str, &str); 3] = [
    (
        "cached-collision.nbt",
        "per-state `getCollisionShape` geometry from block code (`getShape`), not \
         data; the jar's model assets reproduce ~47% of state outlines",
    ),
    (
        "occlusion.nbt",
        "per-state `getOcclusionShape` geometry, same blocker and same 47% ceiling",
    ),
    (
        "worldgen/blocks/cached-support.nbt",
        "per-state `isFaceSturdy` geometry, same blocker and same 47% ceiling",
    ),
];

/// The block movement constants vanilla keeps on `BlockBehaviour`.
#[derive(Debug, Clone, PartialEq)]
pub struct Movement {
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    pub bounce_restitution: f32,
    pub fall_distance_reduction: f32,
    pub fence_gate: bool,
}

impl Default for Movement {
    fn default() -> Movement {
        Movement {
            friction: 0.6,
            speed_factor: 1.0,
            jump_factor: 1.0,
            bounce_restitution: 0.0,
            fall_distance_reduction: 0.0,
            fence_gate: false,
        }
    }
}

/// The `BlockBehaviour.Properties` fields the two tables need.
#[derive(Debug, Clone, PartialEq)]
pub struct Properties {
    pub movement: Movement,
    pub force_solid_on: bool,
    pub force_solid_off: bool,
    pub replaceable: bool,
    pub liquid: bool,
    pub can_occlude: bool,
}

impl Default for Properties {
    fn default() -> Properties {
        Properties {
            movement: Movement::default(),
            force_solid_on: false,
            force_solid_off: false,
            replaceable: false,
            liquid: false,
            can_occlude: true,
        }
    }
}

impl Properties {
    /// `forceSolidOn | forceSolidOff<<1 | replaceable<<2 | liquid<<3 | canOcclude<<4`.
    pub fn bitmask(&self) -> u8 {
        (self.force_solid_on as u8)
            | (self.force_solid_off as u8) << 1
            | (self.replaceable as u8) << 2
            | (self.liquid as u8) << 3
            | (self.can_occlude as u8) << 4
    }
}

/// The derived tables, in `BuiltInRegistries.BLOCK` order.
#[derive(Debug, Clone, PartialEq)]
pub struct Tables {
    pub blocks: Vec<(String, Properties)>,
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

const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const FENCE_GATE: &str = "net/minecraft/world/level/block/FenceGateBlock";

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
}

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
    target: usize,
}

fn decode(bytes: &[u8]) -> Vec<Instruction> {
    type Raw = (usize, u8, Option<usize>, Option<i32>, Option<i32>);
    let mut raw: Vec<Raw> = Vec::new();
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
            0x99..=0xa6 | 0xc6 | 0xc7 => {
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

#[allow(dead_code)]
fn simple(internal: &str) -> &str {
    internal.rsplit('/').next().unwrap_or(internal)
}

fn float_at(class: &ClassFile, instruction: &Instruction) -> Option<f32> {
    match instruction.opcode {
        0x0b..=0x0d => Some((instruction.opcode - 0x0b) as f32),
        0x11 | 0x10 => Some(instruction.immediate? as f32),
        0x12 | 0x13 => match class.constants.get(instruction.index?)? {
            Constant::Float(value) => Some(*value),
            Constant::Integer(value) => Some(*value as f32),
            _ => None,
        },
        _ => None,
    }
}

fn int_at(class: &ClassFile, instruction: &Instruction) -> Option<i32> {
    match instruction.opcode {
        0x02..=0x08 => Some(instruction.opcode as i32 - 0x03),
        0x10 | 0x11 => instruction.immediate,
        0x12 | 0x13 => match class.constants.get(instruction.index?)? {
            Constant::Integer(value) => Some(*value),
            _ => None,
        },
        _ => None,
    }
}

fn field_at(class: &ClassFile, instruction: &Instruction) -> Option<(String, String, String)> {
    if !(0xb2..=0xb5).contains(&instruction.opcode) {
        return None;
    }
    classfile::field_ref(&class.constants, instruction.index?)
}

fn method_at(class: &ClassFile, instruction: &Instruction) -> Option<(String, String, String)> {
    if !(0xb6..=0xb9).contains(&instruction.opcode) {
        return None;
    }
    classfile::method_ref(&class.constants, instruction.index?)
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

fn parameter_count(descriptor: &str) -> usize {
    classfile::descriptor_types(descriptor)
        .len()
        .saturating_sub(1)
}

// ---------------------------------------------------------------------------
// The `Blocks.<clinit>` interpreter
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum PVal {
    Properties(Properties),
    /// A registered block: its properties, which copies can start from.
    Block(Properties),
    /// A registered colour/copper family: its shared properties.
    Family(Properties),
    Id,
    Lambda(String, String),
    Float(f32),
    Int(i32),
    Unknown,
}

struct Vm<'a> {
    jar: &'a Jar,
    /// `Blocks.<field>` -> the properties of what the field holds.
    fields: BTreeMap<String, Properties>,
}

impl<'a> Vm<'a> {
    fn props_for_block(&self, field: &str) -> Option<Properties> {
        self.fields.get(field).cloned()
    }

    fn apply(&self, mut properties: Properties, name: &str, arguments: &[PVal]) -> Properties {
        let number = |index: usize| -> Option<f32> {
            match arguments.get(index) {
                Some(PVal::Float(value)) => Some(*value),
                Some(PVal::Int(value)) => Some(*value as f32),
                _ => None,
            }
        };
        match name {
            "friction" => {
                if let Some(value) = number(0) {
                    properties.movement.friction = value;
                }
            }
            "speedFactor" => {
                if let Some(value) = number(0) {
                    properties.movement.speed_factor = value;
                }
            }
            "jumpFactor" => {
                if let Some(value) = number(0) {
                    properties.movement.jump_factor = value;
                }
            }
            "bounceRestitution" => {
                if let Some(value) = number(0) {
                    properties.movement.bounce_restitution = value;
                }
            }
            "fallDistanceReduction" => {
                if let Some(value) = number(0) {
                    properties.movement.fall_distance_reduction = value;
                }
            }
            "forceSolidOn" => properties.force_solid_on = true,
            "forceSolidOff" => properties.force_solid_off = true,
            "replaceable" => properties.replaceable = true,
            "liquid" => properties.liquid = true,
            "noCollision" | "noOcclusion" => properties.can_occlude = false,
            _ => {}
        }
        properties
    }

    /// Evaluate a method that builds or returns `Properties`.
    fn eval_method(
        &mut self,
        class: &Rc<ClassFile>,
        method_name: &str,
        descriptor: &str,
        arguments: Vec<PVal>,
    ) -> PVal {
        let Some(method) = class
            .methods
            .iter()
            .find(|method| method.name == method_name && method.descriptor == descriptor)
        else {
            return PVal::Unknown;
        };
        let Some(code) = &method.code else {
            return PVal::Unknown;
        };
        let instructions = decode(&code.bytes);
        let mut locals: Vec<PVal> = vec![PVal::Unknown; code.max_locals as usize];
        let mut slot = 0usize;
        if method.access & 0x0008 == 0 {
            locals[0] = PVal::Unknown;
            slot = 1;
        }
        for argument in arguments {
            if let Some(cell) = locals.get_mut(slot) {
                *cell = argument;
            }
            slot += 1;
        }
        let mut stack: Vec<PVal> = Vec::new();
        let mut at = 0usize;
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 100_000 {
                return PVal::Unknown;
            }
            let Some(instruction) = instructions.get(at) else {
                return stack.pop().unwrap_or(PVal::Unknown);
            };
            at += 1;
            match instruction.opcode {
                0x2a..=0x2d => {
                    let slot = (instruction.opcode - 0x2a) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(PVal::Unknown));
                }
                0x15..=0x19 => {
                    let slot = instruction.index.unwrap_or(0);
                    stack.push(locals.get(slot).cloned().unwrap_or(PVal::Unknown));
                }
                0x3a..=0x3e => {
                    let slot = (instruction.opcode - 0x3a) as usize;
                    let value = stack.pop().unwrap_or(PVal::Unknown);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x36..=0x39 => {
                    let slot = instruction.index.unwrap_or(0);
                    let value = stack.pop().unwrap_or(PVal::Unknown);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(PVal::Unknown);
                    stack.push(value);
                }
                0x57 => {
                    stack.pop();
                }
                0x01 => stack.push(PVal::Unknown),
                0x02..=0x08 => stack.push(PVal::Int(instruction.opcode as i32 - 0x03)),
                0x0b..=0x0d => stack.push(PVal::Float((instruction.opcode - 0x0b) as f32)),
                0x10 | 0x11 => stack.push(PVal::Int(instruction.immediate.unwrap_or(0))),
                0x12 | 0x13 => match class.constants.get(instruction.index.unwrap_or(0)) {
                    Some(Constant::Float(value)) => stack.push(PVal::Float(*value)),
                    Some(Constant::Integer(value)) => stack.push(PVal::Int(*value)),
                    _ => stack.push(PVal::Unknown),
                },
                0xb2 => {
                    let (owner, name, descriptor) =
                        field_at(class, instruction).unwrap_or_default();
                    if owner == BLOCKS {
                        stack.push(match self.props_for_block(&name) {
                            Some(properties) => PVal::Block(properties),
                            None => PVal::Unknown,
                        });
                    } else if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                        stack.push(PVal::Id);
                    } else {
                        let _ = descriptor;
                        stack.push(PVal::Unknown);
                    }
                }
                0xb3 => {
                    let (owner, name, descriptor) =
                        field_at(class, instruction).unwrap_or_default();
                    let value = stack.pop().unwrap_or(PVal::Unknown);
                    if owner == BLOCKS && !descriptor.contains("Collection") {
                        if let PVal::Block(properties) = value {
                            self.fields.insert(name, properties);
                        }
                    }
                }
                0xba => match indy_target(class, instruction.index.unwrap_or(0)) {
                    Some((owner, name, _)) => stack.push(PVal::Lambda(owner, name)),
                    None => stack.push(PVal::Unknown),
                },
                0xb8 => {
                    let (owner, name, descriptor) =
                        method_at(class, instruction).unwrap_or_default();
                    let args = parameter_count(&descriptor);
                    let mut popped = Vec::new();
                    for _ in 0..args {
                        popped.push(stack.pop().unwrap_or(PVal::Unknown));
                    }
                    popped.reverse();
                    if owner.ends_with("Properties")
                        && matches!(name.as_str(), "of" | "ofFullCopy" | "ofLegacyCopy")
                    {
                        let properties = if name == "of" {
                            Properties::default()
                        } else {
                            match popped.first() {
                                Some(PVal::Block(base)) => base.clone(),
                                _ => Properties::default(),
                            }
                        };
                        stack.push(PVal::Properties(properties));
                        continue;
                    }
                    if owner.ends_with("Properties")
                        && matches!(name.as_str(), "of" | "ofFullCopy" | "ofLegacyCopy")
                    {
                        let properties = if name == "of" {
                            Properties::default()
                        } else {
                            match popped.first() {
                                Some(PVal::Block(base)) => base.clone(),
                                _ => Properties::default(),
                            }
                        };
                        stack.push(PVal::Properties(properties));
                        continue;
                    }
                    if owner == BLOCKS
                        && descriptor.ends_with(
                            ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
                        )
                    {
                        let helper = self.eval_method(class, &name, &descriptor, popped);
                        stack.push(helper);
                        continue;
                    }
                    if owner == BLOCKS {
                        match name.as_str() {
                            "register" => {
                                let properties = popped
                                    .iter()
                                    .rev()
                                    .find_map(|value| match value {
                                        PVal::Properties(properties) => Some(properties.clone()),
                                        _ => None,
                                    })
                                    .unwrap_or_default();
                                stack.push(PVal::Block(properties));
                            }
                            "registerStair"
                            | "registerSlab"
                            | "registerWall"
                            | "registerLegacyStair" => {
                                let mut properties = popped
                                    .iter()
                                    .rev()
                                    .find_map(|value| match value {
                                        PVal::Block(properties) => Some(properties.clone()),
                                        _ => None,
                                    })
                                    .unwrap_or_default();
                                if name == "registerWall" {
                                    properties.force_solid_on = true;
                                }
                                stack.push(PVal::Block(properties));
                            }
                            _ => stack.push(PVal::Unknown),
                        }
                        continue;
                    }
                    // A lambda that produces `Properties` (colour/copper families).
                    match popped.iter().rev().find_map(|value| match value {
                        PVal::Lambda(owner, name) => Some((owner.clone(), name.clone())),
                        _ => None,
                    }) {
                        Some((lambda_owner, lambda_name)) => {
                            stack.push(self.eval_lambda(&lambda_owner, &lambda_name));
                        }
                        None => stack.push(PVal::Unknown),
                    }
                }
                0xb6 | 0xb7 | 0xb9 => {
                    let (owner, name, descriptor) =
                        method_at(class, instruction).unwrap_or_default();
                    let args = parameter_count(&descriptor);
                    let mut popped = Vec::new();
                    for _ in 0..args {
                        popped.push(stack.pop().unwrap_or(PVal::Unknown));
                    }
                    popped.reverse();
                    let receiver = stack.pop().unwrap_or(PVal::Unknown);
                    if owner.ends_with("Properties") {
                        let properties = match receiver {
                            PVal::Properties(properties) => properties,
                            _ => Properties::default(),
                        };
                        let properties = self.apply(properties, &name, &popped);
                        stack.push(PVal::Properties(properties));
                        continue;
                    }
                    if owner == BLOCKS
                        && matches!(
                            name.as_str(),
                            "registerStair"
                                | "registerSlab"
                                | "registerWall"
                                | "registerLegacyStair"
                        )
                    {
                        let mut properties = popped
                            .iter()
                            .rev()
                            .find_map(|value| match value {
                                PVal::Block(properties) => Some(properties.clone()),
                                _ => None,
                            })
                            .unwrap_or_default();
                        if name == "registerWall" {
                            properties.force_solid_on = true;
                        }
                        stack.push(PVal::Block(properties));
                        continue;
                    }
                    stack.push(PVal::Unknown);
                }
                0x99..=0xa6 | 0xc6 | 0xc7 => {
                    // Nothing here branches; a branch would mean an unmodelled
                    // initialiser shape.
                    at = instruction.target;
                }
                0xa7 => at = instruction.target,
                0xac..=0xb1 => return stack.pop().unwrap_or(PVal::Unknown),
                _ => {}
            }
        }
    }

    /// Evaluate a `Function<Properties>`/family factory lambda for its
    /// `Properties` result (colour arguments do not affect any tracked field).
    fn eval_lambda(&mut self, owner: &str, name: &str) -> PVal {
        let Some(class) = self.jar.class(owner) else {
            return PVal::Unknown;
        };
        let Some(method) = class.methods.iter().find(|method| method.name == name) else {
            return PVal::Unknown;
        };
        let descriptor = method.descriptor.clone();
        let args = vec![PVal::Unknown; parameter_count(&descriptor)];
        self.eval_method(&class, name, &descriptor, args)
    }
}

/// Replay `Blocks.<clinit>` and collect one `Properties` per registered block.
pub fn tables(jar_path: &Path) -> Result<Tables, String> {
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let class = jar
        .class(BLOCKS)
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let initialiser = class
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let instructions = decode(&initialiser.bytes);
    let mut vm = Vm {
        jar: &jar,
        fields: BTreeMap::new(),
    };
    let mut stack: Vec<PVal> = Vec::new();
    let mut blocks: Vec<Properties> = Vec::with_capacity(registry.len());
    for instruction in &instructions {
        if (0x02..=0x08).contains(&instruction.opcode) {
            stack.push(PVal::Int(instruction.opcode as i32 - 0x03));
            continue;
        }
        if let Some(value) = int_at(&class, instruction) {
            stack.push(PVal::Int(value));
            continue;
        }
        if let Some(value) = float_at(&class, instruction) {
            stack.push(PVal::Float(value));
            continue;
        }
        match instruction.opcode {
            0x01 | 0x12 | 0x13 | 0xbb | 0xbd | 0xc0 | 0xc1 => stack.push(PVal::Unknown),
            0x59 => {
                let value = stack.last().cloned().unwrap_or(PVal::Unknown);
                stack.push(value);
            }
            0x57 => {
                stack.pop();
            }
            0xb2 => {
                let (owner, name, _) = field_at(&class, instruction).unwrap_or_default();
                if owner == BLOCKS {
                    stack.push(match vm.fields.get(&name).cloned() {
                        Some(properties) => PVal::Block(properties),
                        None => PVal::Unknown,
                    });
                } else if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                    stack.push(PVal::Id);
                } else {
                    stack.push(PVal::Unknown);
                }
            }
            0xb3 => {
                let (owner, name, descriptor) = field_at(&class, instruction).unwrap_or_default();
                let value = stack.pop().unwrap_or(PVal::Unknown);
                if owner == BLOCKS {
                    let properties = match value {
                        PVal::Block(properties) | PVal::Family(properties) => Some(properties),
                        _ => None,
                    };
                    if let Some(properties) = properties {
                        if !descriptor.contains("Collection") || true {
                            vm.fields.insert(name, properties);
                        }
                    }
                }
            }
            0xba => {
                if let Some((owner, name, _)) = indy_target(&class, instruction.index.unwrap_or(0))
                {
                    if owner == BLOCKS && name == "register" {
                        // The registration method itself is a method reference.
                        stack.push(PVal::Unknown);
                    } else {
                        stack.push(vm.eval_lambda(&owner, &name));
                    }
                } else {
                    stack.push(PVal::Unknown);
                }
            }
            0xb6..=0xb9 => {
                let (owner, name, descriptor) = method_at(&class, instruction).unwrap_or_default();
                let args = parameter_count(&descriptor);
                let mut popped = Vec::new();
                for _ in 0..args {
                    popped.push(stack.pop().unwrap_or(PVal::Unknown));
                }
                popped.reverse();
                if owner.ends_with("Properties") {
                    if matches!(name.as_str(), "of" | "ofFullCopy" | "ofLegacyCopy") {
                        let properties = if name == "of" {
                            Properties::default()
                        } else {
                            match popped.first() {
                                Some(PVal::Block(base)) => base.clone(),
                                _ => Properties::default(),
                            }
                        };
                        stack.push(PVal::Properties(properties));
                        continue;
                    }
                    let receiver = stack.pop().unwrap_or(PVal::Unknown);
                    let properties = match receiver {
                        PVal::Properties(properties) => properties,
                        _ => Properties::default(),
                    };
                    stack.push(PVal::Properties(vm.apply(properties, &name, &popped)));
                    continue;
                }
                if owner == BLOCKS
                    && matches!(
                        name.as_str(),
                        "register"
                            | "registerStair"
                            | "registerSlab"
                            | "registerWall"
                            | "registerLegacyStair"
                    )
                {
                    let properties = match name.as_str() {
                        "register" => popped
                            .iter()
                            .rev()
                            .find_map(|value| match value {
                                PVal::Properties(properties) => Some(properties.clone()),
                                _ => None,
                            })
                            .unwrap_or_default(),
                        helper => {
                            let mut properties = popped
                                .iter()
                                .rev()
                                .find_map(|value| match value {
                                    PVal::Block(properties) => Some(properties.clone()),
                                    _ => None,
                                })
                                .unwrap_or_default();
                            if helper == "registerWall" {
                                properties.force_solid_on = true;
                            }
                            properties
                        }
                    };
                    blocks.push(properties.clone());
                    stack.push(PVal::Block(properties));
                    continue;
                }
                if owner == BLOCKS
                    && descriptor.ends_with(
                        ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
                    )
                {
                    let helper = vm.eval_method(&class, &name, &descriptor, popped);
                    stack.push(helper);
                    continue;
                }
                if owner.ends_with("ColorCollection")
                    || owner.ends_with("WeatheringCopperCollection")
                {
                    let count = if owner.ends_with("WeatheringCopperCollection") {
                        8
                    } else {
                        16
                    };
                    let properties = popped
                        .iter()
                        .rev()
                        .find_map(|value| match value {
                            PVal::Properties(properties) => Some(properties.clone()),
                            PVal::Block(properties) if false => Some(properties.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    for _ in 0..count {
                        blocks.push(properties.clone());
                    }
                    stack.push(PVal::Family(properties));
                    continue;
                }
                stack.push(PVal::Unknown);
            }
            _ => {}
        }
    }
    if blocks.len() != registry.len() {
        return Err(format!(
            "the initialiser produced {} blocks, the registry has {}",
            blocks.len(),
            registry.len()
        ));
    }
    // `fenceGate` is `block instanceof FenceGateBlock`, a class check.
    for (row, properties) in registry.iter().zip(blocks.iter_mut()) {
        let mut current = format!("net/minecraft/world/level/block/{}", row.class);
        for _ in 0..32 {
            if current == FENCE_GATE {
                properties.movement.fence_gate = true;
                break;
            }
            let Some(class) = jar.class(&current) else {
                break;
            };
            if class.super_name.is_empty() {
                break;
            }
            current = class.super_name.clone();
        }
    }
    let mut rows = Vec::with_capacity(registry.len());
    for (row, properties) in registry.iter().zip(blocks) {
        rows.push((row.id.clone(), properties));
    }
    Ok(Tables { blocks: rows })
}
/// The state dictionary shape the three shape tables use, documented so a
/// follow-up module can encode/decode it without guessing.
#[allow(dead_code)]
pub mod shapes {
    /// One entry of the `shapes` list: `canonical` (1 empty, 2 block), `cube`,
    /// `cube_axes` bitmask (x|y|z<<1|z<<2 — axis ordinals), and the per-axis
    /// coordinate arrays as `long` float bits (1/32 units are `x*32`).
    pub const KEYS: [&str; 6] = ["canonical", "cube", "cube_axes", "x", "y", "z"];
    pub const FILE_KEYS: [&str; 3] = ["DataVersion", "states", "cached_shapes"];
}

// ---------------------------------------------------------------------------
// NBT writer
// ---------------------------------------------------------------------------

/// Java `HashMap` iteration order: bucket ascending, insertion order in a bucket.
fn hash_order(names: &[String], capacity: usize) -> Vec<usize> {
    let mut capacity = capacity.max(16);
    while names.len() > capacity * 3 / 4 {
        capacity *= 2;
    }
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); capacity];
    for (index, name) in names.iter().enumerate() {
        let hash = {
            let mut hash: i32 = 0;
            for character in name.encode_utf16() {
                hash = hash.wrapping_mul(31).wrapping_add(character as i32);
            }
            hash as u32
        };
        let spread = hash ^ (hash >> 16);
        buckets[(spread as usize) & (capacity - 1)].push(index);
    }
    buckets.into_iter().flatten().collect()
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn write_float(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn write_int(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// Root: `{DataVersion:int, blocks:{<id>:{movement fields}}}` for the movement
/// table (the stripped copy has no `states`), or `{blocks:{id:byte},
/// DataVersion}` for block-properties.
pub fn movement_nbt(tables: &Tables) -> Vec<u8> {
    // Root keys are inserted DataVersion, states, blocks in the Java writer;
    // the stripped copy keeps the hash order of the remaining keys.
    let mut out = Vec::new();
    let root_keys = [
        "DataVersion".to_string(),
        "states".to_string(),
        "blocks".to_string(),
    ];
    let root_order = hash_order(&root_keys, 16);
    // Two root keys only after the strip; rehash the two present ones so the
    // order is the hash order of the remaining map.
    let present: Vec<String> = vec!["DataVersion".to_string(), "blocks".to_string()];
    let present = {
        let mut reordered: Vec<String> = root_order
            .iter()
            .map(|index| root_keys[*index].clone())
            .filter(|key| present.contains(key))
            .collect();
        // The stripped file is written from the surviving keys in the original
        // iteration order, which is what `reordered` is.
        reordered.shrink_to_fit();
        reordered
    };
    out.push(0x0a); // root compound, unnamed
    write_string(&mut out, "");
    for key in &present {
        match key.as_str() {
            "DataVersion" => {
                out.push(0x03);
                write_string(&mut out, "DataVersion");
                write_int(&mut out, DATA_VERSION);
            }
            "blocks" => {
                out.push(0x0a);
                write_string(&mut out, "blocks");
                let mut block_ids: Vec<Option<String>> = vec![None; tables.blocks.len()];
                for (index, (id, _)) in tables.blocks.iter().enumerate() {
                    block_ids[index] = Some(id.clone());
                }
                let block_names: Vec<String> = block_ids.iter().flatten().cloned().collect();
                let rows: Vec<&(String, Properties)> = hash_order(&block_names, 16)
                    .into_iter()
                    .map(|index| &tables.blocks[index])
                    .collect();
                for (id, properties) in rows {
                    out.push(0x0a);
                    write_string(&mut out, id);
                    let names: Vec<String> = [
                        "friction",
                        "speedFactor",
                        "jumpFactor",
                        "bounceRestitution",
                        "fallDistanceReduction",
                        "fenceGate",
                    ]
                    .iter()
                    .map(|name| name.to_string())
                    .collect();
                    for index in hash_order(&names, 16) {
                        match names[index].as_str() {
                            "friction" => {
                                out.push(0x05);
                                write_string(&mut out, "friction");
                                write_float(&mut out, properties.movement.friction);
                            }
                            "speedFactor" => {
                                out.push(0x05);
                                write_string(&mut out, "speedFactor");
                                write_float(&mut out, properties.movement.speed_factor);
                            }
                            "jumpFactor" => {
                                out.push(0x05);
                                write_string(&mut out, "jumpFactor");
                                write_float(&mut out, properties.movement.jump_factor);
                            }
                            "bounceRestitution" => {
                                out.push(0x05);
                                write_string(&mut out, "bounceRestitution");
                                write_float(&mut out, properties.movement.bounce_restitution);
                            }
                            "fallDistanceReduction" => {
                                out.push(0x05);
                                write_string(&mut out, "fallDistanceReduction");
                                write_float(&mut out, properties.movement.fall_distance_reduction);
                            }
                            _ => {
                                out.push(0x01);
                                write_string(&mut out, "fenceGate");
                                out.push(properties.movement.fence_gate as u8);
                            }
                        }
                    }
                    out.push(0x00);
                }
                out.push(0x00);
            }
            _ => {}
        }
    }
    out.push(0x00);
    out
}

pub fn block_properties_nbt(tables: &Tables) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x0a);
    write_string(&mut out, "");
    // Root key order for the stripped block-properties file: DataVersion, states,
    // blocks, occlusion in the writer; hash order of the survivors is blocks,
    // DataVersion.
    out.push(0x0a);
    write_string(&mut out, "blocks");
    let names: Vec<String> = tables.blocks.iter().map(|(id, _)| id.clone()).collect();
    for index in hash_order(&names, 16) {
        let (id, properties) = &tables.blocks[index];
        out.push(0x01);
        write_string(&mut out, id);
        out.push(properties.bitmask());
    }
    out.push(0x00);
    out.push(0x03);
    write_string(&mut out, "DataVersion");
    write_int(&mut out, DATA_VERSION);
    out.push(0x00);
    out
}

/// Write both tables below `out`, and report what was built.
pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let tables = tables(jar_path)?;
    let mut coverage = Coverage::default();
    for table in TABLES {
        coverage.missing.push(table.to_string());
    }
    for (table, reason) in BLOCKED_TABLES {
        coverage.note(format!("{table}: {reason}"));
    }
    let block_properties = block_properties_nbt(&tables);
    let path = out.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, &block_properties).map_err(|error| format!("{}: {error}", path.display()))?;
    coverage.add(TABLE);
    let movement = movement_nbt(&tables);
    let path = out.join(MOVEMENT);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    fs::write(&path, &movement).map_err(|error| format!("{}: {error}", path.display()))?;
    coverage.add(MOVEMENT);
    coverage.note(format!(
        "{} blocks; block-properties.nbt {} bytes, movement-properties.nbt {} bytes",
        tables.blocks.len(),
        block_properties.len(),
        movement.len()
    ));
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bitmask_matches_vanilla_order() {
        let mut properties = Properties::default();
        assert_eq!(properties.bitmask(), 16, "canOcclude only");
        properties.force_solid_on = true;
        assert_eq!(properties.bitmask(), 17);
        properties.force_solid_off = true;
        assert_eq!(properties.bitmask(), 19);
        properties.replaceable = true;
        assert_eq!(properties.bitmask(), 23);
        properties.liquid = true;
        assert_eq!(properties.bitmask(), 31);
        properties.can_occlude = false;
        assert_eq!(properties.bitmask(), 15);
    }

    #[test]
    fn the_java_hash_order_puts_buckets_first() {
        let names: Vec<String> = [
            "friction",
            "speedFactor",
            "jumpFactor",
            "bounceRestitution",
            "fallDistanceReduction",
            "fenceGate",
        ]
        .iter()
        .map(|name| name.to_string())
        .collect();
        let order = hash_order(&names, 16);
        let ordered: Vec<&str> = order.iter().map(|index| names[*index].as_str()).collect();
        assert_eq!(
            ordered,
            vec![
                "fenceGate",
                "bounceRestitution",
                "fallDistanceReduction",
                "speedFactor",
                "friction",
                "jumpFactor"
            ]
        );
    }
}
