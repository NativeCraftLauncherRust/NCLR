//! Rebuild the non-state sections of Horizon's `block-behaviours.json` from the

//! player's own `client.jar`.

//!

//! `VanillaBlockBehaviourExporter` writes five sections; `states` (the 35,723 per-state

//! rows) and the reflective `fields` maps are other tasks. This module reads the rest

//! out of the jar's bytecode:

//!

//! * `blocks` -?every block in `BuiltInRegistries.BLOCK` order with its simple class

//!   chain up to (excluding) `BlockBehaviour`, its declared interfaces in the exporter's

//!   walk order, `dynamic_shape` (`Block.hasDynamicShape()`, i.e. the

//!   `Properties.dynamicShape()` call in the registration's property chain) and `item`

//!   (`block.asItem()`; `minecraft:air` for blocks without an item). The reflective

//!   `fields` map is task-37's, so it is left out here.

//! * `items` -?every `BlockItem`: its simple class chain up to (excluding) `Item` and the

//!   `block` field (the block it places). `wallBlock`/`attachmentDirection`/`placeSound`

//!   are task-37's reflective fields, so they are left out here.

//! * `fluids` -?the five `BuiltInRegistries.FLUID` ids in registry order.

//! * `fire` -?`FireBlock`'s instance configuration: `igniteOdds`/`burnOdds` from

//!   `FireBlock.bootStrap()`'s `setFlammable(Blocks.X, ignite, burn)` calls, and

//!   `fireDamage` from the `BaseFireBlock` constructor call. The `shapes` field is a

//!   lambda's `toString()` (`<Block$$Lambda/0x...>`), i.e. a JVM identity hash that no

//!   class-file reader can reproduce; it is reported as a gap.

//!

//! Every value comes from the jar the player already has; nothing is taken from

//! `ship.hzr3`.

#![allow(dead_code)]

use std::cell::RefCell;

use std::collections::BTreeMap;

use std::fs;

use std::io::Read;

use std::path::Path;

use std::rc::Rc;

use serde_json::{Map, Value};

#[path = "block.rs"]
mod block;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{field_ref, method_ref, string_constant, ClassFile, Constant};

/// The table these sections belong to.

pub const TABLE: &str = "block-behaviours.json";

/// The sections this module writes, and the one field it cannot.

pub const SECTIONS: [&str; 4] = ["blocks", "items", "fluids", "fire"];

/// `fire.shapes` is the `toString()` of a JVM-generated lambda class; its identity hash

/// changes per run, so it is not reproducible from the class file.

pub const GAP: &str = "fire.shapes: a JVM lambda identity string (`<Block$$Lambda/0x...>`)";

/// What a run of this module produced and what it could not.

#[derive(Debug, Default, Clone, PartialEq, Eq)]

pub struct Coverage {
    pub produced: Vec<String>,

    pub missing: Vec<String>,

    pub notes: Vec<String>,
}

/// The player's `client.jar`, in memory.

pub struct Jar {
    files: BTreeMap<String, Vec<u8>>,

    classes: RefCell<BTreeMap<String, Rc<ClassFile>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;

        let mut zip =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;

        let mut files = BTreeMap::new();

        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;

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

    fn bytes(&self, name: &str) -> Result<&[u8], String> {
        self.files
            .get(name)
            .map(|bytes| bytes.as_slice())
            .ok_or_else(|| format!("{name}: not in the jar"))
    }

    fn class(&self, internal: &str) -> Result<Rc<ClassFile>, String> {
        if let Some(class) = self.classes.borrow().get(internal) {
            return Ok(class.clone());
        }

        let name = format!("{internal}.class");

        let class = Rc::new(
            classfile::parse(self.bytes(&name)?).map_err(|error| format!("{name}: {error}"))?,
        );

        self.classes
            .borrow_mut()
            .insert(internal.to_string(), class.clone());

        Ok(class)
    }

    /// Simple block-class name -> internal name, from the jar's file names.

    fn class_by_simple_name(&self) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = BTreeMap::new();

        for name in self.files.keys() {
            let Some(rest) = name.strip_prefix("net/minecraft/") else {
                continue;
            };

            let Some(internal) = rest.strip_suffix(".class") else {
                continue;
            };

            if !internal.starts_with("world/level/block/") {
                continue;
            }

            let internal = format!("net/minecraft/{internal}");

            let simple = internal.rsplit('/').next().unwrap_or(&internal);

            let simple = simple.to_string();

            out.entry(simple).or_insert(internal);
        }

        out
    }
}

// ---------------------------------------------------------------------------

// Bytecode

// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]

struct Instruction {
    opcode: u8,

    index: Option<usize>,

    immediate: Option<i32>,
}

fn u16_at(code: &[u8], at: usize) -> Result<usize, String> {
    if at + 2 > code.len() {
        return Err(format!("bytecode ends inside an operand at {at}"));
    }

    Ok(((code[at] as usize) << 8) | code[at + 1] as usize)
}

