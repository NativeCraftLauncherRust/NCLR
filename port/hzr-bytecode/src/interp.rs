//! A small JVM that reads the constants out of `Blocks.<clinit>`.
//!
//! The bytecode builds each block's properties with a fluent chain:
//!
//! ```text
//! Properties.of().mapColor(MapColor.STONE).instrument(BASEDRUM)
//!     .requiresCorrectToolForDrops().strength(1.5f, 6.0f)
//! ```
//!
//! and a block often goes through helpers — `logProperties`, `registerStair`,
//! `ColorCollection.registerBlocks` — which is why this evaluates calls
//! recursively, across classes, and resolves `invokedynamic` to the method the
//! bootstrap names.
//!
//! Anything not modelled becomes `Unknown` rather than a guess, and the calls
//! that were not modelled are reported, so a wrong number can never look like a
//! right one.

use crate::classfile::{self, ClassFile, Constant};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// What a block's properties said, as far as the bytecode shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Properties {
    pub map_color: Option<String>,
    pub instrument: Option<String>,
    pub sound: Option<String>,
    pub hardness: Option<f32>,
    pub resistance: Option<f32>,
    pub requires_correct_tool: bool,
    pub random_ticks: bool,
    pub friction: Option<f32>,
    pub speed_factor: Option<f32>,
    pub jump_factor: Option<f32>,
    pub replaceable: bool,
    pub no_collision: bool,
    pub instabreak: bool,
    pub air: bool,
    pub no_loot_table: bool,
    pub liquid: Option<String>,
    pub push_reaction: Option<String>,
    pub piston_push: Option<String>,
    pub offset: Option<String>,
    pub light_emission: Option<i32>,
    /// `mapColor` given a function instead of a constant — not modelled.
    pub map_color_function: bool,
    /// `lightLevel` given a function that is not a constant.
    pub light_function: bool,
    pub unmodelled: Vec<String>,
}

/// A block built by `<clinit>`.
#[derive(Debug, Clone)]
pub struct Block {
    pub name: String,
    pub class: String,
    pub properties: Properties,
}

#[derive(Debug, Clone)]
enum Value {
    Int(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Str(String),
    /// A static field, such as `MapColor.STONE` or `BuiltInRegistries.BLOCK`.
    Enum {
        class: String,
        field: String,
    },
    /// A registry id: `BlockItemId.create("stone")`.
    Id(String),
    /// One member of a colour collection, such as `ColorCollection.white()`.
    Color(String),
    /// A colour collection: sixteen values in dye order.
    Tuple(Vec<Value>),
    Properties(Properties),
    Block {
        name: String,
        class: String,
        properties: Properties,
    },
    /// A lambda: the method its bootstrap names, plus what it captured.
    Lambda {
        class: Rc<ClassFile>,
        method: String,
        descriptor: String,
        captured: Vec<Value>,
    },
    /// The result of `litBlockEmission(n)`: a light level known to be `n`.
    Emission(i32),
    Null,
    Unknown,
}

/// How deep a call chain may go before it counts as unmodelled.
const MAX_DEPTH: usize = 12;
/// How many instructions one method may run, so a loop cannot hang the tool.
const STEP_LIMIT: usize = 400_000;

/// Loads the classes a call needs, by binary name.
pub trait Loader {
    fn load(&mut self, name: &str) -> Option<ClassFile>;
}

/// A loader that reads class files out of a directory of extracted classes.
pub struct Directory {
    root: PathBuf,
    cache: HashMap<String, Option<Rc<ClassFile>>>,
}

impl Directory {
    pub fn new(root: impl Into<PathBuf>) -> Directory {
        Directory {
            root: root.into(),
            cache: HashMap::new(),
        }
    }
}

impl Loader for Directory {
    fn load(&mut self, name: &str) -> Option<ClassFile> {
        if let Some(hit) = self.cache.get(name) {
            return hit.as_ref().map(|class| (**class).clone());
        }
        // A jar lays classes out by full name; a flat folder of extracted
        // classes uses just the file name. Try both.
        let short = name.rsplit('/').next().unwrap_or(name);
        let parsed = std::fs::read(self.root.join(format!("{name}.class")))
            .or_else(|_| std::fs::read(self.root.join(format!("{short}.class"))))
            .ok()
            .and_then(|bytes| classfile::parse(&bytes).ok());
        self.cache.insert(
            name.to_string(),
            parsed.as_ref().map(|class| Rc::new(class.clone())),
        );
        parsed
    }
}

struct Vm<'a> {
    loader: &'a mut dyn Loader,
    classes: HashMap<String, Rc<ClassFile>>,
    /// `BlockItemIds` field name -> registry name, from that class's own bytes.
    ids: HashMap<String, String>,
    known: HashMap<String, Block>,
    blocks: Vec<Block>,
    unmodelled: HashMap<String, usize>,
    /// The method being evaluated, for reporting where a value was lost.
    here: String,
}

