//! `_shared/gameplay.compact.json`, finished: the whole block registry, not just the blocks
//! `hzr-bytecode`'s evaluator reaches.
//!
//! [`gameplay`] builds the table from `interp`'s property evaluation and leaves two gaps:
//!
//! 1. **410 blocks** are registered through `ColorCollection` loops (beds, wool, wool stairs
//!    and slabs, carpets, concrete, concrete powder, terracotta, stained glass and panes,
//!    shulker boxes, candles and candle cakes, banners and wall banners, glazed terracotta).
//!    `interp` does not model `ColorCollection.registerBlocks`, so their chains are never
//!    built. Here the call site is read instead: `BlockItemIds.<FAMILY>` (a
//!    `ColorCollection<String>` of `<colour>_<suffix>` created by `createSimpleColored`), the
//!    `BiFunction` whose return type is the block class, and the `Function<DyeColor,
//!    Properties>` that builds the properties. That lambda is straight-line bytecode, so a
//!    small stack VM runs it once per dye colour and gets the properties of all sixteen
//!    blocks.
//! 2. **Residual per-block fields** are lost to property calls `interp` does not know
//!    (`.liquid()`, `.offsetType(...)`, `.noOcclusion()`, `.ignitedByLava()`,
//!    `.requiresCorrectToolForDrops()` inside helper chains, `.ofFullCopy(Blocks.X)`), or to
//!    the class hierarchy (`block_entity`), the occlusion shape (`can_occlude`) and the
//!    dynamic-shape collision question (`full_collision`). The same VM evaluates the
//!    `Properties` chain of a plain `Blocks.register(...)` call site, and the remaining
//!    fields are derived from `block::registry`'s class chain and `shape_final`'s shapes.
//!
//! What is still out of reach is reported, never guessed at: the fields `gameplay` lists in
//! [`gameplay::MISSING`] need the light engine (`dampening`, `skylight_down`, `solid_render`,
//! `shape_occlusion`, `random_ticks`), the selection-outline geometry (`outline`,
//! `outline_edges`, `outline_offset`) or the item registry mapping (`item`, `render_shape`).
//!
//! Nothing here reads `ship.hzr3`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

use crate::classfile;

#[path = "../../hzr-bytecode/src/interp.rs"]
#[allow(dead_code)]
mod interp;

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

#[path = "gameplay.rs"]
#[allow(dead_code)]
mod gameplay;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

#[path = "shape_final.rs"]
#[allow(dead_code)]
mod shape_final;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = gameplay::TABLE;

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    /// Blocks whose per-block values could not be derived.
    pub uncovered: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// The jar and its bytecode
// ---------------------------------------------------------------------------

/// The jar's class files, read once.
struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    by_short: BTreeMap<String, String>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut files = BTreeMap::new();
        let mut by_short = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|e| e.to_string())?.to_string();
            let Some(internal) = name.strip_suffix(".class") else {
                continue;
            };
            let mut data = Vec::new();
            entry.read_to_end(&mut data).map_err(|e| format!("{name}: {e}"))?;
            by_short.insert(short(internal).to_string(), internal.to_string());
            files.insert(internal.to_string(), data);
        }
        Ok(Jar { files, by_short })
    }

    fn bytes(&self, name: &str) -> Option<&Vec<u8>> {
        if name.contains('/') {
            self.files.get(name)
        } else {
            self.by_short
                .get(name)
                .and_then(|internal| self.files.get(internal))
        }
    }

    fn class(&self, name: &str) -> Option<classfile::ClassFile> {
        classfile::parse(self.bytes(name)?).ok()
    }

    fn internal(&self, simple: &str) -> Option<String> {
        self.by_short.get(simple).cloned()
    }
}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
    at: usize,
}

fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let mut immediate = None;
        let length = match opcode {
            0x02 => {
                immediate = Some(-1);
                1
            }
            0x03..=0x08 => {
                immediate = Some((opcode - 0x03) as i32);
                1
            }
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
            0x12 => {
                index = bytes.get(at + 1).map(|value| *value as usize);
                2
            }
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]) as usize);
                3
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
            0x84 => 3,
            // Branches carry their offset in the operands.
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                immediate = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| i16::from_be_bytes([pair[0], pair[1]]) as i32);
                3
            }
            0xc4 => 4,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            0xaa => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base = at + 1 + padding;
                let low = bytes
                    .get(base + 4..base + 8)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                let high = bytes
                    .get(base + 8..base + 12)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                let (Some(low), Some(high)) = (low, high) else { break };
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
        out.push(Instruction {
            opcode,
            index,
            immediate,
            at,
        });
        at += length;
    }
    out
}

fn constant(class: &classfile::ClassFile, index: usize) -> Option<&classfile::Constant> {
    class.constants.get(index)
}

