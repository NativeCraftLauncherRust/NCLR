//! Rebuild `creative-inventory.json` from the player's own `client.jar`.
//!
//! Vanilla's creative inventory is not a data pack: `CreativeModeTabs.bootstrap`
//! registers the tabs and `CreativeModeTabs.tryRebuildTabContents` fills each
//! tab by running `Output.accept` chains and generator helpers. This module
//! reads the class files directly and evaluates the parts of that code that are
//! self-contained:
//!
//! * tab metadata — id, type, row, column, translated title, icon stack,
//!   visibility (from `CreativeModeTab.shouldDisplay` semantics);
//! * item stacks for tabs whose generators are simple `Items.<field>` accepts,
//!   expanded through `BlockItemIds`/`ItemIds` reference classes, plus the
//!   `ColorCollection` / `WeatheringCopperCollection` helpers.
//!
//! Anything not modelled is reported as a gap for that tab — no item list is
//! guessed. In particular this cannot reproduce:
//! * item stacks built with data components (paintings, potions, enchanted
//!   books, fireworks, instruments, suspicious stews, light/test blocks);
//! * the search tab's tooltip lines and item tags;
//! * tabs whose generators call into registries or code that needs a live game.
//!
//! Values come from `mc263/client.jar` only; nothing is read from `ship.hzr3`.

#![allow(dead_code)]
// `base` (creative_data) and this module each include the shared class-file
// reader by path; that is required because Cargo.toml must not be edited.
#![allow(clippy::duplicate_mod)]

// The seven tabs `creative_data` already reproduces exactly are taken from it
// unchanged; this module only recomputes the tabs it left as gaps.
#[path = "creative_data.rs"]
mod base;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant, Member};

pub const TABLE: &str = "creative-inventory.json";

const CREATIVE_TABS: &str = "net/minecraft/world/item/CreativeModeTabs";
const CREATIVE_TAB: &str = "net/minecraft/world/item/CreativeModeTab";
const CREATIVE_TAB_BUILDER: &str = "net/minecraft/world/item/CreativeModeTab$Builder";
const ITEMS: &str = "net/minecraft/world/item/Items";
const BLOCK_ITEM_IDS: &str = "net/minecraft/references/BlockItemIds";
const ITEM_IDS: &str = "net/minecraft/references/ItemIds";
const ENTITY_TYPE_IDS: &str = "net/minecraft/world/entity/EntityTypeIds";
const BLOCK_IDS: &str = "net/minecraft/references/BlockIds";

const DYE_COLORS: [&str; 16] = [
    "white",
    "light_gray",
    "gray",
    "black",
    "brown",
    "red",
    "orange",
    "yellow",
    "lime",
    "green",
    "cyan",
    "light_blue",
    "blue",
    "purple",
    "magenta",
    "pink",
];

const WEATHERING_STATES: [&str; 4] = ["", "exposed_", "weathered_", "oxidized_"];
const WAXED_STATES: [&str; 4] = [
    "waxed_",
    "waxed_exposed_",
    "waxed_weathered_",
    "waxed_oxidized_",
];

// ---------------------------------------------------------------------------
// jar + class cache
// ---------------------------------------------------------------------------

struct Jar {
    archive: zip::ZipArchive<File>,
}

impl Jar {
    fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { archive })
    }

    fn read(&mut self, name: &str) -> Result<Vec<u8>, String> {
        let mut entry = self
            .archive
            .by_name(name)
            .map_err(|e| format!("{name}: {e}"))?;
        let mut data = Vec::with_capacity(entry.size() as usize + 16);
        entry
            .read_to_end(&mut data)
            .map_err(|e| format!("{name}: {e}"))?;
        Ok(data)
    }

    fn names(&mut self) -> Vec<String> {
        self.archive
            .file_names()
            .filter_map(|name| name.ok().map(|name| name.into_owned()))
            .collect()
    }

    fn read_json(&mut self, name: &str) -> Result<Value, String> {
        let raw = self.read(name)?;
        serde_json::from_slice(&raw).map_err(|e| format!("{name}: {e}"))
    }
}

struct Ctx {
    jar: Jar,
    classes: HashMap<String, Rc<ClassFile>>,
    lang: Option<HashMap<String, String>>,
}

impl Ctx {
    fn new(jar: Jar) -> Self {
        Self {
            jar,
            classes: HashMap::new(),
            lang: None,
        }
    }

    fn class(&mut self, internal_name: &str) -> Result<Rc<ClassFile>, String> {
        if let Some(class) = self.classes.get(internal_name) {
            return Ok(class.clone());
        }
        let data = self
            .jar
            .read(&format!("{internal_name}.class"))
            .map_err(|e| format!("class {internal_name}: {e}"))?;
        let class = Rc::new(classfile::parse(&data).map_err(|e| format!("{internal_name}: {e}"))?);
        self.classes.insert(internal_name.to_owned(), class.clone());
        Ok(class)
    }

    fn lang(&mut self) -> Result<&HashMap<String, String>, String> {
        if self.lang.is_none() {
            let raw = self.jar.read("assets/minecraft/lang/en_us.json")?;
            let value: Value =
                serde_json::from_slice(&raw).map_err(|e| format!("en_us.json: {e}"))?;
            let mut map = HashMap::new();
            if let Value::Object(object) = value {
                for (key, value) in object {
                    if let Value::String(text) = value {
                        map.insert(key, text);
                    }
                }
            }
            self.lang = Some(map);
        }
        Ok(self.lang.as_ref().expect("lang set"))
    }
}

// ---------------------------------------------------------------------------
// bytecode decoder (only what these initialisers use)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Ins {
    Nop,
    AconstNull,
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Ldc(u16),
    Ldc2(u16),
    Bipush(i32),
    Sipush(i32),
    Aload(u8),
    Astore(u8),
    Pop,
    Pop2,
    Dup,
    Aastore,
    Bastore,
    Getstatic(u16),
    Putstatic(u16),
    Invokevirtual(u16),
    Invokespecial(u16),
    Invokestatic(u16),
    Invokeinterface(u16),
    Invokedynamic(u16),
    New(u16),
    NewArray,
    Anewarray(u16),
    Arraylength,
    Checkcast(u16),
    Instanceof(u16),
    Ifeq(u16),
    Ifne(u16),
    Goto(u16),
    Areturn,
    Return,
}

fn decode(code: &[u8], pc: &mut usize) -> Result<Ins, String> {
    let start = *pc;
    if start >= code.len() {
        return Err("pc past code end".into());
    }
    let op = code[*pc];
    *pc += 1;
    let u1 = |pc: &mut usize| -> Result<u8, String> {
        if *pc >= code.len() {
            return Err(format!("truncated opcode at {start}"));
        }
        let v = code[*pc];
        *pc += 1;
        Ok(v)
    };
    let u2 = |pc: &mut usize| -> Result<u16, String> {
        if *pc + 2 > code.len() {
            return Err(format!("truncated opcode at {start}"));
        }
        let v = u16::from_be_bytes([code[*pc], code[*pc + 1]]);
        *pc += 2;
        Ok(v)
    };
    let ins = match op {
        0x00 => Ins::Nop,
        0x01 => Ins::AconstNull,
        0x02..=0x08 => Ins::Int(op as i32 - 3),
        0x09 => Ins::Long(0),
        0x0a => Ins::Long(1),
        0x0b => Ins::Float(0.0),
        0x0c => Ins::Float(1.0),
        0x0d => Ins::Float(2.0),
        0x0e => Ins::Double(0.0),
        0x0f => Ins::Double(1.0),
        0x10 => Ins::Bipush(u1(pc)? as i8 as i32),
        0x11 => Ins::Sipush(u2(pc)? as i16 as i32),
        0x12 => Ins::Ldc(u1(pc)? as u16),
        0x13 => Ins::Ldc(u2(pc)?),
        0x14 => Ins::Ldc2(u2(pc)?),
        0x19 => Ins::Aload(u1(pc)?),
        0x2a => Ins::Aload(0),
        0x2b => Ins::Aload(1),
        0x2c => Ins::Aload(2),
        0x2d => Ins::Aload(3),
        0x3a => Ins::Astore(u1(pc)?),
        0x4b => Ins::Astore(0),
        0x4c => Ins::Astore(1),
        0x4d => Ins::Astore(2),
        0x4e => Ins::Astore(3),
        0x53 => Ins::Aastore,
        0x54 => Ins::Bastore,
        0x57 => Ins::Pop,
        0x58 => Ins::Pop2,
        0x59 => Ins::Dup,
        0xa7 => Ins::Goto(u2(pc)?),
        0xb0 => Ins::Areturn,
        0xb1 => Ins::Return,
        0xb2 => Ins::Getstatic(u2(pc)?),
        0xb3 => Ins::Putstatic(u2(pc)?),
        0xb6 => Ins::Invokevirtual(u2(pc)?),
        0xb7 => Ins::Invokespecial(u2(pc)?),
        0xb8 => Ins::Invokestatic(u2(pc)?),
        0xb9 => {
            let index = u2(pc)?;
            u1(pc)?;
            u1(pc)?;
            Ins::Invokeinterface(index)
        }
        0xba => {
            let index = u2(pc)?;
            u2(pc)?;
            Ins::Invokedynamic(index)
        }
        0xbb => Ins::New(u2(pc)?),
        0xbc => {
            u1(pc)?;
            Ins::NewArray
        }
        0xbd => Ins::Anewarray(u2(pc)?),
        0xbe => Ins::Arraylength,
        0x99 => Ins::Ifeq(u2(pc)?),
        0x9a => Ins::Ifne(u2(pc)?),
        0xc0 => Ins::Checkcast(u2(pc)?),
        0xc1 => Ins::Instanceof(u2(pc)?),
        other => {
            return Err(format!(
                "unsupported opcode {other:#04x} at pc {start} in {start}.."
            ))
        }
    };
    Ok(ins)
}

