//! `_shared/gameplay.compact.json`, exact: every block, every derivable field.
//!
//! [`base`] (`gameplay_base`) starts from `gameplay_final`'s property model and patches the
//! three base-layer fields, but its model is shallow: it never runs the constructor that
//! changes `Properties` (wooden doors/signs/buttons get their sound there, infested blocks
//! their hardness), never runs the parameterised `Blocks` helpers that build a whole chain
//! (`leavesProperties`, `candleProperties`, `registerSlab`, ...), and misses the
//! collection-registered families (`ColorCollection.registerBlocks`,
//! `WeatheringCopperCollection.registerBlocks`). Its `mapColor` is also wrong wherever the
//! value is a `DyeColor`, a `MapColor` constant reached through a copy, or a per-state
//! function.
//!
//! This module re-derives each block's `Properties` from `Blocks.<clinit>` with a small stack
//! VM: it executes the registration statement (including the helper bodies it calls and the
//! block constructor the registration lambda names), resolves `Properties.ofFullCopy` /
//! `ofLegacyCopy` against the already-resolved base block, and then reads the fields the
//! compact table carries. The colour families are run per dye colour; the copper families per
//! `WeatheringCopper$WeatherState`; the per-state `mapColor` lambdas are evaluated for every
//! state through the same state ordering `states` derives.
//!
//! The light-engine fields, the selection-outline geometry and the item/render mapping are
//! still out of reach and are listed in [`MISSING`]. Nothing here reads `ship.hzr3`; the
//! prepared copy under `tmp/ship-extract` is only ever read by the tests.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::Value;

use crate::classfile;

#[path = "gameplay_base.rs"]
#[allow(dead_code)]
mod base;

#[path = "gameplay.rs"]
#[allow(dead_code)]
mod gameplay;

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

#[path = "shape_final.rs"]
#[allow(dead_code)]
mod shape_final;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = gameplay::TABLE;

/// The fields this module derives, in the bundle's order.
pub use gameplay::FIELDS;

/// The fields the reference has and this module does not derive (the light engine, the
/// selection-outline geometry and the item/render registry mapping are not ported).
pub const MISSING: [&str; 11] = [
    "dampening",
    "emission",
    "item",
    "outline",
    "outline_edges",
    "outline_offset",
    "random_ticks",
    "render_shape",
    "shape_occlusion",
    "skylight_down",
    "solid_render",
];

/// What was produced, and what could not be.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    /// Blocks whose per-block row could not be derived and fell back to the base layer.
    pub unresolved: Vec<String>,
    /// Blocks whose `mapColor` function this module could not evaluate.
    pub map_color_functions: Vec<String>,
}

/// The rows this module produces (one per block, `gameplay::FIELDS` order), plus the
/// per-state `mapColor`/`sound` values where they vary.
pub struct Rows {
    pub blocks: Vec<String>,
    pub rows: Vec<BTreeMap<&'static str, Value>>,
    /// `block index -> every state's map color`, when it varies (empty means constant).
    pub map_colors: BTreeMap<usize, Vec<Value>>,
    /// `block index -> every state's sound`, when it varies (empty means constant).
    pub sounds: BTreeMap<usize, Vec<Value>>,
    pub unresolved: Vec<String>,
    pub map_color_functions: Vec<String>,
}

const PROPERTIES: &str = "net/minecraft/world/level/block/state/BlockBehaviour$Properties";

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

// ---------------------------------------------------------------------------
// The jar and its bytecode
// ---------------------------------------------------------------------------

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

    fn class(&self, name: &str) -> Option<classfile::ClassFile> {
        let bytes = if name.contains('/') {
            self.files.get(name)
        } else {
            self.by_short
                .get(name)
                .and_then(|internal| self.files.get(internal))
        }?;
        classfile::parse(bytes).ok()
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

}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

#[derive(Clone, Debug)]
struct Switch {
    low: i32,
    high: i32,
    targets: Vec<i32>,
    default: i32,
}

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
    /// A `tableswitch`/`lookupswitch`'s relative targets (branches use `at` + target).
    switch: Option<Switch>,
    at: usize,
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
    bytes
        .get(at..at + 4)
        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let mut immediate = None;
        let mut switch = None;
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
                let default = i32_at(bytes, base).unwrap_or(0);
                let low = i32_at(bytes, base + 4).unwrap_or(0);
                let high = i32_at(bytes, base + 8).unwrap_or(-1);
                let count = if high >= low {
                    (high as i64 - low as i64 + 1) as usize
                } else {
                    0
                };
                let mut targets = Vec::with_capacity(count);
                for index in 0..count {
                    targets.push(i32_at(bytes, base + 12 + index * 4).unwrap_or(0));
                }
                switch = Some(Switch {
                    low,
                    high,
                    targets,
                    default,
                });
                1 + padding + 12 + count * 4
            }
            0xab => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base = at + 1 + padding;
                let default = i32_at(bytes, base).unwrap_or(0);
                let pairs = i32_at(bytes, base + 4).unwrap_or(0).max(0) as usize;
                let mut targets = Vec::with_capacity(pairs);
                let mut keys = Vec::with_capacity(pairs);
                for index in 0..pairs {
                    keys.push(i32_at(bytes, base + 8 + index * 8).unwrap_or(0));
                    targets.push(i32_at(bytes, base + 12 + index * 8).unwrap_or(0));
                }
                switch = Some(Switch {
                    low: keys.first().copied().unwrap_or(0),
                    high: keys.last().copied().unwrap_or(-1),
                    targets,
                    default,
                });
                1 + padding + 8 + pairs * 8
            }
            _ => 1,
        };
        out.push(Instruction {
            opcode,
            index,
            immediate,
            switch,
            at,
        });
        at += length;
    }
    out
}

fn constant(class: &classfile::ClassFile, index: usize) -> Option<&classfile::Constant> {
    class.constants.get(index)
}