fn utf8_at(class: &classfile::ClassFile, reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match constant(class, index)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

/// `(owner, name, descriptor)` behind a field/method reference.
///
/// The shared parser stores the descriptor in `kind` for every reference kind.
fn member(class: &classfile::ClassFile, index: usize) -> Option<(String, String, String)> {
    let (owner_ref, name_ref, kind) = match constant(class, index)? {
        classfile::Constant::Fieldref { class, name, kind } => (class, name, kind),
        classfile::Constant::Methodref { class, name, kind } => (class, name, kind),
        classfile::Constant::InterfaceMethodref { class, name, kind } => (class, name, kind),
        _ => return None,
    };
    let owner = match constant(class, owner_ref.trim_start_matches('#').parse().ok()?)? {
        classfile::Constant::Class(reference) => utf8_at(class, reference)?,
        _ => return None,
    };
    let name_index: usize = name_ref.trim_start_matches('#').parse().ok()?;
    let (name, descriptor) = match constant(class, name_index)? {
        classfile::Constant::NameAndType { name, kind } => (utf8_at(class, name)?, utf8_at(class, kind)?),
        classfile::Constant::Utf8(text) => (text.clone(), kind.clone()),
        _ => return None,
    };
    Some((owner, name, descriptor))
}

fn argument_count(descriptor: &str) -> usize {
    let mut count = 0usize;
    let mut chars = descriptor.chars();
    if chars.next() != Some('(') {
        return 0;
    }
    loop {
        match chars.next() {
            Some(')') | None => break,
            Some('[') => {
                while let Some(next) = chars.next() {
                    if next != '[' {
                        if next == 'L' {
                            while let Some(inner) = chars.next() {
                                if inner == ';' {
                                    break;
                                }
                            }
                        }
                        break;
                    }
                }
                count += 1;
            }
            Some('L') => {
                while let Some(inner) = chars.next() {
                    if inner == ';' {
                        break;
                    }
                }
                count += 1;
            }
            Some(_) => count += 1,
        }
    }
    count
}

// ---------------------------------------------------------------------------
// The property model and the VM
// ---------------------------------------------------------------------------

/// `BlockBehaviour.Properties` as far as one chain shows it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Props {
    map_color: Option<String>,
    map_color_function: bool,
    sound: Option<String>,
    instrument: Option<String>,
    hardness: Option<f32>,
    resistance: Option<f32>,
    friction: Option<f32>,
    speed: Option<f32>,
    jump: Option<f32>,
    requires_tool: bool,
    random_ticks: bool,
    replaceable: bool,
    liquid: bool,
    ignited_by_lava: bool,
    no_occlusion: bool,
    no_collision: bool,
    offset_type: bool,
    push: Option<String>,
    /// `Properties.ofFullCopy(Blocks.X)`: the `Blocks` field, resolved after the scan.
    copy_of: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Int(i32),
    Float(f32),
    Str(String),
    /// A `getstatic`: owner internal name and field.
    Ref(String, String),
    Props(Props),
    Opaque,
}

fn field_name(value: &Val) -> Option<String> {
    match value {
        Val::Ref(_, field) => Some(field.clone()),
        _ => None,
    }
}

fn as_float(value: &Val) -> Option<f32> {
    match value {
        Val::Int(int) => Some(*int as f32),
        Val::Float(float) => Some(*float),
        _ => None,
    }
}

/// A small stack VM for the property chains of `Blocks.<clinit>`.
struct Vm<'a> {
    jar: &'a Jar,
    classes: RefCell<HashMap<String, Option<std::rc::Rc<classfile::ClassFile>>>>,
    maps: Maps,
    depth: std::cell::Cell<usize>,
}

const PROPERTIES: &str = "net/minecraft/world/level/block/state/BlockBehaviour$Properties";