fn decode(code: &[u8]) -> Result<Vec<Instruction>, String> {
    let mut out = Vec::new();

    let mut at = 0usize;

    while at < code.len() {
        let opcode = code[at];

        let mut index = None;

        let mut immediate = None;

        let length = match opcode {
            0x12 => {
                index = Some(*code.get(at + 1).ok_or("ldc ends the code")? as usize);

                2
            }

            0x13 | 0x14 => {
                index = Some(u16_at(code, at + 1)?);

                3
            }

            0x10 => {
                immediate = Some(*code.get(at + 1).ok_or("bipush ends the code")? as i8 as i32);

                2
            }

            0x11 => {
                immediate = Some(i16::from_be_bytes(
                    code.get(at + 1..at + 3)
                        .ok_or("sipush ends the code")?
                        .try_into()
                        .map_err(|_| "sipush")?,
                ) as i32);

                3
            }

            0x15..=0x19 | 0x36..=0x3a => {
                index = Some(*code.get(at + 1).ok_or("local ends the code")? as usize);

                2
            }

            0x84 => 3,

            0xa9 => 2,

            0x99..=0xa8 | 0xc6 | 0xc7 => 3,

            0xaa => {
                let pad = (4 - ((at + 1) % 4)) % 4;

                let body = at + 1 + pad;

                let low = i32::from_be_bytes(
                    code.get(body + 4..body + 8)
                        .ok_or("tableswitch out of range")?
                        .try_into()
                        .map_err(|_| "tableswitch")?,
                );

                let high = i32::from_be_bytes(
                    code.get(body + 8..body + 12)
                        .ok_or("tableswitch out of range")?
                        .try_into()
                        .map_err(|_| "tableswitch")?,
                );

                1 + pad + 12 + ((high - low + 1).max(0) as usize) * 4
            }

            0xab => {
                let pad = (4 - ((at + 1) % 4)) % 4;

                let body = at + 1 + pad;

                let pairs = i32::from_be_bytes(
                    code.get(body + 4..body + 8)
                        .ok_or("lookupswitch out of range")?
                        .try_into()
                        .map_err(|_| "lookupswitch")?,
                );

                1 + pad + 8 + (pairs.max(0) as usize) * 8
            }

            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = Some(u16_at(code, at + 1)?);

                3
            }

            0xb9 | 0xba => {
                index = Some(u16_at(code, at + 1)?);

                5
            }

            0xbc => 2,

            0xc5 => 4,

            0xc8 | 0xc9 => 5,

            0xc4 => {
                let next = *code.get(at + 1).ok_or("wide out of range")?;

                if next == 0x84 {
                    6
                } else {
                    4
                }
            }

            0x00..=0x0f
            | 0x1a..=0x35
            | 0x3b..=0x83
            | 0x85..=0x98
            | 0xac..=0xb1
            | 0xbe
            | 0xbf
            | 0xc2
            | 0xc3 => 1,

            other => return Err(format!("unmodelled opcode {other:#04x} at {at}")),
        };

        if at + length > code.len() {
            return Err(format!("instruction at {at} runs past the code"));
        }

        out.push(Instruction {
            opcode,

            index,

            immediate,
        });

        at += length;
    }

    Ok(out)
}

fn code_of<'a>(class: &'a ClassFile, name: &str) -> Option<&'a [u8]> {
    class
        .methods
        .iter()
        .find(|member| member.name == name)
        .and_then(|member| member.code.as_ref())
        .map(|code| code.bytes.as_slice())
}

#[derive(Debug, Clone)]

struct Header {
    name: String,

    super_name: String,

    interfaces: Vec<String>,
}

/// Parse a class' own header: internal name, superclass and declared interfaces (in

/// declaration order). `classfile.rs` keeps the first two but skips the interface table.

fn class_header(bytes: &[u8]) -> Result<Header, String> {
    struct Cursor<'a> {
        bytes: &'a [u8],

        at: usize,
    }

    impl<'a> Cursor<'a> {
        fn u1(&mut self) -> Result<u8, String> {
            let value = *self.bytes.get(self.at).ok_or("class file ends")?;

            self.at += 1;

            Ok(value)
        }

        fn u2(&mut self) -> Result<u16, String> {
            Ok(((self.u1()? as u16) << 8) | self.u1()? as u16)
        }

        fn u4(&mut self) -> Result<u32, String> {
            Ok(((self.u2()? as u32) << 16) | self.u2()? as u32)
        }

        fn skip(&mut self, count: usize) -> Result<(), String> {
            if self.at + count > self.bytes.len() {
                return Err("class file ends".into());
            }

            self.at += count;

            Ok(())
        }
    }

    let mut cursor = Cursor { bytes, at: 0 };

    if cursor.u4()? != 0xCAFE_BABE {
        return Err("not a class file".into());
    }

    cursor.u2()?;

    cursor.u2()?;

    let count = cursor.u2()? as usize;

    let mut utf8: Vec<Option<String>> = vec![None; count];

    let mut classes: Vec<Option<usize>> = vec![None; count];

    let mut index = 1usize;

    while index < count {
        let tag = cursor.u1()?;

        match tag {
            1 => {
                let length = cursor.u2()? as usize;

                let start = cursor.at;

                cursor.skip(length)?;

                utf8[index] =
                    Some(String::from_utf8_lossy(&bytes[start..start + length]).into_owned());
            }

            7 => classes[index] = Some(cursor.u2()? as usize),

            8 => {
                cursor.u2()?;
            }

            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => cursor.skip(4)?,

            5 | 6 => {
                cursor.skip(8)?;

                index += 1;
            }

            15 => cursor.skip(3)?,

            16 | 19 | 20 => cursor.skip(2)?,

            other => return Err(format!("constant tag {other}")),
        }

        index += 1;
    }

    let name_of = |at: usize| -> String {
        classes
            .get(at)
            .and_then(|inner| *inner)
            .and_then(|inner| utf8.get(inner).cloned().flatten())
            .unwrap_or_default()
    };

    let _access = cursor.u2()?;

    let this_class = cursor.u2()? as usize;

    let super_class = cursor.u2()? as usize;

    let count = cursor.u2()? as usize;

    let mut interfaces = Vec::with_capacity(count);

    for _ in 0..count {
        let at = cursor.u2()? as usize;

        interfaces.push(name_of(at));
    }

    Ok(Header {
        name: name_of(this_class),

        super_name: name_of(super_class),

        interfaces,
    })
}

fn header(jar: &Jar, internal: &str) -> Result<Header, String> {
    let bytes = jar.bytes(&format!("{internal}.class"))?;

    class_header(bytes)
}