// ---------------------------------------------------------------------------
// small helpers over the shared class-file reader
// ---------------------------------------------------------------------------

fn method(code: &ClassFile, name: &str) -> Result<Member, String> {
    code.method(name)
        .cloned()
        .ok_or_else(|| format!("{}: no method {name}", code.name))
}

fn method_code(code: &ClassFile, name: &str) -> Result<Vec<u8>, String> {
    method(code, name)?
        .code
        .map(|code| code.bytes)
        .ok_or_else(|| format!("{}.{name}: no Code", code.name))
}

fn method_ref(code: &ClassFile, index: u16) -> Result<(String, String, String), String> {
    classfile::method_ref(&code.constants, index as usize)
        .ok_or_else(|| format!("{}: bad method ref #{index}", code.name))
}

fn field_ref(code: &ClassFile, index: u16) -> Result<(String, String, String), String> {
    classfile::field_ref(&code.constants, index as usize)
        .ok_or_else(|| format!("{}: bad field ref #{index}", code.name))
}

fn ldc_string(code: &ClassFile, index: u16) -> Option<String> {
    classfile::string_constant(&code.constants, index as usize)
}

fn class_name(code: &ClassFile, index: u16) -> Result<String, String> {
    match code.constants.get(index as usize) {
        Some(Constant::Class(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            Ok(classfile::utf8(&code.constants, inner))
        }
        other => Err(format!(
            "{}: constant #{index} is not Class: {other:?}",
            code.name
        )),
    }
}

fn utf8_constant(code: &ClassFile, index: usize) -> String {
    classfile::utf8(&code.constants, index)
}

fn param_kinds(descriptor: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut chars = descriptor.chars().peekable();
    if chars.next() != Some('(') {
        return out;
    }
    while let Some(c) = chars.next() {
        match c {
            ')' => break,
            '[' => {
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == 'L' {
                        for inner in chars.by_ref() {
                            if inner == ';' {
                                break;
                            }
                        }
                        break;
                    }
                    if next != '[' {
                        break;
                    }
                }
                out.push('A');
            }
            'L' => {
                for inner in chars.by_ref() {
                    if inner == ';' {
                        break;
                    }
                }
                out.push('A');
            }
            'D' => out.push('D'),
            'J' => out.push('J'),
            'F' => out.push('F'),
            c => out.push(
                if c == 'Z' || c == 'B' || c == 'C' || c == 'S' || c == 'I' {
                    'I'
                } else {
                    panic!("bad descriptor {descriptor}")
                },
            ),
        }
    }
    out
}

fn return_descriptor(descriptor: &str) -> &str {
    descriptor.split(')').nth(1).unwrap_or("V")
}

fn pop_n(stack: &mut Vec<Val>, count: usize) -> Result<Vec<Val>, String> {
    if stack.len() < count {
        return Err(format!(
            "stack underflow: need {count}, have {}",
            stack.len()
        ));
    }
    Ok(stack.split_off(stack.len() - count))
}

// ---------------------------------------------------------------------------
// values
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Val {
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Str(String),
    ClassRef(String),
    Static {
        owner: String,
        name: String,
        desc: String,
    },
    Uninit(String),
    Array(Vec<Val>),
    List(Vec<Val>),
    Key(String),
    Row(String),
    TabType(String),
    Component(String),
    Builder(Box<Builder>),
    Tab(Box<TabDef>),
    Lambda {
        method: String,
        captured: usize,
    },
    Items(Vec<String>),
    BlockId(String),
    Icon {
        id: String,
        count: i32,
    },
    Visibility,
    Output,
    Consumer {
        copper: Option<String>,
    },
    ByState(Vec<String>),
    Registry,
    Null,
    Unknown,
}

impl Val {
    fn as_int(&self) -> Result<i32, String> {
        match self {
            Val::Int(value) => Ok(*value),
            other => Err(format!("expected int, got {other:?}")),
        }
    }
    fn as_str(&self) -> Result<&str, String> {
        match self {
            Val::Str(value) => Ok(value),
            other => Err(format!("expected string, got {other:?}")),
        }
    }
}

#[derive(Clone, Debug)]
struct Builder {
    row: String,
    column: i32,
    tab_type: String,
    title_key: Option<String>,
    icon_method: Option<String>,
    display_method: Option<String>,
    hide_title: bool,
    aligned_right: bool,
    no_scroll: bool,
}

#[derive(Clone, Debug)]
struct TabDef {
    id: String,
    type_name: String,
    row: String,
    column: i32,
    title_key: String,
    icon_method: String,
    display_method: Option<String>,
}

// ---------------------------------------------------------------------------
// reference-class id tables (BlockItemIds / ItemIds / BlockIds / EntityTypeIds)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum RefVal {
    Simple(String),
    Color(Vec<String>),
    Copper(Vec<String>),
    Special(Vec<String>),
    Other,
}

fn color_ids(base: &str) -> Vec<String> {
    DYE_ENUM
        .iter()
        .map(|color| format!("{color}_{base}"))
        .collect()
}

fn copper_ids(base: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(8);
    for state in WEATHERING_STATES {
        out.push(if state.is_empty() {
            base.to_owned()
        } else {
            format!("{state}{base}")
        });
    }
    for state in WAXED_STATES {
        out.push(format!("{state}{base}"));
    }
    out
}

fn special_ids(names: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(8);
    for (index, state) in WEATHERING_STATES.iter().enumerate() {
        let name = names.get(index).map(String::as_str).unwrap_or("");
        out.push(if state.is_empty() {
            name.to_owned()
        } else {
            format!("{state}{name}")
        });
    }
    for (index, state) in WAXED_STATES.iter().enumerate() {
        let name = names.get(index).map(String::as_str).unwrap_or("");
        out.push(format!("{state}{name}"));
    }
    out
}

struct RefTable {
    ids: HashMap<String, Vec<String>>,
    special: HashMap<String, Vec<String>>,
}

fn reference_table(
    ctx: &mut Ctx,
    class_name: &str,
    entity_ids: Option<&HashMap<String, Vec<String>>>,
    extern_refs: &HashMap<String, HashMap<String, String>>,
) -> Result<RefTable, String> {
    let class = ctx.class(class_name)?;
    let code = method_code(&class, "<clinit>")?;
    let mut ids = HashMap::new();
    let mut special: HashMap<String, Vec<String>> = HashMap::new();
    let mut stack: Vec<RefVal> = Vec::new();
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Nop => {}
            Ins::Int(_) => stack.push(RefVal::Other),
            Ins::Ldc(index) => {
                if let Some(text) = ldc_string(&class, index) {
                    stack.push(RefVal::Simple(text));
                } else {
                    stack.push(RefVal::Other);
                }
            }
            Ins::Ldc2(_) => stack.push(RefVal::Other),
            Ins::New(_) => stack.push(RefVal::Other),
            Ins::NewArray => {
                stack.pop();
                stack.push(RefVal::Other);
            }
            Ins::Dup => {
                let value = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| format!("{class_name}: dup on empty stack"))?;
                stack.push(value);
            }
            Ins::Astore(_) => {}
            Ins::Aload(_) => stack.push(RefVal::Other),
            Ins::Pop => {
                stack.pop();
            }
            Ins::Invokespecial(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                let mut values = pop_n_ref(&mut stack, count)?;
                stack.pop(); // receiver
                if name == "<init>" && owner.ends_with("WeatheringCopperCollection$ByState") {
                    let mut names = Vec::new();
                    for value in values.drain(..) {
                        match value {
                            RefVal::Simple(name) => names.push(name),
                            _ => names.push(String::new()),
                        }
                    }
                    let _ = stack.pop(); // the object created by `new`
                    stack.push(RefVal::Special(names));
                } else {
                    stack.push(RefVal::Other);
                }
            }
            Ins::Getstatic(index) => {
                let (owner, field, _) = field_ref(&class, index)?;
                if owner == ENTITY_TYPE_IDS {
                    if let Some(table) = entity_ids {
                        if let Some(value) = table.get(&field) {
                            let id = value.first().cloned().unwrap_or_default();
                            stack.push(RefVal::Simple(id));
                            continue;
                        }
                    }
                    stack.push(RefVal::Other);
                } else if let Some(ids) = extern_refs.get(&owner).and_then(|map| map.get(&field)) {
                    stack.push(RefVal::Simple(ids.clone()));
                } else if owner == class_name && special.contains_key(&field) {
                    stack.push(RefVal::Special(special[&field].clone()));
                } else {
                    stack.push(RefVal::Other);
                }
            }
            Ins::Invokestatic(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                match name.as_str() {
                    "create" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        match values.into_iter().next_back() {
                            Some(RefVal::Simple(text)) => stack.push(RefVal::Simple(text)),
                            _ => stack.push(RefVal::Other),
                        }
                    }
                    "createSimpleColored" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        match values.into_iter().next_back() {
                            Some(RefVal::Simple(text)) => {
                                stack.push(RefVal::Color(color_ids(&text)))
                            }
                            _ => stack.push(RefVal::Other),
                        }
                    }
                    "createSimpleCopper" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        match values.into_iter().next_back() {
                            Some(RefVal::Simple(text)) => {
                                stack.push(RefVal::Copper(copper_ids(&text)))
                            }
                            _ => stack.push(RefVal::Other),
                        }
                    }
                    "createSpawnEgg" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        match values.into_iter().last() {
                            Some(RefVal::Simple(entity)) => {
                                stack.push(RefVal::Simple(format!("{entity}_spawn_egg")))
                            }
                            _ => stack.push(RefVal::Other),
                        }
                    }
                    "createPotterySherd" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        match values.into_iter().last() {
                            Some(RefVal::Simple(pattern)) => {
                                stack.push(RefVal::Simple(format!("{pattern}_pottery_sherd")))
                            }
                            _ => stack.push(RefVal::Other),
                        }
                    }
                    "createMusicDisc" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        match values.into_iter().last() {
                            Some(RefVal::Simple(song)) => {
                                stack.push(RefVal::Simple(format!("music_disc_{song}")))
                            }
                            _ => stack.push(RefVal::Other),
                        }
                    }
                    "createArmorTrimSmithingTemplate" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        match values.into_iter().last() {
                            Some(RefVal::Simple(pattern)) => stack.push(RefVal::Simple(format!(
                                "{pattern}_armor_trim_smithing_template"
                            ))),
                            _ => stack.push(RefVal::Other),
                        }
                    }
                    "same" | "prefixWithState" => {
                        let values = pop_n_ref(&mut stack, count)?;
                        stack.push(values.into_iter().next().unwrap_or(RefVal::Other));
                    }
                    _ => {
                        let _ = owner;
                        pop_n_ref(&mut stack, count)?;
                        stack.push(RefVal::Other);
                    }
                }
            }
            Ins::Invokevirtual(index) => {
                let (_, _, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                pop_n_ref(&mut stack, count)?;
                let receiver = stack.pop().unwrap_or(RefVal::Other);
                stack.push(receiver);
            }
            Ins::Invokedynamic(_) => stack.push(RefVal::Other),
            Ins::Putstatic(index) => {
                let (_, field, _) = field_ref(&class, index)?;
                let value = stack.pop().unwrap_or(RefVal::Other);
                match value {
                    RefVal::Simple(id) => {
                        ids.insert(field, vec![id]);
                    }
                    RefVal::Color(values) | RefVal::Copper(values) => {
                        ids.insert(field, values);
                    }
                    RefVal::Special(names) => {
                        if field.ends_with("_SPECIAL_NAMES") {
                            special.insert(field, names);
                        } else {
                            ids.insert(field, special_ids(&names));
                        }
                    }
                    RefVal::Other => {}
                }
            }
            Ins::Return => break,
            other => {
                return Err(format!(
                    "{class_name}.<clinit>: unsupported instruction {other:?}"
                ))
            }
        }
    }
    Ok(RefTable { ids, special })
}