/// Resolves a `NameAndType`/`Class` reference (`#12`) to the UTF-8 text it names.
fn reference_utf8(class: &classfile::ClassFile, reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match class.constants.get(index)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

fn utf8_at(class: &classfile::ClassFile, reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match class.constants.get(index)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

/// `(owner, name, descriptor)` of a field/method reference.
fn member(class: &classfile::ClassFile, index: usize) -> Option<(String, String, String)> {
    let (owner_ref, name_ref, kind) = match class.constants.get(index)? {
        classfile::Constant::Fieldref { class, name, kind } => (class, name, kind),
        classfile::Constant::Methodref { class, name, kind } => (class, name, kind),
        classfile::Constant::InterfaceMethodref { class, name, kind } => (class, name, kind),
        _ => return None,
    };
    let owner_index: usize = owner_ref.trim_start_matches('#').parse().ok()?;
    let owner = match class.constants.get(owner_index)? {
        classfile::Constant::Class(reference) => utf8_at(class, reference)?,
        _ => return None,
    };
    let name_index: usize = name_ref.trim_start_matches('#').parse().ok()?;
    let (name, descriptor) = match class.constants.get(name_index)? {
        classfile::Constant::NameAndType { name, kind } => {
            (utf8_at(class, name)?, utf8_at(class, kind)?)
        }
        classfile::Constant::Utf8(text) => (text.clone(), kind.clone()),
        _ => return None,
    };
    Some((owner, name, descriptor))
}

/// The implementation method behind an `invokedynamic`, plus the call-site descriptor whose
/// arguments are the lambda's captured values.
fn lambda_ref(
    class: &classfile::ClassFile,
    index: usize,
) -> Option<(String, String, String, String)> {
    let (bootstrap, name_and_type) = match class.constants.get(index)? {
        classfile::Constant::InvokeDynamic {
            bootstrap, name, ..
        } => (*bootstrap, name.clone()),
        _ => return None,
    };
    let type_index: usize = name_and_type.trim_start_matches('#').parse().ok()?;
    let call_descriptor = match class.constants.get(type_index)? {
        classfile::Constant::NameAndType { kind, .. } => reference_utf8(class, kind)?,
        _ => return None,
    };
    let row = class.bootstraps.get(bootstrap)?;
    for argument in &row.arguments {
        let Some(classfile::Constant::MethodHandle { reference }) =
            class.constants.get(*argument)
        else {
            continue;
        };
        if let Some((owner, name, descriptor)) = member(class, *reference) {
            return Some((owner, name, descriptor, call_descriptor));
        }
    }
    None
}

/// The internal name behind a `Class` constant.
fn class_name_at(class: &classfile::ClassFile, index: usize) -> Option<String> {
    match class.constants.get(index)? {
        classfile::Constant::Class(reference) => reference_utf8(class, reference),
        _ => None,
    }
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

fn return_type(descriptor: &str) -> Option<&str> {
    descriptor.find(')').map(|at| &descriptor[at + 1..])
}

fn float_value(value: f32) -> Value {
    let mut text = format!("{value}");
    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
        text.push_str(".0");
    }
    serde_json::from_str(&text).unwrap_or_else(|_| Value::from(value as f64))
}

// ---------------------------------------------------------------------------
// The property model
// ---------------------------------------------------------------------------

/// A captured lambda: the implementation method and its captured arguments.
#[derive(Clone, Debug, PartialEq)]
struct FnRef {
    class: String,
    method: String,
    descriptor: String,
    captured: Vec<Val>,
}

/// Where a `MapColor` comes from.
#[derive(Clone, Debug, PartialEq)]
enum MapColour {
    /// `MapColor.<FIELD>` (a constant `MapColor` argument).
    Constant(String),
    /// `DyeColor.<FIELD>.getMapColor()` / `.getTerracottaColor()`.
    Dye(String, bool),
    /// A `Function<BlockState, MapColor>`.
    Function(FnRef),
    /// A bed: the foot half uses the dye's map colour, the head half `MapColor.WOOL`.
    Bed(String),
    /// `Blocks.<FIELD>.defaultMapColor()`, resolved against that block's own properties.
    Base(String),
}

/// What `ofFullCopy`/`ofLegacyCopy` copied.
#[derive(Clone, Debug, PartialEq)]
enum CopyOf {
    /// `Blocks.<FIELD>` (resolved through the registry id table).
    Field(String),
    /// A block id, when the argument came from a collection pick.
    Id(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Props {
    can_occlude: Option<bool>,
    hardness: Option<f32>,
    resistance: Option<f32>,
    friction: Option<f32>,
    speed: Option<f32>,
    jump: Option<f32>,
    requires_tool: Option<bool>,
    random_ticks: Option<bool>,
    replaceable: Option<bool>,
    ignited_by_lava: Option<bool>,
    liquid: Option<bool>,
    offset: Option<bool>,
    no_collision: bool,
    no_occlusion: bool,
    sound: Option<String>,
    map_color: Option<MapColour>,
    push: Option<String>,
    /// `Some((base, full copy))` for `ofFullCopy`/`ofLegacyCopy`.
    copy_of: Option<(CopyOf, bool)>,
}

impl Props {
    fn mutate(&mut self, name: &str, descriptor: &str, arguments: &[Val]) {
        match name {
            "strength" => {
                if let Some(hardness) = arguments.first().and_then(as_float) {
                    self.hardness = Some(hardness);
                    self.resistance = Some(hardness.max(0.0));
                }
                if let Some(resistance) = arguments.get(1).and_then(as_float) {
                    self.resistance = Some(resistance.max(0.0));
                }
            }
            "destroyTime" => self.hardness = arguments.first().and_then(as_float),
            "explosionResistance" => {
                self.resistance = arguments.first().and_then(as_float).map(|value| value.max(0.0))
            }
            "instabreak" => {
                self.hardness = Some(0.0);
                self.resistance = Some(0.0);
            }
            "mapColor" => match arguments.first() {
                Some(Val::Ref(owner, field)) if short(owner) == "MapColor" => {
                    self.map_color = Some(MapColour::Constant(field.clone()));
                }
                Some(Val::Ref(owner, field)) if short(owner) == "DyeColor" => {
                    let terracotta = descriptor.contains("getTerracotta");
                    self.map_color = Some(MapColour::Dye(field.clone(), terracotta));
                }
                Some(Val::Ref(owner, field)) if short(owner) == "BaseMapColor" => {
                    self.map_color = Some(MapColour::Base(field.clone()));
                }
                Some(Val::Fn(function)) => {
                    self.map_color = Some(MapColour::Function((**function).clone()));
                }
                _ => {}
            },
            "sound" => {
                if let Some(Val::Ref(owner, field)) = arguments.first() {
                    if short(owner) == "SoundType" {
                        self.sound = Some(field.clone());
                    }
                }
            }
            "requiresCorrectToolForDrops" => self.requires_tool = Some(true),
            "randomTicks" => self.random_ticks = Some(true),
            "friction" => self.friction = arguments.first().and_then(as_float),
            "speedFactor" => self.speed = arguments.first().and_then(as_float),
            "jumpFactor" => self.jump = arguments.first().and_then(as_float),
            "replaceable" => self.replaceable = Some(true),
            "noCollision" => {
                self.no_collision = true;
                self.can_occlude = Some(false);
            }
            "noOcclusion" => {
                self.no_occlusion = true;
                self.can_occlude = Some(false);
            }
            "ignitedByLava" => self.ignited_by_lava = Some(true),
            "liquid" => self.liquid = Some(true),
            "offsetType" => {
                if let Some(Val::Ref(_, field)) = arguments.first() {
                    self.offset = Some(field != "NONE");
                }
            }
            "pushReaction" | "pistonPush" => {
                if let Some(Val::Ref(_, field)) = arguments.first() {
                    self.push = Some(field.clone());
                }
            }
            _ => {}
        }
    }
}

fn as_float(value: &Val) -> Option<f32> {
    match value {
        Val::Int(int) => Some(*int as f32),
        Val::Float(float) => Some(*float),
        _ => None,
    }
}

fn field_of(value: &Val) -> Option<String> {
    match value {
        Val::Ref(_, field) => Some(field.clone()),
        _ => None,
    }
}

type ObjRef = Rc<RefCell<BTreeMap<String, Val>>>;

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Int(i32),
    Float(f32),
    Str(String),
    Ref(String, String),
    /// A `new` instance, with its fields.
    Obj(String, ObjRef),
    Props(Box<Props>),
    Fn(Box<FnRef>),
    /// A `Blocks.<FIELD>` block reference (`ofFullCopy(Blocks.X)`).
    BlockField(String),
    /// A resolved block id (`minecraft:copper_block`), from a collection pick.
    BlockId(String),
    /// A `WeatheringCopperCollection`/`ColorCollection` field.
    Collection(String),
    SwitchMap,
    Opaque,
}

/// One registration call the VM ran into.
#[derive(Clone, Debug)]
struct Captured {
    /// The owner class of the registration call.
    call_owner: String,
    /// The id reference: `(owner short, field)`.
    ids: Vec<(String, String)>,
    props: Option<Props>,
    ctor: Option<FnRef>,
    /// Every function argument the call had (constructors and properties functions).
    functions: Vec<FnRef>,
    /// `Some(properties function)` for collection registrations.
    family: Option<FnRef>,
}

/// The tables the VM needs beyond the bytecode.
#[derive(Default)]
struct Tables {
    map_colours: BTreeMap<String, u32>,
    dye_colours: BTreeMap<String, String>,
    dye_terracotta: BTreeMap<String, String>,
    sounds: BTreeMap<String, Value>,
    wood_sound: BTreeMap<String, String>,
    wood_hanging_sound: BTreeMap<String, String>,
    block_set_sound: BTreeMap<String, String>,
    enum_ordinals: BTreeMap<String, BTreeMap<String, i32>>,
}

struct Vm<'a> {
    jar: &'a Jar,
    classes: RefCell<HashMap<String, Option<Rc<classfile::ClassFile>>>>,
    tables: Rc<Tables>,
    captured: RefCell<Vec<Captured>>,
    depth: std::cell::Cell<usize>,
    /// The last `Properties` value a call produced (the constructor's mutation, if any).
    last_props: RefCell<Option<Props>>,
    /// The `(owner, field)` id of the `register*` helper currently running.
    helper_ids: RefCell<Vec<(String, String)>>,
}

impl<'a> Vm<'a> {
    fn new(jar: &'a Jar, tables: Rc<Tables>) -> Vm<'a> {
        Vm {
            jar,
            classes: RefCell::new(HashMap::new()),
            tables,
            captured: RefCell::new(Vec::new()),
            depth: std::cell::Cell::new(0),
            last_props: RefCell::new(None),
            helper_ids: RefCell::new(Vec::new()),
        }
    }

    fn class(&self, name: &str) -> Option<Rc<classfile::ClassFile>> {
        if let Some(hit) = self.classes.borrow().get(name) {
            return hit.clone();
        }
        let parsed = self
            .jar
            .class(name)
            .map(Rc::new);
        self.classes
            .borrow_mut()
            .insert(name.to_string(), parsed.clone());
        parsed
    }

    fn run_method(
        &self,
        class: &Rc<classfile::ClassFile>,
        method: &classfile::Member,
        locals: Vec<Val>,
    ) -> Option<Val> {
        let code = method.code.as_ref()?;
        let instructions = decode(&code.bytes);
        self.execute(class, &instructions, locals).map(|stack| {
            stack.last().cloned().unwrap_or(Val::Opaque)
        })
    }

    fn execute(
        &self,
        class: &Rc<classfile::ClassFile>,
        instructions: &[Instruction],
        locals: Vec<Val>,
    ) -> Option<Vec<Val>> {
        let mut locals = locals;
        let mut stack: Vec<Val> = Vec::new();
        let mut index_at: HashMap<usize, usize> = HashMap::new();
        for (slot, instruction) in instructions.iter().enumerate() {
            index_at.insert(instruction.at, slot);
        }
        let mut pc = 0usize;
        let mut steps = 0usize;
        while pc < instructions.len() {
            steps += 1;
            if steps > 60_000 {
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
                0x15 | 0x19 => stack.push(locals.get(instruction.index?).cloned().unwrap_or(Val::Opaque)),
                0x16 | 0x17 | 0x18 => stack.push(locals.get(instruction.index?).cloned().unwrap_or(Val::Opaque)),
                0x1a..=0x1d => {
                    let slot = (instruction.opcode - 0x1a) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(Val::Opaque));
                }
                0x1e..=0x21 => {
                    let slot = (instruction.opcode - 0x1e) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(Val::Opaque));
                }
                0x22..=0x25 => {
                    let slot = (instruction.opcode - 0x22) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(Val::Opaque));
                }
                0x26..=0x29 => {
                    let slot = (instruction.opcode - 0x26) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(Val::Opaque));
                }
                0x2a..=0x2d => {
                    let slot = (instruction.opcode - 0x2a) as usize;
                    stack.push(locals.get(slot).cloned().unwrap_or(Val::Opaque));
                }
                0x3b..=0x3e => {
                    let slot = (instruction.opcode - 0x3b) as usize;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0x3f..=0x42 => {
                    let slot = (instruction.opcode - 0x3f) as usize;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0x43..=0x46 => {
                    let slot = (instruction.opcode - 0x43) as usize;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0x47..=0x4a => {
                    let slot = (instruction.opcode - 0x47) as usize;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0x4b..=0x4e => {
                    let slot = (instruction.opcode - 0x4b) as usize;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0x36 | 0x37 | 0x38 | 0x39 | 0x3a => {
                    let slot = instruction.index?;
                    let value = stack.pop()?;
                    set_local(&mut locals, slot, value);
                }
                0xb4 => {
                    let (_, field, _) = member(class, instruction.index?)?;
                    let object = stack.pop()?;
                    let value = match object {
                        Val::Obj(_, fields) => fields
                            .borrow()
                            .get(&field)
                            .cloned()
                            .unwrap_or(Val::Opaque),
                        _ => Val::Opaque,
                    };
                    stack.push(value);
                }
                0xb5 => {
                    let (_, field, _) = member(class, instruction.index?)?;
                    let value = stack.pop()?;
                    let object = stack.pop()?;
                    if let Val::Obj(_, fields) = object {
                        fields.borrow_mut().insert(field, value);
                    }
                }
                0x86 => {
                    // i2f
                    let value = stack.pop()?;
                    stack.push(Val::Float(ints(&value) as f32));
                }
                0x8b => {
                    // f2i
                    let value = stack.pop()?;
                    stack.push(match value {
                        Val::Float(float) => Val::Int(float as i32),
                        other => other,
                    });
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
                    let (owner, name, descriptor) = member(class, instruction.index?)?;
                    let simple = short(&owner).to_string();
                    if simple == "Blocks" {
                        if descriptor.contains("WeatheringCopperCollection")
                            || descriptor.contains("ColorCollection")
                        {
                            stack.push(Val::Collection(name.clone()));
                        } else if descriptor.ends_with("/Block;") {
                            stack.push(Val::BlockField(name.clone()));
                        } else {
                            stack.push(Val::Ref(simple, name));
                        }
                    } else if (simple == "BlockItemIds" || simple == "BlockIds")
                        && (descriptor.contains("ColorCollection")
                            || descriptor.contains("WeatheringCopperCollection"))
                    {
                        stack.push(Val::Collection(name.clone()));
                    } else if simple == "Blocks$1" && name.contains("SwitchMap") {
                        stack.push(Val::SwitchMap);
                    } else {
                        let _ = descriptor;
                        stack.push(Val::Ref(simple, name));
                    }
                }
                0xb6 | 0xb7 | 0xb8 | 0xb9 => {
                    let (owner, name, descriptor) = member(class, instruction.index?)?;
                    if (name == "register"
                        && (short(&owner) == "Blocks"
                            || short(&owner) == "ColorCollection"
                            || short(&owner) == "WeatheringCopperCollection"))
                        || (name == "registerBlocks"
                            && (short(&owner) == "ColorCollection"
                                || short(&owner) == "WeatheringCopperCollection"))
                    {
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
                        let mut all = Vec::new();
                        if let Some(receiver) = receiver {
                            all.push(receiver);
                        }
                        all.extend(arguments);
                        self.capture(short(&owner), &name, &descriptor, &all);
                        stack.push(Val::Opaque);
                        continue;
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
                    let helper_id = if short(&owner) == "Blocks"
                        && name.starts_with("register")
                        && name != "register"
                    {
                        arguments.iter().find_map(|value| match value {
                            Val::Ref(id_owner, field)
                                if short(id_owner) == "BlockItemIds"
                                    || short(id_owner) == "BlockIds" =>
                            {
                                Some((short(id_owner).to_string(), field.clone()))
                            }
                            Val::BlockField(field) => {
                                Some(("Blocks".to_string(), field.clone()))
                            }
                            _ => None,
                        })
                    } else {
                        None
                    };
                    if let Some(id) = &helper_id {
                        self.helper_ids.borrow_mut().push(id.clone());
                    }
                    let value =
                        self.call(class, &owner, &name, &descriptor, receiver, arguments)?;
                    if helper_id.is_some() {
                        self.helper_ids.borrow_mut().pop();
                    }
                    let returns = return_type(&descriptor) != Some("V");
                    if instruction.opcode == 0xb7 || !returns {
                        // The constructor's `new` object is already on the stack.
                    } else {
                        stack.push(value);
                    }
                }
                0xba => {
                    // `invokedynamic`: a lambda; capture its evaluated arguments.
                    let (owner, name, descriptor, call_descriptor) =
                        lambda_ref(class, instruction.index.unwrap_or(0))
                            .unwrap_or_else(|| {
                                (String::new(), String::new(), String::new(), String::new())
                            });
                    let count = argument_count(&call_descriptor);
                    let mut captured = Vec::with_capacity(count);
                    for _ in 0..count {
                        captured.push(stack.pop().unwrap_or(Val::Opaque));
                    }
                    captured.reverse();
                    stack.push(Val::Fn(Box::new(FnRef {
                        class: short(&owner).to_string(),
                        method: name,
                        descriptor,
                        captured,
                    })));
                }
                0xa7 => next = jump_target(instructions, &index_at, pc)?,
                0x99..=0xa6 => {
                    let right = stack.pop()?;
                    let left = stack.pop()?;
                    let taken = match instruction.opcode {
                        0x99 => matches!(left, Val::Int(value) if value == 0),
                        0x9a => matches!(left, Val::Int(value) if value != 0),
                        0x9b | 0x9c | 0x9d | 0x9e => false,
                        0x9f => ints(&left) < ints(&right),
                        0xa0 => ints(&left) >= ints(&right),
                        0xa1 => ints(&left) > ints(&right),
                        0xa2 => ints(&left) <= ints(&right),
                        0xa3 => ints(&left) == ints(&right),
                        0xa4 => ints(&left) != ints(&right),
                        0xa5 | 0xa6 => {
                            let equal = left == right;
                            (instruction.opcode == 0xa5) == equal
                        }
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
                0xc0 | 0xc1 => {}
                0xaa | 0xab => {
                    let switch = instruction.switch.as_ref()?;
                    let key = match stack.pop()? {
                        Val::Int(value) => value,
                        _ => return None,
                    };
                    let offset = if key >= switch.low && key <= switch.high {
                        switch.targets.get((key - switch.low) as usize).copied()
                    } else {
                        None
                    }
                    .unwrap_or(switch.default);
                    let target = (instruction.at as i64 + offset as i64) as usize;
                    next = *index_at.get(&target)?;
                }
                0xbb => {
                    let owner = class_name_at(class, instruction.index?)?;
                    stack.push(Val::Obj(
                        short(&owner).to_string(),
                        Rc::new(RefCell::new(BTreeMap::new())),
                    ));
                }
                0xbd => stack.push(Val::Opaque),
                0xbe => {
                    stack.push(Val::Int(0));
                }
                0xbf => return None,
                0x2e | 0x32 | 0x33 | 0x34 | 0x35 => {
                    let index = stack.pop()?;
                    let array = stack.pop()?;
                    match (&array, &index) {
                        (Val::SwitchMap, Val::Int(value)) => stack.push(Val::Int(value + 1)),
                        _ => stack.push(Val::Opaque),
                    }
                }
                0x4f..=0x56 => {
                    stack.pop()?;
                    stack.pop()?;
                    stack.pop()?;
                }
                0x83 => {
                    // l2f: not used by the property chains.
                }
                0xb0 | 0xac | 0xad | 0xae | 0xaf => return Some(vec![stack.pop()?]),
                0xb1 => return Some(stack),
                _ => return None,
            }
            pc = next;
        }
        Some(stack)
    }

    fn capture(&self, owner: &str, name: &str, descriptor: &str, all: &[Val]) {
        let mut ids = Vec::new();
        let mut props = None;
        let mut ctor = None;
        let mut family = None;
        let mut functions = Vec::new();
        for value in all {
            match value {
                Val::Ref(id_owner, field)
                    if short(id_owner) == "BlockItemIds"
                        || short(id_owner) == "BlockIds" =>
                {
                    ids.push((short(id_owner).to_string(), field.clone()))
                }
                Val::Ref(id_owner, field) if short(id_owner) == "Blocks" => {
                    ids.push(("Blocks".to_string(), field.clone()))
                }
                Val::BlockField(field) => ids.push(("Blocks".to_string(), field.clone())),
                Val::Collection(field) => ids.push(("Collection".to_string(), field.clone())),
                Val::Props(props_value) => props = Some((**props_value).clone()),
                Val::Fn(function) => {
                    functions.push((**function).clone());
                    if return_type(&function.descriptor)
                        .map(|returns| returns.contains("Properties"))
                        .unwrap_or(false)
                    {
                        family = Some((**function).clone());
                    } else if ctor.is_none() {
                        ctor = Some((**function).clone());
                    }
                }
                _ => {}
            }
        }
        // A registration has exactly one properties value; helpers may hand it over.
        if ids.is_empty() {
            if let Some(id) = self.helper_ids.borrow().last().cloned() {
                ids.push(id);
            }
        }
        self.captured.borrow_mut().push(Captured {
            call_owner: owner.to_string(),
            ids,
            props,
            ctor,
            functions,
            family: if name == "registerBlocks" { family } else { None },
        });
        let _ = descriptor;
    }

    fn call(
        &self,
        class: &Rc<classfile::ClassFile>,
        owner: &str,
        name: &str,
        descriptor: &str,
        receiver: Option<Val>,
        arguments: Vec<Val>,
    ) -> Option<Val> {
        let simple = short(owner);
        if owner == PROPERTIES || simple == "Properties" {
            match name {
                "of" => {
                    let props = Props::default();
                    *self.last_props.borrow_mut() = Some(props.clone());
                    return Some(Val::Props(Box::new(props)));
                }
                "ofFullCopy" | "ofLegacyCopy" => {
                    let full = name == "ofFullCopy";
                    let copy_of = match arguments.first() {
                        Some(Val::BlockField(field)) => Some(CopyOf::Field(field.clone())),
                        Some(Val::BlockId(id)) => Some(CopyOf::Id(id.clone())),
                        _ => None,
                    };
                    let mut props = Props::default();
                    props.copy_of = copy_of.map(|copy| (copy, full));
                    *self.last_props.borrow_mut() = Some(props.clone());
                    return Some(Val::Props(Box::new(props)));
                }
                _ => {}
            }
            let mut props = match receiver {
                Some(Val::Props(props)) => *props,
                _ => Props::default(),
            };
            props.mutate(name, descriptor, &arguments);
            *self.last_props.borrow_mut() = Some(props.clone());
            return Some(Val::Props(Box::new(props)));
        }
        if simple == "DyeColor" {
            match name {
                "getMapColor" | "getTerracottaColor" => {
                    if let Some(Val::Ref(_, field)) = arguments.first() {
                        let _ = field;
                    }
                    if let Some(Val::Ref(_, field)) = receiver {
                        let table = if name == "getTerracottaColor" {
                            &self.tables.dye_terracotta
                        } else {
                            &self.tables.dye_colours
                        };
                        if let Some(colour) = table.get(&field) {
                            return Some(Val::Ref("MapColor".to_string(), colour.clone()));
                        }
                    }
                    return Some(Val::Opaque);
                }
                _ => return Some(Val::Opaque),
            }
        }
        if simple == "WoodType" {
            if let Some(Val::Ref(_, field)) = receiver {
                let table = if name == "hangingSignSoundType" {
                    &self.tables.wood_hanging_sound
                } else {
                    &self.tables.wood_sound
                };
                if let Some(sound) = table.get(&field) {
                    return Some(Val::Ref("SoundType".to_string(), sound.clone()));
                }
            }
            return Some(Val::Opaque);
        }
        if simple == "BlockSetType" {
            if let Some(Val::Ref(_, field)) = receiver {
                if let Some(sound) = self.tables.block_set_sound.get(&field) {
                    return Some(Val::Ref("SoundType".to_string(), sound.clone()));
                }
            }
            return Some(Val::Opaque);
        }
        if simple == "WeatheringCopper$WeatherState" && name == "ordinal" {
            if let Some(Val::Ref(_, field)) = receiver {
                let ordinals = self.tables.enum_ordinals.get("WeatheringCopper$WeatherState");
                if let Some(ordinal) = ordinals.and_then(|map| map.get(&field)) {
                    return Some(Val::Int(*ordinal));
                }
            }
            return Some(Val::Opaque);
        }
        if simple == "WeatheringCopperCollection" {
            return Some(receiver.unwrap_or(Val::Opaque));
        }
        if simple == "WeatheringCopperCollection$ByState" && name == "pick" {
            let state = field_of(arguments.first()?).unwrap_or_default();
            if let Some(Val::Collection(collection)) = receiver {
                let id = copper_member_id(&collection, &state);
                return Some(Val::BlockId(id));
            }
            return Some(Val::Opaque);
        }
        if simple == "ColorCollection"
            || simple == "ColorCollection$ByColor"
            || simple == "DyeColor$1"
        {
            return Some(Val::Opaque);
        }
        if simple == "Block" && name == "defaultMapColor" {
            if let Some(Val::BlockField(field)) = receiver {
                return Some(Val::Ref("BaseMapColor".to_string(), field));
            }
            return Some(Val::Opaque);
        }
        if simple == "WeatheringCopper" && descriptor.contains("WeatherState") {
            return Some(Val::Opaque);
        }
        // A constructor: run the block class's own `<init>` so a `Properties` change in the
        // constructor chain is seen. Stop at the vanilla base classes, which do not touch it.
        if name == "<init>" {
            if simple == "Object"
                || simple == "Block"
                || simple == "BlockBehaviour"
                || simple == "StateHolder"
                || owner == PROPERTIES
            {
                return Some(Val::Opaque);
            }
            if !self.depth_limit() {
                return Some(Val::Opaque);
            }
            let Some(target) = self.class(owner) else {
                return Some(Val::Opaque);
            };
            let Some(method) = target.methods.iter().find(|method| method.name == "<init>") else {
                return Some(Val::Opaque);
            };
            let mut locals = Vec::with_capacity(arguments.len() + 1);
            locals.push(receiver.unwrap_or(Val::Opaque));
            locals.extend(arguments);
            self.depth.set(self.depth.get() + 1);
            let value = self.run_method(&target, method, locals);
            self.depth.set(self.depth.get() - 1);
            return Some(value.unwrap_or(Val::Opaque));
        }
        // A helper in `Blocks` (or any block class) with a parameterised chain.
        if self.depth.get() < 8 {
            if let Some(target) = self.class(owner) {
                if let Some(method) = target.methods.iter().find(|method| {
                    method.name == name && method.descriptor == descriptor
                }) {
                    if method.code.is_some() {
                        self.depth.set(self.depth.get() + 1);
                        let value = self.run_method(&target, method, arguments);
                        self.depth.set(self.depth.get() - 1);
                        return Some(value.unwrap_or(Val::Opaque));
                    }
                }
            }
        }
        let _ = class;
        Some(Val::Opaque)
    }

    fn depth_limit(&self) -> bool {
        self.depth.get() < 10
    }
}

fn ints(value: &Val) -> i64 {
    match value {
        Val::Int(int) => *int as i64,
        Val::Float(float) => *float as i64,
        _ => 0,
    }
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

/// `COPPER_BLOCK` -> `copper_block`, `CUT_COPPER_STAIRS` -> `cut_copper_stairs`.
fn collection_base(collection: &str) -> String {
    collection.to_lowercase()
}

fn weather_prefix(state: &str) -> &'static str {
    match state {
        "EXPOSED" => "exposed_",
        "WEATHERED" => "weathered_",
        "OXIDIZED" => "oxidized_",
        _ => "",
    }
}

/// The id of the copper member a `WeatheringCopperCollection.pick(state)` names.
fn copper_member_id(collection: &str, state: &str) -> String {
    let base = collection_base(collection);
    let prefix = weather_prefix(state);
    if base == "copper_block" {
        if state == "UNAFFECTED" {
            "minecraft:copper_block".to_string()
        } else {
            format!("minecraft:{prefix}copper")
        }
    } else {
        format!("minecraft:{prefix}{base}")
    }
}

// ---------------------------------------------------------------------------
// Static tables parsed from the jar
// ---------------------------------------------------------------------------

/// `MapColor.<FIELD>` -> `col`.
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
    let instructions = decode(&code.bytes);
    let mut numbers: Vec<i32> = Vec::new();
    for instruction in &instructions {
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

/// The `getstatic` field names of `owner` in one `<clinit>` statement.
fn statement_refs(
    class: &classfile::ClassFile,
    instructions: &[Instruction],
    owner_short: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    for instruction in instructions {
        if instruction.opcode != 0xb2 {
            continue;
        }
        let Some((owner, field, _)) = instruction.index.and_then(|index| member(class, index))
        else {
            continue;
        };
        if short(&owner) == owner_short {
            out.push(field);
        }
    }
    out
}

/// Splits a `<clinit>` into statements and returns `(field, body)` for each `putstatic`.
fn define_statements(
    class: &classfile::ClassFile,
) -> Vec<(String, Vec<Instruction>, Option<usize>)> {
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return Vec::new();
    };
    let instructions = decode(&code.bytes);
    let mut out = Vec::new();
    let mut start = 0usize;
    for (slot, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xb3 {
            continue;
        }
        let Some((owner, field, _)) = instruction.index.and_then(|index| member(class, index))
        else {
            start = slot + 1;
            continue;
        };
        if short(&owner) != short(&class.name) && short(&owner) != "MapColor" {
            start = slot + 1;
            continue;
        }
        out.push((field, instructions[start..slot].to_vec(), None));
        start = slot + 1;
    }
    out
}

/// `DyeColor.<FIELD>` -> first/second `MapColor` argument.
fn dye_colours(jar: &Jar) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut map = BTreeMap::new();
    let mut terracotta = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/item/DyeColor") else {
        return (map, terracotta);
    };
    for (field, body, _) in define_statements(&class) {
        let colours = statement_refs(&class, &body, "MapColor");
        if let Some(first) = colours.first() {
            map.insert(field.clone(), first.clone());
        }
        if let Some(second) = colours.get(1) {
            terracotta.insert(field, second.clone());
        }
    }
    (map, terracotta)
}

/// `WoodType.<FIELD>` -> `soundType`/`hangingSignSoundType` field.
fn wood_sounds(
    jar: &Jar,
    _block_set: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut sound = BTreeMap::new();
    let mut hanging = BTreeMap::new();
    let Some(class) = jar
        .class("net/minecraft/world/level/block/state/properties/WoodType")
    else {
        return (sound, hanging);
    };
    // The `(name, setType)` constructor is the wooden default: `WOOD` + `HANGING_SIGN`.
    let defaults = class
        .methods
        .iter()
        .find(|method| {
            method.name == "<init>"
                && method.descriptor
                    == "(Ljava/lang/String;Lnet/minecraft/world/level/block/state/properties/BlockSetType;)V"
        })
        .and_then(|method| method.code.as_ref())
        .map(|code| {
            let body = decode(&code.bytes);
            statement_refs(&class, &body, "SoundType")
        })
        .unwrap_or_default();
    for (field, body, _) in define_statements(&class) {
        let sounds = statement_refs(&class, &body, "SoundType");
        let plain = sounds
            .first()
            .cloned()
            .or_else(|| defaults.first().cloned());
        let hang = sounds
            .get(1)
            .cloned()
            .or_else(|| defaults.get(1).cloned())
            .or_else(|| plain.clone());
        if let Some(plain) = plain {
            sound.insert(field.clone(), plain);
        }
        if let Some(hang) = hang {
            hanging.insert(field, hang);
        }
    }
    (sound, hanging)
}

/// The first `SoundType` field a constructor body reads (the implicit default of a
/// short constructor overload, e.g. `BlockSetType(String) -> SoundType.WOOD`).
fn constructor_sound(class: &classfile::ClassFile, descriptor: &str) -> Option<String> {
    let method = class
        .methods
        .iter()
        .find(|method| method.name == "<init>" && method.descriptor == descriptor)?;
    let code = method.code.as_ref()?;
    let instructions = decode(&code.bytes);
    for instruction in &instructions {
        if instruction.opcode != 0xb2 {
            continue;
        }
        let Some((owner, field, _)) = instruction.index.and_then(|index| member(class, index))
        else {
            continue;
        };
        if short(&owner) == "SoundType" {
            return Some(field);
        }
    }
    None
}

/// `BlockSetType.<FIELD>` -> `soundType`.
fn block_set_sounds(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar
        .class("net/minecraft/world/level/block/state/properties/BlockSetType")
    else {
        return out;
    };
    let short_ctor = constructor_sound(&class, "(Ljava/lang/String;)V");
    for (field, body, _) in define_statements(&class) {
        let sound = statement_refs(&class, &body, "SoundType")
            .first()
            .cloned()
            .or_else(|| short_ctor.clone());
        if let Some(sound) = sound {
            out.insert(field, sound);
        }
    }
    out
}

/// The enum constants' ordinals, in declaration order.
fn enum_ordinals(jar: &Jar, name: &str) -> BTreeMap<String, i32> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class(name) else {
        return out;
    };
    let mut ordinal = 0;
    for field in &class.fields {
        // Enum constants are the `public static final` fields of the enum's own type.
        if field.descriptor == format!("L{};", class.name) {
            out.insert(field.name.clone(), ordinal);
            ordinal += 1;
        }
    }
    out
}

/// `SoundEvents.<FIELD>` -> `minecraft:<id>`.
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
    out
}

/// `SoundType.<FIELD>` -> the compact sound object.
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
                        let mut sound = serde_json::Map::new();
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
                            let mut event = serde_json::Map::new();
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

/// A block's own registry id(s): `BlockItemIds.<FIELD>` / `BlockIds.<FIELD>`.
fn read_ids(class: &classfile::ClassFile) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut plain = BTreeMap::new();
    let mut colour = BTreeMap::new();
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return (plain, colour);
    };
    let mut strings: Vec<String> = Vec::new();
    let mut mode = 0u8; // 0 = last string, 1 = first string, 2 = colour collection
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) =
                    instruction.index.and_then(|index| constant(&class, index))
                {
                    if let Some(text) = utf8_at(&class, reference) {
                        strings.push(text);
                    }
                }
            }
            0xb8 => {
                if let Some((owner, name, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemIds"
                        || short(&owner) == "BlockItemId"
                        || short(&owner) == "BlockIds"
                    {
                        mode = if name == "createSimpleColored" {
                            2
                        } else if name == "create" {
                            1
                        } else {
                            0
                        };
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemIds" || short(&owner) == "BlockIds" {
                        let text = match mode {
                            1 => strings.first().cloned(),
                            2 => strings.last().cloned(),
                            _ => strings.last().cloned(),
                        };
                        if let Some(text) = text {
                            if mode == 2 {
                                colour.insert(field, text);
                            } else {
                                plain.insert(field, text);
                            }
                        }
                        strings.clear();
                        mode = 0;
                    }
                }
            }
            _ => {}
        }
    }
    (plain, colour)
}

