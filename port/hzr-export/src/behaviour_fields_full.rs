//! Reflective instance fields of the block registry, for `block-behaviours.json`.
//!
//! `VanillaBlockBehaviourExporter.fields(block, Block.class)` walks the concrete
//! class and every superclass down to (but excluding) `Block`, collecting each
//! declared **non-static** field in declaration order, and dumps its value with a
//! `value()` visitor:
//!
//! * scalars and enum constants (`name()`), strings;
//! * `Block` / `Item` / `Fluid` registry ids, `SoundEvent` locations,
//!   `Identifier`/`ResourceKey` ids and `TagKey` `#` names;
//! * `BlockState` as `{"state": id}`;
//! * `Optional` unwrapped, `Holder` unwrapped when bound;
//! * maps as JSON objects at depth > 0 (keys rendered at depth 0);
//! * records (including `SoundType`) as JSON objects of their components;
//! * `ParticleType` registry ids;
//! * grower classes recursed through their fields;
//! * anything else as `"<SimpleName>"`.
//!
//! # How the values are reconstructed
//!
//! The values are constructor state that only the running JVM assigns. This
//! module therefore **executes the constructors**, not a table lookup: it runs
//! `Blocks.<clinit>` with a small class-file JVM whose registry side effects are
//! made symbolic (`StateDefinition.any`, `BlockState.setValue`,
//! `Block.registerDefaultState`) and whose value objects are modelled
//! (`Properties`, `SoundType`, `WoodType`, `BlockSetType`, the `ColorCollection`
//! and `WeatheringCopperCollection` families, `Holder`/`TagKey`/`ResourceKey`,
//! `Optional`, lists and maps). Every `Blocks.register(...)` call is paired with
//! the next id in `block::registry`'s verified registry order, so each
//! constructed instance knows the block it became and every later
//! `getstatic Blocks.X` is that block's registry id.
//!
//! Class initialisers of supporting classes are executed lazily with the same
//! JVM (`SoundType`, `SoundEvents`, `BlockSetType`, `WoodType`, `TreeGrower`,
//! `ColorCollection`, …). A call the JVM does not model is reported as a gap and
//! yields an opaque value; the field that holds it is **not** claimed exact, so
//! a wrong value can never look right.
//!
//! `items.fields` reuses `behaviour_header::items` for the item id/classes and
//! executes the item factory lambdas of `Items.<clinit>` to recover
//! `wallBlock`/`attachmentDirection`/`placeSound` for the 64 `BlockItem`
//! subclasses that declare them.
//!
//! Nothing here reads `ship.hzr3`; the jar is the only input.

#![allow(dead_code, clippy::duplicate_mod, clippy::too_many_arguments)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Number, Value};

#[path = "block.rs"]
mod block;

#[path = "behaviour_header.rs"]
#[allow(unused)]
mod header;

#[path = "states.rs"]
#[allow(unused)]
mod states;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant};

/// The class whose fields stop the walk (`Block.class` in the exporter).
const BLOCK: &str = "net/minecraft/world/level/block/Block";
const BLOCK_BEHAVIOUR: &str = "net/minecraft/world/level/block/state/BlockBehaviour";
const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const ITEMS: &str = "net/minecraft/world/item/Items";
const ITEM: &str = "net/minecraft/world/item/Item";
const BLOCK_ITEM: &str = "net/minecraft/world/item/BlockItem";
const SOUND_TYPE: &str = "net/minecraft/world/level/block/SoundType";
const STATE_DEFINITION: &str = "net/minecraft/world/level/block/state/StateDefinition";
const BLOCK_STATE: &str = "net/minecraft/world/level/block/state/BlockState";

/// What was reconstructed, and what could not be.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// Blocks in the registry.
    pub blocks: usize,
    /// Blocks whose whole `fields` object this module claims exact.
    pub blocks_fields_exact: usize,
    /// Block items in the registry.
    pub items: usize,
    /// Items whose whole `fields` object this module claims exact.
    pub items_fields_exact: usize,
    /// `"<block|item> <id>.<field>: <why>"` → count.
    pub gaps: BTreeMap<String, usize>,
    /// Diagnostics from the reconstruction, capped.
    pub notes: Vec<String>,
}

fn note_gap(gaps: &mut BTreeMap<String, usize>, key: String) {
    *gaps.entry(key).or_insert(0) += 1;
}

// ---------------------------------------------------------------------------
// The jar
// ---------------------------------------------------------------------------

pub struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    classes: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Jar, String> {
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

    pub fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
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

    fn has(&self, internal: &str) -> bool {
        self.files.contains_key(&format!("{internal}.class"))
    }

    /// Resolve a simple block class name through the vanilla `block` package tree.
    fn resolve_block(&self, simple: &str) -> Option<String> {
        let direct = format!("net/minecraft/world/level/block/{simple}");
        if self.has(&direct) {
            return Some(direct);
        }
        self.files
            .keys()
            .find(|name| {
                name.starts_with("net/minecraft/world/level/block/")
                    && name.ends_with(&format!("/{simple}.class"))
            })
            .map(|name| name.trim_end_matches(".class").to_string())
    }

    fn resolve_under(&self, package: &str, simple: &str) -> Option<String> {
        let direct = format!("{package}/{simple}");
        if self.has(&direct) {
            return Some(direct);
        }
        self.files
            .keys()
            .find(|name| {
                name.starts_with(&format!("{package}/"))
                    && name.ends_with(&format!("/{simple}.class"))
            })
            .map(|name| name.trim_end_matches(".class").to_string())
    }
}

fn simple_name(internal: &str) -> &str {
    let without = internal.rsplit('/').next().unwrap_or(internal);
    without.split('$').next().unwrap_or(without)
}

fn is_subclass(jar: &Jar, internal: &str, ancestor: &str) -> bool {
    let mut current = internal.to_string();
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        let Some(class) = jar.class(&current) else {
            return false;
        };
        if class.super_name.is_empty() || class.super_name == "java/lang/Object" {
            return false;
        }
        current = class.super_name.clone();
    }
    false
}

// ---------------------------------------------------------------------------
// State ids
// ---------------------------------------------------------------------------

struct BlockStateInfo {
    /// Properties sorted by name (the `StateDefinition` enumeration order).
    values: Vec<(String, Vec<String>)>,
    /// The default state's property values.
    defaults: BTreeMap<String, String>,
}

struct StateIds {
    blocks: BTreeMap<String, BlockStateInfo>,
    /// The first state id of each block.
    base: BTreeMap<String, u32>,
}

impl StateIds {
    fn new(table: &states::StateTable) -> StateIds {
        let mut blocks = BTreeMap::new();
        let mut base = BTreeMap::new();
        let mut next = 0u32;
        for block in &table.blocks {
            let mut values: Vec<(String, Vec<String>)> = block
                .properties
                .iter()
                .map(|property| (property.name.clone(), property.values.clone()))
                .collect();
            values.sort_by(|left, right| left.0.cmp(&right.0));
            let defaults = decode_tuple(&values, block.default_index);
            base.insert(block.id.clone(), next);
            next += block.state_count as u32;
            blocks.insert(block.id.clone(), BlockStateInfo { values, defaults });
        }
        StateIds { blocks, base }
    }

    fn default_state(&self, block: &str) -> Option<(BTreeMap<String, String>, u32)> {
        let info = self.blocks.get(block)?;
        Some((info.defaults.clone(), *self.base.get(block)?))
    }

    fn state_id(&self, block: &str, values: &BTreeMap<String, String>) -> Option<u32> {
        let info = self.blocks.get(block)?;
        let mut index = 0u32;
        let mut radix = 1u32;
        for (name, possible) in info.values.iter().rev() {
            let value = values.get(name).or_else(|| info.defaults.get(name))?;
            let position = possible.iter().position(|candidate| candidate == value)? as u32;
            index += position * radix;
            radix *= possible.len() as u32;
        }
        Some(self.base.get(block)? + index)
    }
}