fn pop_n_ref(stack: &mut Vec<RefVal>, count: usize) -> Result<Vec<RefVal>, String> {
    if stack.len() < count {
        return Err("reference stack underflow".into());
    }
    Ok(stack.split_off(stack.len() - count))
}

/// `Items.<clinit>`: each `Items.<field>` comes from one `BlockItemIds` /
/// `ItemIds` reference read just before the `putstatic`.
fn items_fields(
    ctx: &mut Ctx,
    block_items: &RefTable,
    item_ids: &RefTable,
) -> Result<HashMap<String, Vec<String>>, String> {
    let class = ctx.class(ITEMS)?;
    let code = method_code(&class, "<clinit>")?;
    let mut fields = HashMap::new();
    let mut pending: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Getstatic(index) => {
                let (owner, field, _) = field_ref(&class, index)?;
                if owner == BLOCK_ITEM_IDS || owner == ITEM_IDS {
                    pending = Some(field);
                }
            }
            Ins::Putstatic(index) => {
                let (_, field, _) = field_ref(&class, index)?;
                if let Some(reference) = pending.take() {
                    if let Some(ids) = block_items
                        .ids
                        .get(&reference)
                        .or_else(|| item_ids.ids.get(&reference))
                    {
                        fields.insert(field, ids.clone());
                    }
                }
            }
            Ins::Return => break,
            _ => {}
        }
    }
    Ok(fields)
}

// ---------------------------------------------------------------------------
// extra data for the tabs `creative_data` could not model
// ---------------------------------------------------------------------------

const POTIONS: &str = "net/minecraft/world/item/alchemy/Potions";
const POTION_IDS: &str = "net/minecraft/world/item/alchemy/PotionIds";
const BANNER_PATTERNS: &str = "net/minecraft/world/level/block/entity/BannerPatterns";
const DECORATED_POT_PATTERNS: &str = "net/minecraft/world/level/block/entity/DecoratedPotPatterns";
const TRIM_PATTERNS: &str = "net/minecraft/world/item/equipment/trim/TrimPatterns";
const JUKEBOX_SONGS: &str = "net/minecraft/world/item/JukeboxSongs";
const RAID: &str = "net/minecraft/world/entity/raid/Raid";
const TEST_BLOCK_MODE: &str = "net/minecraft/world/level/block/state/properties/TestBlockMode";
const FIREWORK_ROCKET_ITEM: &str = "net/minecraft/world/item/FireworkRocketItem";

/// `DyeColor` declaration order: the order `ColorCollection` / `forEach`
/// iterates in and the order color ids are built in.
const DYE_ENUM: [&str; 16] = [
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

/// The explicit order `CreativeModeTabs.bootstrap` passes to
/// `registerColoredItems` (a `List<DyeColor>` in the tab lambda's local 1).
const DYE_REGISTER: [&str; 16] = [
    "white",
    "light_gray",
    "gray",
    "black",
    "brown",
    "red",
    "orange",
    "yellow",
    "lime",
    "green",
    "cyan",
    "light_blue",
    "blue",
    "purple",
    "magenta",
    "pink",
];

fn dye_index(color: &str) -> Option<usize> {
    DYE_ENUM.iter().position(|name| *name == color)
}

#[derive(Default)]
struct MoreData {
    paintings_placeable: Vec<String>,
    paintings_other: Vec<String>,
    instruments: Vec<String>,
    potions: Vec<String>,
    enchantments: Vec<(String, i64)>,
    fireworks: Vec<i64>,
    banner: Option<String>,
    test_modes: Vec<(String, i64)>,
    op_plain: Vec<String>,
}

/// Parse a reference class whose `<clinit>` is a sequence of
/// `ldc "name"; <helper>(...); putstatic Field`.
fn string_ref_map(ctx: &mut Ctx, class_name: &str) -> Result<HashMap<String, String>, String> {
    let class = ctx.class(class_name)?;
    let code = method_code(&class, "<clinit>")?;
    let mut out = HashMap::new();
    let mut pending: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Ldc(index) => {
                if let Some(text) = ldc_string(&class, index) {
                    pending = Some(text);
                }
            }
            Ins::Putstatic(index) => {
                let (_, field, _) = field_ref(&class, index)?;
                if let Some(name) = pending.take() {
                    out.insert(field, name);
                }
            }
            Ins::Return => break,
            _ => {}
        }
    }
    Ok(out)
}

fn tag_values(ctx: &mut Ctx, path: &str) -> Result<Vec<String>, String> {
    let value = ctx.jar.read_json(path)?;
    let mut out = Vec::new();
    let values = value
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{path}: no values"))?;
    for entry in values {
        let text = entry
            .as_str()
            .ok_or_else(|| format!("{path}: non-string tag entry"))?;
        if let Some(name) = text.strip_prefix('#') {
            let inner = name.strip_prefix("minecraft:").unwrap_or(name);
            out.extend(tag_values(
                ctx,
                &format!("data/minecraft/tags/{inner}.json"),
            )?);
        } else {
            out.push(text.strip_prefix("minecraft:").unwrap_or(text).to_owned());
        }
    }
    Ok(out)
}

fn json_files(ctx: &mut Ctx, prefix: &str) -> Result<Vec<(String, Value)>, String> {
    let names: Vec<String> = ctx
        .jar
        .names()
        .into_iter()
        .filter(|name| name.starts_with(prefix) && name.ends_with(".json"))
        .collect();
    let mut out = Vec::new();
    for name in names {
        let id = name[prefix.len()..name.len() - 5].to_owned();
        let value = ctx.jar.read_json(&name)?;
        out.push((id, value));
    }
    Ok(out)
}

fn number_at(value: &Value, key: &str) -> Result<i64, String> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("missing number {key}"))
}

fn paintings(ctx: &mut Ctx) -> Result<(Vec<String>, Vec<String>), String> {
    let entries = json_files(ctx, "data/minecraft/painting_variant/")?;
    let mut variants: HashMap<String, (i64, i64)> = HashMap::new();
    for (id, value) in &entries {
        variants.insert(
            id.clone(),
            (number_at(value, "width")?, number_at(value, "height")?),
        );
    }
    let placeable = tag_values(ctx, "data/minecraft/tags/painting_variant/placeable.json")?;
    let mut placeable: Vec<String> = placeable
        .into_iter()
        .filter(|id| variants.contains_key(id))
        .collect();
    let mut other: Vec<String> = variants
        .keys()
        .filter(|id| !placeable.contains(*id))
        .cloned()
        .collect();
    let sort = |ids: &mut Vec<String>| {
        ids.sort_by_key(|id| {
            let (width, height) = variants[id];
            (width * height, width, id.clone())
        });
    };
    sort(&mut placeable);
    sort(&mut other);
    Ok((placeable, other))
}

fn instruments(ctx: &mut Ctx) -> Result<Vec<String>, String> {
    // `InstrumentTags.GOAT_HORNS` = `regular_goat_horns` then
    // `screaming_goat_horns`, both in tag-file order.
    let mut out = Vec::new();
    for path in [
        "data/minecraft/tags/instrument/regular_goat_horns.json",
        "data/minecraft/tags/instrument/screaming_goat_horns.json",
    ] {
        for id in tag_values(ctx, path)? {
            out.push(id);
        }
    }
    Ok(out)
}