// ---------------------------------------------------------------------------
// Registration scan
// ---------------------------------------------------------------------------

fn build_tables(jar: &Jar) -> Tables {
    let events = sound_events(jar);
    let block_set_sound = block_set_sounds(jar);
    let (wood_sound, wood_hanging_sound) = wood_sounds(jar, &block_set_sound);
    let (dye_colours, dye_terracotta) = dye_colours(jar);
    let mut enum_ordinals = BTreeMap::new();
    enum_ordinals.insert(
        "WeatheringCopper$WeatherState".to_string(),
        self_ordinals(jar, "net/minecraft/world/level/block/WeatheringCopper$WeatherState"),
    );
    Tables {
        map_colours: map_colours(jar),
        dye_colours,
        dye_terracotta,
        sounds: sound_types(jar, &events),
        wood_sound,
        wood_hanging_sound,
        block_set_sound,
        enum_ordinals,
    }
}

fn self_ordinals(jar: &Jar, name: &str) -> BTreeMap<String, i32> {
    enum_ordinals(jar, name)
}

/// One registration found in `Blocks.<clinit>`.
struct Registration {
    props: Option<Props>,
    /// The host block of an `InfestedBlock` (its constructor asks for `host.hardness / 2`).
    host: Option<String>,
}

/// Runs every `<clinit>` statement and collects the registrations it makes.
fn scan_blocks(
    jar: &Jar,
    tables: &Rc<Tables>,
    ids: &BTreeMap<String, String>,
    colour_ids: &BTreeMap<String, String>,
) -> BTreeMap<String, Registration> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/level/block/Blocks") else {
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
    let instructions = decode(&code.bytes);
    let class = Rc::new(class);
    let vm = Vm::new(jar, tables.clone());
    let mut start = 0usize;
    for (slot, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xb3 {
            continue;
        }
        vm.captured.borrow_mut().clear();
        *vm.last_props.borrow_mut() = None;
        let _ = vm.execute(&class, &instructions[start..slot], Vec::new());
        start = slot + 1;
        let captured: Vec<Captured> = vm.captured.borrow_mut().drain(..).collect();
        for capture in captured {
            if let Some(family) = &capture.family {
                if capture.call_owner == "ColorCollection" {
                    let Some((_, field)) = capture
                        .ids
                        .iter()
                        .find(|(owner, _)| owner == "Collection")
                    else {
                        continue;
                    };
                    let Some(suffix) = colour_ids.get(field) else {
                        continue;
                    };
                    for colour in DYE_COLOURS {
                        let dye = Val::Ref("DyeColor".to_string(), colour.to_uppercase());
                        if let Some(props) = run_fn(&vm, family, vec![dye]) {
                            let id = format!("minecraft:{colour}_{suffix}");
                            out.entry(id).or_insert(Registration {
                                props: Some(props),
                                host: None,
                            });
                        }
                    }
                }
                if capture.call_owner == "WeatheringCopperCollection" {
                    let Some((_, field)) = capture
                        .ids
                        .iter()
                        .find(|(owner, _)| owner == "Collection")
                    else {
                        continue;
                    };
                    let base = collection_base(field);
                    for state in ["UNAFFECTED", "EXPOSED", "WEATHERED", "OXIDIZED"] {
                        let value = Val::Ref(
                            "WeatheringCopper$WeatherState".to_string(),
                            state.to_string(),
                        );
                        let Some(mut props) = run_fn(&vm, family, vec![value.clone()]) else {
                            continue;
                        };
                        // The weathering constructor may change the properties (a door or
                        // trapdoor takes its sound from its block set type there).
                        for function in &capture.functions {
                            if argument_count(&function.descriptor) == 2
                                && function.descriptor.contains("Properties")
                                && !return_type(&function.descriptor)
                                    .map(|returns| returns.contains("Properties"))
                                    .unwrap_or(false)
                            {
                                if let Some(after) = run_fn(
                                    &vm,
                                    function,
                                    vec![value.clone(), Val::Props(Box::new(props.clone()))],
                                ) {
                                    props = after;
                                }
                            }
                        }
                        let prefix = weather_prefix(state);
                        let stem = if base == "copper_block" {
                            if state == "UNAFFECTED" {
                                "copper_block".to_string()
                            } else {
                                format!("{prefix}copper")
                            }
                        } else {
                            format!("{prefix}{base}")
                        };
                        let id = format!("minecraft:{stem}");
                        out.entry(id.clone()).or_insert(Registration {
                            props: Some(props.clone()),
                            host: None,
                        });
                        let waxed = format!("minecraft:waxed_{stem}");
                        out.entry(waxed).or_insert(Registration {
                            props: Some(props),
                            host: None,
                        });
                    }
                }
                continue;
            }
            if capture.ids.is_empty() {
                continue;
            }
            let (owner, field) = capture.ids[0].clone();
            let id = match owner.as_str() {
                "BlockItemIds" | "BlockIds" => ids.get(&field).cloned(),
                "Blocks" => Some(field.to_lowercase()),
                "Collection" => Some(field.to_lowercase()),
                _ => None,
            };
            let Some(id) = id else {
                continue;
            };
            let id = format!("minecraft:{id}");
            let mut props = capture.props.clone();
            if let (Some(ctor), Some(base_props)) = (capture.ctor.clone(), props.clone()) {
                if let Some(mut after) = run_ctor(&vm, &ctor, base_props.clone()) {
                    if after.copy_of == base_props.copy_of
                        && after.hardness == base_props.hardness
                        && after.sound == base_props.sound
                    {
                        after = base_props;
                    }
                    props = Some(after);
                }
            }
            out.entry(id.clone()).or_insert(Registration {
                props,
                host: if id.starts_with("minecraft:infested_") { capture.ctor.as_ref().and_then(|ctor| constructor_host(jar, ctor)) } else { None },
            });
        }
    }
    let _ = colour_ids;
    out
}