fn simple(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn hierarchy(jar: &Jar, internal: &str) -> Result<Vec<Header>, String> {
    let mut out = Vec::new();

    let mut current = internal.to_string();

    for _ in 0..24 {
        let Ok(header) = header(jar, &current) else {
            break;
        };

        let next = header.super_name.clone();

        out.push(header);

        if next.is_empty() {
            break;
        }

        current = next;
    }

    Ok(out)
}

/// The exporter's `chain(c, top)`: simple names from `class` up to (excluding) `top`.

fn class_chain(jar: &Jar, internal: &str, top: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();

    let mut current = internal.to_string();

    for _ in 0..24 {
        if current == top {
            return Ok(out);
        }

        let Ok(header) = header(jar, &current) else {
            break;
        };

        out.push(simple(&header.name).to_string());

        if header.super_name.is_empty() {
            break;
        }

        current = header.super_name;
    }

    if current == top {
        Ok(out)
    } else {
        Err(format!("{internal}: class chain does not reach {top}"))
    }
}

/// The exporter's interface walk: for every class from `internal` to `Object`, append

/// its directly declared interfaces in declaration order.

fn interfaces(jar: &Jar, internal: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();

    for header in hierarchy(jar, internal)? {
        for interface in header.interfaces {
            out.push(simple(&interface).to_string());
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------

// Blocks

// ---------------------------------------------------------------------------

/// `Properties.dynamicShape()` reachable from a method body: direct call, a `Blocks`

/// helper that makes it, or a lambda that does.

fn calls_dynamic_shape(
    jar: &Jar,

    class: &Rc<ClassFile>,

    method: &classfile::Member,

    depth: usize,
) -> bool {
    if depth > 3 {
        return false;
    }

    let Some(code) = method.code.as_ref() else {
        return false;
    };

    let Ok(instructions) = decode(&code.bytes) else {
        return false;
    };

    for instruction in instructions {
        match instruction.opcode {
            0xb6 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if name == "dynamicShape" && owner.ends_with("BlockBehaviour$Properties") {
                        return true;
                    }
                }
            }

            0xb8 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if owner.ends_with("/Blocks") {
                        if let Ok(callee) = jar.class(&owner) {
                            if let Some(target) =
                                callee.methods.iter().find(|member| member.name == name)
                            {
                                if calls_dynamic_shape(jar, &callee, target, depth + 1) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }

            0xba => {
                if let Some(index) = instruction.index {
                    if indy_calls_dynamic_shape(jar, class, index, depth + 1) {
                        return true;
                    }
                }
            }

            _ => {}
        }
    }

    false
}

fn bootstrap_impl(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };

    let row = class.bootstraps.get(*bootstrap)?;

    for argument in &row.arguments {
        if let Some(Constant::MethodHandle { reference }) = class.constants.get(*argument) {
            if let Some(target) = method_ref(&class.constants, *reference) {
                return Some(target);
            }
        }
    }

    None
}

fn indy_calls_dynamic_shape(jar: &Jar, class: &Rc<ClassFile>, index: usize, depth: usize) -> bool {
    if depth > 3 {
        return false;
    }

    let Some((owner, name, _)) = bootstrap_impl(class, index) else {
        return false;
    };

    if name == "<init>" {
        return false;
    }

    let Ok(target) = jar.class(&owner) else {
        return false;
    };

    if let Some(member) = target.methods.iter().find(|member| member.name == name) {
        if calls_dynamic_shape(jar, &target, member, depth + 1) {
            return true;
        }
    }

    false
}

/// block id -> `hasDynamicShape()`.

fn dynamic_shapes(jar: &Jar) -> Result<BTreeMap<String, bool>, String> {
    let block_ids = reference_block_ids(jar)?;

    let class = jar.class("net/minecraft/world/level/block/Blocks")?;

    let Some(code) = code_of(&class, "<clinit>") else {
        return Err("Blocks.<clinit>: no code".into());
    };

    let instructions = decode(code)?;

    let mut out = BTreeMap::new();

    let mut pending: Option<Vec<String>> = None;

    let mut pending_at = 0usize;

    for i in 0..instructions.len() {
        let instruction = &instructions[i];

        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    let owner = simple(&owner);

                    if owner == "BlockItemIds" || owner == "BlockIds" {
                        if let Some(ids) = block_ids.get(&name) {
                            pending = Some(ids.clone());

                            pending_at = i;
                        }
                    }
                }
            }

            0xb8 => {
                let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };

                let registers = (owner.ends_with("/Blocks") && name.starts_with("register"))
                    || ((owner.ends_with("ColorCollection")
                        || owner.ends_with("WeatheringCopperCollection"))
                        && (name == "registerBlocks" || name == "zipMap"));

                if !registers {
                    continue;
                }

                let Some(ids) = pending.take() else {
                    continue;
                };

                let mut dynamic = false;

                for instruction in &instructions[pending_at..i] {
                    match instruction.opcode {
                        0xb6 => {
                            if let Some((owner, name, _)) = instruction
                                .index
                                .and_then(|index| method_ref(&class.constants, index))
                            {
                                if name == "dynamicShape"
                                    && owner.ends_with("BlockBehaviour$Properties")
                                {
                                    dynamic = true;
                                }
                            }
                        }

                        0xb8 => {
                            if let Some((owner, name, _)) = instruction
                                .index
                                .and_then(|index| method_ref(&class.constants, index))
                            {
                                if owner.ends_with("/Blocks") {
                                    if let Ok(callee) = jar.class(&owner) {
                                        if let Some(target) =
                                            callee.methods.iter().find(|member| member.name == name)
                                        {
                                            if calls_dynamic_shape(jar, &callee, target, 0) {
                                                dynamic = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        0xba => {
                            if let Some(index) = instruction.index {
                                if indy_calls_dynamic_shape(jar, &class, index, 0) {
                                    dynamic = true;
                                }
                            }
                        }

                        _ => {}
                    }
                }

                for id in ids {
                    out.insert(format!("minecraft:{id}"), dynamic);
                }
            }

            _ => {}
        }
    }

    Ok(out)
}

/// `BlockItemIds`/`BlockIds` field -> block ids, including the colour/copper families.

fn reference_block_ids(jar: &Jar) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut out = BTreeMap::new();

    for internal in [
        "net/minecraft/references/BlockItemIds",
        "net/minecraft/references/BlockIds",
    ] {
        let Ok(class) = jar.class(internal) else {
            continue;
        };

        let Some(code) = code_of(&class, "<clinit>") else {
            continue;
        };

        let Ok(instructions) = decode(code) else {
            continue;
        };

        let mut strings: Vec<String> = Vec::new();

        let mut calls: Vec<String> = Vec::new();

        let mut special: Option<Vec<String>> = None;

        for instruction in instructions {
            match instruction.opcode {
                0x12 | 0x13 => {
                    if let Some(text) = instruction
                        .index
                        .and_then(|index| string_constant(&class.constants, index))
                    {
                        strings.push(text);
                    }
                }

                0xb7 | 0xb8 | 0xb6 => {
                    if let Some((_, name, _)) = instruction
                        .index
                        .and_then(|index| method_ref(&class.constants, index))
                    {
                        calls.push(name);
                    }
                }

                0xb3 => {
                    let Some((_, field, _)) = instruction
                        .index
                        .and_then(|index| field_ref(&class.constants, index))
                    else {
                        continue;
                    };

                    let has = |name: &str| calls.iter().any(|call| call == name);

                    let mut ids: Option<Vec<String>> = None;

                    if has("create") {
                        if let Some(first) = strings.first() {
                            ids = Some(vec![first.clone()]);
                        }
                    } else if has("createSimpleColored") {
                        if let Some(base) = strings.first() {
                            ids = Some(
                                [
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
                                ]
                                .iter()
                                .map(|colour| format!("{colour}_{base}"))
                                .collect(),
                            );
                        }
                    } else if has("createSimpleCopper") {
                        if let Some(base) = strings.first() {
                            ids = Some(
                                [
                                    "",
                                    "exposed_",
                                    "weathered_",
                                    "oxidized_",
                                    "waxed_",
                                    "waxed_exposed_",
                                    "waxed_weathered_",
                                    "waxed_oxidized_",
                                ]
                                .iter()
                                .map(|prefix| format!("{prefix}{base}"))
                                .collect(),
                            );
                        }
                    } else if strings.len() == 4
                        && !calls
                            .iter()
                            .any(|name| name.starts_with("create") || name.starts_with("register"))
                    {
                        special = Some(strings.clone());
                    } else if has("same") && special.is_some() && strings.is_empty() {
                        let names = special.clone().unwrap_or_default();

                        let mut pairs = Vec::new();

                        for group in [
                            &["", "exposed_", "weathered_", "oxidized_"][..],
                            &[
                                "waxed_",
                                "waxed_exposed_",
                                "waxed_weathered_",
                                "waxed_oxidized_",
                            ][..],
                        ] {
                            for (prefix, value) in group.iter().zip(names.iter()) {
                                pairs.push(format!("{prefix}{value}"));
                            }
                        }

                        ids = Some(pairs);
                    }

                    if let Some(ids) = ids {
                        out.insert(field, ids);
                    }

                    strings.clear();

                    calls.clear();
                }

                _ => {}
            }
        }
    }

    Ok(out)
}

/// Blocks beyond the primary one an item registration binds (`Block... others`).
fn extra_block_items(jar: &Jar) -> Result<BTreeMap<String, String>, String> {
    let pairs = block_item_pairs(jar)?;
    let (singles, _) = blocks_fields(jar)?;
    let class = jar.class("net/minecraft/world/item/Items")?;
    let Some(code) = code_of(&class, "<clinit>") else {
        return Ok(BTreeMap::new());
    };
    let instructions = decode(code)?;
    let mut out = BTreeMap::new();
    let mut pending: Option<String> = None;
    let mut pending_at = 0usize;
    for i in 0..instructions.len() {
        let instruction = &instructions[i];
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if simple(&owner) == "BlockItemIds" {
                        pending = Some(name);
                        pending_at = i;
                    }
                }
            }
            0xb8 => {
                let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                let direct = owner.ends_with("/Items");
                let family = owner.ends_with("ColorCollection")
                    || owner.ends_with("WeatheringCopperCollection");
                let registers = name == "registerBlock"
                    || name == "registerBlockItems"
                    || name == "registerItems"
                    || name == "registerItem";
                if !(registers && (direct || family)) {
                    continue;
                }
                let Some(field) = pending.take() else {
                    continue;
                };
                let Some(ids) = pairs.get(&field) else {
                    continue;
                };
                if ids.len() != 1 {
                    continue;
                }
                let (primary, item) = &ids[0];
                for instruction in &instructions[pending_at..i] {
                    if instruction.opcode != 0xb2 {
                        continue;
                    }
                    if let Some((owner, block_field, _)) = instruction
                        .index
                        .and_then(|index| field_ref(&class.constants, index))
                    {
                        if !owner.ends_with("/Blocks") {
                            continue;
                        }
                        if let Some(block) = singles.get(&block_field) {
                            if block != &format!("minecraft:{primary}") {
                                out.insert(block.clone(), format!("minecraft:{item}"));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The wall block a `StandingAndWallBlockItem` also places, from the block's id.
fn wall_block_id(block: &str) -> Option<String> {
    let name = block.strip_prefix("minecraft:")?;
    let wall = if let Some(stem) = name.strip_suffix("_hanging_sign") {
        format!("{stem}_wall_hanging_sign")
    } else if let Some(stem) = name.strip_suffix("_sign") {
        format!("{stem}_wall_sign")
    } else if let Some(stem) = name.strip_suffix("_banner") {
        format!("{stem}_wall_banner")
    } else if let Some(stem) = name.strip_suffix("_coral_fan") {
        format!("{stem}_coral_wall_fan")
    } else if let Some(stem) = name.strip_suffix("_torch") {
        format!("{stem}_wall_torch")
    } else if let Some(stem) = name.strip_suffix("_skull") {
        format!("{stem}_wall_skull")
    } else if let Some(stem) = name.strip_suffix("_head") {
        format!("{stem}_wall_head")
    } else if name == "torch" {
        "wall_torch".to_string()
    } else {
        return None;
    };
    Some(format!("minecraft:{wall}"))
}

/// The `blocks` header section: registry order, no reflective `fields` map.

pub fn blocks(jar_path: &Path) -> Result<Value, String> {
    let jar = Jar::open(jar_path)?;

    let registry = block::registry(jar_path)?;

    let names = jar.class_by_simple_name();

    let dynamic = dynamic_shapes(&jar)?;

    let items = items_map(&jar)?;

    let mut item_by_block: BTreeMap<String, String> = BTreeMap::new();
    for (item, (_, block)) in &items {
        item_by_block.insert(block.clone(), item.clone());
    }
    // Blocks that share an item through `registerBlock(..., Block... others)` and the
    // wall blocks a `StandingAndWallBlockItem` also places.
    for (block, item) in extra_block_items(&jar)? {
        item_by_block.entry(block).or_insert(item);
    }
    let registry_ids: std::collections::BTreeSet<String> =
        registry.iter().map(|entry| entry.id.clone()).collect();
    for (item, (chain, block)) in &items {
        if !chain.iter().any(|name| name == "StandingAndWallBlockItem") {
            continue;
        }
        if let Some(wall) = wall_block_id(block) {
            if registry_ids.contains(&wall) {
                item_by_block.entry(wall).or_insert(item.clone());
            }
        }
    }

    let mut out = Vec::with_capacity(registry.len());

    for entry in &registry {
        let internal = names
            .get(&entry.class)
            .cloned()
            .ok_or_else(|| format!("{}: class {} not in the jar", entry.id, entry.class))?;

        let mut object = Map::new();

        object.insert("name".to_string(), Value::String(entry.id.clone()));

        object.insert(
            "class".to_string(),
            Value::Array(
                class_chain(
                    &jar,
                    &internal,
                    "net/minecraft/world/level/block/state/BlockBehaviour",
                )?
                .into_iter()
                .map(Value::String)
                .collect(),
            ),
        );

        object.insert(
            "interfaces".to_string(),
            Value::Array(
                interfaces(&jar, &internal)?
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );

        object.insert(
            "dynamic_shape".to_string(),
            Value::Bool(dynamic.get(&entry.id).copied().unwrap_or(false)),
        );

        object.insert(
            "item".to_string(),
            Value::String(
                item_by_block
                    .get(&entry.id)
                    .cloned()
                    .unwrap_or_else(|| "minecraft:air".to_string()),
            ),
        );

        out.push(Value::Object(object));
    }

    Ok(Value::Array(out))
}

// ---------------------------------------------------------------------------

// Items

// ---------------------------------------------------------------------------

/// `BlockItemIds` field -> `(block id, item id)` pairs (colour/copper families

/// expanded in `ColorCollection`/`WeatheringCopperCollection` order).

fn block_item_pairs(jar: &Jar) -> Result<BTreeMap<String, Vec<(String, String)>>, String> {
    let class = jar.class("net/minecraft/references/BlockItemIds")?;

    let Some(code) = code_of(&class, "<clinit>") else {
        return Err("BlockItemIds.<clinit>: no code".into());
    };

    let instructions = decode(code)?;

    let mut out = BTreeMap::new();

    let mut strings: Vec<String> = Vec::new();

    let mut calls: Vec<String> = Vec::new();

    let mut special: Option<Vec<String>> = None;

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

    const COPPER: [&str; 8] = [
        "",
        "exposed_",
        "weathered_",
        "oxidized_",
        "waxed_",
        "waxed_exposed_",
        "waxed_weathered_",
        "waxed_oxidized_",
    ];

    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&class.constants, index))
                {
                    strings.push(text);
                }
            }

            0xb7 | 0xb8 | 0xb6 => {
                if let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    calls.push(name);
                }
            }

            0xb3 => {
                let Some((_, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                else {
                    continue;
                };

                let has = |name: &str| calls.iter().any(|call| call == name);

                let mut pairs: Option<Vec<(String, String)>> = None;

                if has("create") && !strings.is_empty() {
                    let block = strings[0].clone();

                    let item = strings.get(1).cloned().unwrap_or_else(|| block.clone());

                    pairs = Some(vec![(block, item)]);
                } else if has("createSimpleColored") {
                    if let Some(base) = strings.first() {
                        pairs = Some(
                            COLORS
                                .iter()
                                .map(|colour| {
                                    let id = format!("{colour}_{base}");

                                    (id.clone(), id)
                                })
                                .collect(),
                        );
                    }
                } else if has("createSimpleCopper") {
                    if let Some(base) = strings.first() {
                        pairs = Some(
                            COPPER
                                .iter()
                                .map(|prefix| {
                                    let id = format!("{prefix}{base}");

                                    (id.clone(), id)
                                })
                                .collect(),
                        );
                    }
                } else if strings.len() == 4
                    && !calls
                        .iter()
                        .any(|name| name.starts_with("create") || name.starts_with("register"))
                {
                    special = Some(strings.clone());
                } else if has("same") && special.is_some() && strings.is_empty() {
                    let names = special.clone().unwrap_or_default();

                    let mut expanded = Vec::new();

                    for (group, prefixes) in
                        [(&names[..], &COPPER[..4]), (&names[..], &COPPER[4..])]
                    {
                        for (value, prefix) in group.iter().zip(prefixes.iter()) {
                            let id = format!("{prefix}{value}");

                            expanded.push((id.clone(), id));
                        }
                    }

                    pairs = Some(expanded);
                }

                if let Some(pairs) = pairs {
                    out.insert(field, pairs);
                }

                strings.clear();

                calls.clear();
            }

            _ => {}
        }
    }

    Ok(out)
}

fn is_block_item(jar: &Jar, internal: &str) -> bool {
    let mut current = internal.to_string();

    for _ in 0..16 {
        if current == "net/minecraft/world/item/BlockItem" {
            return true;
        }

        let Ok(header) = header(jar, &current) else {
            return false;
        };

        if header.super_name.is_empty() {
            return false;
        }

        current = header.super_name;
    }

    false
}

/// `Item::registerBlock` overloads that construct their own `BlockItem`.
const BLOCK_ITEM: &str = "net/minecraft/world/item/BlockItem";

/// Resolve the `BlockItem` subclass a method (lambda) constructs when it registers an
/// item: `registerBlock(id, block, factory)` uses the factory's class, while the
/// `BlockItem`/`Properties`/`UnaryOperator` overloads use plain `BlockItem`.
fn factory_of_method(
    jar: &Jar,
    class: &Rc<ClassFile>,
    method: &classfile::Member,
    depth: usize,
) -> Option<String> {
    if depth > 3 {
        return None;
    }
    let code = method.code.as_ref()?;
    let instructions = decode(&code.bytes).ok()?;
    let mut last_indy: Option<usize> = None;
    for instruction in instructions {
        match instruction.opcode {
            0xba => {
                last_indy = instruction.index;
            }
            0xbb => {
                if let Some(index) = instruction.index {
                    if let Some(Constant::Class(reference)) = class.constants.get(index) {
                        let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
                        if let Some(Constant::Utf8(name)) = class.constants.get(inner) {
                            if is_block_item(jar, name) {
                                return Some(name.clone());
                            }
                        }
                    }
                }
            }
            0xb8 => {
                let Some((owner, name, descriptor)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                if name == "registerBlock" && owner.ends_with("/Items") {
                    let factory = descriptor.contains("BiFunction")
                        || descriptor.contains("java/util/function/Function;");
                    if factory {
                        if let Some(index) = last_indy.take() {
                            if let Some(found) = indy_factory(jar, class, index, depth + 1) {
                                return Some(found);
                            }
                        }
                    } else {
                        return Some(BLOCK_ITEM.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn indy_factory(jar: &Jar, class: &Rc<ClassFile>, index: usize, depth: usize) -> Option<String> {
    let (owner, name, _) = bootstrap_impl(class, index)?;
    if name == "<init>" {
        return is_block_item(jar, &owner).then_some(owner);
    }
    let target = jar.class(&owner).ok()?;
    let method = target
        .methods
        .iter()
        .find(|candidate| candidate.name == name)?;
    factory_of_method(jar, &target, method, depth + 1)
}

fn item_factory_class(
    jar: &Jar,
    items: &Rc<ClassFile>,
    instructions: &[Instruction],
    from: usize,
    to: usize,
) -> Option<String> {
    for instruction in &instructions[from..to] {
        if instruction.opcode != 0xba {
            continue;
        }
        let Some(index) = instruction.index else {
            continue;
        };
        if let Some(found) = indy_factory(jar, items, index, 0) {
            return Some(found);
        }
    }
    // `createBlockItemWithCustomItemName(block)` returns a `BlockItem` factory for the
    // crop/wire items whose item name differs from the block name.
    for instruction in &instructions[from..to] {
        if instruction.opcode != 0xb8 {
            continue;
        }
        if let Some((owner, name, _)) = instruction
            .index
            .and_then(|index| method_ref(&items.constants, index))
        {
            if name == "createBlockItemWithCustomItemName" && owner.ends_with("/Items") {
                return Some(BLOCK_ITEM.to_string());
            }
        }
    }
    None
}
/// BlockItem id -> `(simple class chain up to Item, the block it places)`.

fn items_map(jar: &Jar) -> Result<BTreeMap<String, (Vec<String>, String)>, String> {
    let pairs = block_item_pairs(jar)?;

    let class = jar.class("net/minecraft/world/item/Items")?;

    let Some(code) = code_of(&class, "<clinit>") else {
        return Err("Items.<clinit>: no code".into());
    };

    let instructions = decode(code)?;

    let mut out: BTreeMap<String, (Vec<String>, String)> = BTreeMap::new();

    let mut pending: Option<String> = None;

    let mut pending_at = 0usize;

    for i in 0..instructions.len() {
        let instruction = &instructions[i];

        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if simple(&owner) == "BlockItemIds" {
                        pending = Some(name);

                        pending_at = i;
                    }
                }
            }

            0xb8 => {
                let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };

                let direct = owner.ends_with("/Items");

                let family = owner.ends_with("ColorCollection")
                    || owner.ends_with("WeatheringCopperCollection");

                let registers = name == "registerBlock"
                    || name == "registerBlockItems"
                    || name == "registerItems"
                    || name == "registerItem";

                if !(registers && (direct || family)) {
                    // A `BlockItemIds` getstatic can be followed by unrelated static
                    // calls (`Integer.valueOf`, property construction) before its
                    // registration; only a registration consumes the pending id.
                    continue;
                }

                let Some(field) = pending.take() else {
                    continue;
                };

                let Some(ids) = pairs.get(&field) else {
                    continue;
                };

                let factory = item_factory_class(jar, &class, &instructions, pending_at, i);

                let internal = match factory {
                    Some(class) => class,

                    None if direct && name == "registerBlock" => {
                        "net/minecraft/world/item/BlockItem".to_string()
                    }

                    None => continue,
                };

                let Ok(chain) = class_chain(jar, &internal, "net/minecraft/world/item/Item") else {
                    continue;
                };

                for (block, item) in ids {
                    out.insert(
                        format!("minecraft:{item}"),
                        (chain.clone(), format!("minecraft:{block}")),
                    );
                }
            }

            _ => {}
        }
    }

    Ok(out)
}

/// The `items` section: class chain plus the `block` field only (the other reflective

/// fields are task-37's).

pub fn items(jar_path: &Path) -> Result<Value, String> {
    let jar = Jar::open(jar_path)?;

    let map = items_map(&jar)?;

    let mut out = Map::new();

    for (id, (chain, block)) in map {
        let mut fields = Map::new();

        fields.insert("block".to_string(), Value::String(block));

        let mut entry = Map::new();

        entry.insert(
            "class".to_string(),
            Value::Array(chain.into_iter().map(Value::String).collect()),
        );

        entry.insert("fields".to_string(), Value::Object(fields));

        out.insert(id, Value::Object(entry));
    }

    Ok(Value::Object(out))
}

// ---------------------------------------------------------------------------

// Fluids

// ---------------------------------------------------------------------------

/// The five fluid registry ids, in `BuiltInRegistries.FLUID` order.

pub fn fluids(jar_path: &Path) -> Result<Value, String> {
    let jar = Jar::open(jar_path)?;

    let ids = fluid_ids(&jar)?;

    Ok(Value::Array(ids.into_iter().map(Value::String).collect()))
}

/// `FluidIds` field -> id.

fn fluid_id_fields(jar: &Jar) -> BTreeMap<String, String> {
    let Ok(class) = jar.class("net/minecraft/world/level/material/FluidIds") else {
        return BTreeMap::new();
    };

    let Some(code) = code_of(&class, "<clinit>") else {
        return BTreeMap::new();
    };

    let Ok(instructions) = decode(code) else {
        return BTreeMap::new();
    };

    let mut out = BTreeMap::new();

    let mut pending: Option<String> = None;

    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&class.constants, index))
                {
                    pending = Some(text);
                }
            }

            0xb3 => {
                if let Some((_, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if let Some(id) = pending.take() {
                        out.insert(field, id);
                    }
                }
            }

            _ => {}
        }
    }

    out
}

fn fluid_ids(jar: &Jar) -> Result<Vec<String>, String> {
    let class = jar.class("net/minecraft/world/level/material/Fluids")?;

    let Some(code) = code_of(&class, "<clinit>") else {
        return Err("Fluids.<clinit>: no code".into());
    };

    let instructions = decode(code)?;

    let fields = fluid_id_fields(jar);

    let mut out = Vec::new();

    let mut pending: Option<String> = None;

    for instruction in instructions {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if owner.ends_with("/FluidIds") {
                        pending = fields.get(&name).cloned();
                    }
                }
            }

            0xb8 => {
                if let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if name == "register" {
                        if let Some(id) = pending.take() {
                            out.push(format!("minecraft:{id}"));
                        }
                    }
                }
            }

            _ => {}
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------

// Fire

// ---------------------------------------------------------------------------

/// `Blocks.<field>` -> block id, from `Blocks.<clinit>`'s `putstatic` order.

fn blocks_fields(
    jar: &Jar,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, Vec<String>>), String> {
    let ids = reference_block_ids(jar)?;
    let class = jar.class("net/minecraft/world/level/block/Blocks")?;
    let Some(code) = code_of(&class, "<clinit>") else {
        return Ok((BTreeMap::new(), BTreeMap::new()));
    };
    let instructions = decode(code)?;
    let mut singles = BTreeMap::new();
    let mut collections = BTreeMap::new();
    let mut pending: Option<Vec<String>> = None;
    for instruction in instructions {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    let owner = simple(&owner);
                    if owner == "BlockItemIds" || owner == "BlockIds" {
                        if let Some(block_ids) = ids.get(&name) {
                            pending = Some(block_ids.clone());
                        }
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if owner.ends_with("/Blocks") {
                        if let Some(block_ids) = pending.take() {
                            if block_ids.len() == 1 {
                                singles.insert(field, format!("minecraft:{}", block_ids[0]));
                            } else {
                                collections.insert(
                                    field,
                                    block_ids
                                        .into_iter()
                                        .map(|id| format!("minecraft:{id}"))
                                        .collect(),
                                );
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok((singles, collections))
}

fn int_value(instruction: &Instruction) -> Option<i32> {
    match instruction.opcode {
        0x02..=0x08 => Some(instruction.opcode as i32 - 0x03),

        0x10 | 0x11 => instruction.immediate,

        _ => None,
    }
}

/// `fire`: the `FireBlock` instance configuration, except the lambda `shapes` string.

pub fn fire(jar_path: &Path) -> Result<Value, String> {
    let jar = Jar::open(jar_path)?;
    let (singles, collections) = blocks_fields(&jar)?;

    // fireDamage: `BaseFireBlock.<init>(properties, 1.0f)` from FireBlock's constructor.
    let mut fire_damage = 1.0f32;
    if let Ok(class) = jar.class("net/minecraft/world/level/block/FireBlock") {
        if let Some(code) = code_of(&class, "<init>") {
            if let Ok(instructions) = decode(code) {
                let mut last_float: Option<f32> = None;
                for instruction in &instructions {
                    match instruction.opcode {
                        0x0b => last_float = Some(0.0),
                        0x0c => last_float = Some(1.0),
                        0x0d => last_float = Some(2.0),
                        0x12 | 0x13 => {
                            if let Some(index) = instruction.index {
                                if let Some(Constant::Float(value)) = class.constants.get(index) {
                                    last_float = Some(*value);
                                }
                            }
                        }
                        0xb7 => {
                            if let Some((owner, name, _)) = instruction
                                .index
                                .and_then(|index| method_ref(&class.constants, index))
                            {
                                if owner.ends_with("/BaseFireBlock") && name == "<init>" {
                                    if let Some(value) = last_float {
                                        fire_damage = value;
                                    }
                                    return fire_with_damage(
                                        &jar,
                                        &singles,
                                        &collections,
                                        fire_damage,
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    fire_with_damage(&jar, &singles, &collections, fire_damage)
}

/// The lambda a `forEach` consumer names, as the two ints it passes to `setFlammable`.
fn for_each_odds(jar: &Jar, class: &Rc<ClassFile>, index: usize) -> Option<(i32, i32)> {
    let (owner, name, _) = bootstrap_impl(class, index)?;
    let target = jar.class(&owner).ok()?;
    let method = target
        .methods
        .iter()
        .find(|candidate| candidate.name == name)?;
    let code = method.code.as_ref()?;
    let instructions = decode(&code.bytes).ok()?;
    let mut ints = Vec::new();
    for instruction in &instructions {
        if let Some(value) = int_value(instruction) {
            ints.push(value);
        }
    }
    if ints.len() >= 2 {
        Some((ints[ints.len() - 2], ints[ints.len() - 1]))
    } else {
        None
    }
}

fn fire_with_damage(
    jar: &Jar,
    singles: &BTreeMap<String, String>,
    collections: &BTreeMap<String, Vec<String>>,
    fire_damage: f32,
) -> Result<Value, String> {
    let mut ignite = Map::new();
    let mut burn = Map::new();
    let class = jar.class("net/minecraft/world/level/block/FireBlock")?;
    let Some(member) = class
        .methods
        .iter()
        .find(|member| member.name == "bootStrap")
    else {
        return Err("FireBlock.bootStrap: no code".into());
    };
    let Some(code) = member.code.as_ref() else {
        return Err("FireBlock.bootStrap: no code".into());
    };
    let instructions = decode(&code.bytes)?;
    let mut last_block: Option<String> = None;
    let mut last_collection: Option<Vec<String>> = None;
    let mut last_indy: Option<usize> = None;
    let mut ints: Vec<i32> = Vec::new();
    for instruction in &instructions {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if owner.ends_with("/Blocks") {
                        if let Some(id) = singles.get(&field) {
                            last_block = Some(id.clone());
                            last_collection = None;
                        } else if let Some(members) = collections.get(&field) {
                            last_collection = Some(members.clone());
                            last_block = None;
                        }
                    }
                }
            }
            0xba => last_indy = instruction.index,
            0xb6 => {
                let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                if name == "forEach" && owner.ends_with("ColorCollection") {
                    if let (Some(members), Some(indy)) = (last_collection.take(), last_indy) {
                        if let Some((ignite_value, burn_value)) = for_each_odds(jar, &class, indy) {
                            for id in members {
                                ignite.insert(id.clone(), Value::Number(ignite_value.into()));
                                burn.insert(id, Value::Number(burn_value.into()));
                            }
                        }
                    }
                    ints.clear();
                } else if owner.ends_with("/FireBlock") && name == "setFlammable" {
                    if let Some(id) = last_block.take() {
                        if ints.len() >= 2 {
                            ignite.insert(id.clone(), Value::Number(ints[ints.len() - 2].into()));
                            burn.insert(id, Value::Number(ints[ints.len() - 1].into()));
                        }
                    }
                    ints.clear();
                }
            }
            0xb3 => {
                last_block = None;
                last_collection = None;
                ints.clear();
            }
            _ => {
                if let Some(value) = int_value(instruction) {
                    ints.push(value);
                }
            }
        }
    }

    let mut object = Map::new();
    object.insert("igniteOdds".to_string(), Value::Object(ignite));
    object.insert("burnOdds".to_string(), Value::Object(burn));
    object.insert("fireDamage".to_string(), Value::from(fire_damage as f64));
    Ok(Value::Object(object))
}

// ---------------------------------------------------------------------------

// The module's output

// ---------------------------------------------------------------------------

/// The four sections this module can rebuild, plus what is still missing.

pub fn sections(jar_path: &Path) -> Result<(Value, Coverage), String> {
    if !jar_path.exists() {
        return Err(format!("{}: not found", jar_path.display()));
    }

    let mut root = Map::new();

    root.insert("blocks".to_string(), blocks(jar_path)?);

    root.insert("items".to_string(), items(jar_path)?);

    root.insert("fluids".to_string(), fluids(jar_path)?);

    root.insert("fire".to_string(), fire(jar_path)?);

    let mut coverage = Coverage::default();

    coverage.produced = vec![
        "blocks (header: name/class/interfaces/dynamic_shape/item)".to_string(),
        "items (BlockItem class chain + block field)".to_string(),
        "fluids (5 registry order)".to_string(),
        "fire (igniteOdds/burnOdds/fireDamage)".to_string(),
    ];

    coverage.missing = vec![
        "blocks.fields".to_string(),
        "items.fields (wallBlock/attachmentDirection/placeSound)".to_string(),
        GAP.to_string(),
    ];

    coverage.notes = vec![
        "blocks.fields and items.fields are the reflective value() dump, task-37's scope"
            .to_string(),
        GAP.to_string(),
    ];

    Ok((Value::Object(root), coverage))
}

/// Write the four sections as a standalone JSON document (an input for the eventual

/// full `block-behaviours.json`: `states` comes from `states.rs`, `fields` from the

/// reflective-field task).

pub fn write(jar_path: &Path, out: &Path) -> Result<Coverage, String> {
    let (document, coverage) = sections(jar_path)?;

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }

    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("{}: {error}", out.display()))?;

    fs::write(out, text + "\n").map_err(|error| format!("{}: {error}", out.display()))?;

    Ok(coverage)
}