fn potions(ctx: &mut Ctx) -> Result<Vec<String>, String> {
    let ids = string_ref_map(ctx, POTION_IDS)?;
    let class = ctx.class(POTIONS)?;
    let code = method_code(&class, "<clinit>")?;
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Getstatic(index) => {
                let (owner, field, _) = field_ref(&class, index)?;
                if owner == POTION_IDS {
                    pending = ids.get(&field).cloned();
                }
            }
            Ins::Putstatic(index) => {
                if pending.is_some() {
                    let (owner, _, _) = field_ref(&class, index)?;
                    if owner == POTIONS {
                        out.push(pending.take().unwrap());
                    }
                }
            }
            Ins::Return => break,
            _ => {}
        }
    }
    Ok(out)
}

fn enchantments(ctx: &mut Ctx) -> Result<Vec<(String, i64)>, String> {
    let mut entries = json_files(ctx, "data/minecraft/enchantment/")?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    for (id, value) in entries {
        out.push((id, number_at(&value, "max_level")?));
    }
    Ok(out)
}

fn fireworks(ctx: &mut Ctx) -> Result<Vec<i64>, String> {
    let class = ctx.class(FIREWORK_ROCKET_ITEM)?;
    let code = method_code(&class, "<clinit>")?;
    let mut out = Vec::new();
    let mut ints: Vec<i64> = Vec::new();
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Int(value) => ints.push(value as i64),
            Ins::Bipush(value) => ints.push(value as i64),
            Ins::Sipush(value) => ints.push(value as i64),
            Ins::Bastore => {
                // `newarray byte; dup; iconst index; iconst value; bastore`
                if let Some(value) = ints.last().copied() {
                    out.push(value);
                }
                ints.clear();
            }
            Ins::Return => break,
            _ => {}
        }
    }
    Ok(out)
}

fn test_modes(ctx: &mut Ctx) -> Result<Vec<(String, i64)>, String> {
    let class = ctx.class(TEST_BLOCK_MODE)?;
    let code = method_code(&class, "<clinit>")?;
    let mut out = Vec::new();
    let mut strings: Vec<String> = Vec::new();
    let mut ints: Vec<i64> = Vec::new();
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Ldc(index) => {
                if let Some(text) = ldc_string(&class, index) {
                    strings.push(text);
                }
            }
            Ins::Int(value) => ints.push(value as i64),
            Ins::Bipush(value) => ints.push(value as i64),
            Ins::Putstatic(_) => {
                // enum fields are stored right after the constructor call; the
                // last two strings are (enum constant, mode name) and the last
                // int is the serialized id.
                if strings.len() >= 2 {
                    if let (Some(mode), Some(id)) = (strings.last(), ints.last()) {
                        out.push((mode.clone(), *id));
                    }
                }
                strings.clear();
                ints.clear();
            }
            Ins::Return => break,
            _ => {}
        }
    }
    Ok(out)
}