/// Runs a registration lambda with its captured arguments and one extra value.
fn run_fn(vm: &Vm, function: &FnRef, extra: Vec<Val>) -> Option<Props> {
    let class = vm.class(&function.class)?;
    let method = class
        .methods
        .iter()
        .find(|method| method.name == function.method && method.descriptor == function.descriptor)?;
    let mut locals = function.captured.clone();
    locals.extend(extra);
    *vm.last_props.borrow_mut() = None;
    let _ = vm.run_method(&class, method, locals);
    vm.last_props.borrow().clone()
}

/// Runs a registration lambda with the properties value it was handed.
fn run_ctor(vm: &Vm, function: &FnRef, props: Props) -> Option<Props> {
    run_fn(vm, function, vec![Val::Props(Box::new(props))])
}

// ---------------------------------------------------------------------------
// Copy resolution
// ---------------------------------------------------------------------------

fn default_props() -> Props {
    Props {
        can_occlude: Some(true),
        hardness: Some(0.0),
        resistance: Some(0.0),
        friction: Some(0.6),
        speed: Some(1.0),
        jump: Some(1.0),
        requires_tool: Some(false),
        random_ticks: Some(false),
        replaceable: Some(false),
        ignited_by_lava: Some(false),
        liquid: Some(false),
        offset: Some(false),
        no_collision: false,
        no_occlusion: false,
        sound: None,
        map_color: None,
        push: None,
        copy_of: None,
    }
}