impl<'a> Vm<'a> {
    fn new(jar: &'a Jar, maps: Maps) -> Vm<'a> {
        Vm {
            jar,
            classes: RefCell::new(HashMap::new()),
            maps,
            depth: std::cell::Cell::new(0),
        }
    }

    fn class(&self, name: &str) -> Option<std::rc::Rc<classfile::ClassFile>> {
        if let Some(hit) = self.classes.borrow().get(name) {
            return hit.clone();
        }
        let parsed = self
            .jar
            .bytes(name)
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(std::rc::Rc::new);
        self.classes
            .borrow_mut()
            .insert(name.to_string(), parsed.clone());
        parsed
    }

    fn method<'c>(
        &self,
        class: &'c classfile::ClassFile,
        name: &str,
    ) -> Option<&'c classfile::Member> {
        class.methods.iter().find(|method| method.name == name)
    }

    /// Runs a whole method body.
    fn run(
        &self,
        class: &std::rc::Rc<classfile::ClassFile>,
        method: &classfile::Member,
        args: Vec<Val>,
    ) -> Option<Val> {
        let code = method.code.as_ref()?;
        let instructions = decode(&code.bytes);
        self.execute(class, &instructions, args, None)
    }

    /// Runs a slice of `Blocks.<clinit>` up to `stop` and returns the stack it leaves.
    fn run_until(
        &self,
        class: &std::rc::Rc<classfile::ClassFile>,
        instructions: &[Instruction],
        stop: &str,
    ) -> Option<Vec<Val>> {
        self.execute_stack(class, instructions, Vec::new(), Some(stop))
    }

    fn execute(
        &self,
        class: &std::rc::Rc<classfile::ClassFile>,
        instructions: &[Instruction],
        args: Vec<Val>,
        stop: Option<&str>,
    ) -> Option<Val> {
        let stack = self.execute_stack(class, instructions, args, stop)?;
        stack.last().cloned()
    }

    fn execute_stack(
        &self,
        class: &std::rc::Rc<classfile::ClassFile>,
        instructions: &[Instruction],
        args: Vec<Val>,
        stop: Option<&str>,
    ) -> Option<Vec<Val>> {
        let mut index_at: HashMap<usize, usize> = HashMap::new();
        for (slot, instruction) in instructions.iter().enumerate() {
            index_at.insert(instruction.at, slot);
        }
        let mut locals: Vec<Val> = args;
        let mut stack: Vec<Val> = Vec::new();
        let mut pc = 0usize;
        let mut steps = 0usize;
        while pc < instructions.len() {
            steps += 1;
            if steps > 40_000 {
                return None;
            }
            let instruction = &instructions[pc];
            let mut next = pc + 1;
            match instruction.opcode {
                0x00 => {}
                0x01 => stack.push(Val::Opaque),
                0x02 => stack.push(Val::Int(-1)),
                0x03..=0x08 => stack.push(Val::Int((instruction.opcode - 0x03) as i32)),
                0x0b => stack.push(Val::Float(0.0)),
                0x0c => stack.push(Val::Float(1.0)),
                0x0d => stack.push(Val::Float(2.0)),
                0x10 | 0x11 => stack.push(Val::Int(instruction.immediate?)),
                0x12 | 0x13 => match constant(class, instruction.index?) {
                    Some(classfile::Constant::Integer(value)) => stack.push(Val::Int(*value)),
                    Some(classfile::Constant::Float(value)) => stack.push(Val::Float(*value)),
                    Some(classfile::Constant::String(reference)) => {
                        stack.push(Val::Str(utf8_at(class, reference)?))
                    }
                    Some(classfile::Constant::Double(value)) => {
                        stack.push(Val::Float(*value as f32))
                    }
                    Some(classfile::Constant::Long(value)) => stack.push(Val::Int(*value as i32)),
                    _ => stack.push(Val::Opaque),
                },
                0x15 | 0x19 => {
                    stack.push(locals.get(instruction.index?).cloned()?);
                }
                0x2a..=0x2d => {
                    let slot = (instruction.opcode - 0x2a) as usize;
                    stack.push(locals.get(slot).cloned()?);
                }
                0x3b..=0x3e => {
                    let slot = (instruction.opcode - 0x3b) as usize;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0x4b..=0x4e => {
                    let slot = (instruction.opcode - 0x4b) as usize;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0x57 => {
                    stack.pop()?;
                }
                0x58 => {
                    stack.pop()?;
                    stack.pop()?;
                }
                0x59 => {
                    let value = stack.last()?.clone();
                    stack.push(value);
                }
                0xb3 => {
                    stack.pop()?;
                }
                0xb2 => {
                    let (owner, name, _) = member(class, instruction.index?)?;
                    stack.push(Val::Ref(owner, name));
                }
                0xb6 | 0xb7 | 0xb8 | 0xb9 => {
                    let (owner, name, descriptor) = member(class, instruction.index?)?;
                    if let Some(stop) = stop {
                        if name == stop {
                            return Some(stack);
                        }
                    }
                    let count = argument_count(&descriptor);
                    let mut arguments = Vec::with_capacity(count);
                    for _ in 0..count {
                        arguments.push(stack.pop()?);
                    }
                    arguments.reverse();
                    let receiver = if instruction.opcode == 0xb8 {
                        None
                    } else {
                        Some(stack.pop()?)
                    };
                    let returns = !descriptor.ends_with(")V");
                    let value = self.call(owner, name, descriptor, receiver, arguments)?;
                    if instruction.opcode != 0xb7 && returns {
                        stack.push(value);
                    }
                }
                0xba => {
                    if let Some(stop) = stop {
                        if let Some((_, name, _)) = lambda_ref(class, instruction.index.unwrap_or(0)) {
                            if name == stop {
                                return Some(stack);
                            }
                        }
                    }
                    stack.push(Val::Opaque);
                }
                0xa7 => {
                    next = jump_target(instructions, &index_at, pc)?;
                }
                0x99..=0xa6 => {
                    let right = stack.pop()?;
                    let left = stack.pop()?;
                    let taken = match instruction.opcode {
                        0x99 => matches!(left, Val::Int(value) if value == 0),
                        0x9a => matches!(left, Val::Int(value) if value != 0),
                        0x9b | 0x9c | 0x9d | 0x9e => false,
                        0x9f | 0xa0 | 0xa1 | 0xa2 | 0xa3 | 0xa4 => false,
                        0xa5 => left == right,
                        0xa6 => left != right,
                        _ => false,
                    };
                    if taken {
                        next = jump_target(instructions, &index_at, pc)?;
                    }
                }
                0xc6 | 0xc7 => {
                    let value = stack.pop()?;
                    let is_null = matches!(value, Val::Opaque);
                    let taken = (instruction.opcode == 0xc6) == is_null;
                    if taken {
                        next = jump_target(instructions, &index_at, pc)?;
                    }
                }
                0xbb => {
                    let (owner, _, _) = member(class, instruction.index?)?;
                    stack.push(Val::Ref(owner, String::new()));
                }
                0xbd | 0xc0 | 0xc1 | 0x14 => stack.push(Val::Opaque),
                0xb0 | 0xac | 0xad | 0xae | 0xaf => {
                    let value = stack.pop()?;
                    return Some(vec![value]);
                }
                0xb1 => return Some(stack),
                other => {
                    if std::env::var("GF_DEBUG").is_ok() {
                        eprintln!(
                            "DBG vm stop opcode={other:#04x} at={} class={} method={}",
                            instruction.at,
                            class.name,
                            method_name(instructions)
                        );
                    }
                    return None;
                }
            }
            pc = next;
        }
        Some(stack)
    }

    fn call(
        &self,
        owner: String,
        name: String,
        descriptor: String,
        receiver: Option<Val>,
        arguments: Vec<Val>,
    ) -> Option<Val> {
        if owner == PROPERTIES {
            match name.as_str() {
                "of" => return Some(Val::Props(Props::default())),
                "ofFullCopy" => {
                    let mut props = Props::default();
                    props.copy_of = arguments.first().and_then(field_name);
                    return Some(Val::Props(props));
                }
                _ => {}
            }
            let mut props = match receiver {
                Some(Val::Props(props)) => props,
                _ => Props::default(),
            };
            props.mutate(&name, &arguments);
            return Some(Val::Props(props));
        }
        if short(&owner) == "DyeColor" && name == "getMapColor" {
            let dye = match &receiver {
                Some(Val::Ref(_, field)) => field.clone(),
                _ => return None,
            };
            return Some(Val::Str(self.maps.dye_map_colour.get(&dye)?.clone()));
        }
        if short(&owner) == "Blocks" {
            // Registration helpers have side effects (and can call themselves); the chain
            // being read is the argument, not the helper.
            if name.starts_with("register") || self.depth.get() > 6 {
                return Some(Val::Opaque);
            }
            if let Some(class) = self.class(&owner) {
                if let Some(method) = self.method(&class, &name) {
                    self.depth.set(self.depth.get() + 1);
                    let value = self.run(&class, method, arguments);
                    self.depth.set(self.depth.get() - 1);
                    return value;
                }
            }
        }
        if short(&owner) == "ColorCollection" {
            // `pick`/`asList` are not needed for the properties themselves.
            return Some(Val::Opaque);
        }
        Some(Val::Opaque)
    }
}

fn method_name(_instructions: &[Instruction]) -> &'static str {
    ""
}