fn op_plain_items(
    ctx: &mut Ctx,
    items: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    let class = ctx.class(CREATIVE_TABS)?;
    let method = method(&class, "lambda$bootstrap$33")?;
    let code = method
        .code
        .ok_or_else(|| "op lambda has no Code".to_string())?
        .bytes;
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Getstatic(index) => {
                let (owner, field, _) = field_ref(&class, index)?;
                if owner == ITEMS {
                    if let Some(ids) = items.get(&field) {
                        if ids.len() == 1 {
                            pending = Some(ids[0].clone());
                        }
                    }
                }
            }
            Ins::Invokeinterface(index) => {
                let (owner, name, _) = method_ref(&class, index)?;
                if owner == "net/minecraft/world/item/CreativeModeTab$Output" && name == "accept" {
                    if let Some(id) = pending.take() {
                        out.push(id);
                    }
                }
            }
            Ins::Invokestatic(index) => {
                let (owner, name, _) = method_ref(&class, index)?;
                if owner == TEST_BLOCK_MODE && name == "values" {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn banner(ctx: &mut Ctx, items: &HashMap<String, Vec<String>>) -> Result<String, String> {
    let patterns = string_ref_map(ctx, BANNER_PATTERNS)?;
    let raid = ctx.class(RAID)?;
    let method = method(&raid, "getBannerComponentPatch")?;
    let code = method
        .code
        .ok_or_else(|| "Raid.getBannerComponentPatch has no Code".to_string())?
        .bytes;
    let mut pattern: Option<String> = None;
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Getstatic(index) => {
                let (owner, field, _) = field_ref(&raid, index)?;
                if owner == BANNER_PATTERNS {
                    if let Some(id) = patterns.get(&field) {
                        pattern = Some(id.clone());
                    }
                } else if owner == "net/minecraft/world/item/DyeColor" {
                    if let Some(pattern) = pattern.take() {
                        entries.push((field.to_ascii_lowercase(), pattern));
                    }
                }
            }
            Ins::Invokevirtual(index) => {
                let (_, name, _) = method_ref(&raid, index)?;
                if name == "build" {
                    break;
                }
            }
            Ins::Return => break,
            _ => {}
        }
    }

    // The banner's display-name component, from Raid.<clinit>.
    let clinit = method_code(&raid, "<clinit>")?;
    let mut last_string: Option<String> = None;
    let mut name_key: Option<String> = None;
    let mut pc = 0usize;
    while let Ok(ins) = decode(&clinit, &mut pc) {
        match ins {
            Ins::Ldc(index) => {
                if let Some(text) = ldc_string(&raid, index) {
                    last_string = Some(text);
                }
            }
            Ins::Putstatic(index) => {
                let (_, field, _) = field_ref(&raid, index)?;
                if field == "OMINOUS_BANNER_PATTERN_NAME" {
                    name_key = last_string.clone();
                }
            }
            Ins::Return => break,
            _ => {}
        }
    }
    let name_key = name_key.ok_or_else(|| "no ominous banner name".to_string())?;
    let item = items
        .get("BANNER")
        .and_then(|ids| ids.first())
        .ok_or_else(|| "no white banner id".to_string())?
        .clone();
    let mut pattern_text = String::new();
    for (index, (color, pattern)) in entries.iter().enumerate() {
        if index > 0 {
            pattern_text.push(',');
        }
        pattern_text.push_str(&format!(
            "{{color:\"{color}\",pattern:\"minecraft:{pattern}\"}}"
        ));
    }
    Ok(format!(
        "{{components:{{\"minecraft:banner_patterns\":[{pattern_text}],\
         \"minecraft:item_name\":{{translate:\"{name_key}\"}},\
         \"minecraft:rarity\":\"uncommon\",\
         \"minecraft:tooltip_display\":{{hidden_components:[\"minecraft:banner_patterns\"]}}}},\
         count:1,id:\"minecraft:{item}\"}}"
    ))
}

fn push_paintings(ids: &[String], output: &mut Vec<String>) {
    for id in ids {
        output.push(format!(
            "{{components:{{\"minecraft:painting/variant\":\"minecraft:{id}\"}},count:1,id:\"minecraft:painting\"}}"
        ));
    }
}

fn push_instruments(data: &MoreData, output: &mut Vec<String>) {
    for id in &data.instruments {
        if id == "ponder_goat_horn" {
            // The goat horn item's default instrument is ponder, so that stack
            // serializes without a component.
            output.push(snbt("goat_horn"));
        } else {
            output.push(format!(
                "{{components:{{\"minecraft:instrument\":\"minecraft:{id}\"}},count:1,id:\"minecraft:goat_horn\"}}"
            ));
        }
    }
}

fn push_fireworks(data: &MoreData, output: &mut Vec<String>) {
    for duration in &data.fireworks {
        if *duration <= 1 {
            // Flight duration 1 is the component's default, so the stack
            // serializes as the plain rocket.
            output.push(snbt("firework_rocket"));
        } else {
            output.push(format!(
                "{{components:{{\"minecraft:fireworks\":{{flight_duration:{duration}b}}}},count:1,id:\"minecraft:firework_rocket\"}}"
            ));
        }
    }
}

fn push_potions(potion_item: &str, data: &MoreData, output: &mut Vec<String>) {
    for id in &data.potions {
        output.push(format!(
            "{{components:{{\"minecraft:potion_contents\":{{potion:\"minecraft:{id}\"}}}},count:1,id:\"minecraft:{potion_item}\"}}"
        ));
    }
}

fn push_enchantments(data: &MoreData, output: &mut Vec<String>) {
    for (id, level) in &data.enchantments {
        output.push(format!(
            "{{components:{{\"minecraft:stored_enchantments\":{{\"minecraft:{id}\":{level}}}}},count:1,id:\"minecraft:enchanted_book\"}}"
        ));
    }
}

/// The `Optional.ifPresent` in a tab's generator lambda: which registry it
/// feeds is determined by the tab (each remaining tab has exactly one).
fn append_optional_generator(
    tab_id: &str,
    data: &MoreData,
    output: &mut Vec<String>,
) -> Result<(), String> {
    match tab_id {
        "minecraft:functional_blocks" => {
            push_paintings(&data.paintings_placeable, output);
            Ok(())
        }
        "minecraft:op_blocks" => {
            push_paintings(&data.paintings_other, output);
            Ok(())
        }
        "minecraft:tools_and_utilities" => {
            push_instruments(data, output);
            Ok(())
        }
        "minecraft:combat" => {
            push_potions("tipped_arrow", data, output);
            Ok(())
        }
        "minecraft:ingredients" => {
            push_enchantments(data, output);
            Ok(())
        }
        other => Err(format!("{other}: unmodelled Optional.ifPresent generator")),
    }
}
fn build_op_items(
    _ctx: &mut Ctx,
    items: &HashMap<String, Vec<String>>,
    data: &MoreData,
) -> Result<Vec<String>, String> {
    let mut output: Vec<String> = data.op_plain.iter().map(|id| snbt(id)).collect();
    let test_block = items
        .get("TEST_BLOCK")
        .and_then(|ids| ids.first())
        .ok_or_else(|| "no test_block id".to_string())?;
    for (mode, id) in &data.test_modes {
        if *id == 0 {
            output.push(snbt(test_block));
        } else {
            output.push(format!(
                "{{components:{{\"minecraft:block_state\":{{mode:\"{mode}\"}}}},count:1,id:\"minecraft:{test_block}\"}}"
            ));
        }
    }
    let light = items
        .get("LIGHT")
        .and_then(|ids| ids.first())
        .ok_or_else(|| "no light id".to_string())?;
    // `lambda$bootstrap$33`: `bipush 15; istore_2` before the light loop.
    let start = 15;
    for level in (0..=start).rev() {
        if level == start {
            output.push(snbt(light));
        } else {
            output.push(format!(
                "{{components:{{\"minecraft:block_state\":{{level:\"{level}\"}}}},count:1,id:\"minecraft:{light}\"}}"
            ));
        }
    }
    push_paintings(&data.paintings_other, &mut output);
    let mut seen = HashSet::new();
    output.retain(|stack| seen.insert(stack.clone()));
    Ok(output)
}

fn more_data(ctx: &mut Ctx, items: &HashMap<String, Vec<String>>) -> Result<MoreData, String> {
    let (paintings_placeable, paintings_other) = paintings(ctx)?;
    Ok(MoreData {
        paintings_placeable,
        paintings_other,
        instruments: instruments(ctx)?,
        potions: potions(ctx)?,
        enchantments: enchantments(ctx)?,
        fireworks: fireworks(ctx)?,
        banner: banner(ctx, items).ok(),
        test_modes: test_modes(ctx)?,
        op_plain: op_plain_items(ctx, items)?,
    })
}

fn lambda_impl(
    class: &ClassFile,
    cp_index: u16,
) -> Result<(String, String, String, usize), String> {
    let (bootstrap, kind) = match class.constants.get(cp_index as usize) {
        Some(Constant::InvokeDynamic {
            bootstrap, kind, ..
        }) => (*bootstrap, kind),
        other => {
            return Err(format!(
                "{}: #{cp_index} is not invokedynamic: {other:?}",
                class.name
            ))
        }
    };
    if kind != "I" {
        return Err(format!("{}: dynamic constant #{cp_index}", class.name));
    }
    let entry = class
        .bootstraps
        .get(bootstrap)
        .ok_or_else(|| format!("{}: no bootstrap #{bootstrap}", class.name))?;
    let impl_handle = *entry
        .arguments
        .get(1)
        .ok_or_else(|| format!("{}: bootstrap #{bootstrap} has no impl handle", class.name))?;
    let reference = match class.constants.get(impl_handle) {
        Some(Constant::MethodHandle { reference }) => *reference,
        other => {
            return Err(format!(
                "{}: bootstrap #{bootstrap} impl is not a MethodHandle: {other:?}",
                class.name
            ))
        }
    };
    let (owner, name, descriptor) = classfile::method_ref(&class.constants, reference)
        .ok_or_else(|| format!("{}: bad impl method ref", class.name))?;
    let sam_index = *entry
        .arguments
        .first()
        .ok_or_else(|| format!("{}: bootstrap #{bootstrap} has no sam type", class.name))?;
    let sam_descriptor = match class.constants.get(sam_index) {
        Some(Constant::MethodType(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            utf8_constant(class, inner)
        }
        other => {
            return Err(format!(
                "{}: bootstrap #{bootstrap} sam is not MethodType: {other:?}",
                class.name
            ))
        }
    };
    let captured = param_kinds(&descriptor)
        .len()
        .saturating_sub(param_kinds(&sam_descriptor).len());
    Ok((owner, name, descriptor, captured))
}

fn copper_lambda_kind(ctx: &mut Ctx, class_name: &str, method: &str) -> Option<String> {
    let class = ctx.class(class_name).ok()?;
    let code = method_code(&class, method).ok()?;
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc).ok()?;
        if let Ins::Invokevirtual(index) = ins {
            if let Ok((_, name, _)) = method_ref(&class, index) {
                if name == "waxed" {
                    return Some("waxed".into());
                }
                if name == "weathering" {
                    return Some("weathering".into());
                }
            }
        }
        if matches!(ins, Ins::Return | Ins::Areturn) {
            break;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CreativeModeTabs.bootstrap: tab metadata
// ---------------------------------------------------------------------------

fn tab_keys(ctx: &mut Ctx) -> Result<HashMap<String, String>, String> {
    let class = ctx.class(CREATIVE_TABS)?;
    let code = method_code(&class, "<clinit>")?;
    let mut keys = HashMap::new();
    let mut pending: Option<String> = None;
    let mut from_create_key = false;
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Ldc(index) => {
                pending = ldc_string(&class, index);
                from_create_key = false;
            }
            Ins::Invokestatic(index) => {
                let (_, name, _) = method_ref(&class, index)?;
                if name == "createKey" {
                    from_create_key = true;
                }
            }
            Ins::Putstatic(index) => {
                let (_, field, _) = field_ref(&class, index)?;
                if from_create_key {
                    if let Some(value) = pending.take() {
                        keys.insert(field, value);
                    }
                }
                pending = None;
                from_create_key = false;
            }
            Ins::Return => break,
            _ => {}
        }
    }
    Ok(keys)
}

fn bootstrap_tabs(ctx: &mut Ctx) -> Result<Vec<TabDef>, String> {
    let class = ctx.class(CREATIVE_TABS)?;
    let keys = tab_keys(ctx)?;
    let bootstrap = method(&class, "bootstrap")?;
    let code = bootstrap
        .code
        .ok_or_else(|| "CreativeModeTabs.bootstrap has no Code".to_string())?
        .bytes;
    let mut stack: Vec<Val> = Vec::new();
    let mut locals: Vec<Val> = vec![Val::Registry, Val::Unknown, Val::Unknown, Val::Unknown];
    let mut tabs = Vec::new();
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Nop => {}
            Ins::AconstNull => stack.push(Val::Null),
            Ins::Int(value) => stack.push(Val::Int(value)),
            Ins::Long(value) => stack.push(Val::Long(value)),
            Ins::Float(value) => stack.push(Val::Float(value)),
            Ins::Double(value) => stack.push(Val::Double(value)),
            Ins::Ldc(index) => {
                if let Some(text) = ldc_string(&class, index) {
                    stack.push(Val::Str(text));
                } else {
                    stack.push(Val::Unknown);
                }
            }
            Ins::Ldc2(_) => stack.push(Val::Unknown),
            Ins::Bipush(value) => stack.push(Val::Int(value)),
            Ins::Sipush(value) => stack.push(Val::Int(value)),
            Ins::Aload(index) => {
                stack.push(locals.get(index as usize).cloned().unwrap_or(Val::Unknown))
            }
            Ins::Astore(index) => {
                let value = stack.pop().unwrap_or(Val::Unknown);
                let index = index as usize;
                if locals.len() <= index {
                    locals.resize(index + 1, Val::Unknown);
                }
                locals[index] = value;
            }
            Ins::Pop => {
                stack.pop();
            }
            Ins::Pop2 => {
                stack.pop();
                stack.pop();
            }
            Ins::Dup => {
                let value = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| "bootstrap: dup on empty stack".to_string())?;
                stack.push(value);
            }
            Ins::Aastore => {
                let value = stack.pop().ok_or("aastore value")?;
                let index = stack.pop().ok_or("aastore index")?.as_int()? as usize;
                let array = stack.pop().ok_or("aastore array")?;
                match array {
                    Val::Array(mut values) => {
                        if values.len() <= index {
                            values.resize(index + 1, Val::Null);
                        }
                        values[index] = value;
                        stack.push(Val::Array(values));
                    }
                    other => return Err(format!("aastore on {other:?}")),
                }
            }
            Ins::Anewarray(_) => {
                let count = stack.pop().ok_or("anewarray count")?.as_int()? as usize;
                stack.push(Val::Array(vec![Val::Null; count]));
            }
            Ins::NewArray => {
                stack.pop();
                stack.push(Val::Array(Vec::new()));
            }
            Ins::Getstatic(index) => {
                let (owner, name, desc) = field_ref(&class, index)?;
                let value = if owner == CREATIVE_TABS {
                    keys.get(&name)
                        .map(|id| Val::Key(id.clone()))
                        .unwrap_or(Val::Unknown)
                } else if owner == "net/minecraft/world/item/CreativeModeTab$Row" {
                    Val::Row(name)
                } else if owner == "net/minecraft/world/item/CreativeModeTab$Type" {
                    Val::TabType(name)
                } else {
                    let _ = desc;
                    Val::Unknown
                };
                stack.push(value);
            }
            Ins::Invokestatic(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                match (owner.as_str(), name.as_str()) {
                    ("net/minecraft/network/chat/Component", "translatable") => {
                        let args = pop_n(&mut stack, count)?;
                        let key = args
                            .first()
                            .and_then(|value| value.as_str().ok())
                            .unwrap_or("")
                            .to_owned();
                        stack.push(Val::Component(key));
                    }
                    (CREATIVE_TAB, "builder") => {
                        let args = pop_n(&mut stack, count)?;
                        let column = args.get(1).and_then(|v| v.as_int().ok()).unwrap_or(0);
                        let row = match args.first() {
                            Some(Val::Row(row)) => row.clone(),
                            other => return Err(format!("CreativeModeTab.builder row: {other:?}")),
                        };
                        stack.push(Val::Builder(Box::new(Builder {
                            row,
                            column,
                            tab_type: "CATEGORY".into(),
                            title_key: None,
                            icon_method: None,
                            display_method: None,
                            hide_title: false,
                            aligned_right: false,
                            no_scroll: false,
                        })));
                    }
                    ("java/util/List", "of") => {
                        let args = pop_n(&mut stack, count)?;
                        match args.into_iter().next() {
                            Some(Val::Array(values)) => stack.push(Val::List(values)),
                            other => stack.push(Val::List(vec![other.unwrap_or(Val::Unknown)])),
                        }
                    }
                    ("net/minecraft/core/Registry", "register") => {
                        let args = pop_n(&mut stack, count)?;
                        let key = match args.get(1) {
                            Some(Val::Key(key)) => key.clone(),
                            other => return Err(format!("Registry.register key: {other:?}")),
                        };
                        let mut tab = match args.get(2) {
                            Some(Val::Tab(tab)) => (**tab).clone(),
                            other => return Err(format!("Registry.register tab: {other:?}")),
                        };
                        tab.id = format!("minecraft:{key}");
                        tabs.push(tab.clone());
                        stack.push(Val::Tab(Box::new(tab)));
                    }
                    _ => {
                        pop_n(&mut stack, count)?;
                        stack.push(Val::Unknown);
                    }
                }
            }
            Ins::Invokevirtual(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                let mut args = pop_n(&mut stack, count)?;
                let receiver = stack.pop().ok_or("bootstrap: missing receiver")?;
                match (owner.as_str(), name.as_str(), receiver) {
                    (CREATIVE_TAB_BUILDER, "title", Val::Builder(mut builder)) => {
                        if let Some(Val::Component(key)) = args.pop() {
                            builder.title_key = Some(key);
                        }
                        stack.push(Val::Builder(builder));
                    }
                    (CREATIVE_TAB_BUILDER, "icon", Val::Builder(mut builder)) => {
                        if let Some(Val::Lambda { method, .. }) = args.pop() {
                            builder.icon_method = Some(method);
                        }
                        stack.push(Val::Builder(builder));
                    }
                    (CREATIVE_TAB_BUILDER, "displayItems", Val::Builder(mut builder)) => {
                        if let Some(Val::Lambda { method, .. }) = args.pop() {
                            builder.display_method = Some(method);
                        }
                        stack.push(Val::Builder(builder));
                    }
                    (CREATIVE_TAB_BUILDER, "build", Val::Builder(builder)) => {
                        let title_key = builder
                            .title_key
                            .clone()
                            .ok_or("tab builder has no title")?;
                        let icon_method = builder
                            .icon_method
                            .clone()
                            .ok_or("tab builder has no icon")?;
                        stack.push(Val::Tab(Box::new(TabDef {
                            id: String::new(),
                            type_name: builder.tab_type.clone(),
                            row: builder.row.clone(),
                            column: builder.column,
                            title_key,
                            icon_method,
                            display_method: builder.display_method.clone(),
                        })));
                    }
                    (CREATIVE_TAB_BUILDER, "type", Val::Builder(mut builder)) => {
                        if let Some(Val::TabType(tab_type)) = args.pop() {
                            builder.tab_type = tab_type;
                        }
                        stack.push(Val::Builder(builder));
                    }
                    (CREATIVE_TAB_BUILDER, "alignedRight", Val::Builder(mut builder)) => {
                        builder.aligned_right = true;
                        stack.push(Val::Builder(builder));
                    }
                    (CREATIVE_TAB_BUILDER, "hideTitle", Val::Builder(mut builder)) => {
                        builder.hide_title = true;
                        stack.push(Val::Builder(builder));
                    }
                    (CREATIVE_TAB_BUILDER, "noScrollBar", Val::Builder(mut builder)) => {
                        builder.no_scroll = true;
                        stack.push(Val::Builder(builder));
                    }
                    (CREATIVE_TAB_BUILDER, "backgroundTexture", Val::Builder(builder)) => {
                        stack.push(Val::Builder(builder));
                    }
                    (_, _, receiver) => {
                        let _ = args;
                        stack.push(receiver);
                    }
                }
            }
            Ins::Invokedynamic(index) => {
                let (owner, method_name, _descriptor, captured) = lambda_impl(&class, index)?;
                if captured > 0 {
                    pop_n(&mut stack, captured)?;
                }
                if owner != CREATIVE_TABS {
                    return Err(format!("lambda target {owner}.{method_name}"));
                }
                stack.push(Val::Lambda {
                    method: method_name,
                    captured,
                });
            }
            Ins::Checkcast(_) => {}
            Ins::Return | Ins::Areturn => break,
            other => {
                return Err(format!(
                    "CreativeModeTabs.bootstrap: unsupported instruction {other:?}"
                ))
            }
        }
    }
    Ok(tabs)
}