/// `delta` applied over `base`; the `copy` kind decides whether `jumpFactor` carries over.
fn merge_props(delta: &Props, base: &Props, full_copy: bool) -> Props {
    Props {
        can_occlude: delta.can_occlude.or(base.can_occlude).or(Some(true)),
        hardness: delta.hardness.or(base.hardness).or(Some(0.0)),
        resistance: delta.resistance.or(base.resistance).or(Some(0.0)),
        friction: delta.friction.or(base.friction).or(Some(0.6)),
        speed: delta.speed.or(base.speed).or(Some(1.0)),
        jump: if full_copy {
            delta.jump.or(base.jump).or(Some(1.0))
        } else {
            delta.jump.or(Some(1.0))
        },
        requires_tool: delta.requires_tool.or(base.requires_tool).or(Some(false)),
        random_ticks: delta.random_ticks.or(base.random_ticks).or(Some(false)),
        replaceable: delta.replaceable.or(base.replaceable).or(Some(false)),
        ignited_by_lava: delta
            .ignited_by_lava
            .or(base.ignited_by_lava)
            .or(Some(false)),
        liquid: delta.liquid.or(base.liquid).or(Some(false)),
        offset: delta.offset.or(base.offset).or(Some(false)),
        no_collision: delta.no_collision || base.no_collision,
        no_occlusion: delta.no_occlusion || base.no_occlusion,
        sound: delta.sound.clone().or_else(|| base.sound.clone()),
        map_color: delta
            .map_color
            .clone()
            .or_else(|| base.map_color.clone()),
        push: delta
            .push
            .clone()
            .or_else(|| base.push.clone())
            .or_else(|| Some("PUSH_PULL".to_string())),
        copy_of: None,
    }
}