impl<'a> Vm<'a> {
    fn note(&mut self, what: String) {
        *self.unmodelled.entry(what).or_default() += 1;
    }

    fn class(&mut self, name: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.classes.get(name) {
            return Some(hit.clone());
        }
        let loaded = self.loader.load(name)?;
        let shared = Rc::new(loaded);
        self.classes.insert(name.to_string(), shared.clone());
        Some(shared)
    }

    /// Run one method and return its result.
    fn call(
        &mut self,
        class: Rc<ClassFile>,
        method: &classfile::Member,
        arguments: Vec<Value>,
        depth: usize,
    ) -> Value {
        if depth > MAX_DEPTH {
            return Value::Unknown;
        }
        let Some(code) = &method.code else {
            return Value::Unknown;
        };
        let outer = std::mem::replace(&mut self.here, method.name.clone());
        let result = self.body(class, method, code, arguments, depth);
        self.here = outer;
        return result;
    }

    fn body(
        &mut self,
        class: Rc<ClassFile>,
        method: &classfile::Member,
        code: &classfile::Code,
        arguments: Vec<Value>,
        depth: usize,
    ) -> Value {
        let instructions = decode(&code.bytes);
        let mut stack: Vec<Value> = Vec::new();
        let mut locals: Vec<Value> = vec![Value::Unknown; code.max_locals as usize];
        for (slot, value) in arguments.into_iter().enumerate() {
            if let Some(cell) = locals.get_mut(slot) {
                *cell = value;
            }
        }
        let mut steps = 0usize;
        let mut at = 0usize;
        while at < instructions.len() {
            steps += 1;
            if steps > STEP_LIMIT {
                self.note(format!("{}: step limit", method.name));
                return Value::Unknown;
            }
            let instruction = instructions[at].clone();
            at += 1;
            let opcode = instruction.opcode;
            match opcode {
                0x00 => {}
                0x01 => stack.push(Value::Null),
                0x02..=0x08 => stack.push(Value::Int(opcode as i32 - 0x03)),
                0x09..=0x0a => stack.push(Value::Long(opcode as i64 - 0x09)),
                0x0b..=0x0d => stack.push(Value::Float((opcode - 0x0b) as f32)),
                0x0e..=0x0f => stack.push(Value::Double((opcode - 0x0e) as f64)),
                0x10 | 0x11 => stack.push(Value::Int(instruction.immediate.unwrap_or(0))),
                0x12 | 0x13 | 0x14 => {
                    stack.push(constant(&class.constants, instruction.index.unwrap_or(0)))
                }
                0x15..=0x19 => {
                    let slot = instruction.index.unwrap_or(0);
                    stack.push(locals.get(slot).cloned().unwrap_or(Value::Unknown));
                }
                0x1a..=0x1d => {
                    let slot = (opcode - 0x1a) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(Value::Unknown));
                }
                0x2a..=0x2d => {
                    let slot = (opcode - 0x2a) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(Value::Unknown));
                }
                0x36..=0x3a => {
                    let slot = instruction.index.unwrap_or(0);
                    let value = stack.pop().unwrap_or(Value::Unknown);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x3b..=0x3e => {
                    let slot = (opcode - 0x3b) as usize;
                    let value = stack.pop().unwrap_or(Value::Unknown);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x4b..=0x4e => {
                    let slot = (opcode - 0x4b) as usize;
                    let value = stack.pop().unwrap_or(Value::Unknown);
                    if let Some(cell) = locals.get_mut(slot) {
                        *cell = value;
                    }
                }
                0x57 => {
                    stack.pop();
                }
                0x58 => {
                    stack.pop();
                    stack.pop();
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(Value::Unknown);
                    stack.push(value);
                }
                0x5a => {
                    let a = stack.pop().unwrap_or(Value::Unknown);
                    let b = stack.pop().unwrap_or(Value::Unknown);
                    stack.push(a.clone());
                    stack.push(b);
                    stack.push(a);
                }
                0x5c => {
                    let a = stack.pop().unwrap_or(Value::Unknown);
                    let b = stack.pop().unwrap_or(Value::Unknown);
                    stack.push(b.clone());
                    stack.push(a.clone());
                    stack.push(b);
                    stack.push(a);
                }
                0xc0 => {
                    // checkcast: the value keeps its identity.
                }
                0xb2 => {
                    let (owner, name, _) =
                        classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                            .unwrap_or_default();
                    if owner.ends_with("BlockItemIds") {
                        let registry = self.ids.get(&name).cloned().unwrap_or_else(|| lower(&name));
                        stack.push(Value::Id(registry));
                        continue;
                    }
                    if owner.ends_with("Blocks") {
                        if let Some(block) = self.known.get(&name) {
                            stack.push(Value::Block {
                                name: block.name.clone(),
                                class: block.class.clone(),
                                properties: block.properties.clone(),
                            });
                            continue;
                        }
                    }
                    stack.push(Value::Enum {
                        class: owner,
                        field: name,
                    });
                }
                0xb3 => {
                    // `putstatic Blocks.REDSTONE_WIRE` names the block exactly;
                    // the registry id it was registered with can differ
                    // (`BlockItemIds.REDSTONE_DUST`).
                    let popped = stack.pop().unwrap_or(Value::Unknown);
                    if let Value::Block {
                        name,
                        class: block_class,
                        properties,
                    } = popped
                    {
                        if let Some((owner, field, descriptor)) =
                            classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                        {
                            if owner.ends_with("Blocks")
                                && descriptor.starts_with("Lnet/minecraft/world/level/block/Block")
                            {
                                let renamed = lower(&field);
                                if let Some(entry) =
                                    self.blocks.iter_mut().find(|entry| entry.name == name)
                                {
                                    entry.name = renamed.clone();
                                }
                                let built = Block {
                                    name: renamed.clone(),
                                    class: block_class,
                                    properties,
                                };
                                self.known.insert(renamed.clone(), built.clone());
                                stack.push(Value::Block {
                                    name: built.name,
                                    class: built.class,
                                    properties: built.properties,
                                });
                                continue;
                            }
                        }
                        stack.push(Value::Block {
                            name,
                            class: block_class,
                            properties,
                        });
                    }
                }
                0xb8 => {
                    let value = self.invoke_static(
                        class.clone(),
                        instruction.index.unwrap_or(0),
                        &mut stack,
                        depth,
                    );
                    stack.push(value);
                }
                0xbb => {
                    let name = class_name_at(&class.constants, instruction.index.unwrap_or(0));
                    let short = name.rsplit('/').next().unwrap_or(&name).to_string();
                    stack.push(Value::Block {
                        name: String::new(),
                        class: short,
                        properties: Properties::default(),
                    });
                }
                0xb4 | 0xb6 | 0xb7 | 0xb9 => {
                    let value = self.invoke_instance(
                        class.clone(),
                        instruction.index.unwrap_or(0),
                        &mut stack,
                        depth,
                    );
                    stack.push(value);
                }
                0xba => {
                    let count = indy_arguments(&class.constants, instruction.index.unwrap_or(0));
                    let mut captured = Vec::with_capacity(count);
                    for _ in 0..count {
                        captured.push(stack.pop().unwrap_or(Value::Unknown));
                    }
                    captured.reverse();
                    // String concatenation is an invokedynamic too, and block
                    // names in the colour families are built with it.
                    if let Some(text) = concat(
                        &class.constants,
                        &class.bootstraps,
                        instruction.index.unwrap_or(0),
                        &captured,
                    ) {
                        stack.push(Value::Str(text));
                        continue;
                    }
                    stack.push(self.lambda(
                        class.clone(),
                        instruction.index.unwrap_or(0),
                        captured,
                    ));
                }
                0xa7 => {
                    at = instruction.target;
                }
                0x99..=0x9e | 0xa0..=0xa4 | 0xc6 | 0xc7 => {
                    let taken = match opcode {
                        0x99 => int_value(&stack) == Some(0),
                        0x9a => int_value(&stack) != Some(0),
                        0x9b => int_value(&stack).map(|v| v < 0).unwrap_or(false),
                        0x9c => int_value(&stack).map(|v| v >= 0).unwrap_or(false),
                        0x9d => int_value(&stack).map(|v| v > 0).unwrap_or(false),
                        0x9e => int_value(&stack).map(|v| v <= 0).unwrap_or(false),
                        0xa0 => compare(&stack, -1),
                        0xa1 => compare(&stack, 0),
                        0xa2 => compare(&stack, 1),
                        0xa3 => compare(&stack, 0),
                        0xa4 => compare(&stack, 2),
                        0xc6 => matches!(stack.last(), Some(Value::Null)),
                        _ => !matches!(stack.last(), Some(Value::Null)),
                    };
                    stack.pop();
                    if taken {
                        at = instruction.target;
                    }
                }
                0xac..=0xb0 => {
                    return stack.pop().unwrap_or(Value::Unknown);
                }
                0xb1 => return Value::Null,
                _ => {}
            }
        }
        Value::Unknown
    }