// ---------------------------------------------------------------------------
// icon + display item evaluation
// ---------------------------------------------------------------------------

fn items_value(fields: &HashMap<String, Vec<String>>, field: &str) -> Result<Val, String> {
    fields
        .get(field)
        .map(|ids| Val::Items(ids.clone()))
        .ok_or_else(|| format!("Items.{field} has no registry id"))
}

fn eval_icon(
    ctx: &mut Ctx,
    items: &HashMap<String, Vec<String>>,
    blocks: &RefTable,
    block_items: &RefTable,
    method_name: &str,
) -> Result<String, String> {
    let class = ctx.class(CREATIVE_TABS)?;
    let code = method_code(&class, method_name)?;
    let mut stack: Vec<Val> = Vec::new();
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Nop => {}
            Ins::New(index) => {
                let name = class_name(&class, index)?;
                stack.push(Val::Uninit(name));
            }
            Ins::Dup => {
                let value = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| format!("{method_name}: dup on empty stack"))?;
                stack.push(value);
            }
            Ins::Int(value) => stack.push(Val::Int(value)),
            Ins::Bipush(value) => stack.push(Val::Int(value)),
            Ins::Sipush(value) => stack.push(Val::Int(value)),
            Ins::Getstatic(index) => {
                let (owner, field, _) = field_ref(&class, index)?;
                if owner == ITEMS {
                    stack.push(items_value(items, &field)?);
                } else if owner == BLOCK_IDS || owner == "net/minecraft/world/level/block/Blocks" {
                    // Block icons name `Blocks.<field>`; the id is the block
                    // item's reference id (or `BlockIds` for item-less blocks).
                    let ids = block_items
                        .ids
                        .get(&field)
                        .or_else(|| blocks.ids.get(&field))
                        .ok_or_else(|| format!("Blocks.{field} has no registry id"))?;
                    if ids.len() == 1 {
                        stack.push(Val::BlockId(ids[0].clone()));
                    } else {
                        stack.push(Val::Items(ids.clone()));
                    }
                } else {
                    stack.push(Val::Unknown);
                }
            }
            Ins::Invokespecial(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                let args = pop_n(&mut stack, count)?;
                stack.pop(); // receiver
                if owner != "net/minecraft/world/item/ItemStack" || name != "<init>" {
                    return Err(format!("{method_name}: icon ctor {owner}.{name}"));
                }
                let count_value = match args.get(1) {
                    Some(Val::Int(value)) => *value,
                    _ => 1,
                };
                let like = args
                    .first()
                    .ok_or_else(|| format!("{method_name}: icon arg"))?;
                let id = match like {
                    Val::Items(ids) if ids.len() == 1 => ids[0].clone(),
                    Val::BlockId(id) => id.clone(),
                    other => return Err(format!("{method_name}: icon item {other:?}")),
                };
                stack.push(Val::Icon {
                    id,
                    count: count_value,
                });
            }
            Ins::Invokevirtual(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                pop_n(&mut stack, count)?;
                let receiver = stack
                    .pop()
                    .ok_or_else(|| format!("{method_name}: receiver"))?;
                if owner.ends_with("ColorCollection") {
                    let index = DYE_ENUM.iter().position(|color| *color == name.as_str());
                    if let (Some(index), Val::Items(ids)) = (index, &receiver) {
                        if let Some(id) = ids.get(index) {
                            stack.push(Val::Items(vec![id.clone()]));
                            continue;
                        }
                    }
                }
                return Err(format!(
                    "{method_name}: unsupported icon call {owner}.{name}"
                ));
            }
            Ins::Areturn => {
                let icon = stack
                    .pop()
                    .ok_or_else(|| format!("{method_name}: areturn"))?;
                return match icon {
                    Val::Icon { id, count } => {
                        Ok(format!("{{count:{count},id:\"minecraft:{id}\"}}"))
                    }
                    other => Err(format!("{method_name}: returned {other:?}")),
                };
            }
            Ins::Checkcast(_) => {}
            other => {
                return Err(format!(
                    "{method_name}: unsupported icon instruction {other:?}"
                ))
            }
        }
    }
}

fn snbt(id: &str) -> String {
    format!("{{count:1,id:\"minecraft:{id}\"}}")
}