fn set_local(locals: &mut Vec<Val>, slot: usize, value: Val) {
    while locals.len() <= slot {
        locals.push(Val::Opaque);
    }
    locals[slot] = value;
}

fn jump_target(
    instructions: &[Instruction],
    index_at: &HashMap<usize, usize>,
    pc: usize,
) -> Option<usize> {
    let instruction = instructions.get(pc)?;
    let target = (instruction.at as i64 + instruction.immediate? as i64) as usize;
    index_at.get(&target).copied()
}

impl Props {
    fn mutate(&mut self, name: &str, arguments: &[Val]) {
        match name {
            "strength" => {
                if let Some(hardness) = arguments.first().and_then(as_float) {
                    self.hardness = Some(hardness);
                    self.resistance = Some(hardness);
                }
                if let Some(resistance) = arguments.get(1).and_then(as_float) {
                    self.resistance = Some(resistance);
                }
            }
            "destroyTime" => self.hardness = arguments.first().and_then(as_float),
            "explosionResistance" => self.resistance = arguments.first().and_then(as_float),
            "mapColor" => {
                if let Some(field) = arguments.first().and_then(field_name) {
                    self.map_color = Some(field);
                } else {
                    self.map_color_function = true;
                }
            }
            "instrument" => self.instrument = arguments.first().and_then(field_name),
            "sound" => self.sound = arguments.first().and_then(field_name),
            "requiresCorrectToolForDrops" => self.requires_tool = true,
            "randomTicks" => self.random_ticks = true,
            "friction" => self.friction = arguments.first().and_then(as_float),
            "speedFactor" => self.speed = arguments.first().and_then(as_float),
            "jumpFactor" => self.jump = arguments.first().and_then(as_float),
            "replaceable" => self.replaceable = true,
            "noCollision" => self.no_collision = true,
            "noOcclusion" => self.no_occlusion = true,
            "ignitedByLava" => self.ignited_by_lava = true,
            "liquid" => self.liquid = true,
            "offsetType" => {
                if let Some(Val::Ref(_, field)) = arguments.first() {
                    if field != "NONE" {
                        self.offset_type = true;
                    }
                }
            }
            "pushReaction" | "pistonPush" => {
                if let Some(field) = arguments.first().and_then(field_name) {
                    self.push = Some(field);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

fn map_colours(jar: &Jar) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/level/material/MapColor") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut numbers: Vec<i32> = Vec::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => numbers.clear(),
            0x12 | 0x13 => {
                if let Some(classfile::Constant::Integer(value)) =
                    instruction.index.and_then(|index| constant(&class, index))
                {
                    numbers.push(*value);
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "MapColor" {
                        if let Some(col) = numbers.get(1) {
                            out.insert(field, *col as u32);
                        }
                        numbers.clear();
                    }
                }
            }
            _ => {
                if let Some(value) = instruction.immediate {
                    numbers.push(value);
                }
            }
        }
    }
    out
}

fn dye_map_colours(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/item/DyeColor") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut first: Option<String> = None;
    let mut seen_new = false;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => {
                seen_new = true;
                first = None;
            }
            0xb2 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if seen_new && short(&owner) == "MapColor" && first.is_none() {
                        first = Some(field);
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "DyeColor" && seen_new {
                        if let Some(colour) = first.take() {
                            out.insert(field, colour);
                        }
                        seen_new = false;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn sound_events(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/sounds/SoundEvents") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut id: Option<String> = None;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) =
                    instruction.index.and_then(|index| constant(&class, index))
                {
                    if let Some(text) = utf8_at(&class, reference) {
                        id = Some(format!("minecraft:{text}"));
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "SoundEvents" {
                        if let Some(id) = id.take() {
                            out.insert(field, id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out.entry("EMPTY".to_string())
        .or_insert_with(|| "minecraft:empty".to_string());
    out
}

fn sound_types(jar: &Jar, events: &BTreeMap<String, String>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/level/block/SoundType") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut numbers: Vec<f32> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => {
                numbers.clear();
                used.clear();
            }
            0x0b..=0x0d => numbers.push(match instruction.opcode {
                0x0b => 0.0,
                0x0c => 1.0,
                _ => 2.0,
            }),
            0x12 | 0x13 => {
                if let Some(classfile::Constant::Float(value)) =
                    instruction.index.and_then(|index| constant(&class, index))
                {
                    numbers.push(*value);
                }
            }
            0xb2 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "SoundEvents" {
                        used.push(field);
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "SoundType" && used.len() == 5 {
                        let mut sound = Map::new();
                        sound.insert(
                            "volume".to_string(),
                            float_value(numbers.first().copied().unwrap_or(1.0)),
                        );
                        sound.insert(
                            "pitch".to_string(),
                            float_value(numbers.get(1).copied().unwrap_or(1.0)),
                        );
                        for (slot, name) in
                            ["break", "step", "place", "hit", "fall"].iter().enumerate()
                        {
                            let id = events.get(&used[slot]).cloned().unwrap_or_default();
                            let mut event = Map::new();
                            event.insert("id".to_string(), Value::String(id));
                            sound.insert(name.to_string(), Value::Object(event));
                        }
                        out.insert(field, Value::Object(sound));
                        numbers.clear();
                        used.clear();
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `BlockItemIds.<FIELD>` -> the suffix `createSimpleColored` prefixes with the colour name.
fn colour_suffixes(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/references/BlockItemIds") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut suffix: Option<String> = None;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) =
                    instruction.index.and_then(|index| constant(&class, index))
                {
                    if let Some(text) = utf8_at(&class, reference) {
                        suffix = Some(text);
                    }
                }
            }
            0xb8 => {
                if let Some((owner, name, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemIds" && name != "createSimpleColored" {
                        suffix = None;
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemIds" {
                        if let Some(suffix) = suffix.take() {
                            out.insert(field, suffix);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn float_value(value: f32) -> Value {
    let mut text = format!("{value}");
    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
        text.push_str(".0");
    }
    serde_json::from_str(&text).unwrap_or_else(|_| Value::from(value as f64))
}

const DYE_COLOURS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// The static tables the VM and the facts read.
#[derive(Clone, Default)]
struct Maps {
    dye_map_colour: BTreeMap<String, String>,
    colours: BTreeMap<String, u32>,
    sounds: BTreeMap<String, Value>,
    suffixes: BTreeMap<String, String>,
}

impl Maps {
    fn read(jar: &Jar) -> Maps {
        let events = sound_events(jar);
        Maps {
            dye_map_colour: dye_map_colours(jar),
            colours: map_colours(jar),
            sounds: sound_types(jar, &events),
            suffixes: colour_suffixes(jar),
        }
    }
}

// ---------------------------------------------------------------------------
// The scan of `Blocks.<clinit>`
// ---------------------------------------------------------------------------

/// One colour family: the ids' suffix, the block class and the properties lambda.
#[derive(Clone, Debug)]
struct Family {
    ids: String,
    class: String,
    properties: String,
}

/// What the `Blocks.<clinit>` scan found.
struct Scan {
    families: Vec<Family>,
    /// `Blocks.<FAMILY>` field -> family index.
    family_fields: BTreeMap<String, usize>,
    /// `BlockItemId` field -> the colour family it belongs to (None for plain blocks).
    registrations: Vec<(String, Vec<Instruction>)>,
}

fn scan_blocks(class: &classfile::ClassFile) -> Scan {
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return Scan {
            families: Vec::new(),
            family_fields: BTreeMap::new(),
            registrations: Vec::new(),
        };
    };
    let instructions = decode(&code.bytes);
    let mut families: Vec<Family> = Vec::new();
    let mut family_fields: BTreeMap<String, usize> = BTreeMap::new();
    let mut pending_ids: Option<String> = None;
    let mut indy: Vec<(String, String, String)> = Vec::new();
    let mut registered = false;
    let mut registrations: Vec<(String, Vec<Instruction>)> = Vec::new();
    let mut segment_start = 0usize;
    let mut pending_slot = 0usize;
    let mut stat_ids = 0usize;
    let mut stat_colour_ids = 0usize;
    let mut stat_indy = 0usize;
    let mut stat_register = 0usize;
    let mut stat_register_blocks = 0usize;
    for (slot, instruction) in instructions.iter().enumerate() {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, descriptor)) =
                    instruction.index.and_then(|index| member(class, index))
                {
                    if short(&owner) == "BlockItemIds" {
                        if descriptor.contains("ColorCollection") {
                            stat_colour_ids += 1;
                            pending_ids = Some(name);
                            indy.clear();
                        } else if descriptor.contains("references/BlockItemId;") {
                            stat_ids += 1;
                            pending_ids = Some(name);
                            pending_slot = slot;
                        }
                    }
                }
            }
            0xba => {
                if let Some(reference) = lambda_ref(class, instruction.index.unwrap_or(0)) {
                    stat_indy += 1;
                    indy.push(reference);
                }
            }
            0xb8 => {
                let Some((owner, name, _)) =
                    instruction.index.and_then(|index| member(class, index))
                else {
                    continue;
                };
                if short(&owner) == "ColorCollection" && name == "registerBlocks" {
                    stat_register_blocks += 1;
                    if let Some(ids) = pending_ids.take() {
                        if indy.len() >= 2 {
                            let properties = indy[indy.len() - 1].1.clone();
                            let block_class =
                                return_class(&indy[indy.len() - 2].2).unwrap_or_default();
                            families.push(Family {
                                ids,
                                class: block_class,
                                properties,
                            });
                            registered = true;
                        }
                    }
                    indy.clear();
                }
                if short(&owner) == "Blocks" && name == "register" {
                    stat_register += 1;
                    let id = pending_ids.take();
                    if let Some(id) = id {
                        registrations.push((id, instructions[pending_slot..=slot].to_vec()));
                    }
                    segment_start = slot + 1;
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(class, index))
                {
                    if short(&owner) == "Blocks" && registered && !families.is_empty() {
                        family_fields.insert(field, families.len() - 1);
                        registered = false;
                    }
                }
            }
            _ => {}
        }
    }
    eprintln!(
        "DBG scan ids={stat_ids} colour_ids={stat_colour_ids} indy={stat_indy} register={stat_register} register_blocks={stat_register_blocks}"
    );
    Scan {
        families,
        family_fields,
        registrations,
    }
}

/// The implementation method behind an `invokedynamic`: `(owner, name, descriptor)`.
///
/// A lambda bootstrap's arguments are `(samMethodType, implMethod, instantiatedMethodType)`,
/// so the implementation is the first `MethodHandle` among them, not the first argument.
fn lambda_ref(class: &classfile::ClassFile, index: usize) -> Option<(String, String, String)> {
    let bootstrap = match constant(class, index)? {
        classfile::Constant::InvokeDynamic { bootstrap, .. } => *bootstrap,
        _ => return None,
    };
    let row = class.bootstraps.get(bootstrap)?;
    for argument in &row.arguments {
        let Some(classfile::Constant::MethodHandle { reference }) = constant(class, *argument)
        else {
            continue;
        };
        if let Some(found) = member(class, *reference) {
            return Some(found);
        }
    }
    None
}

/// The class a method returns (as a simple name, for a (A, B) -> C factory).
fn return_class(descriptor: &str) -> Option<String> {
    let tail = descriptor.split(')').nth(1)?;
    let inner = tail.strip_prefix('L')?.strip_suffix(';')?;
    Some(short(inner).to_string())
}

// ---------------------------------------------------------------------------
// The facts
// ---------------------------------------------------------------------------

/// What one block contributes to the table.
#[derive(Clone, Debug, Default, PartialEq)]
struct BlockFacts {
    props: Props,
    block_entity: bool,
    class: String,
}

/// The per-block values and the gaps.
pub struct Rows {
    pub blocks: Vec<String>,
    /// One row per block, `gameplay::FIELDS` order (`can_occlude` is `null` without shapes).
    pub rows: Vec<BTreeMap<&'static str, Value>>,
    pub uncovered: Vec<(String, String)>,
}

/// Derives every block's per-block row from the jar.
pub fn rows(jar_path: &Path) -> Result<Rows, String> {
    derive(jar_path, None)
}

/// Derives the rows; with `shapes` it also settles `can_occlude` from the occlusion shape.
fn derive(jar_path: &Path, shapes: Option<&shape_final::Evaluated>) -> Result<Rows, String> {
    let jar = Jar::open(jar_path)?;
    let maps = Maps::read(&jar);
    let vm = Vm::new(&jar, maps.clone());
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    if registry.len() != state_table.blocks.len() {
        return Err("the block registry and the state table disagree".into());
    }
    let blocks_class = std::rc::Rc::new(
        jar.class("net/minecraft/world/level/block/Blocks")
            .ok_or("the jar has no Blocks class")?,
    );
    // ids_of reads BlockItemIds.<clinit>, not Blocks.
    let ids_class = jar
        .class("net/minecraft/references/BlockItemIds")
        .ok_or("the jar has no BlockItemIds class")?;
    let ids = interp::ids_of(&ids_class);
    let mut loader = InterpLoader {
        jar: &jar,
        cache: RefCell::new(HashMap::new()),
    };
    let mut interp_blocks: BTreeMap<String, interp::Properties> = BTreeMap::new();
    for block in interp::blocks_with(&mut loader, &ids) {
        interp_blocks.insert(format!("minecraft:{}", block.name), block.properties);
    }
    let scan = scan_blocks(&blocks_class);
    // Colour families: one property chain per (family, colour).
    // Colour families: one property chain per (family, colour).
    let mut colour_props: BTreeMap<String, Props> = BTreeMap::new();
    for family in &scan.families {
        let Some(suffix) = maps.suffixes.get(&family.ids) else {
            continue;
        };
        let Some(method) = blocks_method(&blocks_class, &family.properties) else {
            continue;
        };
        for colour in DYE_COLOURS {
            let dye = Val::Ref(
                "net/minecraft/world/item/DyeColor".to_string(),
                colour.to_ascii_uppercase(),
            );
            if let Some(Val::Props(props)) = vm.run(&blocks_class, method, vec![dye]) {
                colour_props.insert(format!("minecraft:{colour}_{suffix}"), props);
            }
        }
    }
    // Plain registrations: the inline property chain, run to the `register` call.
    let mut window_props: BTreeMap<String, Props> = BTreeMap::new();
    for (ids_field, window) in &scan.registrations {
        let Some(block_id) = ids.get(ids_field) else {
            continue;
        };
        let result = vm.run_until(&blocks_class, window, "register");
        if std::env::var("GF_DEBUG").is_ok() && window_props.is_empty() {
            eprintln!(
                "DBG window id={block_id} len={} opcodes={:?} result={:?}",
                window.len(),
                window.iter().take(14).map(|instruction| instruction.opcode).collect::<Vec<_>>(),
                result.as_ref().map(|stack| stack.len())
            );
        }
        if let Some(stack) = result {
            if let Some(Val::Props(props)) = stack.iter().rev().find(|value| matches!(value, Val::Props(_))) {
                window_props.insert(format!("minecraft:{block_id}"), props.clone());
            }
        }
    }
    eprintln!(
        "DBG families={} registrations={} colour_props={} window_props={}",
        scan.families.len(),
        scan.registrations.len(),
        colour_props.len(),
        window_props.len()
    );
    // The final properties: `interp`'s answer for the blocks it reaches (it is exact for the
    // plain fields), overlaid with the calls only the VM sees.
    let mut props_by_id: BTreeMap<String, Props> = BTreeMap::new();
    for block in &registry {
        let base = interp_blocks.get(&block.id).map(props_from_interp);
        let extra = window_props.get(&block.id).cloned();
        let props = match (base, extra) {
            (Some(base), Some(extra)) => Some(overlay(base, extra)),
            (Some(base), None) => Some(base),
            (None, Some(extra)) => Some(extra),
            (None, None) => colour_props.get(&block.id).cloned(),
        };
        if let Some(props) = props {
            props_by_id.insert(block.id.clone(), props);
        }
    }
    let known: BTreeSet<String> = registry.iter().map(|block| block.id.clone()).collect();
    for _ in 0..4 {
        let snapshot = props_by_id.clone();
        let mut changed = false;
        for (id, props) in props_by_id.iter_mut() {
            let Some(field) = &props.copy_of else {
                continue;
            };
            let target = format!("minecraft:{}", field.to_lowercase());
            if !known.contains(&target) {
                continue;
            }
            if let Some(base) = snapshot.get(&target) {
                let copy = Props {
                    copy_of: None,
                    ..base.clone()
                };
                *props = merge(props.clone(), copy);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut rows = Vec::with_capacity(registry.len());
    let mut uncovered = Vec::new();
    let mut state = 0usize;
    for (index, block) in registry.iter().enumerate() {
        let internal = jar.internal(&block.class).unwrap_or_default();
        let block_entity = implements(
            &jar,
            &internal,
            "net/minecraft/world/level/block/EntityBlock",
        );
        let props = props_by_id
            .get(&block.id)
            .cloned()
            .or_else(|| interp_blocks.get(&block.id).map(props_from_interp));
        let Some(props) = props else {
            uncovered.push((block.id.clone(), "no property chain reaches it".to_string()));
            rows.push(empty_row());
            state += state_table.blocks[index].state_count;
            continue;
        };
        let mut row = block_value_row(&props, &maps);
        // `can_occlude` is `!noOcclusion()` and an occlusion shape that is not empty.
        let occludes = if let Some(shapes) = shapes {
            match shapes.occlusion.get(state) {
                Some(Some(shape)) => shape.canonical != 0,
                Some(None) => {
                    uncovered.push((
                        block.id.clone(),
                        "no occlusion shape (dynamic or unmodelled)".to_string(),
                    ));
                    true
                }
                None => false,
            }
        } else {
            true
        };
        row.insert(
            "can_occlude",
            if shapes.is_some() {
                Value::Bool(!props.no_occlusion && occludes)
            } else {
                Value::Null
            },
        );
        row.insert("block_entity", Value::Bool(block_entity));
        rows.push(row);
        state += state_table.blocks[index].state_count;
    }
    eprintln!(
        "DBG uncovered={} first={:?}",
        uncovered.len(),
        uncovered.iter().take(30).map(|(id, _)| id.as_str()).collect::<Vec<_>>()
    );
    Ok(Rows {
        blocks: registry.iter().map(|block| block.id.clone()).collect(),
        rows,
        uncovered,
    })
}

/// `extra` (the VM's call list) settles the calls `interp` does not model, and only those.
fn overlay(base: Props, extra: Props) -> Props {
    Props {
        map_color: base.map_color.or(extra.map_color),
        map_color_function: base.map_color_function || extra.map_color_function,
        sound: base.sound.or(extra.sound),
        instrument: base.instrument.or(extra.instrument),
        hardness: base.hardness.or(extra.hardness),
        resistance: base.resistance.or(extra.resistance),
        friction: base.friction.or(extra.friction),
        speed: base.speed.or(extra.speed),
        jump: base.jump.or(extra.jump),
        requires_tool: base.requires_tool || extra.requires_tool,
        random_ticks: base.random_ticks || extra.random_ticks,
        replaceable: base.replaceable || extra.replaceable,
        liquid: base.liquid || extra.liquid,
        ignited_by_lava: base.ignited_by_lava || extra.ignited_by_lava,
        no_occlusion: base.no_occlusion || extra.no_occlusion,
        no_collision: base.no_collision || extra.no_collision,
        offset_type: base.offset_type || extra.offset_type,
        push: base.push.or(extra.push),
        copy_of: base.copy_of.or(extra.copy_of),
    }
}

/// `copy` settles what `props` did not say, keeping the calls `props` made later.
fn merge(props: Props, copy: Props) -> Props {
    Props {
        map_color: props.map_color.or(copy.map_color),
        map_color_function: props.map_color_function || copy.map_color_function,
        sound: props.sound.or(copy.sound),
        instrument: props.instrument.or(copy.instrument),
        hardness: props.hardness.or(copy.hardness),
        resistance: props.resistance.or(copy.resistance),
        friction: props.friction.or(copy.friction),
        speed: props.speed.or(copy.speed),
        jump: props.jump.or(copy.jump),
        requires_tool: props.requires_tool || copy.requires_tool,
        random_ticks: props.random_ticks || copy.random_ticks,
        replaceable: props.replaceable || copy.replaceable,
        liquid: props.liquid || copy.liquid,
        ignited_by_lava: props.ignited_by_lava || copy.ignited_by_lava,
        no_occlusion: props.no_occlusion || copy.no_occlusion,
        no_collision: props.no_collision || copy.no_collision,
        offset_type: props.offset_type || copy.offset_type,
        push: props.push.or(copy.push),
        copy_of: None,
    }
}

fn empty_row() -> BTreeMap<&'static str, Value> {
    let mut row = BTreeMap::new();
    for field in gameplay::FIELDS {
        row.insert(field, Value::Null);
    }
    row
}

/// The per-block fields of one block, as the table stores them.
fn block_value_row(props: &Props, maps: &Maps) -> BTreeMap<&'static str, Value> {
    let mut row: BTreeMap<&'static str, Value> = BTreeMap::new();
    row.insert(
        "explosion_resistance",
        float_value(props.resistance.unwrap_or(0.0)),
    );
    row.insert("friction", float_value(props.friction.unwrap_or(0.6)));
    row.insert("hardness", float_value(props.hardness.unwrap_or(0.0)));
    row.insert("ignited_by_lava", Value::Bool(props.ignited_by_lava));
    row.insert("jump_factor", float_value(props.jump.unwrap_or(1.0)));
    row.insert("liquid", Value::Bool(props.liquid));
    let colour = props
        .map_color
        .as_ref()
        .and_then(|name| maps.colours.get(name).copied());
    row.insert("map_color", Value::from(colour.unwrap_or(0) as i64));
    row.insert("offset", Value::Bool(props.offset_type));
    row.insert(
        "push",
        Value::String(props.push.clone().unwrap_or_else(|| "PUSH_PULL".to_string())),
    );
    row.insert("replaceable", Value::Bool(props.replaceable));
    row.insert("requires_tool", Value::Bool(props.requires_tool));
    let sound = props
        .sound
        .as_ref()
        .and_then(|name| maps.sounds.get(name))
        .cloned();
    row.insert("sound", sound.unwrap_or(Value::Null));
    row.insert("speed_factor", float_value(props.speed.unwrap_or(1.0)));
    row
}

/// `interp`'s class loader over the same jar.
struct InterpLoader<'a> {
    jar: &'a Jar,
    cache: RefCell<HashMap<String, Option<classfile::ClassFile>>>,
}

impl<'a> interp::Loader for InterpLoader<'a> {
    fn load(&mut self, name: &str) -> Option<classfile::ClassFile> {
        if let Some(hit) = self.cache.borrow().get(name) {
            return hit.clone();
        }
        let parsed = self.jar.class(name);
        self.cache
            .borrow_mut()
            .insert(name.to_string(), parsed.clone());
        parsed
    }
}

fn blocks_method<'a>(
    class: &'a classfile::ClassFile,
    name: &str,
) -> Option<&'a classfile::Member> {
    class.methods.iter().find(|method| method.name == name)
}

/// The `interp` property model as this module's own.
fn props_from_interp(base: &interp::Properties) -> Props {
    Props {
        map_color: base.map_color.clone(),
        map_color_function: base.map_color_function,
        sound: base.sound.clone(),
        instrument: base.instrument.clone(),
        hardness: base.hardness,
        resistance: base.resistance,
        friction: base.friction,
        speed: base.speed_factor,
        jump: base.jump_factor,
        requires_tool: base.requires_correct_tool,
        random_ticks: base.random_ticks,
        replaceable: base.replaceable,
        liquid: base.liquid.is_some(),
        ignited_by_lava: base.unmodelled.iter().any(|call| call == "ignitedByLava"),
        no_occlusion: base.unmodelled.iter().any(|call| call == "noOcclusion"),
        no_collision: base.no_collision,
        offset_type: base
            .unmodelled
            .iter()
            .any(|call| call == "offsetType" || call == "offset"),
        push: base.push_reaction.clone(),
        copy_of: None,
    }
}

/// Whether `start` or one of its supertypes implements `target` (both internal names).
fn implements(jar: &Jar, start: &str, target: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut queue = vec![start.to_string()];
    while let Some(name) = queue.pop() {
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        if name == target {
            return true;
        }
        let Some(bytes) = jar.bytes(&name) else { continue };
        if let Some(list) = interfaces(bytes) {
            queue.extend(list);
        }
        if let Some(class) = jar.class(&name) {
            if !class.super_name.is_empty() {
                queue.push(class.super_name.clone());
            }
        }
    }
    false
}

fn interfaces(bytes: &[u8]) -> Option<Vec<String>> {
    let mut at = 10usize;
    let count = u16::from_be_bytes([*bytes.get(8)?, *bytes.get(9)?]) as usize;
    let mut pool: Vec<Option<String>> = vec![None; count];
    let mut index = 1;
    while index < count {
        let tag = *bytes.get(at)?;
        at += 1;
        match tag {
            1 => {
                let length = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
                at += 2;
                pool[index] =
                    Some(String::from_utf8_lossy(bytes.get(at..at + length)?).into_owned());
                at += length;
            }
            7 => {
                pool[index] = Some(format!(
                    "#{}",
                    u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?])
                ));
                at += 2;
            }
            8 | 16 | 19 | 20 => at += 2,
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => at += 4,
            5 | 6 => {
                at += 8;
                index += 1;
            }
            15 => at += 3,
            _ => return None,
        }
        index += 1;
    }
    at += 6;
    let count = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
    at += 2;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let class = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
        at += 2;
        let reference = pool.get(class)?.clone()?;
        let inner: usize = reference.trim_start_matches('#').parse().ok()?;
        out.push(pool.get(inner)?.clone()?);
    }
    Some(out)
}

/// The compact document, rendered through [`gameplay`]'s encoder.
pub fn render(facts: &gameplay::Facts) -> String {
    gameplay::render(facts)
}

/// Builds the whole compact document from the jar.
pub fn facts(jar_path: &Path) -> Result<gameplay::Facts, String> {
    let shapes = shape_final::evaluate(jar_path)?;
    let rows = derive(jar_path, Some(&shapes))?;
    let state_table = states::state_table(jar_path)?;
    if shapes.full_block.len() != state_table.state_count() {
        return Err("shape_final does not cover every state".into());
    }
    let fields: Vec<String> = gameplay::FIELDS.iter().map(|field| field.to_string()).collect();
    let mut values = Vec::with_capacity(shapes.full_block.len());
    let mut blocks_per_state = Vec::with_capacity(shapes.full_block.len());
    let mut state = 0usize;
    for (index, row) in state_table.blocks.iter().enumerate() {
        for _ in 0..row.state_count {
            let mut value = Vec::with_capacity(fields.len());
            for field in &fields {
                if field == "full_collision" {
                    value.push(Value::Bool(shapes.full_block[state].unwrap_or(false)));
                } else {
                    value.push(rows.rows[index].get(field.as_str()).cloned().unwrap_or(Value::Null));
                }
            }
            values.push(value);
            blocks_per_state.push(index);
            state += 1;
        }
    }
    Ok(gameplay::Facts {
        fields,
        blocks: rows.blocks,
        values,
        blocks_per_state,
        uncovered: rows.uncovered,
    })
}

/// Builds the table and writes it below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let facts = facts(jar_path)?;
    let coverage = check(&facts);
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, gameplay::render(&facts)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(coverage)
}

/// Whether every block and field is derived; the gaps are reported otherwise.
pub fn check(facts: &gameplay::Facts) -> Coverage {
    let mut coverage = Coverage::default();
    coverage.produced.push(TABLE.to_string());
    coverage.uncovered = facts.uncovered.clone();
    for field in gameplay::MISSING {
        coverage.missing.push(format!("{TABLE}#{field}"));
    }
    if !coverage.uncovered.is_empty() {
        coverage.notes.push(format!(
            "{} of {} blocks have no per-block values",
            coverage.uncovered.len(),
            facts.blocks.len()
        ));
    }
    coverage
}