fn copy_target(copy: &CopyOf, ids: &BTreeMap<String, String>) -> Option<String> {
    match copy {
        CopyOf::Id(id) => Some(id.clone()),
        CopyOf::Field(field) => ids
            .get(field)
            .map(|id| format!("minecraft:{id}"))
            .or_else(|| Some(format!("minecraft:{}", field.to_lowercase()))),
    }
}

fn flatten(
    raw: &BTreeMap<String, Props>,
    ids: &BTreeMap<String, String>,
    tables: &Tables,
) -> BTreeMap<String, Props> {
    let mut resolved: BTreeMap<String, Props> = BTreeMap::new();
    for (id, props) in raw {
        resolved.insert(id.clone(), merge_props(props, &default_props(), true));
    }
    for _ in 0..16 {
        let snapshot = resolved.clone();
        let mut changed = false;
        for (id, props) in raw {
            let mut out = if let Some((copy, full)) = &props.copy_of {
                let base = copy_target(copy, ids)
                    .filter(|target| target != id)
                    .and_then(|target| snapshot.get(&target).cloned())
                    .unwrap_or_else(default_props);
                merge_props(props, &base, *full)
            } else {
                merge_props(props, &default_props(), true)
            };
            out.copy_of = None;
            resolve_base_colours(&mut out, &snapshot, ids, tables);
            if resolved.get(id) != Some(&out) {
                resolved.insert(id.clone(), out);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    resolved
}

/// Replaces `Blocks.X.defaultMapColor()` placeholders with the map colour X's own
/// properties settled on.
fn resolve_base_colours(
    props: &mut Props,
    snapshot: &BTreeMap<String, Props>,
    ids: &BTreeMap<String, String>,
    tables: &Tables,
) {
    fn colour_of(
        field: &str,
        snapshot: &BTreeMap<String, Props>,
        ids: &BTreeMap<String, String>,
        tables: &Tables,
    ) -> Option<String> {
        let id = ids
            .get(field)
            .map(|id| format!("minecraft:{id}"))
            .unwrap_or_else(|| format!("minecraft:{}", field.to_lowercase()));
        match snapshot.get(&id).and_then(|props| props.map_color.clone()) {
            Some(MapColour::Constant(name)) => Some(name),
            Some(MapColour::Dye(dye, terracotta)) => {
                let table = if terracotta {
                    &tables.dye_terracotta
                } else {
                    &tables.dye_colours
                };
                table.get(&dye).cloned()
            }
            _ => None,
        }
    }
    if let Some(MapColour::Base(field)) = &props.map_color {
        if let Some(name) = colour_of(field, snapshot, ids, tables) {
            props.map_color = Some(MapColour::Constant(name));
        }
    }
    if let Some(MapColour::Function(function)) = &mut props.map_color {
        for value in function.captured.iter_mut() {
            if let Val::Ref(owner, field) = value {
                if short(owner) == "BaseMapColor" {
                    if let Some(name) = colour_of(field, snapshot, ids, tables) {
                        *value = Val::Ref("MapColor".to_string(), name);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-state map colour / sound
// ---------------------------------------------------------------------------

/// The value index of `name` in the state whose local index is `index`.
///
/// `StateDefinition` sorts its properties by name and varies the first one slowest, so the
/// id's mixed radix is over the name-sorted property list (not the NBT order the table
/// stores them in).
fn state_value(
    properties: &[states::Property],
    index: usize,
    name: &str,
) -> Option<usize> {
    let mut order: Vec<&states::Property> = properties.iter().collect();
    order.sort_by(|left, right| left.name.cmp(&right.name));
    let position = order
        .iter()
        .position(|property| property.name.eq_ignore_ascii_case(name))?;
    let mut stride = 1usize;
    for property in order.iter().skip(position + 1) {
        stride *= property.values.len().max(1);
    }
    let count = order[position].values.len().max(1);
    Some((index / stride) % count)
}

fn state_value_name(
    properties: &[states::Property],
    index: usize,
    name: &str,
) -> Option<String> {
    let at = state_value(properties, index, name)?;
    let mut order: Vec<&states::Property> = properties.iter().collect();
    order.sort_by(|left, right| left.name.cmp(&right.name));
    order
        .iter()
        .find(|property| property.name.eq_ignore_ascii_case(name))
        .and_then(|property| property.values.get(at).cloned())
}

/// `MapColor` field name -> its colour value.
fn map_colour_value(tables: &Tables, field: &str) -> Option<u32> {
    tables.map_colours.get(field).copied()
}

fn dye_colour_value(tables: &Tables, dye: &str, terracotta: bool) -> Option<u32> {
    let table = if terracotta {
        &tables.dye_terracotta
    } else {
        &tables.dye_colours
    };
    let field = table.get(dye)?;
    map_colour_value(tables, field)
}

/// Evaluates a `Function<BlockState, MapColor>` for one state.
fn evaluate_map_fn(
    tables: &Tables,
    function: &FnRef,
    properties: &[states::Property],
    state: usize,
    copper_colours: &BTreeMap<String, u32>,
) -> Option<u32> {
    match function.method.as_str() {
        "lambda$logProperties$0" => {
            let on_axis = fn_arg_colour(tables, function, 0)?;
            let off_axis = fn_arg_colour(tables, function, 1)?;
            let axis = state_value_name(properties, state, "axis").unwrap_or_default();
            Some(if axis == "y" { on_axis } else { off_axis })
        }
        "lambda$netherStemProperties$0" => fn_arg_colour(tables, function, 0),
        "lambda$static$45" => {
            let dye = match function.captured.first() {
                Some(Val::Ref(owner, field)) if short(owner) == "DyeColor" => field.clone(),
                _ => return None,
            };
            let part = state_value_name(properties, state, "part").unwrap_or_default();
            if part == "foot" {
                dye_colour_value(tables, &dye, false)
            } else {
                map_colour_value(tables, "WOOL")
            }
        }
        "lambda$static$80" => {
            let age = state_value_name(properties, state, "age")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            Some(if age >= 6 {
                map_colour_value(tables, "COLOR_YELLOW")?
            } else {
                map_colour_value(tables, "PLANT")?
            })
        }
        "lambda$waterloggedMapColor$0" => {
            let dry = fn_arg_colour(tables, function, 0)?;
            let wet = state_value_name(properties, state, "waterlogged").unwrap_or_default();
            Some(if wet == "true" {
                map_colour_value(tables, "WATER")?
            } else {
                dry
            })
        }
        other => {
            // The copper collections' map colour: `COPPER_BLOCK.pick(weatherState)`, whose
            // colour is the plain copper block's own map colour.
            let _ = other;
            match function.captured.first() {
                Some(Val::Ref(owner, state_name))
                    if short(owner) == "WeatheringCopper$WeatherState" =>
                {
                    copper_colours.get(state_name).copied()
                }
                _ => None,
            }
        }
    }
}

fn fn_arg_colour(tables: &Tables, function: &FnRef, at: usize) -> Option<u32> {
    match function.captured.get(at) {
        Some(Val::Ref(owner, field)) if short(owner) == "MapColor" => {
            map_colour_value(tables, field)
        }
        _ => None,
    }
}

/// The sound a block's state gets. Only `DecoratedPotBlock` varies it per state.
/// A colour family registered through `ColorCollection.zipMap` (stairs/slabs and the like):
/// the colour and the family suffix are in the block's own id, and the family's map colour is
/// the dye's map colour (terracotta uses its own shade, banners are wood, candles are wool
/// for white).
fn inferred_family_colour(id: &str, tables: &Tables) -> Option<MapColour> {
    let name = id.strip_prefix("minecraft:")?;
    let mut colours: Vec<&str> = DYE_COLOURS.to_vec();
    colours.sort_by_key(|colour| std::cmp::Reverse(colour.len()));
    let (colour, suffix) = colours.iter().find_map(|colour| {
        name.strip_prefix(&format!("{colour}_"))
            .map(|suffix| (*colour, suffix))
    })?;
    let dye = colour.to_uppercase();
    if sufix_is(suffix, "terracotta") {
        return Some(MapColour::Dye(dye, true));
    }
    if suffix == "banner" || suffix == "wall_banner" {
        return Some(MapColour::Constant("WOOD".to_string()));
    }
    if suffix == "bed" {
        return Some(MapColour::Bed(dye));
    }
    if suffix == "candle" {
        return Some(MapColour::Constant(if colour == "white" {
            "WOOL".to_string()
        } else {
            tables.dye_colours.get(&dye)?.clone()
        }));
    }
    if suffix == "candle_cake" {
        return Some(MapColour::Constant("NONE".to_string()));
    }
    if suffix.contains("pane") {
        return Some(MapColour::Constant("NONE".to_string()));
    }
    Some(MapColour::Dye(dye, false))
}

fn sufix_is(suffix: &str, family: &str) -> bool {
    suffix == family || suffix == &format!("{family}_stairs") || suffix == &format!("{family}_slab")
}

/// Fills a fallback row's `map_color` from a colour-family rule (used for the `zipMap`
/// families, whose properties function this module does not run).
fn apply_family_colour(
    tables: &Tables,
    properties: &[states::Property],
    state_count: usize,
    spec: &MapColour,
    index: usize,
    row: &mut BTreeMap<&'static str, Value>,
    map_colors: &mut BTreeMap<usize, Vec<Value>>,
) {
    match spec {
        MapColour::Constant(name) => {
            row.insert(
                "map_color",
                Value::from(map_colour_value(tables, name).unwrap_or(0) as i64),
            );
        }
        MapColour::Dye(dye, terracotta) => {
            row.insert(
                "map_color",
                Value::from(dye_colour_value(tables, dye, *terracotta).unwrap_or(0) as i64),
            );
        }
        MapColour::Bed(dye) => {
            let foot = dye_colour_value(tables, dye, false).unwrap_or(0);
            let head = map_colour_value(tables, "WOOL").unwrap_or(0);
            let mut values = Vec::with_capacity(state_count);
            for state in 0..state_count {
                let part = state_value_name(properties, state, "part").unwrap_or_default();
                values.push(Value::from(if part == "foot" {
                    foot as i64
                } else {
                    head as i64
                }));
            }
            if let Some(first) = values.first() {
                row.insert("map_color", first.clone());
            }
            map_colors.insert(index, values);
        }
        MapColour::Function(_) | MapColour::Base(_) => {}
    }
}

/// The `Blocks.<FIELD>` block a constructor references (the infested blocks' host).
fn constructor_host(jar: &Jar, function: &FnRef) -> Option<String> {
    let class = jar.class("net/minecraft/world/level/block/Blocks")?;
    let method = class
        .methods
        .iter()
        .find(|method| method.name == function.method && method.descriptor == function.descriptor)?;
    let code = method.code.as_ref()?;
    for instruction in decode(&code.bytes) {
        if instruction.opcode != 0xb2 {
            continue;
        }
        let Some((owner, field, descriptor)) =
            instruction.index.and_then(|index| member(&class, index))
        else {
            continue;
        };
        if short(&owner) == "Blocks" && descriptor.ends_with("/Block;") {
            return Some(field);
        }
    }
    None
}

/// The sound a block's state gets. Only `DecoratedPotBlock` varies it per state.
fn state_sound(
    tables: &Tables,
    block: &str,
    properties: &[states::Property],
    state: usize,
    base: &Value,
) -> Value {
    if block == "minecraft:decorated_pot" {
        let cracked = state_value_name(properties, state, "cracked")
            .map(|value| value == "true")
            .unwrap_or(false);
        let name = if cracked {
            "DECORATED_POT_CRACKED"
        } else {
            "DECORATED_POT"
        };
        if let Some(sound) = tables.sounds.get(name) {
            return sound.clone();
        }
    }
    base.clone()
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// Derives every block's row from the jar.
pub fn rows(jar_path: &Path) -> Result<Rows, String> {
    let jar = Jar::open(jar_path)?;
    let tables = Rc::new(build_tables(&jar));
    let item_ids_class = jar
        .class("net/minecraft/references/BlockItemIds")
        .ok_or("no BlockItemIds")?;
    let (mut ids, mut colour_ids) = read_ids(&item_ids_class);
    if let Some(block_ids_class) = jar.class("net/minecraft/references/BlockIds") {
        let (resource_ids, resource_colours) = read_ids(&block_ids_class);
        for (field, id) in resource_ids {
            ids.entry(field).or_insert(id);
        }
        for (field, id) in resource_colours {
            colour_ids.entry(field).or_insert(id);
        }
    }
    let registrations = scan_blocks(&jar, &tables, &ids, &colour_ids);
    let raw: BTreeMap<String, Props> = registrations
        .iter()
        .filter_map(|(id, registration)| {
            registration
                .props
                .clone()
                .map(|props| (id.clone(), props))
        })
        .collect();
    let resolved = flatten(&raw, &ids, &tables);

    let base_rows = base::rows(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    let registry = block::registry(jar_path)?;
    let state_index: BTreeMap<&str, usize> = state_table
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.id.as_str(), index))
        .collect();
    let mut class_by_id: BTreeMap<&str, &str> = BTreeMap::new();
    for block in &registry {
        class_by_id.insert(block.id.as_str(), block.class.as_str());
    }
    let mut unresolved = Vec::new();
    let mut map_color_functions = Vec::new();
    let mut map_colors = BTreeMap::new();
    let mut sounds = BTreeMap::new();
    let mut copper_colours: BTreeMap<String, u32> = BTreeMap::new();
    let mut default_colours: BTreeMap<String, u32> = BTreeMap::new();
    let default_index = |index: usize| {
        state_table
            .blocks
            .get(index)
            .map(|block| block.default_index)
            .unwrap_or(0)
    };
    for (state_name, id) in [
        ("UNAFFECTED", "minecraft:copper_block"),
        ("EXPOSED", "minecraft:exposed_copper"),
        ("WEATHERED", "minecraft:weathered_copper"),
        ("OXIDIZED", "minecraft:oxidized_copper"),
    ] {
        if let Some(MapColour::Constant(name)) =
            resolved.get(id).and_then(|props| props.map_color.clone())
        {
            if let Some(value) = map_colour_value(&tables, &name) {
                copper_colours.insert(state_name.to_string(), value);
            }
        }
    }
    let mut result_rows = Vec::with_capacity(base_rows.blocks.len());
    for (index, block) in base_rows.blocks.iter().enumerate() {
        let fallback = &base_rows.rows[index];
        let states_for_block = state_index
            .get(block.as_str())
            .and_then(|index| state_table.blocks.get(*index));
        let (state_count, properties) = match states_for_block {
            Some(block_states) => (
                block_states.state_count,
                block_states.properties.clone(),
            ),
            None => (1usize, Vec::new()),
        };
        let Some(props) = resolved.get(block) else {
            let mut row = fallback.clone();
            if let Some(spec) = inferred_family_colour(block, &tables) {
                apply_family_colour(
                    &tables,
                    &properties,
                    state_count,
                    &spec,
                    index,
                    &mut row,
                    &mut map_colors,
                );
            }
            if let Some(value) = row.get("map_color").and_then(Value::as_u64) {
                default_colours.insert(block.clone(), value as u32);
            }
            result_rows.push(row);
            if gameplay::FIELDS.iter().any(|field| {
                field != &"full_collision"
                    && fallback.get(*field).map_or(true, Value::is_null)
            }) {
                unresolved.push(block.clone());
            }
            continue;
        };
        let mut row: BTreeMap<&'static str, Value> = BTreeMap::new();
        let block_entity = match class_by_id.get(block.as_str()) {
            Some(class) => implements(&jar, class, "net/minecraft/world/level/block/EntityBlock"),
            None => false,
        };
        row.insert("block_entity", Value::Bool(block_entity));
        row.insert(
            "can_occlude",
            Value::Bool(props.can_occlude.unwrap_or(true)),
        );
        row.insert(
            "explosion_resistance",
            float_value(props.resistance.unwrap_or(0.0)),
        );
        row.insert("friction", float_value(props.friction.unwrap_or(0.6)));
        row.insert("hardness", float_value(props.hardness.unwrap_or(0.0)));
        row.insert(
            "ignited_by_lava",
            Value::Bool(props.ignited_by_lava.unwrap_or(false)),
        );
        row.insert("jump_factor", float_value(props.jump.unwrap_or(1.0)));
        row.insert("liquid", Value::Bool(props.liquid.unwrap_or(false)));
        row.insert("offset", Value::Bool(props.offset.unwrap_or(false)));
        row.insert(
            "push",
            Value::String(
                props
                    .push
                    .clone()
                    .unwrap_or_else(|| "PUSH_PULL".to_string()),
            ),
        );
        row.insert(
            "replaceable",
            Value::Bool(props.replaceable.unwrap_or(false)),
        );
        row.insert(
            "requires_tool",
            Value::Bool(props.requires_tool.unwrap_or(false)),
        );
        row.insert("speed_factor", float_value(props.speed.unwrap_or(1.0)));
        let sound = props
            .sound
            .as_ref()
            .and_then(|name| tables.sounds.get(name))
            .cloned()
            .unwrap_or_else(|| {
                tables
                    .sounds
                    .get("STONE")
                    .cloned()
                    .unwrap_or(Value::Null)
            });
        row.insert("sound", sound.clone());
        // `map_color`: constant, dye, bed, or a per-state function.
        let inferred;
        let spec = match &props.map_color {
            Some(spec) => Some(spec),
            None => {
                inferred = inferred_family_colour(block, &tables);
                inferred.as_ref()
            }
        };
        let mut constant_colour = 0u32;
        let mut function_values: Option<Vec<Value>> = None;
        match spec {
            Some(MapColour::Constant(name)) => {
                constant_colour = map_colour_value(&tables, name).unwrap_or(0);
            }
            Some(MapColour::Dye(dye, terracotta)) => {
                constant_colour = dye_colour_value(&tables, dye, *terracotta).unwrap_or(0);
            }
            Some(MapColour::Bed(dye)) => {
                let mut values = Vec::with_capacity(state_count);
                let foot = dye_colour_value(&tables, dye, false).unwrap_or(0);
                let head = map_colour_value(&tables, "WOOL").unwrap_or(0);
                for state in 0..state_count {
                    let part = state_value_name(&properties, state, "part").unwrap_or_default();
                    values.push(Value::from(if part == "foot" {
                        foot as i64
                    } else {
                        head as i64
                    }));
                }
                function_values = Some(values);
            }
            Some(MapColour::Base(field)) => {
                let target = ids
                    .get(field)
                    .map(|id| format!("minecraft:{id}"))
                    .unwrap_or_else(|| format!("minecraft:{}", field.to_lowercase()));
                constant_colour = default_colours
                    .get(&target)
                    .copied()
                    .or_else(|| {
                        let at = base_rows
                            .blocks
                            .iter()
                            .position(|name| *name == target)?;
                        base_rows.rows[at]
                            .get("map_color")
                            .and_then(Value::as_u64)
                            .map(|value| value as u32)
                    })
                    .unwrap_or(0);
            }
            Some(MapColour::Function(function)) => {
                let mut values = Vec::with_capacity(state_count);
                let mut exact = true;
                for state in 0..state_count {
                    match evaluate_map_fn(&tables, function, &properties, state, &copper_colours) {
                        Some(colour) => values.push(Value::from(colour as i64)),
                        None => {
                            exact = false;
                            break;
                        }
                    }
                }
                let distinct: BTreeSet<String> =
                    values.iter().map(|value| value.to_string()).collect();
                if exact && distinct.len() <= 1 {
                    if let Some(value) = values.first() {
                        constant_colour = value.as_u64().unwrap_or(0) as u32;
                    }
                } else if exact {
                    function_values = Some(values);
                } else {
                    map_color_functions.push(block.clone());
                    constant_colour = fallback
                        .get("map_color")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;
                }
            }
            None => {}
        }
        row.insert("map_color", Value::from(constant_colour as i64));
        let default_value = function_values
            .as_ref()
            .and_then(|values| values.get(default_index(index)))
            .and_then(Value::as_u64)
            .unwrap_or(constant_colour as u64);
        default_colours.insert(block.clone(), default_value as u32);
        // Per-state overrides.
        if let Some(values) = function_values {
            map_colors.insert(index, values);
        }
        if block == "minecraft:decorated_pot" {
            let mut values = Vec::with_capacity(state_count);
            for state in 0..state_count {
                values.push(state_sound(&tables, block, &properties, state, &sound));
            }
            if let Some(first) = values.first() {
                row.insert("sound", first.clone());
            }
            sounds.insert(index, values);
        }
        if let Some(host) = registrations.get(block).and_then(|entry| entry.host.as_ref()) {
            let host_id = format!("minecraft:{}", host.to_lowercase());
            let host_hardness = resolved
                .get(&host_id)
                .and_then(|props| props.hardness)
                .or_else(|| {
                    let at = base_rows.blocks.iter().position(|name| *name == host_id)?;
                    base_rows.rows[at]
                        .get("hardness")
                        .and_then(Value::as_f64)
                        .map(|value| value as f32)
                })
                .unwrap_or(0.0);
            row.insert("hardness", float_value(host_hardness / 2.0));
            row.insert("explosion_resistance", float_value(0.75));
        }
        if gameplay::FIELDS.iter().any(|field| {
            field != &"full_collision"
                && row.get(*field).map_or(true, Value::is_null)
        }) {
            unresolved.push(block.clone());
        }
        result_rows.push(row);    }
    Ok(Rows {
        blocks: base_rows.blocks,
        rows: result_rows,
        map_colors,
        sounds,
        unresolved,
        map_color_functions,
    })
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

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

/// The compact document as JSON.
pub fn document(jar_path: &Path) -> Result<Value, String> {
    let rows = rows(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    let shapes = shape_final::evaluate(jar_path)?;
    if shapes.full_block.len() != state_table.state_count() {
        return Err("the shapes do not cover every state".into());
    }
    let fields: Vec<String> = gameplay::FIELDS.iter().map(|field| field.to_string()).collect();
    let mut values: Vec<Vec<Value>> = Vec::with_capacity(shapes.full_block.len());
    let mut blocks_per_state: Vec<usize> = Vec::with_capacity(shapes.full_block.len());
    let mut state = 0usize;
    for (block_index, block_states) in state_table.blocks.iter().enumerate() {
        for local in 0..block_states.state_count {
            let row = &rows.rows[block_index];
            let mut value = Vec::with_capacity(fields.len());
            for field in &fields {
                if field == "full_collision" {
                    value.push(Value::Bool(shapes.full_block[state].unwrap_or(false)));
                } else if field == "map_color" {
                    value.push(
                        rows.map_colors
                            .get(&block_index)
                            .and_then(|colors| colors.get(local))
                            .cloned()
                            .or_else(|| row.get("map_color").cloned())
                            .unwrap_or(Value::Null),
                    );
                } else if field == "sound" {
                    value.push(
                        rows.sounds
                            .get(&block_index)
                            .and_then(|sounds| sounds.get(local))
                            .cloned()
                            .or_else(|| row.get("sound").cloned())
                            .unwrap_or(Value::Null),
                    );
                } else {
                    value.push(row.get(field.as_str()).cloned().unwrap_or(Value::Null));
                }
            }
            values.push(value);
            blocks_per_state.push(block_index);
            state += 1;
        }
    }
    let facts = gameplay::Facts {
        fields,
        blocks: rows.blocks.clone(),
        values,
        blocks_per_state,
        uncovered: rows
            .unresolved
            .iter()
            .map(|block| (block.clone(), "no property chain reaches it".to_string()))
            .collect(),
    };
    let text = gameplay::render(&facts);
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Builds the table and writes it below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let rows = rows(jar_path)?;
    let document = document(jar_path)?;
    let text = serde_json::to_string(&document).map_err(|error| error.to_string())?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut coverage = Coverage::default();
    coverage.produced.push(TABLE.to_string());
    coverage.unresolved = rows.unresolved.clone();
    coverage.map_color_functions = rows.map_color_functions.clone();
    coverage
        .missing
        .extend(MISSING.iter().map(|field| format!("{TABLE}#{field}")));
    if !coverage.unresolved.is_empty() {
        coverage.notes.push(format!(
            "{} of {} blocks still have no row",
            coverage.unresolved.len(),
            rows.blocks.len()
        ));
    }
    Ok(coverage)
}