fn eval_display(
    ctx: &mut Ctx,
    items: &HashMap<String, Vec<String>>,
    data: &MoreData,
    tab_id: &str,
    method_name: &str,
) -> Result<Vec<String>, String> {
    let class = ctx.class(CREATIVE_TABS)?;
    let method = method(&class, method_name)?;
    let code = method
        .code
        .ok_or_else(|| format!("{method_name}: no Code"))?
        .bytes;
    let params = param_kinds(&method.descriptor);
    let output_local = params.len().saturating_sub(1);
    let mut locals: Vec<Val> = params.iter().map(|_| Val::Unknown).collect::<Vec<_>>();
    if let Some(slot) = locals.get_mut(output_local) {
        *slot = Val::Output;
    }
    let mut stack: Vec<Val> = Vec::new();
    let mut output: Vec<String> = Vec::new();
    let mut pc = 0usize;
    loop {
        let ins = decode(&code, &mut pc)?;
        match ins {
            Ins::Nop => {}
            Ins::AconstNull => stack.push(Val::Null),
            Ins::Int(value) => stack.push(Val::Int(value)),
            Ins::Long(value) => stack.push(Val::Long(value)),
            Ins::Float(value) => stack.push(Val::Float(value)),
            Ins::Double(value) => stack.push(Val::Double(value)),
            Ins::Ldc(index) => {
                if let Some(text) = ldc_string(&class, index) {
                    stack.push(Val::Str(text));
                } else {
                    stack.push(Val::Unknown);
                }
            }
            Ins::Ldc2(_) => stack.push(Val::Unknown),
            Ins::Bipush(value) => stack.push(Val::Int(value)),
            Ins::Sipush(value) => stack.push(Val::Int(value)),
            Ins::Aload(index) => {
                stack.push(locals.get(index as usize).cloned().unwrap_or(Val::Unknown))
            }
            Ins::Astore(index) => {
                let value = stack.pop().unwrap_or(Val::Unknown);
                let index = index as usize;
                if locals.len() <= index {
                    locals.resize(index + 1, Val::Unknown);
                }
                locals[index] = value;
            }
            Ins::Pop => {
                stack.pop();
            }
            Ins::Pop2 => {
                stack.pop();
                stack.pop();
            }
            Ins::Dup => {
                let value = stack
                    .last()
                    .cloned()
                    .ok_or_else(|| format!("{method_name}: dup on empty stack"))?;
                stack.push(value);
            }
            Ins::Anewarray(_) => {
                let count = stack.pop().ok_or("anewarray count")?.as_int()? as usize;
                stack.push(Val::Array(vec![Val::Null; count]));
            }
            Ins::NewArray => {
                stack.pop();
                stack.push(Val::Array(Vec::new()));
            }
            Ins::Aastore => {
                let value = stack.pop().ok_or("aastore value")?;
                let index = stack.pop().ok_or("aastore index")?.as_int()? as usize;
                let array = stack.pop().ok_or("aastore array")?;
                match array {
                    Val::Array(mut values) => {
                        if values.len() <= index {
                            values.resize(index + 1, Val::Null);
                        }
                        values[index] = value;
                        stack.push(Val::Array(values));
                    }
                    other => return Err(format!("{method_name}: aastore on {other:?}")),
                }
            }
            Ins::Getstatic(index) => {
                let (owner, field, _) = field_ref(&class, index)?;
                let value = if owner == ITEMS {
                    items_value(items, &field)?
                } else if owner == "net/minecraft/world/item/CreativeModeTab$TabVisibility" {
                    Val::Visibility
                } else {
                    Val::Unknown
                };
                stack.push(value);
            }
            Ins::Invokeinterface(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                let args = pop_n(&mut stack, count)?;
                stack.pop(); // receiver
                if name == "lookup" || name == "lookupOrThrow" {
                    stack.push(Val::Unknown);
                    continue;
                }
                if name != "accept" {
                    return Err(format!("{method_name}: interface {owner}.{name}"));
                }
                let item = args
                    .first()
                    .ok_or_else(|| format!("{method_name}: accept with no item"))?;
                let stack_kind = desc.starts_with("(Lnet/minecraft/world/item/ItemStack;");
                match item {
                    Val::Items(ids) if ids.len() == 1 && !stack_kind => output.push(snbt(&ids[0])),
                    Val::Str(ready) if stack_kind => output.push(ready.clone()),
                    Val::Items(ids) if ids.len() == 1 && stack_kind => output.push(snbt(&ids[0])),
                    Val::Items(ids) => {
                        return Err(format!(
                            "{method_name}: accept of a {} ids collection",
                            ids.len()
                        ))
                    }
                    other => return Err(format!("{method_name}: accept of {other:?}")),
                }
                stack.push(Val::Unknown);
            }
            Ins::Invokevirtual(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                let mut args = pop_n(&mut stack, count)?;
                let receiver = stack
                    .pop()
                    .ok_or_else(|| format!("{method_name}: receiver"))?;
                match (name.as_str(), receiver) {
                    ("holders", _) => {
                        stack.push(Val::Unknown);
                    }
                    ("ifPresent", _) => {
                        append_optional_generator(tab_id, data, &mut output)?;
                        stack.push(Val::Unknown);
                    }
                    ("white", Val::Items(ids)) if owner.ends_with("ColorCollection") => {
                        let id = ids
                            .first()
                            .ok_or_else(|| format!("{method_name}: empty ColorCollection"))?;
                        stack.push(Val::Items(vec![id.clone()]));
                    }
                    (color, Val::Items(ids))
                        if owner.ends_with("ColorCollection") && dye_index(color).is_some() =>
                    {
                        let index = dye_index(color).unwrap();
                        let id = ids
                            .get(index)
                            .ok_or_else(|| format!("{method_name}: color collection too short"))?;
                        stack.push(Val::Items(vec![id.clone()]));
                    }
                    ("forEach", Val::Items(ids))
                        if owner.ends_with("ColorCollection")
                            || owner.ends_with("WeatheringCopperCollection") =>
                    {
                        for id in ids {
                            output.push(snbt(&id));
                        }
                        let _ = &mut args;
                        stack.push(Val::Unknown);
                    }
                    ("waxed", Val::Items(ids)) if owner.ends_with("WeatheringCopperCollection") => {
                        stack.push(Val::ByState(ids[4..].to_vec()));
                    }
                    ("weathering", Val::Items(ids))
                        if owner.ends_with("WeatheringCopperCollection") =>
                    {
                        stack.push(Val::ByState(ids[..4].to_vec()));
                    }
                    ("unaffected", Val::ByState(ids)) => {
                        let id = ids
                            .first()
                            .ok_or_else(|| format!("{method_name}: empty ByState"))?;
                        stack.push(Val::Items(vec![id.clone()]));
                    }
                    ("forEach", Val::ByState(ids)) => {
                        for id in ids {
                            output.push(snbt(&id));
                        }
                        let _ = &mut args;
                        stack.push(Val::Unknown);
                    }
                    (other, receiver) => {
                        return Err(format!(
                            "{method_name}: virtual {owner}.{other}({count}) on {receiver:?}"
                        ))
                    }
                }
            }
            Ins::Invokestatic(index) => {
                let (owner, name, desc) = method_ref(&class, index)?;
                let count = param_kinds(&desc).len();
                let args = pop_n(&mut stack, count)?;
                match name.as_str() {
                    "requireNonNull" => {
                        stack.push(
                            args.get(args.len().saturating_sub(1))
                                .cloned()
                                .unwrap_or(Val::Unknown),
                        );
                    }
                    "registerColoredItems" => {
                        let collection = args
                            .get(2)
                            .ok_or_else(|| format!("{method_name}: registerColoredItems arg"))?;
                        match collection {
                            Val::Items(ids) => {
                                // The captured `List<DyeColor>` is in the
                                // bootstrap order, not `DyeColor` enum order.
                                for color in DYE_REGISTER {
                                    if let Some(index) = dye_index(color) {
                                        if let Some(id) = ids.get(index) {
                                            output.push(snbt(id));
                                        }
                                    }
                                }
                            }
                            other => {
                                return Err(format!(
                                    "{method_name}: registerColoredItems {other:?}"
                                ))
                            }
                        }
                        stack.push(Val::Unknown);
                    }
                    "generatePresetPaintings" => {
                        if tab_id == "minecraft:functional_blocks" {
                            push_paintings(&data.paintings_placeable, &mut output);
                        } else {
                            push_paintings(&data.paintings_other, &mut output);
                        }
                        stack.push(Val::Unknown);
                    }
                    "generateFireworksAllDurations" => {
                        push_fireworks(data, &mut output);
                        stack.push(Val::Unknown);
                    }
                    "generatePotionEffectTypes" => {
                        if tab_id == "minecraft:combat" {
                            push_potions("tipped_arrow", data, &mut output);
                            stack.push(Val::Unknown);
                        } else {
                            return Err(format!(
                                "{method_name}: potion items in {tab_id} not modelled"
                            ));
                        }
                    }
                    "generateEnchantmentBookTypesOnlyMaxLevel"
                    | "generateEnchantmentBookTypesAllLevels" => {
                        push_enchantments(data, &mut output);
                        stack.push(Val::Unknown);
                    }
                    "getOminousBannerInstance" => match &data.banner {
                        Some(banner) => stack.push(Val::Str(banner.clone())),
                        None => return Err(format!("{method_name}: banner data unavailable")),
                    },
                    "generateSuspiciousStews" | "generateOminousBottles" => {
                        return Err(format!(
                            "{method_name}: {name} needs effect/amplifier components not modelled"
                        ))
                    }
                    "copperBlockFamilies" => {
                        let consumer = args
                            .first()
                            .cloned()
                            .ok_or_else(|| format!("{method_name}: copperBlockFamilies arg"))?;
                        let kind = match consumer {
                            Val::Consumer { copper: Some(kind) } => kind,
                            other => {
                                return Err(format!(
                                    "{method_name}: copperBlockFamilies consumer {other:?}"
                                ))
                            }
                        };
                        for field in [
                            "COPPER_BLOCK",
                            "CHISELED_COPPER",
                            "COPPER_GRATE",
                            "CUT_COPPER",
                            "CUT_COPPER_STAIRS",
                            "CUT_COPPER_SLAB",
                            "COPPER_BARS",
                            "COPPER_DOOR",
                            "COPPER_TRAPDOOR",
                            "COPPER_BULB",
                            "COPPER_CHAIN",
                        ] {
                            let ids = items
                                .get(field)
                                .ok_or_else(|| format!("Items.{field} has no ids"))?;
                            let range = if kind == "waxed" { 4..8 } else { 0..4 };
                            for id in &ids[range] {
                                output.push(snbt(id));
                            }
                        }
                        stack.push(Val::Unknown);
                    }
                    _ => {
                        let _ = owner;
                        stack.push(Val::Unknown);
                    }
                }
            }
            Ins::Invokedynamic(index) => {
                let (owner, target, _desc, captured) = lambda_impl(&class, index)?;
                if owner == CREATIVE_TABS {
                    if captured > 0 {
                        pop_n(&mut stack, captured)?;
                    }
                    let copper = copper_lambda_kind(ctx, CREATIVE_TABS, &target);
                    stack.push(Val::Consumer { copper });
                } else {
                    // A bound method reference such as `output::accept`; the
                    // captured receiver sits below the indy call site.
                    stack.pop();
                    stack.push(Val::Consumer { copper: None });
                }
            }
            Ins::Checkcast(_) => {}
            Ins::Return => break,
            Ins::Ifeq(_) | Ins::Ifne(_) | Ins::Goto(_) => {
                return Err(format!(
                    "{method_name}: branch (permissions/per-feature gating)"
                ))
            }
            other => return Err(format!("{method_name}: unsupported instruction {other:?}")),
        }
    }
    // `ItemDisplayBuilder.tabContents` is a `ItemStackLinkedSet`, so duplicate
    // stacks inside one tab are dropped, first occurrence wins.
    let mut seen = HashSet::new();
    output.retain(|stack| seen.insert(stack.clone()));
    Ok(output)
}