    /// The lambda an `invokedynamic` names, resolved through the class's
    /// `BootstrapMethods` attribute.
    fn lambda(&mut self, class: Rc<ClassFile>, index: usize, captured: Vec<Value>) -> Value {
        let Some(Constant::InvokeDynamic { bootstrap, .. }) = class.constants.get(index) else {
            return Value::Unknown;
        };
        let Some(entry) = class.bootstraps.get(*bootstrap) else {
            return Value::Unknown;
        };
        // The implementation is the first MethodHandle among the arguments.
        for argument in &entry.arguments {
            let Some(Constant::MethodHandle { reference }) = class.constants.get(*argument) else {
                continue;
            };
            let Some(Constant::Methodref {
                class: owner, name, ..
            }) = class.constants.get(*reference)
            else {
                continue;
            };
            let owner_index: usize = owner.trim_start_matches('#').parse().unwrap_or(0);
            let nat_index: usize = name.trim_start_matches('#').parse().unwrap_or(0);
            let owner_name = class_name_at(&class.constants, owner_index);
            let (method, descriptor) = match class.constants.get(nat_index) {
                Some(Constant::NameAndType { name, kind }) => {
                    let name_index: usize = name.trim_start_matches('#').parse().unwrap_or(0);
                    let kind_index: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
                    (
                        classfile::utf8(&class.constants, name_index),
                        classfile::utf8(&class.constants, kind_index),
                    )
                }
                _ => continue,
            };
            let target = if owner_name == class.name {
                class.clone()
            } else {
                match self.class(&owner_name) {
                    Some(target) => target,
                    None => continue,
                }
            };
            return Value::Lambda {
                class: target,
                method,
                descriptor,
                captured,
            };
        }
        Value::Unknown
    }