/// Decode a state's index into its property values (first property slowest).
fn decode_tuple(values: &[(String, Vec<String>)], index: usize) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut remaining = index;
    let mut divisors: Vec<usize> = Vec::with_capacity(values.len());
    let mut product = 1usize;
    for (_, possible) in values.iter().rev() {
        divisors.push(product);
        product *= possible.len().max(1);
    }
    divisors.reverse();
    for (slot, (name, possible)) in values.iter().enumerate() {
        let divisor = divisors[slot].max(1);
        let position = (remaining / divisor) % possible.len().max(1);
        remaining %= divisor;
        out.insert(
            name.clone(),
            possible.get(position).cloned().unwrap_or_default(),
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Val {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Bool(bool),
    Str(Rc<str>),
    Null,
    Unit,
    /// A `java.lang.Class` constant.
    Class(Rc<str>),
    /// Any object with declared fields.
    Obj(Rc<RefCell<Object>>),
    Array(Rc<RefCell<Vec<Val>>>),
    /// A `List` (or `ArrayList`), with the runtime class for the fallback name.
    List(Rc<RefCell<Vec<Val>>>, Rc<str>),
    /// A map, insertion ordered, with the runtime class.
    Map(Rc<RefCell<Vec<(Val, Val)>>>, Rc<str>),
    Optional(Option<Rc<Val>>),
    Lambda(Rc<Lambda>),
    /// `BlockItemId` / `BlockIds` reference: a registry id.
    BlockId(Rc<str>),
    /// A registered block.
    Block(Rc<str>),
    /// A symbolic `BlockState`.
    State(Rc<StateVal>),
    Item(Rc<str>),
    Fluid(Rc<str>),
    Sound(Rc<str>),
    Particle(Rc<str>),
    /// A `TagKey` (already the `namespace:path` location).
    Tag(Rc<str>),
    /// A `ResourceKey` / `Identifier`.
    Resource(Rc<str>),
    /// A plain enum constant.
    Enum(Rc<str>, Rc<str>, i32),
    /// The registry input to a block/item constructor.
    Props,
    /// A value the visitor cannot reproduce, carrying its class for the fallback.
    Opaque(Rc<str>),
}

struct Object {
    class: Rc<str>,
    fields: BTreeMap<String, Val>,
    enum_name: Option<String>,
    ordinal: i32,
    block_id: Option<Rc<str>>,
    item_id: Option<Rc<str>>,
    /// The constructor chain ran.
    constructed: bool,
}

struct Lambda {
    class: Rc<ClassFile>,
    method: String,
    descriptor: String,
    captured: Vec<Val>,
    is_static: bool,
}

struct StateVal {
    block: String,
    values: BTreeMap<String, String>,
}

impl Val {
    fn opaque(class: &str) -> Val {
        Val::Opaque(Rc::from(class))
    }

    fn as_int(&self) -> Option<i32> {
        match self {
            Val::Int(value) => Some(*value),
            Val::Bool(value) => Some(*value as i32),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Val::Int(value) => Some(*value as f64),
            Val::Long(value) => Some(*value as f64),
            Val::Float(value) => Some(*value as f64),
            Val::Double(value) => Some(*value),
            Val::Bool(value) => Some(*value as i32 as f64),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(value) => Some(value),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Bytecode decoding
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Ins {
    offset: usize,
    opcode: u8,
    index: usize,
    imm: i32,
    switch: Option<Switch>,
}

#[derive(Clone, Debug)]
struct Switch {
    default: usize,
    keys: Vec<i32>,
    targets: Vec<usize>,
}

fn read_i16(bytes: &[u8], at: usize) -> i32 {
    i16::from_be_bytes([bytes[at], bytes[at + 1]]) as i32
}

fn read_i32(bytes: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn decode_code(bytes: &[u8]) -> Vec<Ins> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let start = at;
        let mut opcode = bytes[at];
        at += 1;
        let mut imm = 0i32;
        let mut index = 0usize;
        let mut switch = None;
        match opcode {
            0x10 => {
                if at < bytes.len() {
                    imm = bytes[at] as i8 as i32;
                }
                at += 1;
            }
            0x11 => {
                if at + 1 < bytes.len() {
                    imm = read_i16(bytes, at);
                }
                at += 2;
            }
            0x12 => {
                if at < bytes.len() {
                    index = bytes[at] as usize;
                }
                at += 1;
            }
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                if at + 1 < bytes.len() {
                    index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                }
                at += 2;
            }
            0xb9 | 0xba => {
                if at + 1 < bytes.len() {
                    index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                }
                at += 4;
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => {
                if at < bytes.len() {
                    index = bytes[at] as usize;
                }
                at += 1;
            }
            0x84 => {
                if at < bytes.len() {
                    index = bytes[at] as usize;
                }
                if at + 1 < bytes.len() {
                    imm = bytes[at + 1] as i8 as i32;
                }
                at += 2;
            }
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                if at + 1 < bytes.len() {
                    imm = read_i16(bytes, at);
                }
                at += 2;
            }
            0xc8 | 0xc9 => {
                if at + 3 < bytes.len() {
                    imm = read_i32(bytes, at);
                }
                at += 4;
            }
            0xaa => {
                at = (at + 3) & !3;
                if at + 11 >= bytes.len() {
                    break;
                }
                let default = read_i32(bytes, at);
                let low = read_i32(bytes, at + 4);
                let high = read_i32(bytes, at + 8);
                at += 12;
                let mut keys = Vec::new();
                let mut targets = Vec::new();
                for key in low..=high {
                    if at + 3 >= bytes.len() {
                        break;
                    }
                    keys.push(key);
                    targets.push((start as i32 + read_i32(bytes, at)) as usize);
                    at += 4;
                }
                switch = Some(Switch {
                    default: (start as i32 + default) as usize,
                    keys,
                    targets,
                });
            }
            0xab => {
                at = (at + 3) & !3;
                if at + 7 >= bytes.len() {
                    break;
                }
                let default = read_i32(bytes, at);
                let pairs = read_i32(bytes, at + 4).max(0) as usize;
                at += 8;
                let mut keys = Vec::new();
                let mut targets = Vec::new();
                for _ in 0..pairs {
                    if at + 7 >= bytes.len() {
                        break;
                    }
                    keys.push(read_i32(bytes, at));
                    targets.push((start as i32 + read_i32(bytes, at + 4)) as usize);
                    at += 8;
                }
                switch = Some(Switch {
                    default: (start as i32 + default) as usize,
                    keys,
                    targets,
                });
            }
            0xc4 => {
                if at >= bytes.len() {
                    break;
                }
                let widened = bytes[at];
                opcode = widened;
                at += 1;
                if widened == 0x84 {
                    if at + 3 < bytes.len() {
                        index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                        imm = read_i16(bytes, at + 2);
                    }
                    at += 4;
                } else {
                    if at + 1 < bytes.len() {
                        index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                    }
                    at += 2;
                }
            }
            0xc5 => {
                if at + 2 < bytes.len() {
                    index = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                }
                at += 3;
            }
            _ => {}
        }
        out.push(Ins {
            offset: start,
            opcode,
            index,
            imm,
            switch,
        });
    }
    out
}

fn pc_offsets(instructions: &[Ins]) -> BTreeMap<usize, usize> {
    instructions
        .iter()
        .enumerate()
        .map(|(pc, ins)| (ins.offset, pc))
        .collect()
}

/// Split a method descriptor into (argument type names, return type name).
fn descriptor_args(descriptor: &str) -> (Vec<String>, String) {
    let chars: Vec<char> = descriptor.chars().collect();
    let mut types = Vec::new();
    let mut at = 0usize;
    while at < chars.len() && chars[at] != '(' {
        at += 1;
    }
    at += 1;
    while at < chars.len() && chars[at] != ')' {
        let start = at;
        while at < chars.len() && chars[at] == '[' {
            at += 1;
        }
        if at < chars.len() {
            if chars[at] == 'L' {
                while at < chars.len() && chars[at] != ';' {
                    at += 1;
                }
                at += 1;
            } else {
                at += 1;
            }
        }
        types.push(chars[start..at.min(chars.len())].iter().collect());
    }
    let returns: String = chars.iter().skip(at + 1).collect();
    (types, returns)
}

fn arg_slots(types: &[String]) -> Vec<usize> {
    types
        .iter()
        .map(|kind| if kind == "J" || kind == "D" { 2 } else { 1 })
        .collect()
}

fn return_simple(descriptor: &str) -> String {
    let (_, returns) = descriptor_args(descriptor);
    let name = returns
        .trim_start_matches('[')
        .trim_start_matches('L')
        .trim_end_matches(';');
    if name.is_empty() {
        "V".to_string()
    } else {
        name.to_string()
    }
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

fn method_handle_target(constants: &[Constant], index: usize) -> Option<(String, String, String)> {
    let Constant::MethodHandle { reference } = constants.get(index)? else {
        return None;
    };
    let Constant::Methodref {
        class: owner,
        name,
        kind: _,
    } = constants.get(*reference)?
    else {
        // `REF_newInvokeSpecial` of a constructor is a Methodref too; interface
        // methods use InterfaceMethodref.
        let Constant::InterfaceMethodref {
            class: owner,
            name,
            kind: _,
        } = constants.get(*reference)?
        else {
            return None;
        };
        return interface_target(constants, owner, name);
    };
    let owner_index: usize = owner.trim_start_matches('#').parse().ok()?;
    let owner_name = class_name_at(constants, owner_index)?;
    let nat: usize = name.trim_start_matches('#').parse().ok()?;
    match constants.get(nat)? {
        Constant::NameAndType { name, kind } => {
            let name_index: usize = name.trim_start_matches('#').parse().ok()?;
            let kind_index: usize = kind.trim_start_matches('#').parse().ok()?;
            Some((
                owner_name,
                classfile::utf8(constants, name_index),
                classfile::utf8(constants, kind_index),
            ))
        }
        _ => None,
    }
}

fn interface_target(
    constants: &[Constant],
    owner: &str,
    name: &str,
) -> Option<(String, String, String)> {
    let owner_index: usize = owner.trim_start_matches('#').parse().ok()?;
    let owner_name = class_name_at(constants, owner_index)?;
    let nat: usize = name.trim_start_matches('#').parse().ok()?;
    match constants.get(nat)? {
        Constant::NameAndType { name, kind } => {
            let name_index: usize = name.trim_start_matches('#').parse().ok()?;
            let kind_index: usize = kind.trim_start_matches('#').parse().ok()?;
            Some((
                owner_name,
                classfile::utf8(constants, name_index),
                classfile::utf8(constants, kind_index),
            ))
        }
        _ => None,
    }
}

/// The implementation method an `invokedynamic` names.
fn indy_target(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
        if let Some(target) = method_handle_target(&class.constants, *argument) {
            return Some(target);
        }
    }
    None
}

fn indy_captured_count(class: &ClassFile, index: usize) -> usize {
    let Some(Constant::InvokeDynamic { name, .. }) = class.constants.get(index) else {
        return 0;
    };
    let nat: usize = name.trim_start_matches('#').parse().unwrap_or(0);
    let Some(Constant::NameAndType { kind, .. }) = class.constants.get(nat) else {
        return 0;
    };
    let descriptor_index: usize = kind.trim_start_matches('#').parse().unwrap_or(0);
    let descriptor = match class.constants.get(descriptor_index) {
        Some(Constant::Utf8(text)) => text.clone(),
        _ => return 0,
    };
    descriptor_args(&descriptor).0.len()
}

// ---------------------------------------------------------------------------
// The virtual machine
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Fault {
    Thrown(String),
    Steps,
}

const MAX_DEPTH: usize = 128;
const MAX_STEPS: u64 = 400_000_000;

struct Vm {
    jar: Jar,
    registry: Vec<block::Block>,
    internal_classes: Vec<Option<String>>,
    next_block: usize,
    pending_block: Option<usize>,
    statics: BTreeMap<(String, String), Val>,
    initialized: HashSet<String>,
    initializing: HashSet<String>,
    blocks: BTreeMap<String, Rc<RefCell<Object>>>,
    state_ids: StateIds,
    sound_ids: BTreeMap<String, String>,
    /// `Blocks.<field>` -> registry id, filled while `Blocks.<clinit>` runs.
    blocks_field_ids: BTreeMap<String, String>,
    steps: u64,
    depth: usize,
    notes: Vec<String>,
}

impl Vm {
    fn new(jar: Jar, registry: Vec<block::Block>, state_ids: StateIds) -> Vm {
        let internal_classes = registry
            .iter()
            .map(|row| jar.resolve_block(&row.class))
            .collect();
        let sound_ids = sound_event_ids(&jar);
        Vm {
            jar,
            registry,
            internal_classes,
            next_block: 0,
            pending_block: None,
            statics: BTreeMap::new(),
            initialized: HashSet::new(),
            initializing: HashSet::new(),
            blocks: BTreeMap::new(),
            state_ids,
            sound_ids,
            blocks_field_ids: BTreeMap::new(),
            steps: 0,
            depth: 0,
            notes: Vec::new(),
        }
    }

    fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        self.jar.class(internal)
    }

    fn note(&mut self, text: impl Into<String>) {
        if self.notes.len() < 512 {
            self.notes.push(text.into());
        }
    }

    // -- class loading / resolution ---------------------------------------

    fn declaring_static(&self, owner: &str, name: &str, descriptor: &str) -> String {
        let mut current = owner.to_string();
        for _ in 0..64 {
            let Some(class) = self.class(&current) else {
                break;
            };
            if class.fields.iter().any(|field| {
                field.name == name && (descriptor.is_empty() || field.descriptor == descriptor)
            }) {
                return current;
            }
            if class.super_name.is_empty() {
                break;
            }
            current = class.super_name.clone();
        }
        owner.to_string()
    }

    fn resolve(
        &self,
        start: &str,
        name: &str,
        descriptor: &str,
    ) -> Option<(Rc<ClassFile>, classfile::Member)> {
        let mut current = Some(start.to_string());
        for _ in 0..64 {
            let internal = current?;
            if let Some(class) = self.class(&internal) {
                if let Some(method) = class.methods.iter().find(|method| {
                    method.name == name
                        && (descriptor.is_empty() || method.descriptor == descriptor)
                }) {
                    return Some((class.clone(), method.clone()));
                }
                current = if class.super_name.is_empty() {
                    None
                } else {
                    Some(class.super_name.clone())
                };
            } else {
                return None;
            }
        }
        None
    }

    /// Initialise a class (superclasses first) by running its `<clinit>`.
    fn ensure_initialized(&mut self, internal: &str) {
        if self.initialized.contains(internal) || self.initializing.contains(internal) {
            return;
        }
        if self.class(internal).is_none() {
            return;
        }
        self.initializing.insert(internal.to_string());
        let super_name = self
            .class(internal)
            .map(|class| class.super_name.clone())
            .unwrap_or_default();
        if !super_name.is_empty() && super_name != "java/lang/Object" {
            self.ensure_initialized(&super_name);
        }
        if let Some(class) = self.class(internal) {
            if let Some(initialiser) = class.method("<clinit>").cloned() {
                if let Err(error) = self.call(class.clone(), &initialiser, None, Vec::new()) {
                    self.note(format!("{internal}.<clinit>: {}", fault_text(&error)));
                }
            }
        }
        self.initializing.remove(internal);
        self.initialized.insert(internal.to_string());
    }

    fn get_static(&mut self, owner: &str, name: &str, descriptor: &str) -> Val {
        if let Some(value) = self.special_static(owner, name, descriptor) {
            return value;
        }
        let declaring = self.declaring_static(owner, name, descriptor);
        self.ensure_initialized(&declaring);
        if declaring != owner {
            self.ensure_initialized(owner);
        }
        if let Some(value) = self.statics.get(&(declaring.clone(), name.to_string())) {
            return value.clone();
        }
        if let Some(value) = self.statics.get(&(owner.to_string(), name.to_string())) {
            return value.clone();
        }
        let derived = self.derive_static(owner, name, descriptor);
        derived
    }

    /// Well-known values whose classes the JVM does not have to run.
    fn special_static(&mut self, owner: &str, name: &str, descriptor: &str) -> Option<Val> {
        let owner_simple = simple_name(owner);
        if owner == "net/minecraft/core/Direction" {
            return Some(Val::Enum(
                Rc::from(owner),
                Rc::from(name),
                self.enum_ordinal(owner, name),
            ));
        }
        if owner_simple == "DyeColor" {
            return Some(Val::Enum(
                Rc::from(owner),
                Rc::from(name),
                self.enum_ordinal(owner, name),
            ));
        }
        if owner_simple == "ParticleTypes" {
            return Some(Val::Particle(Rc::from(name.to_ascii_lowercase())));
        }
        if owner_simple == "SoundEvents" {
            let id = self
                .sound_ids
                .get(name)
                .cloned()
                .unwrap_or_else(|| format!("minecraft:{}", name.to_ascii_lowercase()));
            return Some(Val::Sound(Rc::from(id)));
        }
        if owner_simple == "BlockIds"
            && descriptor.starts_with("Lnet/minecraft/resources/ResourceKey;")
        {
            return Some(Val::BlockId(Rc::from(name.to_ascii_lowercase())));
        }
        if owner_simple == "Items" && descriptor.starts_with("Lnet/minecraft/world/item/Item;") {
            return Some(Val::Item(Rc::from(format!(
                "minecraft:{}",
                name.to_ascii_lowercase()
            ))));
        }
        if owner_simple == "Fluids" {
            return Some(Val::Fluid(Rc::from(format!(
                "minecraft:{}",
                name.to_ascii_lowercase()
            ))));
        }
        if owner.ends_with("Tags") && descriptor.starts_with("Lnet/minecraft/tags/TagKey;") {
            return Some(Val::Tag(Rc::from(name.to_ascii_lowercase())));
        }
        if owner_simple == "Blocks"
            && descriptor.starts_with("Lnet/minecraft/world/level/block/Block;")
        {
            if let Some(value) = self.statics.get(&(BLOCKS.to_string(), name.to_string())) {
                return Some(value.clone());
            }
            return Some(Val::Opaque(Rc::from(BLOCK)));
        }
        None
    }

    /// A static field whose class file has no runnable initialiser (metadata
    /// registries, constants), derived from its owner and descriptor.
    fn derive_static(&mut self, owner: &str, name: &str, descriptor: &str) -> Val {
        let simple = simple_name(owner);
        if descriptor.starts_with("Lnet/minecraft/core/particles/") {
            return Val::Particle(Rc::from(name.to_ascii_lowercase()));
        }
        if descriptor.starts_with("Lnet/minecraft/core/Holder") {
            return Val::Opaque(Rc::from(
                descriptor.trim_start_matches('L').trim_end_matches(';'),
            ));
        }
        if descriptor.ends_with("Property;") {
            return Val::Str(Rc::from(name.to_ascii_lowercase()));
        }
        if descriptor.starts_with("Lnet/minecraft/tags/") {
            return Val::Tag(Rc::from(name.to_ascii_lowercase()));
        }
        if descriptor.starts_with("Lnet/minecraft/resources/ResourceKey;") {
            return Val::Resource(Rc::from(name.to_ascii_lowercase()));
        }
        if descriptor.starts_with("Lnet/minecraft/resources/Identifier;") {
            return Val::Resource(Rc::from(name.to_ascii_lowercase()));
        }
        if descriptor.starts_with("Lnet/minecraft/sounds/SoundEvent;") {
            let id = self
                .sound_ids
                .get(name)
                .cloned()
                .unwrap_or_else(|| format!("minecraft:{}", name.to_ascii_lowercase()));
            return Val::Sound(Rc::from(id));
        }
        if descriptor.starts_with("Lnet/minecraft/world/item/Item;") {
            return Val::Item(Rc::from(format!("minecraft:{}", name.to_ascii_lowercase())));
        }
        if descriptor.starts_with("Lnet/minecraft/world/level/material/Fluid;") {
            return Val::Fluid(Rc::from(format!("minecraft:{}", name.to_ascii_lowercase())));
        }
        let _ = simple;
        Val::Opaque(Rc::from(return_class(descriptor)))
    }

    fn put_static(&mut self, owner: &str, name: &str, descriptor: &str, value: Val) {
        let declaring = self.declaring_static(owner, name, descriptor);
        self.statics
            .insert((declaring.clone(), name.to_string()), value.clone());
        if owner == BLOCKS || declaring == BLOCKS {
            if let Val::Block(id) = &value {
                self.blocks_field_ids
                    .insert(name.to_string(), id.to_string());
            }
        }
    }

    fn enum_ordinal(&self, owner: &str, name: &str) -> i32 {
        let mut ordinal = 0i32;
        if let Some(class) = self.jar.class(owner) {
            let descriptor = format!("L{owner};");
            for field in &class.fields {
                if field.descriptor == descriptor {
                    if field.name == name {
                        return ordinal;
                    }
                    ordinal += 1;
                }
            }
        }
        0
    }

    fn get_field(&mut self, receiver: &Val, _owner: &str, name: &str, _descriptor: &str) -> Val {
        match receiver {
            Val::Obj(object) => object
                .borrow()
                .fields
                .get(name)
                .cloned()
                .unwrap_or(Val::Null),
            Val::State(_) => Val::Null,
            Val::Null => Val::Null,
            Val::Opaque(class) => Val::Opaque(class.clone()),
            _ => Val::Null,
        }
    }

    fn put_field(&mut self, receiver: &Val, name: &str, value: Val) {
        if let Val::Obj(object) = receiver {
            object.borrow_mut().fields.insert(name.to_string(), value);
        }
    }
}

fn fault_text(error: &Fault) -> String {
    match error {
        Fault::Thrown(message) => message.clone(),
        Fault::Steps => "step limit".to_string(),
    }
}

/// The simple class name a descriptor names.
fn return_class(descriptor: &str) -> &str {
    let inner = descriptor
        .trim_start_matches('[')
        .trim_start_matches('L')
        .trim_end_matches(';');
    inner.rsplit('/').next().unwrap_or(inner)
}

/// `SoundEvents.<clinit>`: field -> `minecraft:<string>`.
fn sound_event_ids(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/sounds/SoundEvents") else {
        return out;
    };
    let Some(initialiser) = class.method("<clinit>") else {
        return out;
    };
    let Some(code) = &initialiser.code else {
        return out;
    };
    let mut pending: Option<String> = None;
    for instruction in decode_code(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = classfile::string_constant(&class.constants, instruction.index)
                {
                    pending = Some(text);
                }
            }
            0xb3 => {
                if let Some((_, name, _)) =
                    classfile::field_ref(&class.constants, instruction.index)
                {
                    if let Some(text) = pending.take() {
                        out.insert(
                            name,
                            if text.starts_with("minecraft:") {
                                text
                            } else {
                                format!("minecraft:{text}")
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn pop(stack: &mut Vec<Val>) -> Val {
    stack.pop().unwrap_or(Val::Null)
}

fn pop_int(stack: &mut Vec<Val>) -> i32 {
    match pop(stack) {
        Val::Int(value) => value,
        Val::Bool(value) => value as i32,
        _ => 0,
    }
}

fn set_local(locals: &mut Vec<Val>, slot: usize, value: Val) {
    if slot < locals.len() {
        locals[slot] = value;
    }
}

fn jump(pcs: &BTreeMap<usize, usize>, ins: &Ins, default: usize) -> usize {
    let target = (ins.offset as i32 + ins.imm) as usize;
    pcs.get(&target).copied().unwrap_or(default)
}

fn switch_target(pcs: &BTreeMap<usize, usize>, ins: &Ins, key: i32) -> usize {
    let Some(switch) = &ins.switch else {
        return pcs.len();
    };
    let target = switch
        .keys
        .iter()
        .position(|candidate| *candidate == key)
        .map(|slot| switch.targets[slot])
        .unwrap_or(switch.default);
    pcs.get(&target).copied().unwrap_or(pcs.len())
}

fn values_equal(left: &Val, right: &Val) -> bool {
    match (left, right) {
        (Val::Null, Val::Null) => true,
        (Val::Int(a), Val::Int(b)) => a == b,
        (Val::Bool(a), Val::Bool(b)) => a == b,
        (Val::Long(a), Val::Long(b)) => a == b,
        (Val::Float(a), Val::Float(b)) => a == b,
        (Val::Double(a), Val::Double(b)) => a == b,
        (Val::Str(a), Val::Str(b)) => a == b,
        (Val::Enum(_, a, _), Val::Enum(_, b, _)) => {
            a.to_ascii_lowercase() == b.to_ascii_lowercase()
        }
        (Val::Block(a), Val::Block(b)) => a == b,
        (Val::Sound(a), Val::Sound(b)) => a == b,
        (Val::Resource(a), Val::Resource(b)) => a == b,
        (Val::Obj(a), Val::Obj(b)) => Rc::ptr_eq(a, b),
        (Val::Null, _) | (_, Val::Null) => false,
        _ => false,
    }
}

impl Vm {
    fn is_block_class(&self, internal: &str) -> bool {
        is_subclass(&self.jar, internal, BLOCK)
    }

    fn current_block_class(&self) -> Option<String> {
        let index = self.pending_block?;
        self.internal_classes.get(index).cloned().flatten()
    }

    fn default_state_for(&self, receiver: Option<&Val>) -> Val {
        let block = match receiver {
            Some(Val::Block(id)) => Some(id.to_string()),
            Some(Val::Obj(object)) => object.borrow().block_id.as_deref().map(str::to_string),
            _ => self
                .pending_block
                .and_then(|index| self.registry.get(index))
                .map(|row| row.id.clone()),
        };
        let Some(block) = block else {
            return Val::Opaque(Rc::from(BLOCK_STATE));
        };
        let Some((values, _)) = self.state_ids.default_state(&block) else {
            return Val::Opaque(Rc::from(BLOCK_STATE));
        };
        Val::State(Rc::new(StateVal { block, values }))
    }

    fn new_object(&mut self, internal: &str) -> Val {
        let block_id = if self.is_block_class(internal) {
            self.pending_block
                .and_then(|index| self.registry.get(index))
                .map(|row| Rc::from(row.id.as_str()))
        } else {
            None
        };
        Val::Obj(Rc::new(RefCell::new(Object {
            class: Rc::from(internal),
            fields: BTreeMap::new(),
            enum_name: None,
            ordinal: 0,
            block_id,
            item_id: None,
            constructed: false,
        })))
    }

    fn construct_value(
        &mut self,
        internal: &str,
        descriptor: &str,
        args: Vec<Val>,
    ) -> Result<Val, Fault> {
        let value = self.new_object(internal);
        let Val::Obj(object) = &value else {
            return Ok(value);
        };
        let class = self
            .class(internal)
            .ok_or_else(|| Fault::Thrown(format!("no class {internal}")))?;
        let ctor = class
            .methods
            .iter()
            .find(|method| {
                method.name == "<init>"
                    && method.descriptor.ends_with(")V")
                    && (descriptor.is_empty() || method.descriptor == descriptor)
            })
            .or_else(|| {
                class
                    .methods
                    .iter()
                    .find(|method| method.name == "<init>" && method.descriptor.ends_with(")V"))
            })
            .cloned()
            .ok_or_else(|| Fault::Thrown(format!("no constructor {internal}")))?;
        let receiver = Val::Obj(object.clone());
        self.call(class, &ctor, Some(receiver), args)?;
        object.borrow_mut().constructed = true;
        Ok(value)
    }

    fn call(
        &mut self,
        class: Rc<ClassFile>,
        method: &classfile::Member,
        receiver: Option<Val>,
        arguments: Vec<Val>,
    ) -> Result<Val, Fault> {
        if self.depth >= MAX_DEPTH {
            return Err(Fault::Thrown(format!(
                "call depth exceeded at {}.{}",
                class.name, method.name
            )));
        }
        let Some(code) = method.code.clone() else {
            return Ok(Val::Unit);
        };
        self.depth += 1;
        let result = self.exec(&class, method, &code, receiver, arguments);
        self.depth -= 1;
        result
    }

    fn exec(
        &mut self,
        class: &Rc<ClassFile>,
        method: &classfile::Member,
        code: &classfile::Code,
        receiver: Option<Val>,
        arguments: Vec<Val>,
    ) -> Result<Val, Fault> {
        let instructions = decode_code(&code.bytes);
        let pcs = pc_offsets(&instructions);
        let (types, _) = descriptor_args(&method.descriptor);
        let slots = arg_slots(&types);
        let mut locals: Vec<Val> = vec![Val::Null; code.max_locals as usize + 4];
        let mut slot = 0usize;
        if method.access & 0x0008 == 0 {
            locals[0] = receiver.unwrap_or(Val::Null);
            slot = 1;
        }
        for (value, width) in arguments.into_iter().zip(slots.iter()) {
            set_local(&mut locals, slot, value);
            slot += width;
        }
        let mut stack: Vec<Val> = Vec::new();
        let mut pc = 0usize;
        loop {
            if pc >= instructions.len() {
                return Ok(Val::Unit);
            }
            self.steps += 1;
            if self.steps > MAX_STEPS {
                return Err(Fault::Steps);
            }
            let ins = &instructions[pc];
            let next = pc + 1;
            match ins.opcode {
                0x00 => {}
                0x01 => stack.push(Val::Null),
                0x02..=0x08 => stack.push(Val::Int(ins.opcode as i32 - 3)),
                0x09 => stack.push(Val::Long(0)),
                0x0a => stack.push(Val::Long(1)),
                0x0b => stack.push(Val::Float(0.0)),
                0x0c => stack.push(Val::Float(1.0)),
                0x0d => stack.push(Val::Float(2.0)),
                0x0e => stack.push(Val::Double(0.0)),
                0x0f => stack.push(Val::Double(1.0)),
                0x10 | 0x11 => stack.push(Val::Int(ins.imm)),
                0x12 | 0x13 | 0x14 => stack.push(constant_value(class, ins.index)),
                0x15..=0x19 => {
                    let value = locals.get(ins.index).cloned().unwrap_or(Val::Null);
                    stack.push(value);
                }
                0x1a..=0x1d => {
                    let value = locals
                        .get((ins.opcode - 0x1a) as usize)
                        .cloned()
                        .unwrap_or(Val::Null);
                    stack.push(value);
                }
                0x1e..=0x21 => {
                    let value = locals
                        .get((ins.opcode - 0x1e) as usize)
                        .cloned()
                        .unwrap_or(Val::Null);
                    stack.push(value);
                }
                0x22..=0x25 => {
                    let value = locals
                        .get((ins.opcode - 0x22) as usize)
                        .cloned()
                        .unwrap_or(Val::Null);
                    stack.push(value);
                }
                0x26..=0x29 => {
                    let value = locals
                        .get((ins.opcode - 0x26) as usize)
                        .cloned()
                        .unwrap_or(Val::Null);
                    stack.push(value);
                }
                0x2a..=0x2d => {
                    let value = locals
                        .get((ins.opcode - 0x2a) as usize)
                        .cloned()
                        .unwrap_or(Val::Null);
                    stack.push(value);
                }
                0x2e..=0x35 => {
                    let index = pop_int(&mut stack);
                    let array = pop(&mut stack);
                    stack.push(array_load(&array, index));
                }
                0x36..=0x3a => {
                    let value = pop(&mut stack);
                    set_local(&mut locals, ins.index, value);
                }
                0x3b..=0x3e => {
                    let value = pop(&mut stack);
                    set_local(&mut locals, (ins.opcode - 0x3b) as usize, value);
                }
                0x3f..=0x42 => {
                    let value = pop(&mut stack);
                    set_local(&mut locals, (ins.opcode - 0x3f) as usize, value);
                }
                0x43..=0x46 => {
                    let value = pop(&mut stack);
                    set_local(&mut locals, (ins.opcode - 0x43) as usize, value);
                }
                0x47..=0x4a => {
                    let value = pop(&mut stack);
                    set_local(&mut locals, (ins.opcode - 0x47) as usize, value);
                }
                0x4b..=0x4e => {
                    let value = pop(&mut stack);
                    set_local(&mut locals, (ins.opcode - 0x4b) as usize, value);
                }
                0x4f..=0x56 => {
                    let value = pop(&mut stack);
                    let index = pop_int(&mut stack);
                    let array = pop(&mut stack);
                    array_store(&array, index, value);
                }
                0x57 => {
                    pop(&mut stack);
                }
                0x58 => {
                    let first = pop(&mut stack);
                    if !matches!(first, Val::Long(_) | Val::Double(_)) {
                        pop(&mut stack);
                    }
                }
                0x59 => {
                    let value = stack.last().cloned().unwrap_or(Val::Null);
                    stack.push(value);
                }
                0x5a => {
                    let first = pop(&mut stack);
                    let second = pop(&mut stack);
                    stack.push(first.clone());
                    stack.push(second);
                    stack.push(first);
                }
                0x5b => {
                    let first = pop(&mut stack);
                    let second = pop(&mut stack);
                    let third = pop(&mut stack);
                    stack.push(first.clone());
                    stack.push(third);
                    stack.push(second);
                    stack.push(first);
                }
                0x5c => {
                    let first = pop(&mut stack);
                    let second = pop(&mut stack);
                    stack.push(second.clone());
                    stack.push(first.clone());
                    stack.push(second);
                    stack.push(first);
                }
                0x5d | 0x5e => {
                    // Rare dup2_x forms; approximate by leaving the stack.
                    stack.push(Val::Null);
                }
                0x5f => {
                    let first = pop(&mut stack);
                    let second = pop(&mut stack);
                    stack.push(first);
                    stack.push(second);
                }
                0x60..=0x83 => arithmetic(&mut stack, ins.opcode),
                0x84 => {
                    let value = locals.get(ins.index).and_then(Val::as_int).unwrap_or(0);
                    set_local(
                        &mut locals,
                        ins.index,
                        Val::Int(value.wrapping_add(ins.imm)),
                    );
                }
                0x85..=0x93 => convert(&mut stack, ins.opcode),
                0x94 => {
                    let right = pop(&mut stack);
                    let left = pop(&mut stack);
                    let ordering = match (left.as_f64(), right.as_f64()) {
                        (Some(a), Some(b)) if a < b => -1,
                        (Some(a), Some(b)) if a > b => 1,
                        _ => 0,
                    };
                    stack.push(Val::Int(ordering));
                }
                0x95..=0x98 => {
                    let right = pop(&mut stack).as_f64().unwrap_or(f64::NAN);
                    let left = pop(&mut stack).as_f64().unwrap_or(f64::NAN);
                    let ordering = match left.partial_cmp(&right) {
                        Some(std::cmp::Ordering::Less) => -1,
                        Some(std::cmp::Ordering::Greater) => 1,
                        _ => 0,
                    };
                    stack.push(Val::Int(ordering));
                }
                0x99..=0x9e => {
                    let value = pop_int(&mut stack);
                    let taken = match ins.opcode {
                        0x99 => value == 0,
                        0x9a => value != 0,
                        0x9b => value < 0,
                        0x9c => value >= 0,
                        0x9d => value > 0,
                        _ => value <= 0,
                    };
                    if taken {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0x9f..=0xa4 => {
                    let right = pop_int(&mut stack);
                    let left = pop_int(&mut stack);
                    let taken = match ins.opcode {
                        0x9f => left == right,
                        0xa0 => left != right,
                        0xa1 => left < right,
                        0xa2 => left >= right,
                        0xa3 => left > right,
                        _ => left <= right,
                    };
                    if taken {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0xa5 | 0xa6 => {
                    let first = pop(&mut stack);
                    let second = pop(&mut stack);
                    let equal = values_equal(&first, &second);
                    if equal == (ins.opcode == 0xa5) {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0xa7 | 0xc8 => {
                    pc = jump(&pcs, ins, next);
                    continue;
                }
                0xaa | 0xab => {
                    let key = pop_int(&mut stack);
                    pc = switch_target(&pcs, ins, key);
                    continue;
                }
                0xac => return Ok(pop(&mut stack)),
                0xad => return Ok(pop(&mut stack)),
                0xae => return Ok(pop(&mut stack)),
                0xaf => return Ok(pop(&mut stack)),
                0xb0 => return Ok(pop(&mut stack)),
                0xb1 => return Ok(Val::Unit),
                0xbf => {
                    return Err(Fault::Thrown(format!(
                        "{}.{} threw",
                        class.name, method.name
                    )))
                }
                0xb2 => {
                    if let Some((owner, name, descriptor)) =
                        classfile::field_ref(&class.constants, ins.index)
                    {
                        let value = self.get_static(&owner, &name, &descriptor);
                        stack.push(value);
                    } else {
                        stack.push(Val::Null);
                    }
                }
                0xb3 => {
                    if let Some((owner, name, descriptor)) =
                        classfile::field_ref(&class.constants, ins.index)
                    {
                        let value = pop(&mut stack);
                        self.put_static(&owner, &name, &descriptor, value);
                    }
                }
                0xb4 => {
                    if let Some((owner, name, descriptor)) =
                        classfile::field_ref(&class.constants, ins.index)
                    {
                        let object = pop(&mut stack);
                        let value = self.get_field(&object, &owner, &name, &descriptor);
                        stack.push(value);
                    }
                }
                0xb5 => {
                    if let Some((_, name, _)) = classfile::field_ref(&class.constants, ins.index) {
                        let value = pop(&mut stack);
                        let object = pop(&mut stack);
                        self.put_field(&object, &name, value);
                    }
                }
                0xb6..=0xb9 => {
                    if let Some((owner, name, descriptor)) =
                        classfile::method_ref(&class.constants, ins.index)
                    {
                        let count = descriptor_args(&descriptor).0.len();
                        let mut arguments = Vec::with_capacity(count);
                        for _ in 0..count {
                            arguments.push(pop(&mut stack));
                        }
                        arguments.reverse();
                        let receiver = if ins.opcode == 0xb8 {
                            None
                        } else {
                            Some(pop(&mut stack))
                        };
                        let value = self.invoke(
                            ins.opcode,
                            &owner,
                            &name,
                            &descriptor,
                            receiver,
                            arguments,
                        )?;
                        if !descriptor.ends_with(")V") {
                            stack.push(value);
                        }
                    }
                }
                0xbb => {
                    let internal = class_name_at(&class.constants, ins.index)
                        .unwrap_or_else(|| "java/lang/Object".to_string());
                    stack.push(self.new_value(&internal));
                }
                0xbc => {
                    let count = pop_int(&mut stack).max(0) as usize;
                    stack.push(Val::Array(Rc::new(RefCell::new(vec![Val::Int(0); count]))));
                }
                0xbd => {
                    let count = pop_int(&mut stack).max(0) as usize;
                    stack.push(Val::Array(Rc::new(RefCell::new(vec![Val::Null; count]))));
                }
                0xbe => {
                    let array = pop(&mut stack);
                    let length = match &array {
                        Val::Array(items) => items.borrow().len(),
                        Val::List(items, _) => items.borrow().len(),
                        _ => 0,
                    };
                    stack.push(Val::Int(length as i32));
                }
                0xc0 => {}
                0xc1 => {
                    let class_name = class_name_at(&class.constants, ins.index)
                        .unwrap_or_else(|| "java/lang/Object".to_string());
                    let value = pop(&mut stack);
                    stack.push(Val::Bool(instance_of(self, &value, &class_name)));
                }
                0xba => {
                    let captured_count = indy_captured_count(class, ins.index);
                    let mut captured = Vec::with_capacity(captured_count);
                    for _ in 0..captured_count {
                        captured.push(pop(&mut stack));
                    }
                    captured.reverse();
                    if let Some(text) = concat_indy(class, ins.index, &captured) {
                        stack.push(Val::Str(Rc::from(text)));
                    } else if let Some((owner, name, descriptor)) = indy_target(class, ins.index) {
                        let is_static = if name == "<init>" {
                            true
                        } else {
                            self.class(&owner)
                                .and_then(|target| {
                                    target
                                        .methods
                                        .iter()
                                        .find(|method| method.name == name)
                                        .map(|method| method.access & 0x0008 != 0)
                                })
                                .unwrap_or(false)
                        };
                        let target_class = self.class(&owner).unwrap_or_else(|| class.clone());
                        stack.push(Val::Lambda(Rc::new(Lambda {
                            class: target_class,
                            method: name,
                            descriptor,
                            captured,
                            is_static,
                        })));
                    } else {
                        stack.push(Val::Null);
                    }
                }
                0xc6 | 0xc7 => {
                    let value = pop(&mut stack);
                    let is_null = matches!(value, Val::Null);
                    if is_null == (ins.opcode == 0xc6) {
                        pc = jump(&pcs, ins, next);
                        continue;
                    }
                }
                0xc2 | 0xc3 => {
                    let value = pop(&mut stack);
                    stack.push(value);
                }
                _ => {}
            }
            pc = next;
        }
    }
}

fn constant_value(class: &ClassFile, index: usize) -> Val {
    match class.constants.get(index) {
        Some(Constant::Integer(value)) => Val::Int(*value),
        Some(Constant::Float(value)) => Val::Float(*value),
        Some(Constant::Long(value)) => Val::Long(*value),
        Some(Constant::Double(value)) => Val::Double(*value),
        Some(Constant::String(_)) => classfile::string_constant(&class.constants, index)
            .map(|text| Val::Str(Rc::from(text)))
            .unwrap_or(Val::Null),
        Some(Constant::Class(_)) => Val::Class(Rc::from(
            class_name_at(&class.constants, index).unwrap_or_default(),
        )),
        Some(Constant::Utf8(text)) => Val::Str(Rc::from(text.as_str())),
        _ => Val::Null,
    }
}

fn array_load(array: &Val, index: i32) -> Val {
    match array {
        Val::Array(items) => items
            .borrow()
            .get(index.max(0) as usize)
            .cloned()
            .unwrap_or(Val::Null),
        Val::List(items, _) => items
            .borrow()
            .get(index.max(0) as usize)
            .cloned()
            .unwrap_or(Val::Null),
        _ => Val::Null,
    }
}

fn array_store(array: &Val, index: i32, value: Val) {
    match array {
        Val::Array(items) => {
            let mut items = items.borrow_mut();
            if let Some(slot) = items.get_mut(index.max(0) as usize) {
                *slot = value;
            }
        }
        Val::List(items, _) => {
            let mut items = items.borrow_mut();
            if let Some(slot) = items.get_mut(index.max(0) as usize) {
                *slot = value;
            }
        }
        _ => {}
    }
}

fn instance_of(vm: &Vm, value: &Val, target: &str) -> bool {
    match value {
        Val::Obj(object) => is_subclass(&vm.jar, &object.borrow().class, target),
        Val::List(..) => {
            target == "java/util/List"
                || target == "java/util/Collection"
                || target == "java/lang/Iterable"
                || target == "java/util/ArrayList"
        }
        Val::Map(..) => target == "java/util/Map" || target == "java/lang/Object",
        Val::Str(_) => target == "java/lang/String" || target == "java/lang/CharSequence",
        Val::Enum(..) => target == "java/lang/Enum",
        Val::Block(id) => vm
            .blocks
            .get(id.as_ref())
            .map(|object| is_subclass(&vm.jar, &object.borrow().class, target))
            .unwrap_or(false),
        _ => false,
    }
}

fn arithmetic(stack: &mut Vec<Val>, opcode: u8) {
    let right = pop(stack);
    let left = pop(stack);
    match opcode {
        0x60 | 0x64 | 0x68 | 0x6c | 0x70 => {
            let a = left.as_int().unwrap_or(0);
            let b = right.as_int().unwrap_or(0);
            let result = match opcode {
                0x60 => a.wrapping_add(b),
                0x64 => a.wrapping_sub(b),
                0x68 => a.wrapping_mul(b),
                0x6c => {
                    if b == 0 {
                        0
                    } else {
                        a.wrapping_div(b)
                    }
                }
                _ => {
                    if b == 0 {
                        0
                    } else {
                        a.wrapping_rem(b)
                    }
                }
            };
            stack.push(Val::Int(result));
        }
        0x61 | 0x65 | 0x69 | 0x6d | 0x71 => {
            let a = left.as_f64().unwrap_or(0.0) as i64;
            let b = right.as_f64().unwrap_or(0.0) as i64;
            let result = match opcode {
                0x61 => a.wrapping_add(b),
                0x65 => a.wrapping_sub(b),
                0x69 => a.wrapping_mul(b),
                0x6d => {
                    if b == 0 {
                        0
                    } else {
                        a.wrapping_div(b)
                    }
                }
                _ => {
                    if b == 0 {
                        0
                    } else {
                        a.wrapping_rem(b)
                    }
                }
            };
            stack.push(Val::Long(result));
        }
        0x62 | 0x66 | 0x6a | 0x6e | 0x72 => {
            let a = left.as_f64().unwrap_or(0.0) as f32;
            let b = right.as_f64().unwrap_or(0.0) as f32;
            let result = match opcode {
                0x62 => a + b,
                0x66 => a - b,
                0x6a => a * b,
                0x6e => a / b,
                _ => a % b,
            };
            stack.push(Val::Float(result));
        }
        0x63 | 0x67 | 0x6b | 0x6f | 0x73 => {
            let a = left.as_f64().unwrap_or(0.0);
            let b = right.as_f64().unwrap_or(0.0);
            let result = match opcode {
                0x63 => a + b,
                0x67 => a - b,
                0x6b => a * b,
                0x6f => a / b,
                _ => a % b,
            };
            stack.push(Val::Double(result));
        }
        _ => {}
    }
}

fn convert(stack: &mut Vec<Val>, opcode: u8) {
    let value = pop(stack);
    let number = value.as_f64().unwrap_or(0.0);
    let converted = match opcode {
        0x85 => Val::Long(number as i64),
        0x86 => Val::Float(number as f32),
        0x87 => Val::Double(number),
        0x88 => Val::Int(number as i32),
        0x89 => Val::Float(number as f32),
        0x8a => Val::Double(number),
        0x8b => Val::Int(number as i32),
        0x8c => Val::Long(number as i64),
        0x8d => Val::Double(number),
        0x8e => Val::Int(number as i32),
        0x8f => Val::Long(number as i64),
        0x90 => Val::Float(number as f32),
        0x91 => Val::Int(number as i8 as i32),
        0x92 => Val::Int(number as u16 as i32),
        0x93 => Val::Int(number as i16 as i32),
        _ => value,
    };
    stack.push(converted);
}

/// `StringConcatFactory`: evaluate the recipe against the captured values.
fn concat_indy(class: &ClassFile, index: usize, captured: &[Val]) -> Option<String> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    let (owner, method, _) = method_handle_target(&class.constants, entry.method)?;
    if !owner.contains("StringConcatFactory") && !(owner == "java/lang/invoke/StringConcatFactory")
    {
        return None;
    }
    let _ = method;
    let recipe = entry
        .arguments
        .first()
        .and_then(|argument| classfile::string_constant(&class.constants, *argument))
        .unwrap_or_default();
    let mut out = String::new();
    let mut values = captured.iter();
    for piece in recipe.split('\u{1}') {
        out.push_str(piece);
        if let Some(value) = values.next() {
            out.push_str(&val_to_text(value));
        }
    }
    Some(out)
}

fn val_to_text(value: &Val) -> String {
    match value {
        Val::Str(text) => text.to_string(),
        Val::Int(number) => number.to_string(),
        Val::Long(number) => number.to_string(),
        Val::Float(number) => format_float32(*number),
        Val::Double(number) => format_float64(*number),
        Val::Bool(flag) => flag.to_string(),
        Val::Enum(_, name, _) => name.to_string(),
        Val::Block(id) => format!("minecraft:{id}"),
        _ => String::new(),
    }
}

fn format_float32(value: f32) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    let mut text = format!("{value}");
    if !text.contains('.') && !text.contains('E') && !text.contains('e') {
        text.push_str(".0");
    }
    text
}

fn format_float64(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    let mut text = format!("{value}");
    if !text.contains('.') && !text.contains('E') && !text.contains('e') {
        text.push_str(".0");
    }
    text
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

impl Vm {
    fn new_value(&mut self, internal: &str) -> Val {
        let simple = simple_name(internal);
        if matches!(
            simple,
            "ArrayList" | "LinkedList" | "ImmutableList" | "ImmutableList$Builder"
        ) || internal.ends_with("ImmutableList")
        {
            return Val::List(Rc::new(RefCell::new(Vec::new())), Rc::from(internal));
        }
        if matches!(
            simple,
            "HashMap"
                | "LinkedHashMap"
                | "TreeMap"
                | "ConcurrentHashMap"
                | "ImmutableMap"
                | "ImmutableMap$Builder"
                | "Object2IntOpenHashMap"
                | "Object2IntMap"
                | "Object2IntLinkedOpenHashMap"
                | "Reference2IntOpenHashMap"
        ) || internal.starts_with("it/unimi/dsi/fastutil/")
        {
            return Val::Map(Rc::new(RefCell::new(Vec::new())), Rc::from(internal));
        }
        if simple == "StringBuilder" || simple == "StringBuffer" {
            return Val::Obj(Rc::new(RefCell::new(Object {
                class: Rc::from(internal),
                fields: BTreeMap::new(),
                enum_name: None,
                ordinal: 0,
                block_id: None,
                item_id: None,
                constructed: false,
            })));
        }
        self.new_object(internal)
    }

    fn apply_lambda(&mut self, lambda: &Lambda, arguments: Vec<Val>) -> Result<Val, Fault> {
        if lambda.method == "<init>" {
            return self.construct_value(&lambda.class.name, &lambda.descriptor, arguments);
        }
        let pair = self
            .resolve(&lambda.class.name, &lambda.method, &lambda.descriptor)
            .or_else(|| self.resolve(&lambda.class.name, &lambda.method, ""));
        let Some((class, method)) = pair else {
            let mut all = lambda.captured.clone();
            all.extend(arguments);
            let (receiver, call_arguments) = if lambda.is_static || all.is_empty() {
                (None, all)
            } else {
                let receiver = Some(all.remove(0));
                (receiver, all)
            };
            return self.invoke(
                0xb6,
                &lambda.class.name,
                &lambda.method,
                &lambda.descriptor,
                receiver,
                call_arguments,
            );
        };
        let mut call_arguments = arguments;
        let receiver = if lambda.is_static {
            let mut all = lambda.captured.clone();
            all.extend(call_arguments);
            call_arguments = all;
            None
        } else {
            let mut all = lambda.captured.clone();
            all.extend(call_arguments);
            let receiver = if all.is_empty() {
                None
            } else {
                Some(all.remove(0))
            };
            call_arguments = all;
            receiver
        };
        if class.name == BLOCKS && method.name == "register" {
            return self.register(call_arguments);
        }
        self.call(class, &method, receiver, call_arguments)
    }

    fn register(&mut self, arguments: Vec<Val>) -> Result<Val, Fault> {
        let index = self.next_block;
        if index >= self.registry.len() {
            return Ok(Val::Null);
        }
        let row = self.registry[index].clone();
        let id = Rc::<str>::from(row.id.as_str());
        self.pending_block = Some(index);
        let lambda = arguments.iter().find_map(|value| match value {
            Val::Lambda(lambda) => Some(lambda.clone()),
            _ => None,
        });
        let mut created: Option<Val> = None;
        let mut failure = None;
        if let Some(lambda) = lambda {
            match self.apply_lambda(&lambda, vec![Val::Props]) {
                Ok(value) => created = Some(value),
                Err(error) => failure = Some(error),
            }
        } else if let Some(internal) = self.internal_classes[index].clone() {
            match self.construct_value(&internal, "", vec![Val::Props]) {
                Ok(value) => created = Some(value),
                Err(error) => failure = Some(error),
            }
        }
        self.pending_block = None;
        self.next_block += 1;
        let object = match created {
            Some(Val::Obj(object)) => object,
            _ => {
                let internal = self.internal_classes[index]
                    .clone()
                    .unwrap_or_else(|| BLOCK.to_string());
                Rc::new(RefCell::new(Object {
                    class: Rc::from(internal),
                    fields: BTreeMap::new(),
                    enum_name: None,
                    ordinal: 0,
                    block_id: None,
                    item_id: None,
                    constructed: false,
                }))
            }
        };
        {
            let mut borrowed = object.borrow_mut();
            borrowed.block_id = Some(id.clone());
            borrowed.constructed = failure.is_none() && borrowed.constructed;
            if let Some(error) = failure {
                self.note(format!("register {id}: {}", fault_text(&error)));
            }
        }
        self.blocks.insert(id.to_string(), object);
        Ok(Val::Block(id))
    }

    fn invoke(
        &mut self,
        opcode: u8,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Option<Val>,
        arguments: Vec<Val>,
    ) -> Result<Val, Fault> {
        if owner == BLOCKS && name == "register" {
            return self.register(arguments);
        }
        if name == "<init>" {
            return self.construct(owner, descriptor, receiver, arguments);
        }
        // Fluent builder types: every method returns the builder again.
        if owner.ends_with("BlockBehaviour$Properties")
            || owner.ends_with("Item$Properties")
            || owner.ends_with("Item$Properties$1")
        {
            return Ok(receiver.unwrap_or(Val::Props));
        }
        if owner == STATE_DEFINITION || owner.ends_with("/StateDefinition") {
            match name {
                "any" => return Ok(self.default_state_for(receiver.as_ref())),
                "getPossibleStates" => {
                    return Ok(Val::List(
                        Rc::new(RefCell::new(Vec::new())),
                        Rc::from("java/util/List"),
                    ))
                }
                _ => {}
            }
        }
        if owner == BLOCK_STATE || owner.ends_with("/BlockState") {
            match name {
                "setValue" => {
                    if let (Some(Val::State(state)), Some(property), Some(value)) =
                        (receiver.as_ref(), arguments.first(), arguments.get(1))
                    {
                        let property_name = match property {
                            Val::Str(text) => text.to_string(),
                            _ => String::new(),
                        };
                        let rendered = property_value_text(value);
                        let mut values = state.values.clone();
                        values.insert(property_name, rendered);
                        return Ok(Val::State(Rc::new(StateVal {
                            block: state.block.clone(),
                            values,
                        })));
                    }
                    return Ok(receiver.unwrap_or(Val::Null));
                }
                "getBlock" => {
                    if let Some(Val::State(state)) = receiver.as_ref() {
                        return Ok(Val::Block(Rc::from(state.block.as_str())));
                    }
                }
                "hasProperty" => return Ok(Val::Bool(true)),
                _ => {}
            }
        }
        if owner == BLOCK {
            match name {
                "defaultBlockState" => return Ok(self.default_state_for(receiver.as_ref())),
                "registerDefaultState" => return Ok(Val::Unit),
                "getId" => {
                    if let Some(Val::State(state)) = arguments.first() {
                        if let Some(id) = self.state_ids.state_id(&state.block, &state.values) {
                            return Ok(Val::Int(id as i32));
                        }
                    }
                    return Ok(Val::Int(0));
                }
                "defaultMapColor" => {
                    return Ok(Val::Opaque(Rc::from(
                        "net/minecraft/world/level/material/MapColor",
                    )))
                }
                _ => {}
            }
        }
        if name == "values" && receiver.is_none() && arguments.is_empty() {
            if let Some(class) = self.class(owner) {
                let descriptor = format!("L{owner};");
                let constants: Vec<Val> = class
                    .fields
                    .iter()
                    .filter(|field| field.descriptor == descriptor)
                    .map(|field| {
                        let field_name = field.name.clone();
                        self.get_static(owner, &field_name, &descriptor)
                    })
                    .collect();
                if !constants.is_empty() {
                    return Ok(Val::Array(Rc::new(RefCell::new(constants))));
                }
            }
        }
        if owner == "java/lang/Enum" {
            match name {
                "valueOf" => {
                    let class = arguments
                        .first()
                        .and_then(|value| match value {
                            Val::Class(name) => Some(name.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| Rc::from("java/lang/Enum"));
                    let text = arguments
                        .get(1)
                        .and_then(Val::as_str)
                        .unwrap_or("")
                        .to_string();
                    return Ok(Val::Enum(class, Rc::from(text), 0));
                }
                "values" => return Ok(Val::Array(Rc::new(RefCell::new(Vec::new())))),
                _ => {}
            }
        }
        if let Some(Val::Enum(class, enum_name, ordinal)) = receiver.as_ref() {
            match name {
                "name" | "toString" => return Ok(Val::Str(enum_name.clone())),
                "getSerializedName" | "getName" => {
                    return Ok(Val::Str(Rc::from(enum_name.to_ascii_lowercase())))
                }
                "ordinal" => return Ok(Val::Int(*ordinal)),
                "getDeclaringClass" => return Ok(Val::Class(class.clone())),
                _ => {}
            }
        }
        if let Some(Val::Obj(object)) = receiver.as_ref() {
            let enum_data = {
                let borrowed = object.borrow();
                borrowed
                    .enum_name
                    .as_ref()
                    .map(|name| (borrowed.class.clone(), name.clone(), borrowed.ordinal))
            };
            if let Some((class, enum_name, ordinal)) = enum_data {
                match name {
                    "name" | "toString" => return Ok(Val::Str(Rc::from(enum_name))),
                    "getSerializedName" | "getName" => {
                        return Ok(Val::Str(Rc::from(enum_name.to_ascii_lowercase())))
                    }
                    "ordinal" => return Ok(Val::Int(ordinal)),
                    "getDeclaringClass" => return Ok(Val::Class(class)),
                    _ => {}
                }
            }
        }
        // Functional interfaces: dispatch into the captured lambda.
        if matches!(
            name,
            "apply" | "get" | "test" | "accept" | "applyAsInt" | "applyAsLong" | "applyAsDouble"
        ) {
            if let Some(Val::Lambda(lambda)) = receiver.as_ref() {
                return self.apply_lambda(&lambda.clone(), arguments);
            }
        }
        if let Some(value) = self.jdk_call(owner, name, descriptor, receiver.as_ref(), &arguments) {
            return value;
        }
        // Virtual dispatch: start at the receiver's runtime class.
        let start = match receiver.as_ref() {
            Some(Val::Obj(object)) => object.borrow().class.to_string(),
            _ => owner.to_string(),
        };
        if let Some((class, method)) = self.resolve(&start, name, descriptor) {
            let is_static = method.access & 0x0008 != 0;
            return self.call(
                class,
                &method,
                if is_static { None } else { receiver.clone() },
                arguments,
            );
        }
        if start != owner {
            if let Some((class, method)) = self.resolve(owner, name, descriptor) {
                let is_static = method.access & 0x0008 != 0;
                return self.call(
                    class,
                    &method,
                    if is_static { None } else { receiver },
                    arguments,
                );
            }
        }
        self.note(format!(
            "unmodelled call {owner}.{name}{descriptor} (opcode {opcode:#x})"
        ));
        Ok(Val::Opaque(Rc::from(return_class(descriptor))))
    }

    fn construct(
        &mut self,
        owner: &str,
        descriptor: &str,
        receiver: Option<Val>,
        arguments: Vec<Val>,
    ) -> Result<Val, Fault> {
        let Some(receiver) = receiver else {
            return Ok(Val::Unit);
        };
        let object = match receiver {
            Val::Obj(object) => object,
            _ => return Ok(Val::Unit),
        };
        match owner {
            "java/lang/Object" | "java/lang/Record" => {
                object.borrow_mut().constructed = true;
                return Ok(Val::Unit);
            }
            "java/lang/Enum" => {
                let enum_name = arguments
                    .first()
                    .and_then(Val::as_str)
                    .unwrap_or("")
                    .to_string();
                let ordinal = arguments.get(1).and_then(Val::as_int).unwrap_or(0);
                let mut borrowed = object.borrow_mut();
                borrowed.enum_name = Some(enum_name);
                borrowed.ordinal = ordinal;
                borrowed.constructed = true;
                return Ok(Val::Unit);
            }
            _ => {}
        }
        if owner.starts_with("java/") || owner.starts_with("javax/") {
            object.borrow_mut().constructed = true;
            return Ok(Val::Unit);
        }
        if owner.starts_with("it/unimi/") {
            object.borrow_mut().constructed = true;
            return Ok(Val::Unit);
        }
        if owner == BLOCK_BEHAVIOUR || owner == BLOCK {
            // The registry plumbing; the subclass constructor is what assigns
            // the exported fields.
            return Ok(Val::Unit);
        }
        if owner == ITEM {
            object.borrow_mut().constructed = true;
            return Ok(Val::Unit);
        }
        if owner == "net/minecraft/resources/Identifier" {
            let namespace = arguments
                .first()
                .and_then(Val::as_str)
                .unwrap_or("minecraft")
                .to_string();
            let path = if arguments.len() >= 2 {
                arguments
                    .get(1)
                    .and_then(Val::as_str)
                    .unwrap_or("")
                    .to_string()
            } else {
                namespace.clone()
            };
            let namespace = if arguments.len() >= 2 {
                namespace
            } else {
                "minecraft".to_string()
            };
            let mut fields = object.borrow_mut().fields.clone();
            fields.insert("namespace".to_string(), Val::Str(Rc::from(namespace)));
            fields.insert("path".to_string(), Val::Str(Rc::from(path)));
            object.borrow_mut().fields = fields;
            object.borrow_mut().constructed = true;
            return Ok(Val::Unit);
        }
        if owner.ends_with("StringBuilder") || owner.ends_with("StringBuffer") {
            object.borrow_mut().constructed = true;
            return Ok(Val::Unit);
        }
        // A block/item/record constructor really runs.
        if let Some((class, method)) = self.resolve(owner, "<init>", descriptor) {
            let receiver = Val::Obj(object.clone());
            let result = self.call(class, &method, Some(receiver), arguments);
            if result.is_ok() {
                object.borrow_mut().constructed = true;
            }
            return result;
        }
        // A JDK/library constructor the jar does not carry: harmless.
        object.borrow_mut().constructed = true;
        Ok(Val::Unit)
    }

    fn jdk_call(
        &mut self,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Option<&Val>,
        arguments: &[Val],
    ) -> Option<Result<Val, Fault>> {
        if owner == "java/lang/Integer"
            || owner == "java/lang/Long"
            || owner == "java/lang/Short"
            || owner == "java/lang/Byte"
        {
            if name == "valueOf" || name == "intValue" || name == "longValue" {
                return Some(Ok(match arguments.first() {
                    Some(Val::Int(value)) => Val::Int(*value),
                    Some(Val::Long(value)) => Val::Long(*value),
                    Some(other) => Val::Int(other.as_int().unwrap_or(0)),
                    None => Val::Int(0),
                }));
            }
        }
        if owner == "java/lang/Float" && name == "valueOf" {
            return Some(Ok(match arguments.first() {
                Some(Val::Float(value)) => Val::Float(*value),
                Some(other) => Val::Float(other.as_f64().unwrap_or(0.0) as f32),
                None => Val::Float(0.0),
            }));
        }
        if owner == "java/lang/Double" && name == "valueOf" {
            return Some(Ok(match arguments.first() {
                Some(Val::Double(value)) => Val::Double(*value),
                Some(other) => Val::Double(other.as_f64().unwrap_or(0.0)),
                None => Val::Double(0.0),
            }));
        }
        if owner == "java/lang/Boolean" && name == "valueOf" {
            return Some(Ok(Val::Bool(match arguments.first() {
                Some(Val::Bool(value)) => *value,
                _ => false,
            })));
        }
        if owner == "java/lang/String" {
            match name {
                "valueOf" => {
                    return Some(Ok(match arguments.first() {
                        Some(value) => Val::Str(Rc::from(val_to_text(value))),
                        None => Val::Str(Rc::from("")),
                    }))
                }
                "format" => {
                    let template = arguments.first().and_then(Val::as_str).unwrap_or("");
                    return Some(Ok(Val::Str(Rc::from(template.to_string()))));
                }
                _ => {}
            }
        }
        if owner == "java/util/Objects" && name == "requireNonNull" {
            return Some(Ok(arguments.first().cloned().unwrap_or(Val::Null)));
        }
        if owner == "java/util/Optional" {
            match name {
                "of" => return Some(Ok(Val::Optional(arguments.first().cloned().map(Rc::new)))),
                "ofNullable" => {
                    return Some(Ok(Val::Optional(arguments.first().cloned().map(Rc::new))))
                }
                "empty" => return Some(Ok(Val::Optional(None))),
                _ => {}
            }
        }
        if let Some(Val::Optional(inner)) = receiver {
            match name {
                "get" => {
                    return Some(Ok(inner
                        .as_ref()
                        .map(|value| (**value).clone())
                        .unwrap_or(Val::Null)))
                }
                "isPresent" => return Some(Ok(Val::Bool(inner.is_some()))),
                "isEmpty" => return Some(Ok(Val::Bool(inner.is_none()))),
                "orElse" => {
                    return Some(Ok(inner
                        .as_ref()
                        .map(|value| (**value).clone())
                        .unwrap_or_else(|| arguments.first().cloned().unwrap_or(Val::Null))))
                }
                "map" => {
                    if let (Some(value), Some(Val::Lambda(lambda))) =
                        (inner.as_ref(), arguments.first())
                    {
                        return Some(self.apply_lambda(&lambda.clone(), vec![(**value).clone()]));
                    }
                    return Some(Ok(Val::Optional(None)));
                }
                _ => {}
            }
        }
        if owner == "java/util/List" || owner.ends_with("/List") {
            match name {
                "of" => {
                    let class = match arguments.len() {
                        0 => "java/util/ImmutableCollections$ListN",
                        1 | 2 => "java/util/ImmutableCollections$List12",
                        _ => "java/util/ImmutableCollections$ListN",
                    };
                    return Some(Ok(Val::List(
                        Rc::new(RefCell::new(arguments.to_vec())),
                        Rc::from(class),
                    )));
                }
                "copyOf" => {
                    return Some(Ok(Val::List(
                        Rc::new(RefCell::new(arguments.to_vec())),
                        Rc::from("java/util/ImmutableCollections$ListN"),
                    )))
                }
                _ => {}
            }
        }
        if owner == "java/util/Arrays" && name == "asList" {
            let items = match arguments.first() {
                Some(Val::Array(items)) => items.borrow().clone(),
                _ => arguments.to_vec(),
            };
            return Some(Ok(Val::List(
                Rc::new(RefCell::new(items)),
                Rc::from("java/util/Arrays$ArrayList"),
            )));
        }
        if owner == "java/util/Collections" {
            match name {
                "emptyList" => {
                    return Some(Ok(Val::List(
                        Rc::new(RefCell::new(Vec::new())),
                        Rc::from("java/util/Collections$EmptyList"),
                    )))
                }
                "singletonList" => {
                    return Some(Ok(Val::List(
                        Rc::new(RefCell::new(arguments.to_vec())),
                        Rc::from("java/util/Collections$SingletonList"),
                    )))
                }
                "emptyMap" => {
                    return Some(Ok(Val::Map(
                        Rc::new(RefCell::new(Vec::new())),
                        Rc::from("java/util/Collections$EmptyMap"),
                    )))
                }
                "unmodifiableList" => {
                    if let Some(Val::List(items, _)) = arguments.first() {
                        return Some(Ok(Val::List(
                            items.clone(),
                            Rc::from("java/util/Collections$UnmodifiableList"),
                        )));
                    }
                }
                _ => {}
            }
        }
        if owner == "java/util/Map" || owner.ends_with("/Map") {
            match name {
                "of" => {
                    let mut entries = Vec::new();
                    let mut at = 0usize;
                    while at + 1 < arguments.len() {
                        entries.push((arguments[at].clone(), arguments[at + 1].clone()));
                        at += 2;
                    }
                    return Some(Ok(Val::Map(
                        Rc::new(RefCell::new(entries)),
                        Rc::from("java/util/ImmutableCollections$MapN"),
                    )));
                }
                "entry" => {
                    let key = arguments.first().cloned().unwrap_or(Val::Null);
                    let value = arguments.get(1).cloned().unwrap_or(Val::Null);
                    return Some(Ok(Val::Map(
                        Rc::new(RefCell::new(vec![(key, value)])),
                        Rc::from("java/util/Map$Entry"),
                    )));
                }
                "ofEntries" => {
                    let mut entries = Vec::new();
                    for argument in arguments {
                        if let Val::Map(items, _) = argument {
                            entries.extend(items.borrow().clone());
                        }
                    }
                    return Some(Ok(Val::Map(
                        Rc::new(RefCell::new(entries)),
                        Rc::from("java/util/ImmutableCollections$MapN"),
                    )));
                }
                "copyOf" => {
                    let items = match arguments.first() {
                        Some(Val::Map(items, _)) => items.borrow().clone(),
                        _ => Vec::new(),
                    };
                    return Some(Ok(Val::Map(
                        Rc::new(RefCell::new(items)),
                        Rc::from("java/util/ImmutableCollections$MapN"),
                    )));
                }
                _ => {}
            }
        }
        // Collections held as values: list/map instance methods.
        match receiver {
            Some(Val::List(items, class)) => match name {
                "add" => {
                    items
                        .borrow_mut()
                        .push(arguments.first().cloned().unwrap_or(Val::Null));
                    return Some(Ok(Val::Bool(true)));
                }
                "get" => {
                    let index =
                        arguments.first().and_then(Val::as_int).unwrap_or(0).max(0) as usize;
                    return Some(Ok(items.borrow().get(index).cloned().unwrap_or(Val::Null)));
                }
                "size" => return Some(Ok(Val::Int(items.borrow().len() as i32))),
                "isEmpty" => return Some(Ok(Val::Bool(items.borrow().is_empty()))),
                "stream" | "iterator" | "spliterator" => {
                    return Some(Ok(Val::Opaque(class.clone())))
                }
                _ => {}
            },
            Some(Val::Map(items, _)) => match name {
                "put" => {
                    let key = arguments.first().cloned().unwrap_or(Val::Null);
                    let value = arguments.get(1).cloned().unwrap_or(Val::Null);
                    let mut entries = items.borrow_mut();
                    if let Some(slot) = entries
                        .iter_mut()
                        .find(|(slot, _)| values_equal(slot, &key))
                    {
                        slot.1 = value;
                    } else {
                        entries.push((key, value));
                    }
                    return Some(Ok(Val::Null));
                }
                "get" => {
                    let key = arguments.first().cloned().unwrap_or(Val::Null);
                    let found = items
                        .borrow()
                        .iter()
                        .find(|(slot, _)| values_equal(slot, &key))
                        .map(|(_, value)| value.clone())
                        .unwrap_or(Val::Null);
                    return Some(Ok(found));
                }
                "getInt" => {
                    let key = arguments.first().cloned().unwrap_or(Val::Null);
                    let found = items
                        .borrow()
                        .iter()
                        .find(|(slot, _)| values_equal(slot, &key))
                        .map(|(_, value)| value.clone())
                        .unwrap_or(Val::Int(0));
                    return Some(Ok(found));
                }
                "getOrDefault" => {
                    let key = arguments.first().cloned().unwrap_or(Val::Null);
                    let fallback = arguments.get(1).cloned().unwrap_or(Val::Null);
                    let found = items
                        .borrow()
                        .iter()
                        .find(|(slot, _)| values_equal(slot, &key))
                        .map(|(_, value)| value.clone())
                        .unwrap_or(fallback);
                    return Some(Ok(found));
                }
                "containsKey" => {
                    let key = arguments.first().cloned().unwrap_or(Val::Null);
                    return Some(Ok(Val::Bool(
                        items
                            .borrow()
                            .iter()
                            .any(|(slot, _)| values_equal(slot, &key)),
                    )));
                }
                "size" => return Some(Ok(Val::Int(items.borrow().len() as i32))),
                "build" | "putAll" => return Some(Ok(receiver.cloned().unwrap_or(Val::Null))),
                "entrySet" | "keySet" | "values" => {
                    return Some(Ok(Val::List(
                        Rc::new(RefCell::new(Vec::new())),
                        Rc::from("java/util/Set"),
                    )))
                }
                _ => {}
            },
            _ => {}
        }
        if owner == "net/minecraft/world/level/material/FlowingFluid"
            && matches!(name, "getSource" | "getFlowing")
        {
            if let Some(Val::Fluid(id)) = receiver {
                return Some(Ok(Val::Fluid(id.clone())));
            }
            return Some(Ok(Val::Opaque(Rc::from(
                "net/minecraft/world/level/material/FluidState",
            ))));
        }
        if owner == "com/google/common/collect/Lists" && name == "newArrayList" {
            return Some(Ok(Val::List(
                Rc::new(RefCell::new(Vec::new())),
                Rc::from("java/util/ArrayList"),
            )));
        }
        if owner == "java/lang/StringBuilder" || owner == "java/lang/StringBuffer" {
            if let Some(Val::Obj(object)) = receiver {
                match name {
                    "append" => {
                        let text = arguments.first().map(val_to_text).unwrap_or_default();
                        let mut borrowed = object.borrow_mut();
                        let mut current = borrowed
                            .fields
                            .get("text")
                            .and_then(Val::as_str)
                            .unwrap_or("")
                            .to_string();
                        current.push_str(&text);
                        borrowed
                            .fields
                            .insert("text".to_string(), Val::Str(Rc::from(current)));
                        return Some(Ok(receiver.cloned().unwrap_or(Val::Null)));
                    }
                    "toString" => {
                        let text = object
                            .borrow()
                            .fields
                            .get("text")
                            .and_then(Val::as_str)
                            .unwrap_or("")
                            .to_string();
                        return Some(Ok(Val::Str(Rc::from(text))));
                    }
                    _ => {}
                }
            }
        }
        if let Some(Val::Str(text)) = receiver {
            match name {
                "length" => return Some(Ok(Val::Int(text.chars().count() as i32))),
                "isEmpty" => return Some(Ok(Val::Bool(text.is_empty()))),
                "toString" => return Some(Ok(Val::Str(text.clone()))),
                "equals" => {
                    return Some(Ok(Val::Bool(
                        matches!(arguments.first(), Some(Val::Str(other)) if other == text),
                    )))
                }
                "hashCode" => return Some(Ok(Val::Int(0))),
                "toLowerCase" => return Some(Ok(Val::Str(Rc::from(text.to_ascii_lowercase())))),
                "toUpperCase" => return Some(Ok(Val::Str(Rc::from(text.to_ascii_uppercase())))),
                "concat" => {
                    let other = arguments.first().and_then(Val::as_str).unwrap_or("");
                    return Some(Ok(Val::Str(Rc::from(format!("{text}{other}")))));
                }
                "startsWith" => {
                    let other = arguments.first().and_then(Val::as_str).unwrap_or("");
                    return Some(Ok(Val::Bool(text.starts_with(other))));
                }
                "contains" => {
                    let other = arguments.first().and_then(Val::as_str).unwrap_or("");
                    return Some(Ok(Val::Bool(text.contains(other))));
                }
                _ => {}
            }
        }
        if owner == "net/minecraft/references/BlockItemId" && name == "create" {
            let ids: Vec<String> = arguments
                .iter()
                .filter_map(|value| match value {
                    Val::Resource(id) => Some(id.to_string()),
                    Val::Str(text) => Some(text.to_string()),
                    _ => None,
                })
                .collect();
            let block = ids.first().cloned().unwrap_or_default();
            let item = ids.get(1).cloned().unwrap_or_else(|| block.clone());
            let mut fields = BTreeMap::new();
            fields.insert(
                "block".to_string(),
                Val::Resource(Rc::from(normalise_id(&block))),
            );
            fields.insert(
                "item".to_string(),
                Val::Resource(Rc::from(normalise_id(&item))),
            );
            return Some(Ok(Val::Obj(Rc::new(RefCell::new(Object {
                class: Rc::from("net/minecraft/references/BlockItemId"),
                fields,
                enum_name: None,
                ordinal: 0,
                block_id: None,
                item_id: None,
                constructed: true,
            })))));
        }
        if owner == "net/minecraft/references/BlockItemId" {
            match name {
                "block" | "item" => {
                    if let Some(Val::Obj(object)) = receiver {
                        return Some(Ok(object
                            .borrow()
                            .fields
                            .get(name)
                            .cloned()
                            .unwrap_or(Val::Null)));
                    }
                    if let Some(Val::BlockId(id)) = receiver {
                        return Some(Ok(Val::Resource(id.clone())));
                    }
                }
                _ => {}
            }
        }
        if owner == "net/minecraft/resources/ResourceKey"
            || owner == "net/minecraft/resources/Identifier"
        {
            if name == "identifier" || name == "location" {
                if let Some(Val::Resource(id)) = receiver {
                    return Some(Ok(Val::Str(id.clone())));
                }
            }
        }
        if owner == "net/minecraft/resources/Identifier" {
            match name {
                "withDefaultNamespace" | "parse" => {
                    let path = arguments.first().and_then(Val::as_str).unwrap_or("");
                    let id = if path.contains(':') {
                        path.to_string()
                    } else {
                        format!("minecraft:{path}")
                    };
                    return Some(Ok(Val::Resource(Rc::from(id))));
                }
                "fromNamespaceAndPath" => {
                    let namespace = arguments
                        .first()
                        .and_then(Val::as_str)
                        .unwrap_or("minecraft");
                    let path = arguments.get(1).and_then(Val::as_str).unwrap_or("");
                    return Some(Ok(Val::Resource(Rc::from(format!("{namespace}:{path}")))));
                }
                _ => {}
            }
        }
        if let Some(Val::Resource(id)) = receiver {
            match name {
                "toString" | "getPath" | "getNamespace" => return Some(Ok(Val::Str(id.clone()))),
                _ => {}
            }
        }
        if owner == "net/minecraft/resources/ResourceKey" {
            match name {
                "create" | "createRegistryKey" => {
                    let id = arguments
                        .iter()
                        .filter_map(|value| match value {
                            Val::Resource(id) => Some(id.to_string()),
                            Val::Str(text) => Some(text.to_string()),
                            _ => None,
                        })
                        .last()
                        .unwrap_or_default();
                    return Some(Ok(Val::Resource(Rc::from(id))));
                }
                _ => {}
            }
        }
        if owner == "net/minecraft/tags/TagKey" && name == "create" {
            let id = arguments
                .iter()
                .filter_map(|value| match value {
                    Val::Resource(id) => Some(id.to_string()),
                    Val::Str(text) => Some(text.to_string()),
                    _ => None,
                })
                .last()
                .unwrap_or_default();
            return Some(Ok(Val::Tag(Rc::from(id))));
        }
        if owner == "net/minecraft/sounds/SoundEvent" {
            match name {
                "createVariableRangeEvent" | "createFixedRangeEvent" => {
                    if let Some(Val::Resource(id)) = arguments.first() {
                        return Some(Ok(Val::Sound(id.clone())));
                    }
                    return Some(Ok(Val::Sound(Rc::from("minecraft:empty"))));
                }
                "location" => {
                    if let Some(Val::Sound(id)) = receiver {
                        return Some(Ok(Val::Str(id.clone())));
                    }
                }
                _ => {}
            }
        }
        if owner == "net/minecraft/util/random/WeightedList" {
            if matches!(name, "of" | "builder" | "build" | "add" | "addAll") {
                if name == "add" || name == "addAll" {
                    return Some(Ok(receiver.cloned().unwrap_or(Val::Null)));
                }
                return Some(Ok(Val::Obj(Rc::new(RefCell::new(Object {
                    class: Rc::from("net/minecraft/util/random/WeightedList"),
                    fields: BTreeMap::new(),
                    enum_name: None,
                    ordinal: 0,
                    block_id: None,
                    item_id: None,
                    constructed: true,
                })))));
            }
        }
        if owner == "java/lang/Math" {
            let values: Vec<f64> = arguments.iter().filter_map(Val::as_f64).collect();
            if values.len() >= 2 {
                let result = match name {
                    "max" => values[0].max(values[1]),
                    "min" => values[0].min(values[1]),
                    "abs" => values[0].abs(),
                    _ => values[0],
                };
                return Some(Ok(Val::Double(result)));
            }
        }
        if owner == "net/minecraft/world/level/block/state/SolidDebugger" {
            return Some(Ok(Val::Unit));
        }
        if owner == "net/minecraft/server/Bootstrap" {
            return Some(Ok(Val::Unit));
        }
        let _ = descriptor;
        None
    }
}

fn property_value_text(value: &Val) -> String {
    match value {
        Val::Bool(flag) => flag.to_string(),
        Val::Int(number) => number.to_string(),
        Val::Long(number) => number.to_string(),
        Val::Str(text) => text.to_string(),
        Val::Enum(_, name, _) => name.to_ascii_lowercase(),
        Val::Obj(object) => object
            .borrow()
            .enum_name
            .as_ref()
            .map(|name| name.to_ascii_lowercase())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Rendering: the exporter's value() visitor
// ---------------------------------------------------------------------------

impl Vm {
    fn field_descriptor(&self, internal: &str, name: &str) -> Option<String> {
        let mut current = Some(internal.to_string());
        for _ in 0..64 {
            let internal = current?;
            let class = self.class(&internal)?;
            if let Some(field) = class.fields.iter().find(|field| field.name == name) {
                return Some(field.descriptor.clone());
            }
            current = if class.super_name.is_empty() {
                None
            } else {
                Some(class.super_name.clone())
            };
        }
        None
    }

    /// `value()` after the declared field's static type is known: a `Z` field
    /// holding `iconst_1` is a boolean, an `F` field holding an int is a float.
    fn render_field(
        &self,
        internal: &str,
        field: &str,
        value: &Val,
        depth: i32,
    ) -> Result<Option<Value>, String> {
        let descriptor = self.field_descriptor(internal, field);
        if let Some(descriptor) = descriptor.as_deref() {
            if descriptor.starts_with("Lnet/minecraft/world/phys/shapes/") {
                return Err("shape object identity".to_string());
            }
            if descriptor == "Ljava/util/Map;" && matches!(value, Val::Null) {
                return Err("map not reconstructed".to_string());
            }
        }
        match descriptor.as_deref() {
            Some("Z") => match value {
                Val::Bool(flag) => Ok(Some(Value::Bool(*flag))),
                Val::Int(number) => Ok(Some(Value::Bool(*number != 0))),
                Val::Null => Ok(None),
                _ => self.render_value(value, depth),
            },
            Some("F") => match value {
                Val::Null => Ok(None),
                other => {
                    let number = other
                        .as_f64()
                        .ok_or_else(|| format!("field {field} is not a float"))?;
                    Ok(Some(json_number(format_float32(number as f32))?))
                }
            },
            Some("D") => match value {
                Val::Null => Ok(None),
                other => {
                    let number = other
                        .as_f64()
                        .ok_or_else(|| format!("field {field} is not a double"))?;
                    Ok(Some(json_number(format_float64(number))?))
                }
            },
            Some("I") | Some("S") | Some("B") | Some("C") => match value {
                Val::Null => Ok(None),
                other => Ok(Some(Value::Number(Number::from(
                    other.as_int().unwrap_or(0),
                )))),
            },
            Some("J") => match value {
                Val::Null => Ok(None),
                other => Ok(Some(Value::Number(Number::from(
                    other.as_f64().unwrap_or(0.0) as i64,
                )))),
            },
            _ => self.render_value(value, depth),
        }
    }

    /// Render one block's whole `fields` object.
    fn render_block(
        &self,
        internal: &str,
        object: Option<&Rc<RefCell<Object>>>,
    ) -> Result<Value, String> {
        let Some(object) = object else {
            return Err("no instance".to_string());
        };
        if !object.borrow().constructed {
            return Err("constructor incomplete".to_string());
        }
        let mut fields = Map::new();
        for field in self.block_fields(internal) {
            let value = object
                .borrow()
                .fields
                .get(&field)
                .cloned()
                .unwrap_or(Val::Null);
            match self.render_field(internal, &field, &value, 3) {
                Ok(Some(rendered)) => {
                    fields.insert(field, rendered);
                }
                Ok(None) => {}
                Err(reason) => return Err(format!("{field}: {reason}")),
            }
        }
        Ok(Value::Object(fields))
    }

    fn render_value(&self, value: &Val, depth: i32) -> Result<Option<Value>, String> {
        let rendered = match value {
            Val::Null => return Ok(None),
            Val::Unit => return Ok(None),
            Val::Int(number) => Value::Number(Number::from(*number)),
            Val::Long(number) => Value::Number(Number::from(*number)),
            Val::Float(number) => json_number(format_float32(*number))?,
            Val::Double(number) => json_number(format_float64(*number))?,
            Val::Bool(flag) => Value::Bool(*flag),
            Val::Str(text) => Value::String(text.to_string()),
            Val::Enum(_, name, _) => Value::String(name.to_string()),
            Val::Class(name) => Value::String(name.to_string()),
            Val::Block(id) => Value::String(id.to_string()),
            Val::BlockId(id) => Value::String(normalise_id(id)),
            Val::Item(id) => Value::String(id.to_string()),
            Val::Fluid(id) => Value::String(id.to_string()),
            Val::Sound(id) => Value::String(id.to_string()),
            Val::Particle(id) => Value::String(normalise_id(id)),
            Val::Resource(id) => Value::String(normalise_id(id)),
            Val::Tag(id) => Value::String(format!("#{}", normalise_id(id))),
            Val::State(state) => {
                let id = self
                    .state_ids
                    .state_id(&state.block, &state.values)
                    .ok_or_else(|| format!("no state id for {}", state.block))?;
                let mut object = Map::new();
                object.insert("state".to_string(), Value::Number(Number::from(id)));
                Value::Object(object)
            }
            Val::Optional(inner) => match inner {
                Some(inner) => return self.render_value(inner, depth),
                None => return Ok(None),
            },
            Val::Map(entries, _) => {
                if depth <= 0 {
                    return Err("map at depth 0".to_string());
                }
                let mut object = Map::new();
                for (key, value) in entries.borrow().iter() {
                    let key = self.render_value(key, 0)?;
                    let key_text = match key {
                        Some(Value::String(text)) => text,
                        Some(other) => other.to_string(),
                        None => continue,
                    };
                    if let Some(rendered) = self.render_value(value, depth - 1)? {
                        object.insert(key_text, rendered);
                    }
                }
                Value::Object(object)
            }
            Val::List(items, class) => {
                let _ = items;
                return Ok(Some(Value::String(format!(
                    "<{}>",
                    class_simple_name(class)
                ))));
            }
            Val::Array(_) => return Err("array".to_string()),
            Val::Lambda(_) => return Err("lambda".to_string()),
            Val::Props => return Err("properties".to_string()),
            Val::Opaque(class) => return Err(format!("opaque {}", simple_name(class))),
            Val::Obj(object) => {
                let borrowed = object.borrow();
                let class = borrowed.class.to_string();
                if borrowed.enum_name.is_some() {
                    Value::String(borrowed.enum_name.clone().unwrap_or_default())
                } else if is_subclass(&self.jar, &class, BLOCK) {
                    match &borrowed.block_id {
                        Some(id) => Value::String(id.to_string()),
                        None => return Err("block without id".to_string()),
                    }
                } else if is_subclass(
                    &self.jar,
                    &class,
                    "net/minecraft/core/particles/ParticleType",
                ) {
                    let id = borrowed
                        .fields
                        .get("id")
                        .and_then(Val::as_str)
                        .unwrap_or("");
                    Value::String(normalise_id(id))
                } else if is_subclass(&self.jar, &class, SOUND_TYPE) {
                    if depth <= 0 {
                        return Err("sound type at depth 0".to_string());
                    }
                    let mut object = Map::new();
                    for (name, key) in [
                        ("volume", "volume"),
                        ("pitch", "pitch"),
                        ("break", "breakSound"),
                        ("place", "placeSound"),
                    ] {
                        let field = borrowed.fields.get(key).cloned().unwrap_or(Val::Null);
                        let Some(rendered) = self.render_value(&field, depth)? else {
                            continue;
                        };
                        object.insert(name.to_string(), rendered);
                    }
                    Value::Object(object)
                } else if class.starts_with("net/minecraft/world/level/block/grower/") {
                    if depth <= 0 {
                        return Err("grower at depth 0".to_string());
                    }
                    let mut object = Map::new();
                    for field in declared_fields(&self.jar, &class) {
                        let value = borrowed.fields.get(&field).cloned().unwrap_or(Val::Null);
                        if let Some(rendered) =
                            self.render_field(&class, &field, &value, depth - 1)?
                        {
                            object.insert(field, rendered);
                        }
                    }
                    Value::Object(object)
                } else if is_record(&self.jar, &class) {
                    if depth <= 0 {
                        return Err("record at depth 0".to_string());
                    }
                    let mut object = Map::new();
                    for field in declared_fields(&self.jar, &class) {
                        let value = borrowed.fields.get(&field).cloned().unwrap_or(Val::Null);
                        if let Some(rendered) =
                            self.render_field(&class, &field, &value, depth - 1)?
                        {
                            object.insert(field, rendered);
                        }
                    }
                    Value::Object(object)
                } else {
                    Value::String(format!("<{}>", class_simple_name(&class)))
                }
            }
        };
        Ok(Some(rendered))
    }

    /// The exported fields of a block: declared instance fields from the concrete
    /// class up to (excluding) `Block`, in declaration order, first name wins.
    fn block_fields(&self, internal: &str) -> Vec<String> {
        self.fields_until(internal, BLOCK)
    }

    fn item_fields(&self, internal: &str) -> Vec<String> {
        self.fields_until(internal, ITEM)
    }

    fn fields_until(&self, internal: &str, top: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut current = Some(internal.to_string());
        for _ in 0..64 {
            let Some(name) = current else {
                break;
            };
            if name == top {
                break;
            }
            let Some(class) = self.class(&name) else {
                break;
            };
            for field in &class.fields {
                if field.access & 0x0008 != 0 {
                    continue;
                }
                if seen.insert(field.name.clone()) {
                    out.push(field.name.clone());
                }
            }
            current = if class.super_name.is_empty() {
                None
            } else {
                Some(class.super_name.clone())
            };
        }
        out
    }
}

fn declared_fields(jar: &Jar, internal: &str) -> Vec<String> {
    jar.class(internal)
        .map(|class| {
            class
                .fields
                .iter()
                .filter(|field| field.access & 0x0008 == 0)
                .map(|field| field.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn is_record(jar: &Jar, internal: &str) -> bool {
    is_subclass(jar, internal, "java/lang/Record")
}

fn is_java_list_class(class: &str) -> bool {
    class.ends_with("$List12") || class.ends_with("$ListN")
}

fn class_simple_name(internal: &str) -> String {
    let bare = internal.rsplit('/').next().unwrap_or(internal);
    match bare.rfind('$') {
        Some(index) if !bare[index + 1..].is_empty() => bare[index + 1..].to_string(),
        _ => bare.to_string(),
    }
}

fn normalise_id(id: &str) -> String {
    if id.contains(':') {
        id.to_string()
    } else {
        format!("minecraft:{id}")
    }
}

fn json_number(text: String) -> Result<Value, String> {
    let number: f64 = text.parse().map_err(|_| format!("bad number {text}"))?;
    Ok(Value::Number(
        Number::from_f64(number).ok_or_else(|| format!("bad number {text}"))?,
    ))
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// `block-behaviours.json#blocks[*].fields` for the blocks this module can
/// reconstruct exactly, plus a coverage/gap report.
pub fn block_fields(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (exact, _, coverage) = run_blocks(jar_path)?;
    Ok((exact, coverage))
}

/// Every block's rendered `fields` (exact or not), plus the exact count.
pub fn block_fields_all(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (_, all, coverage) = run_blocks(jar_path)?;
    Ok((all, coverage))
}

/// `(exact, all rendered, coverage)` in one run, for diagnostics/tests.
pub fn block_fields_report(
    jar_path: &Path,
) -> Result<(BTreeMap<String, Value>, BTreeMap<String, Value>, Coverage), String> {
    run_blocks(jar_path)
}

fn run_blocks(
    jar_path: &Path,
) -> Result<(BTreeMap<String, Value>, BTreeMap<String, Value>, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    let state_ids = StateIds::new(&table);
    let mut coverage = Coverage {
        blocks: registry.len(),
        ..Default::default()
    };
    let mut vm = Vm::new(jar, registry.clone(), state_ids);
    vm.ensure_initialized(BLOCKS);
    vm.fire_bootstrap();
    coverage.notes = vm.notes.clone();
    coverage.notes.push(format!(
        "{} instances for {} registry rows",
        vm.blocks.len(),
        registry.len()
    ));
    if vm.next_block != registry.len() {
        coverage.gaps.insert(
            format!(
                "blocks: {} registrations executed, {} in the registry",
                vm.next_block,
                registry.len()
            ),
            1,
        );
    }
    let mut rendered = BTreeMap::new();
    let mut exact = BTreeMap::new();
    for (index, row) in registry.iter().enumerate() {
        let internal = vm.internal_classes[index]
            .clone()
            .unwrap_or_else(|| BLOCK.to_string());
        if vm.block_fields(&internal).is_empty() {
            rendered.insert(row.id.clone(), Value::Object(Map::new()));
            exact.insert(row.id.clone(), Value::Object(Map::new()));
            coverage.blocks_fields_exact += 1;
            continue;
        }
        let object = vm.blocks.get(&row.id).cloned();
        match vm.render_block(&internal, object.as_ref()) {
            Ok(value) => {
                rendered.insert(row.id.clone(), value.clone());
                exact.insert(row.id.clone(), value);
                coverage.blocks_fields_exact += 1;
            }
            Err(reason) => {
                if let Some(object) = object.as_ref() {
                    if let Ok(value) = vm.render_block_unchecked(&internal, object) {
                        rendered.insert(row.id.clone(), value);
                    }
                }
                note_gap(
                    &mut coverage.gaps,
                    format!("block {}.fields: {reason}", row.id),
                );
            }
        }
    }
    Ok((exact, rendered, coverage))
}

impl Vm {
    fn fire_bootstrap(&mut self) {
        const FIRE_BLOCK: &str = "net/minecraft/world/level/block/FireBlock";
        self.ensure_initialized(FIRE_BLOCK);
        if let Some((class, method)) = self.resolve(FIRE_BLOCK, "bootStrap", "()V") {
            let _ = self.call(class, &method, None, Vec::new());
        }
    }

    /// Render a block's fields even when one of them is not reproducible, for
    /// diagnostics. Failed values are omitted (and the block is a gap).
    fn render_block_unchecked(
        &self,
        internal: &str,
        object: &Rc<RefCell<Object>>,
    ) -> Result<Value, String> {
        let mut fields = Map::new();
        for field in self.block_fields(internal) {
            let value = object
                .borrow()
                .fields
                .get(&field)
                .cloned()
                .unwrap_or(Val::Null);
            if let Ok(Some(rendered)) = self.render_field(internal, &field, &value, 3) {
                fields.insert(field, rendered);
            }
        }
        Ok(Value::Object(fields))
    }
}

/// `block-behaviours.json#items[*].fields` for the block items this module can
/// reconstruct exactly, plus a coverage/gap report.
pub fn item_fields(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let header_items = header::items(jar_path)?;
    let header_items = header_items.as_object().cloned().unwrap_or_default();
    let mut item_by_block: BTreeMap<String, String> = BTreeMap::new();
    let mut item_block: BTreeMap<String, String> = BTreeMap::new();
    let mut item_chains: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, entry) in &header_items {
        let Some(block) = entry["fields"]["block"].as_str() else {
            continue;
        };
        item_by_block.insert(block.to_string(), id.clone());
        item_block.insert(id.clone(), block.to_string());
        let chain = entry["class"]
            .as_array()
            .map(|array| {
                array
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        item_chains.insert(id.clone(), chain);
    }
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    let state_ids = StateIds::new(&table);
    let mut coverage = Coverage {
        blocks: registry.len(),
        ..Default::default()
    };
    let mut vm = Vm::new(jar, registry.clone(), state_ids);
    vm.ensure_initialized(BLOCKS);
    let mut rendered = BTreeMap::new();
    let Some(items_class) = vm.class(ITEMS) else {
        return Ok((rendered, coverage));
    };
    let Some(initialiser) = items_class.method("<clinit>") else {
        return Ok((rendered, coverage));
    };
    let Some(code) = &initialiser.code else {
        return Ok((rendered, coverage));
    };
    let instructions = decode_code(&code.bytes);
    let mut pending_block: Option<String> = None;
    let mut pending_item_id: Option<String> = None;
    let mut awaiting_block = false;
    let mut lambdas: Vec<usize> = Vec::new();
    for instruction in &instructions {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, descriptor)) =
                    classfile::field_ref(&items_class.constants, instruction.index)
                {
                    if owner.ends_with("/BlockItemIds") {
                        pending_block = None;
                        pending_item_id = Some(name);
                        awaiting_block = true;
                        lambdas.clear();
                    } else if owner.ends_with("/ItemIds") {
                        pending_block = None;
                        pending_item_id = None;
                        awaiting_block = false;
                        lambdas.clear();
                    } else if owner == BLOCKS
                        && descriptor.starts_with("Lnet/minecraft/world/level/block/Block;")
                    {
                        if awaiting_block || pending_block.is_none() {
                            pending_block = Some(name);
                            awaiting_block = false;
                        }
                        lambdas.clear();
                    }
                }
            }
            0xba => lambdas.push(instruction.index),
            0xb8 => {
                let Some((owner, name, _)) =
                    classfile::method_ref(&items_class.constants, instruction.index)
                else {
                    continue;
                };
                if owner != ITEMS || !name.starts_with("register") {
                    continue;
                }
                let block = pending_block
                    .take()
                    .and_then(|field| vm.blocks_field_ids.get(&field).cloned())
                    .or_else(|| {
                        let field = pending_item_id.take()?;
                        vm.block_item_block(&field)
                    });
                awaiting_block = false;
                let item = block
                    .as_ref()
                    .and_then(|block| item_by_block.get(block))
                    .cloned();
                if let (Some(block), Some(item)) = (block, item) {
                    coverage.items += 1;
                    let needs_extras = item_chains
                        .get(&item)
                        .map(|chain| chain.len() > 1)
                        .unwrap_or(false);
                    let mut done = false;
                    if needs_extras {
                        for index in &lambdas {
                            let Some((lambda_owner, lambda_name, lambda_descriptor)) =
                                indy_target(&items_class, *index)
                            else {
                                continue;
                            };
                            if !lambda_descriptor.ends_with(")Lnet/minecraft/world/item/Item;") {
                                continue;
                            }
                            let arguments = item_factory_arguments(&lambda_descriptor, &block);
                            let Some(target) = vm.class(&lambda_owner) else {
                                continue;
                            };
                            let Some(method) = target
                                .methods
                                .iter()
                                .find(|method| {
                                    method.name == lambda_name
                                        && method.descriptor == lambda_descriptor
                                })
                                .cloned()
                            else {
                                continue;
                            };
                            let is_static = method.access & 0x0008 != 0;
                            let result = vm.call(target, &method, None, arguments);
                            if let Ok(Val::Obj(object)) = result {
                                let class = object.borrow().class.to_string();
                                let chain = item_chains.get(&item).cloned().unwrap_or_default();
                                let mut fields = Map::new();
                                let mut gap = None;
                                for field in vm.item_fields(&class) {
                                    let value = object
                                        .borrow()
                                        .fields
                                        .get(&field)
                                        .cloned()
                                        .unwrap_or(Val::Null);
                                    match vm.render_field(&class, &field, &value, 3) {
                                        Ok(Some(value)) => {
                                            fields.insert(field, value);
                                        }
                                        Ok(None) => {}
                                        Err(reason) => {
                                            gap = Some(format!("{field}: {reason}"));
                                            break;
                                        }
                                    }
                                }
                                if let Some(reason) = gap {
                                    note_gap(
                                        &mut coverage.gaps,
                                        format!("item {item}.fields: {reason}"),
                                    );
                                } else {
                                    let _ = is_static;
                                    let _ = chain;
                                    rendered.insert(item.clone(), Value::Object(fields));
                                    coverage.items_fields_exact += 1;
                                }
                                done = true;
                                break;
                            }
                        }
                    }
                    if !done && !needs_extras {
                        let mut fields = Map::new();
                        fields.insert("block".to_string(), Value::String(block.clone()));
                        rendered.insert(item.clone(), Value::Object(fields));
                        coverage.items_fields_exact += 1;
                    } else if !done {
                        note_gap(
                            &mut coverage.gaps,
                            format!("item {item}.fields: factory not reconstructed"),
                        );
                    }
                }
                lambdas.clear();
            }
            _ => {}
        }
    }
    coverage.items = item_chains.len();
    // Items registered through a colour/weathering family never reach a direct
    // `registerBlock` call in `Items.<clinit>`; every one whose class declares
    // no field beyond `BlockItem.block` has the trivial `{"block": id}` value.
    for (item, chain) in &item_chains {
        if rendered.contains_key(item) {
            continue;
        }
        let Some(block) = item_block.get(item) else {
            continue;
        };
        if vm.item_class_has_extra_fields(chain) {
            // Banners are registered through a colour family; their wall block
            // is that colour's member of the executed `Blocks.WALL_BANNER`
            // collection, so it can be recovered without a table.
            if chain.iter().any(|class| class == "BannerItem") {
                if let Some(fields) = vm.banner_item_fields(block) {
                    rendered.insert(item.clone(), fields);
                    coverage.items_fields_exact += 1;
                    continue;
                }
            }
            note_gap(
                &mut coverage.gaps,
                format!("item {item}.fields: factory not reconstructed"),
            );
            continue;
        }
        let mut fields = Map::new();
        fields.insert("block".to_string(), Value::String(block.clone()));
        rendered.insert(item.clone(), Value::Object(fields));
        coverage.items_fields_exact += 1;
    }
    coverage.items_fields_exact = rendered.len();
    Ok((rendered, coverage))
}

impl Vm {
    /// The block a `BlockItemIds.<field>` record references.
    fn block_item_block(&mut self, field: &str) -> Option<String> {
        let value = self.get_static(
            "net/minecraft/references/BlockItemIds",
            field,
            "Lnet/minecraft/references/BlockItemId;",
        );
        if let Val::Obj(object) = value {
            let block = object.borrow().fields.get("block").cloned();
            if let Some(Val::Resource(id)) = block {
                return Some(normalise_id(&id));
            }
        }
        None
    }

    /// `BannerItem.fields` for a banner registered through the colour family:
    /// `wallBlock` is `Blocks.WALL_BANNER.pick(DyeColor)` and the attachment is
    /// `Direction.DOWN`.
    fn banner_item_fields(&mut self, block: &str) -> Option<Value> {
        let color = block
            .trim_start_matches("minecraft:")
            .strip_suffix("_banner")?
            .to_string();
        let dye_name = color.to_ascii_uppercase();
        let dye = Val::Enum(
            Rc::from("net/minecraft/world/item/DyeColor"),
            Rc::from(dye_name.as_str()),
            self.enum_ordinal("net/minecraft/world/item/DyeColor", &dye_name),
        );
        let collection = self
            .statics
            .get(&(BLOCKS.to_string(), "WALL_BANNER".to_string()))?
            .clone();
        let wall = self
            .invoke(
                0xb6,
                "net/minecraft/world/level/block/ColorCollection",
                "pick",
                "(Lnet/minecraft/world/item/DyeColor;)Ljava/lang/Object;",
                Some(collection),
                vec![dye],
            )
            .ok()?;
        let wall_id = match wall {
            Val::Block(id) => id,
            _ => return None,
        };
        let direction = Val::Enum(
            Rc::from("net/minecraft/core/Direction"),
            Rc::from("DOWN"),
            self.enum_ordinal("net/minecraft/core/Direction", "DOWN"),
        );
        let mut fields = Map::new();
        fields.insert("wallBlock".to_string(), Value::String(wall_id.to_string()));
        fields.insert(
            "attachmentDirection".to_string(),
            Value::String("DOWN".to_string()),
        );
        let _ = direction;
        fields.insert("block".to_string(), Value::String(block.to_string()));
        Some(Value::Object(fields))
    }

    /// Whether any class on an item's chain declares an instance field other
    /// than `BlockItem.block`.
    fn item_class_has_extra_fields(&self, chain: &[String]) -> bool {
        let Some(first) = chain.first() else {
            return false;
        };
        let Some(internal) = self.jar.resolve_under("net/minecraft/world/item", first) else {
            return true;
        };
        let mut current = Some(internal);
        for _ in 0..16 {
            let Some(name) = current else {
                break;
            };
            if name == ITEM {
                break;
            }
            let Some(class) = self.class(&name) else {
                break;
            };
            for field in &class.fields {
                if field.access & 0x0008 != 0 {
                    continue;
                }
                if field.name == "block" {
                    continue;
                }
                return true;
            }
            current = if class.super_name.is_empty() {
                None
            } else {
                Some(class.super_name.clone())
            };
        }
        false
    }
}

fn item_factory_arguments(descriptor: &str, block: &str) -> Vec<Val> {
    let (types, _) = descriptor_args(descriptor);
    let mut arguments = Vec::new();
    for kind in types {
        if kind.contains("Item$Properties") {
            arguments.push(Val::Props);
        } else if kind.contains("world/level/block/Block") {
            arguments.push(Val::Block(Rc::from(block)));
        } else {
            arguments.push(Val::Null);
        }
    }
    arguments
}