fn permissions_gated(ctx: &mut Ctx, method_name: &str) -> bool {
    let Ok(class) = ctx.class(CREATIVE_TABS) else {
        return false;
    };
    let Ok(code) = method_code(&class, method_name) else {
        return false;
    };
    let mut pc = 0usize;
    while let Ok(ins) = decode(&code, &mut pc) {
        if let Ins::Invokevirtual(index) = ins {
            if let Ok((_, name, _)) = method_ref(&class, index) {
                if name == "hasPermissions" {
                    return true;
                }
            }
        }
        if matches!(ins, Ins::Return | Ins::Areturn) {
            break;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// public export
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Tab {
    pub id: String,
    pub type_name: String,
    pub row: String,
    pub column: i32,
    pub title: String,
    pub icon: String,
    pub visible: bool,
    pub items: Option<Vec<String>>,
    pub permissions_only: Option<Vec<usize>>,
    pub gap: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Export {
    pub tabs: Vec<Tab>,
    pub gaps: Vec<String>,
}

impl Export {
    /// Tabs whose item list was reproduced exactly (and whose metadata is known).
    pub fn complete(&self) -> Vec<&Tab> {
        self.tabs
            .iter()
            .filter(|tab| tab.items.is_some() && tab.gap.is_none())
            .collect()
    }

    /// The subset table: schema 3 and only complete tabs.
    pub fn to_value(&self) -> Value {
        let mut tabs = Vec::new();
        for tab in self.complete() {
            tabs.push(tab_value(tab, true));
        }
        let mut root = Map::new();
        root.insert("schema".into(), Value::from(3));
        root.insert("tabs".into(), Value::Array(tabs));
        Value::Object(root)
    }

    /// Every tab's metadata, with items omitted (used to check the fields this
    /// port does reproduce for tabs whose item lists it cannot).
    pub fn metadata_value(&self) -> Value {
        let mut tabs = Vec::new();
        for tab in &self.tabs {
            tabs.push(tab_value(tab, false));
        }
        let mut root = Map::new();
        root.insert("schema".into(), Value::from(3));
        root.insert("tabs".into(), Value::Array(tabs));
        Value::Object(root)
    }
}

fn tab_value(tab: &Tab, with_items: bool) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), Value::String(tab.id.clone()));
    object.insert("type".into(), Value::String(tab.type_name.clone()));
    object.insert("row".into(), Value::String(tab.row.clone()));
    object.insert("column".into(), Value::from(tab.column));
    object.insert("title".into(), Value::String(tab.title.clone()));
    object.insert("icon".into(), Value::String(tab.icon.clone()));
    object.insert("visible".into(), Value::Bool(tab.visible));
    if with_items {
        if let Some(items) = &tab.items {
            object.insert(
                "items".into(),
                Value::Array(items.iter().cloned().map(Value::String).collect()),
            );
        }
    }
    if let Some(indices) = &tab.permissions_only {
        object.insert(
            "permissions_only".into(),
            Value::Array(indices.iter().map(|index| Value::from(*index)).collect()),
        );
    }
    Value::Object(object)
}

pub fn generate(jar_path: impl AsRef<Path>) -> Result<Export, String> {
    let jar = Jar::open(jar_path)?;
    let mut ctx = Ctx::new(jar);

    let entity_ids = reference_table(&mut ctx, ENTITY_TYPE_IDS, None, &HashMap::new())?;
    let block_items = reference_table(&mut ctx, BLOCK_ITEM_IDS, None, &HashMap::new())?;
    let mut extern_refs: HashMap<String, HashMap<String, String>> = HashMap::new();
    extern_refs.insert(
        DECORATED_POT_PATTERNS.to_owned(),
        string_ref_map(&mut ctx, DECORATED_POT_PATTERNS)?,
    );
    extern_refs.insert(
        TRIM_PATTERNS.to_owned(),
        string_ref_map(&mut ctx, TRIM_PATTERNS)?,
    );
    extern_refs.insert(
        JUKEBOX_SONGS.to_owned(),
        string_ref_map(&mut ctx, JUKEBOX_SONGS)?,
    );
    let item_ids = reference_table(&mut ctx, ITEM_IDS, Some(&entity_ids.ids), &extern_refs)?;
    let block_ids = reference_table(&mut ctx, BLOCK_IDS, None, &HashMap::new())?;
    let items = items_fields(&mut ctx, &block_items, &item_ids)?;
    let data = more_data(&mut ctx, &items)?;
    let tabs_def = bootstrap_tabs(&mut ctx)?;

    let lang = ctx.lang()?.clone();
    let mut tabs = Vec::new();
    let mut gaps = Vec::new();
    let mut seen = HashSet::new();
    for def in &tabs_def {
        if !seen.insert(def.id.clone()) {
            continue;
        }
        let title = match lang.get(&def.title_key) {
            Some(title) => title.clone(),
            None => {
                let gap = format!(
                    "{}: title key {} missing from en_us.json",
                    def.id, def.title_key
                );
                gaps.push(gap.clone());
                String::new()
            }
        };
        let icon = match eval_icon(&mut ctx, &items, &block_ids, &block_items, &def.icon_method) {
            Ok(icon) => icon,
            Err(error) => {
                gaps.push(format!("{}: icon: {error}", def.id));
                String::new()
            }
        };
        let gated = def
            .display_method
            .as_deref()
            .map(|method| permissions_gated(&mut ctx, method))
            .unwrap_or(false);
        let (item_list, gap) = match def.display_method.as_deref() {
            None => (Some(Vec::new()), None),
            Some(method) => {
                let result = if def.id == "minecraft:op_blocks" {
                    build_op_items(&mut ctx, &items, &data)
                } else {
                    eval_display(&mut ctx, &items, &data, &def.id, method)
                };
                match result {
                    Ok(list) => (Some(list), None),
                    Err(error) => {
                        let reason = format!("{}: items: {error}", def.id);
                        gaps.push(reason.clone());
                        (None, Some(reason))
                    }
                }
            }
        };
        let visible = if def.type_name == "CATEGORY" {
            !gated
        } else {
            true
        };
        let gap = gap.or_else(|| {
            if title.is_empty() || icon.is_empty() {
                Some(format!("{}: metadata incomplete", def.id))
            } else {
                None
            }
        });
        let permissions_only = if def.id == "minecraft:op_blocks" {
            item_list
                .as_ref()
                .map(|items| (0..items.len()).collect::<Vec<usize>>())
        } else {
            None
        };
        tabs.push(Tab {
            id: def.id.clone(),
            type_name: def.type_name.clone(),
            row: def.row.clone(),
            column: def.column,
            title,
            icon,
            visible,
            items: item_list,
            permissions_only,
            gap,
        });
    }
    Ok(Export { tabs, gaps })
}

/// All 14 tabs: the seven `creative_data` already reproduces exactly are taken
/// from it unchanged; the remaining tabs come from the extended evaluator.
pub fn generate_all(jar_path: impl AsRef<Path>) -> Result<Export, String> {
    let path = jar_path.as_ref();
    let base_export = base::generate(path)?;
    let local = generate(path)?;
    let mut tabs = Vec::new();
    for base_tab in base_export.tabs {
        if base_tab.items.is_some() && base_tab.gap.is_none() {
            tabs.push(Tab {
                id: base_tab.id,
                type_name: base_tab.type_name,
                row: base_tab.row,
                column: base_tab.column,
                title: base_tab.title,
                icon: base_tab.icon,
                visible: base_tab.visible,
                items: base_tab.items,
                permissions_only: None,
                gap: base_tab.gap,
            });
        } else if let Some(local_tab) = local.tabs.iter().find(|tab| tab.id == base_tab.id) {
            tabs.push(local_tab.clone());
        } else {
            tabs.push(Tab {
                id: base_tab.id,
                type_name: base_tab.type_name,
                row: base_tab.row,
                column: base_tab.column,
                title: base_tab.title,
                icon: base_tab.icon,
                visible: base_tab.visible,
                items: base_tab.items,
                permissions_only: None,
                gap: base_tab.gap,
            });
        }
    }
    let gaps = tabs
        .iter()
        .filter_map(|tab| tab.gap.clone())
        .collect::<Vec<_>>();
    Ok(Export { tabs, gaps })
}

pub fn export(jar_path: impl AsRef<Path>) -> Result<Value, String> {
    generate_all(jar_path).map(|export| export.to_value())
}

pub fn write(jar_path: impl AsRef<Path>, out: impl AsRef<Path>) -> Result<Export, String> {
    let export = generate_all(jar_path)?;
    let text = serde_json::to_string(&export.to_value()).map_err(|e| e.to_string())?;
    let out = out.as_ref();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(out, text + "\n").map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(export)
}