    fn invoke_static(
        &mut self,
        class: Rc<ClassFile>,
        index: usize,
        stack: &mut Vec<Value>,
        depth: usize,
    ) -> Value {
        let (owner, name, descriptor) =
            classfile::method_ref(&class.constants, index).unwrap_or_default();
        let types = classfile::descriptor_types(&descriptor);
        let count = types.len().saturating_sub(1);
        let mut arguments = Vec::with_capacity(count);
        for _ in 0..count {
            arguments.push(stack.pop().unwrap_or(Value::Unknown));
        }
        arguments.reverse();

        if owner.ends_with("Properties") && name == "of" {
            return Value::Properties(Properties::default());
        }
        if owner.ends_with("Properties") && (name == "ofFullCopy" || name == "ofLegacyCopy") {
            return match arguments.first() {
                Some(Value::Properties(properties)) => Value::Properties(properties.clone()),
                Some(Value::Block { properties, .. }) => Value::Properties(properties.clone()),
                _ => Value::Properties(Properties::default()),
            };
        }
        if owner.ends_with("BlockItemId") && name == "create" {
            if let Some(Value::Str(text)) = arguments.first() {
                return Value::Id(text.clone());
            }
            return Value::Unknown;
        }
        if owner.ends_with("Blocks") && name == "register" {
            let mut id = None;
            let mut properties = None;
            for argument in arguments.clone() {
                match argument {
                    Value::Id(text) => id = Some(text),
                    Value::Enum { field, .. } => id = Some(lower(&field)),
                    Value::Properties(found) => properties = Some(found),
                    Value::Block {
                        name,
                        properties: found,
                        ..
                    } => {
                        if !name.is_empty() {
                            id = Some(name);
                        }
                        properties = Some(found);
                    }
                    _ => {}
                }
            }
            if id.is_none() {
                let shape = self.here.clone();
                let _ = &arguments;
                self.note(format!("register without an id (from {shape})"));
            }
            let built = Block {
                name: id.clone().unwrap_or_default(),
                class: "Block".to_string(),
                properties: properties.unwrap_or_default(),
            };
            if let Some(name) = &id {
                self.known.insert(name.clone(), built.clone());
            }
            self.blocks.push(built.clone());
            return Value::Block {
                name: built.name,
                class: built.class,
                properties: built.properties,
            };
        }
        if owner.ends_with("Blocks") && name == "litBlockEmission" {
            if let Some(Value::Int(level)) = arguments.first() {
                return Value::Emission(*level);
            }
            return Value::Unknown;
        }
        // Any other static call: evaluate the target class's method.
        let target = if owner == class.name {
            Some(class)
        } else {
            self.class(&owner)
        };
        if let Some(target) = target {
            if let Some(method) = target
                .methods
                .iter()
                .find(|method| method.name == name)
                .cloned()
            {
                return self.call(target.clone(), &method, arguments, depth + 1);
            }
        }
        self.note(format!("{owner}.{name}"));
        Value::Unknown
    }

    fn invoke_instance(
        &mut self,
        class: Rc<ClassFile>,
        index: usize,
        stack: &mut Vec<Value>,
        depth: usize,
    ) -> Value {
        let (owner, name, descriptor) =
            classfile::method_ref(&class.constants, index).unwrap_or_default();
        let types = classfile::descriptor_types(&descriptor);
        let count = types.len().saturating_sub(1);
        let mut arguments = Vec::with_capacity(count);
        for _ in 0..count {
            arguments.push(stack.pop().unwrap_or(Value::Unknown));
        }
        arguments.reverse();
        let target = stack.pop().unwrap_or(Value::Unknown);

        // A real collection first: it holds the ids the block is registered
        // under. Only an unknown receiver falls back to a bare colour name.
        if let Value::Tuple(items) = &target {
            if let Some(index) = color_index(&name) {
                if let Some(value) = items.get(index) {
                    return value.clone();
                }
            }
            return Value::Unknown;
        }
        // `ColorCollection.VALUES`: its members are the colours themselves.
        if name != "apply" && !name.starts_with("lambda$") && count == 0 {
            if target_is_collection(&owner) {
                return Value::Color(colour_name(&name));
            }
        }
        if name == "apply" || name == "accept" || name == "test" || name == "get" {
            if let Value::Lambda {
                class: lambda_class,
                method,
                captured,
                ..
            } = &target
            {
                let mut all = captured.clone();
                all.extend(arguments.clone());
                let found = lambda_class
                    .methods
                    .iter()
                    .find(|candidate| candidate.name == *method)
                    .cloned();
                if let Some(found) = found {
                    let lambda_class = lambda_class.clone();
                    return self.call(lambda_class, &found, all, depth + 1);
                }
            }
        }
        if name == "<init>" {
            // A `ColorCollection` is a sixteen-field record; keep its contents
            // so `white()`..`black()` can return the real values.
            if owner.ends_with("ColorCollection") && arguments.len() == 16 {
                return Value::Tuple(arguments);
            }
            return target;
        }
        let properties = match target {
            Value::Properties(properties) | Value::Block { properties, .. } => properties,
            Value::Color(color) => {
                // `DyeColor.getName()` gives the piece block names are built
                // from; anything else keeps the colour as its identity.
                if name == "getName" || name == "getSerializedName" || name == "toString" {
                    return Value::Str(color);
                }
                return Value::Color(color);
            }
            Value::Unknown => return Value::Unknown,
            other => return other,
        };
        let updated = mutate(&properties, &name, &arguments);
        Value::Properties(updated)
    }
}

fn target_is_collection(owner: &str) -> bool {
    owner.ends_with("ColorCollection")
}

fn int_value(stack: &[Value]) -> Option<i32> {
    match stack.last() {
        Some(Value::Int(value)) => Some(*value),
        _ => None,
    }
}

/// `-1` for less, `0` for equal, `1` for greater, `2` for not-equal.
fn compare(stack: &[Value], kind: i32) -> bool {
    let b = stack.last();
    let a = stack.iter().rev().nth(1);
    let (Some(Value::Int(a)), Some(Value::Int(b))) = (a, b) else {
        return false;
    };
    match kind {
        -1 => a < b,
        0 => a == b,
        1 => a > b,
        2 => a != b,
        _ => false,
    }
}

/// The dye order a `ColorCollection` record stores its sixteen values in.
const COLORS: [&str; 16] = [
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

/// `ColorCollection` names its accessors in camel case (`lightBlue`); the
/// registry spells the same colour `light_blue`.
fn colour_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for character in name.chars() {
        if character.is_ascii_uppercase() {
            out.push('_');
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

fn color_index(name: &str) -> Option<usize> {
    let normalised = colour_name(name);
    COLORS.iter().position(|color| *color == normalised)
}

fn lower(field: &str) -> String {
    field.to_ascii_lowercase()
}

fn mutate(properties: &Properties, name: &str, arguments: &[Value]) -> Properties {
    let mut out = properties.clone();
    let float_at = |index: usize| -> Option<f32> {
        match arguments.get(index) {
            Some(Value::Float(value)) => Some(*value),
            Some(Value::Double(value)) => Some(*value as f32),
            Some(Value::Int(value)) => Some(*value as f32),
            _ => None,
        }
    };
    let enum_at = |index: usize| -> Option<String> {
        match arguments.get(index) {
            Some(Value::Enum { field, .. }) => Some(field.clone()),
            Some(Value::Str(text)) => Some(text.clone()),
            _ => None,
        }
    };
    match name {
        "strength" => {
            if let Some(value) = float_at(0) {
                out.hardness = Some(value);
                out.resistance = Some(value);
            }
            if let Some(value) = float_at(1) {
                out.resistance = Some(value);
            }
        }
        "destroyTime" => out.hardness = float_at(0),
        "explosionResistance" => out.resistance = float_at(0),
        "mapColor" => match arguments.first() {
            Some(Value::Enum { field, .. }) => out.map_color = Some(field.clone()),
            Some(Value::Str(text)) => out.map_color = Some(text.clone()),
            // A colour-dependent map colour: known to differ, not yet modelled.
            Some(Value::Color(_)) | Some(Value::Lambda { .. }) => out.map_color_function = true,
            _ => out.map_color_function = true,
        },
        "instrument" => out.instrument = enum_at(0),
        "sound" => out.sound = enum_at(0),
        "requiresCorrectToolForDrops" => out.requires_correct_tool = true,
        "randomTicks" => out.random_ticks = true,
        "friction" => out.friction = float_at(0),
        "speedFactor" => out.speed_factor = float_at(0),
        "jumpFactor" => out.jump_factor = float_at(0),
        "replaceable" => out.replaceable = true,
        "noCollision" => out.no_collision = true,
        "instabreak" => out.instabreak = true,
        "air" => out.air = true,
        "noLootTable" => out.no_loot_table = true,
        "liquid" => out.liquid = enum_at(0),
        "pushReaction" => out.push_reaction = enum_at(0),
        "pistonPush" => out.piston_push = enum_at(0),
        "offset" => out.offset = enum_at(0),
        "lightLevel" => match arguments.first() {
            Some(Value::Emission(level)) => out.light_emission = Some(*level),
            Some(Value::Int(level)) => out.light_emission = Some(*level),
            _ => out.light_function = true,
        },
        other => out.unmodelled.push(other.to_string()),
    }
    out
}

fn run(
    loader: &mut dyn Loader,
    ids: &HashMap<String, String>,
) -> (Vec<Block>, HashMap<String, usize>) {
    let Some(class) = loader.load("net/minecraft/world/level/block/Blocks") else {
        return (Vec::new(), HashMap::new());
    };
    let class = Rc::new(class);
    let mut vm = Vm {
        loader,
        classes: HashMap::new(),
        ids: ids.clone(),
        known: HashMap::new(),
        blocks: Vec::new(),
        unmodelled: HashMap::new(),
        here: String::new(),
    };
    vm.classes.insert(
        "net/minecraft/world/level/block/Blocks".to_string(),
        class.clone(),
    );
    if let Some(method) = class.method("<clinit>").cloned() {
        vm.call(class.clone(), &method, Vec::new(), 0);
    }
    (vm.blocks, vm.unmodelled)
}

/// The blocks `Blocks.<clinit>` registers, with a loader for the classes it calls.
pub fn blocks_with(loader: &mut dyn Loader, ids: &HashMap<String, String>) -> Vec<Block> {
    run(loader, ids).0
}

/// The calls the evaluator did not model, and how often.
pub fn unmodelled_with(
    loader: &mut dyn Loader,
    ids: &HashMap<String, String>,
) -> HashMap<String, usize> {
    run(loader, ids).1
}

/// `BlockItemIds.<clinit>`: field name to registry name.
pub fn ids_of(class: &ClassFile) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(method) = class.method("<clinit>") else {
        return out;
    };
    let Some(code) = &method.code else {
        return out;
    };
    let mut pending: Vec<String> = Vec::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) =
                    classfile::string_constant(&class.constants, instruction.index.unwrap_or(0))
                {
                    pending.push(text);
                }
            }
            0xb3 => {
                let (_, name, _) =
                    classfile::field_ref(&class.constants, instruction.index.unwrap_or(0))
                        .unwrap_or_default();
                if let Some(value) = pending.pop() {
                    out.insert(name, value);
                }
            }
            _ => {}
        }
    }
    out
}

fn constant(constants: &[Constant], index: usize) -> Value {
    match constants.get(index) {
        Some(Constant::Integer(value)) => Value::Int(*value),
        Some(Constant::Float(value)) => Value::Float(*value),
        Some(Constant::Long(value)) => Value::Long(*value),
        Some(Constant::Double(value)) => Value::Double(*value),
        Some(Constant::String(_)) | Some(Constant::Utf8(_)) => {
            classfile::string_constant(constants, index)
                .map(Value::Str)
                .unwrap_or(Value::Unknown)
        }
        Some(Constant::Class(_)) => Value::Str(class_name_at(constants, index)),
        _ => Value::Unknown,
    }
}

fn class_name_at(constants: &[Constant], index: usize) -> String {
    match constants.get(index) {
        Some(Constant::Class(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            match constants.get(inner) {
                Some(Constant::Utf8(text)) => text.clone(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    }
}

/// If this call site is `StringConcatFactory.makeConcatWithConstants`, the
/// string it would produce.
fn concat(
    constants: &[Constant],
    bootstraps: &[classfile::Bootstrap],
    index: usize,
    captured: &[Value],
) -> Option<String> {
    let Constant::InvokeDynamic { bootstrap, .. } = constants.get(index)? else {
        return None;
    };
    let entry = bootstraps.get(*bootstrap)?;
    let Constant::MethodHandle { reference } = constants.get(entry.method)? else {
        return None;
    };
    let Constant::Methodref { name, .. } = constants.get(*reference)? else {
        return None;
    };
    let nat: usize = name.trim_start_matches('#').parse().ok()?;
    let (method_name, _) = match constants.get(nat)? {
        Constant::NameAndType { name, kind } => {
            let n: usize = name.trim_start_matches('#').parse().ok()?;
            let k: usize = kind.trim_start_matches('#').parse().ok()?;
            (classfile::utf8(constants, n), classfile::utf8(constants, k))
        }
        _ => return None,
    };
    if method_name != "makeConcatWithConstants" && method_name != "makeConcat" {
        return None;
    }
    // The first bootstrap argument is the recipe: text with \u0001 placeholders.
    let recipe = entry
        .arguments
        .first()
        .and_then(|index| classfile::string_constant(constants, *index))
        .unwrap_or_default();
    let mut out = String::new();
    let mut values = captured.iter();
    for piece in recipe.split('\u{1}') {
        out.push_str(piece);
        match values.next() {
            Some(Value::Str(text)) => out.push_str(text),
            Some(Value::Color(color)) => out.push_str(color),
            Some(Value::Int(number)) => out.push_str(&number.to_string()),
            _ => {}
        }
    }
    Some(out)
}

/// How many values an `invokedynamic` call site takes from the stack.
fn indy_arguments(constants: &[Constant], index: usize) -> usize {
    let Some(Constant::InvokeDynamic { name, .. }) = constants.get(index) else {
        return 0;
    };
    let nat_index: usize = name.trim_start_matches('#').parse().unwrap_or(0);
    let Some(Constant::NameAndType { kind, .. }) = constants.get(nat_index) else {
        return 0;
    };
    let descriptor_index: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
    let descriptor = match constants.get(descriptor_index) {
        Some(Constant::Utf8(text)) => text.clone(),
        _ => return 0,
    };
    classfile::descriptor_types(&descriptor)
        .len()
        .saturating_sub(1)
}

/// A decoded instruction, with branch targets resolved to an index.
#[derive(Debug, Clone)]
pub struct Instruction {
    pub opcode: u8,
    pub index: Option<usize>,
    pub immediate: Option<i32>,
    pub target: usize,
    pub length: usize,
}

/// Decode a method body into instructions, remembering branch targets.
pub fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut raw: Vec<(usize, u8, Option<usize>, Option<i32>, usize, Option<i32>)> = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let start = at;
        let mut index = None;
        let mut immediate = None;
        let mut branch = None;
        let length = match opcode {
            0x12 => {
                index = bytes.get(at + 1).map(|b| *b as usize);
                2
            }
            0x13 | 0x14 => {
                index = Some(u16::from_be_bytes([bytes[at + 1], bytes[at + 2]]) as usize);
                3
            }
            0x10 => {
                immediate = Some(bytes[at + 1] as i8 as i32);
                2
            }
            0x11 => {
                immediate = Some(i16::from_be_bytes([bytes[at + 1], bytes[at + 2]]) as i32);
                3
            }
            0x15..=0x19 | 0x36..=0x3a => {
                index = Some(bytes[at + 1] as usize);
                2
            }
            0x84 => {
                index = Some(bytes[at + 1] as usize);
                immediate = Some(bytes[at + 2] as i8 as i32);
                3
            }
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                branch = Some(i16::from_be_bytes([bytes[at + 1], bytes[at + 2]]) as i32);
                3
            }
            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = Some(u16::from_be_bytes([bytes[at + 1], bytes[at + 2]]) as usize);
                3
            }
            0xb9 | 0xba => {
                index = Some(u16::from_be_bytes([bytes[at + 1], bytes[at + 2]]) as usize);
                5
            }
            0xc5 => 4,
            0xaa | 0xab => break,
            _ => 1,
        };
        raw.push((start, opcode, index, immediate, length, branch));
        at = start + length;
    }
    let offsets: Vec<usize> = raw.iter().map(|entry| entry.0).collect();
    let position = |offset: i32| -> usize {
        let wanted = offset as usize;
        offsets
            .iter()
            .position(|start| *start == wanted)
            .unwrap_or(offsets.len())
    };
    raw.iter()
        .map(
            |(start, opcode, index, immediate, length, branch)| Instruction {
                opcode: *opcode,
                index: *index,
                immediate: *immediate,
                target: branch
                    .map(|delta| position((*start as i32 + delta) as i32))
                    .unwrap_or(0),
                length: *length,
            },
        )
        .collect()
}

/// Where the class files come from, when the caller has a folder of them.
pub fn directory(root: impl AsRef<Path>) -> Directory {
    Directory::new(root.as_ref())
}
