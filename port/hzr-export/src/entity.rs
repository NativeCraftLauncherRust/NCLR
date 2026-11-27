//! Rebuild Horizon's `entity-types.json` and `attributes.json` from the
//! player's own `client.jar`, without reading `ship.hzr3`.
//!
//! Vanilla does not ship entity types or default attributes as data: they live
//! in static initialisers and `createAttributes()` factories in the jar's class
//! files. This module therefore contains a small, purpose-built JVM class-file
//! reader and a straight-line bytecode interpreter for exactly the shapes
//! vanilla uses:
//!
//! * `EntityTypeIds.<clinit>`          -> entity id names
//! * `MobCategory.<clinit>`            -> category display names
//! * `EntityTypes.<clinit>`            -> registration order, builder chains
//!   (category/dimensions/eye height/fire immunity) and the type ids
//! * `Attributes.<clinit>`             -> attribute registry order + defaults
//! * `DefaultAttributes.<clinit>`      -> entity type -> attribute supplier
//! * `<Mob>.createAttributes()` chains -> default attribute base values
//!
//! Only opcodes actually emitted by those methods are interpreted; anything
//! else fails loudly with the class/method/pc instead of inventing a value.
//!
//! Public entry points:
//! * [`export_entity_types`] -> `serde_json::Value` of `entity-types.json`
//! * [`export_attributes`]   -> `serde_json::Value` of `attributes.json`
//! * [`generate`]            -> both at once
//! * [`write_entity_types`] / [`write_attributes`] -> write pretty JSON files
//!
//! The test harness in `tests/entity.rs` includes this file directly because
//! the crate's `main.rs` is a binary and must not be edited.

// Until `main.rs` wires this module in as a CLI command, the only compiled
// consumer is the integration test, so parts of the class-file reader and the
// convenience write helpers have no in-tree caller yet.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Number, Value};

const ENTITY_TYPE_IDS: &str = "net/minecraft/world/entity/EntityTypeIds";
const ENTITY_TYPES: &str = "net/minecraft/world/entity/EntityTypes";
const ENTITY_TYPE_BUILDER: &str = "net/minecraft/world/entity/EntityType$Builder";
const MOB_CATEGORY: &str = "net/minecraft/world/entity/MobCategory";
const ATTRIBUTES: &str = "net/minecraft/world/entity/ai/attributes/Attributes";
const RANGED_ATTRIBUTE: &str = "net/minecraft/world/entity/ai/attributes/RangedAttribute";
const DEFAULT_ATTRIBUTES: &str = "net/minecraft/world/entity/ai/attributes/DefaultAttributes";
const ATTRIBUTE_SUPPLIER: &str = "net/minecraft/world/entity/ai/attributes/AttributeSupplier";
const ATTRIBUTE_BUILDER: &str =
    "net/minecraft/world/entity/ai/attributes/AttributeSupplier$Builder";

// ---------------------------------------------------------------------------
// jar access
// ---------------------------------------------------------------------------

/// Minimal read-only view of the client jar.
pub struct Jar {
    archive: zip::ZipArchive<File>,
}

impl Jar {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { archive })
    }

    pub fn read(&mut self, name: &str) -> Result<Vec<u8>, String> {
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

    fn read_class(&mut self, internal_name: &str) -> Result<Vec<u8>, String> {
        self.read(&format!("{internal_name}.class"))
            .map_err(|e| format!("class {internal_name}: {e}"))
    }
}

// ---------------------------------------------------------------------------
// class file reader
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Cp {
    Empty,
    Utf8(String),
    Integer(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class(u16),
    String(u16),
    FieldRef(u16, u16),
    MethodRef(u16, u16),
    InterfaceMethodRef(u16, u16),
    NameAndType(u16, u16),
    MethodHandle,
    MethodType(u16),
    Dynamic(u16, u16),
    InvokeDynamic(u16, u16),
    Module(u16),
    Package(u16),
}

#[derive(Clone, Debug)]
pub struct Method {
    pub name: String,
    pub desc: String,
    pub code: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct ClassFile {
    pub name: String,
    pub super_name: Option<String>,
    pub methods: Vec<Method>,
    constants: Vec<Cp>,
}

impl ClassFile {
    fn utf8(&self, index: u16) -> Result<&str, String> {
        match self.constants.get(index as usize) {
            Some(Cp::Utf8(text)) => Ok(text),
            other => Err(format!(
                "{}: constant #{index} is not Utf8 ({other:?})",
                self.name
            )),
        }
    }

    fn class_name(&self, index: u16) -> Result<&str, String> {
        match self.constants.get(index as usize) {
            Some(Cp::Class(name_index)) => self.utf8(*name_index),
            other => Err(format!(
                "{}: constant #{index} is not Class ({other:?})",
                self.name
            )),
        }
    }

    fn member_ref(&self, index: u16) -> Result<(&str, &str, &str), String> {
        let (class_index, nt_index) = match self.constants.get(index as usize) {
            Some(Cp::FieldRef(class, nt)) => (*class, *nt),
            Some(Cp::MethodRef(class, nt)) => (*class, *nt),
            Some(Cp::InterfaceMethodRef(class, nt)) => (*class, *nt),
            other => {
                return Err(format!(
                    "{}: constant #{index} is not a member ref ({other:?})",
                    self.name
                ))
            }
        };
        let owner = self.class_name(class_index)?;
        let (name, desc) = match self.constants.get(nt_index as usize) {
            Some(Cp::NameAndType(name, desc)) => (self.utf8(*name)?, self.utf8(*desc)?),
            other => {
                return Err(format!(
                    "{}: constant #{nt_index} is not NameAndType ({other:?})",
                    self.name
                ))
            }
        };
        Ok((owner, name, desc))
    }

    fn field_ref(&self, index: u16) -> Result<(&str, &str, &str), String> {
        self.member_ref(index)
    }

    fn method_ref(&self, index: u16) -> Result<(&str, &str, &str), String> {
        self.member_ref(index)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if self.remaining() < count {
            return Err("truncated class file".into());
        }
        let slice = &self.bytes[self.pos..self.pos + count];
        self.pos += count;
        Ok(slice)
    }

    fn u1(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u2(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u4(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u8(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn skip(&mut self, count: usize) -> Result<(), String> {
        self.take(count).map(|_| ())
    }
}

fn constant_utf8(constants: &[Cp], index: u16) -> Result<&str, String> {
    match constants.get(index as usize) {
        Some(Cp::Utf8(text)) => Ok(text),
        other => Err(format!("bad Utf8 constant #{index}: {other:?}")),
    }
}

fn skip_attributes(reader: &mut Reader<'_>) -> Result<(), String> {
    let count = reader.u2()?;
    for _ in 0..count {
        reader.u2()?; // attribute_name_index
        let length = reader.u4()? as usize;
        reader.skip(length)?;
    }
    Ok(())
}

pub fn parse_class(data: &[u8]) -> Result<ClassFile, String> {
    let mut reader = Reader::new(data);
    let magic = reader.u4()?;
    if magic != 0xCAFE_BABE {
        return Err(format!("bad class magic {magic:#x}"));
    }
    reader.u2()?; // minor_version
    reader.u2()?; // major_version

    let constant_count = reader.u2()?;
    let mut constants: Vec<Cp> = Vec::with_capacity(constant_count as usize);
    constants.push(Cp::Empty);
    let mut index = 1u16;
    while index < constant_count {
        let tag = reader.u1()?;
        let entry = match tag {
            1 => {
                let length = reader.u2()? as usize;
                let bytes = reader.take(length)?;
                Cp::Utf8(String::from_utf8_lossy(bytes).into_owned())
            }
            3 => Cp::Integer(reader.u4()? as i32),
            4 => Cp::Float(f32::from_bits(reader.u4()?)),
            5 => Cp::Long(reader.u8()? as i64),
            6 => Cp::Double(f64::from_bits(reader.u8()?)),
            7 => Cp::Class(reader.u2()?),
            8 => Cp::String(reader.u2()?),
            9 => Cp::FieldRef(reader.u2()?, reader.u2()?),
            10 => Cp::MethodRef(reader.u2()?, reader.u2()?),
            11 => Cp::InterfaceMethodRef(reader.u2()?, reader.u2()?),
            12 => Cp::NameAndType(reader.u2()?, reader.u2()?),
            15 => {
                reader.u1()?;
                reader.u2()?;
                Cp::MethodHandle
            }
            16 => Cp::MethodType(reader.u2()?),
            17 => Cp::Dynamic(reader.u2()?, reader.u2()?),
            18 => Cp::InvokeDynamic(reader.u2()?, reader.u2()?),
            19 => Cp::Module(reader.u2()?),
            20 => Cp::Package(reader.u2()?),
            other => return Err(format!("unknown constant pool tag {other}")),
        };
        let wide = matches!(entry, Cp::Long(_) | Cp::Double(_));
        constants.push(entry);
        index += 1;
        if wide {
            constants.push(Cp::Empty);
            index += 1;
        }
    }

    reader.u2()?; // access_flags
    let this_class = reader.u2()?;
    let super_class = reader.u2()?;
    let name = match constants.get(this_class as usize) {
        Some(Cp::Class(name_index)) => constant_utf8(&constants, *name_index)?.to_owned(),
        other => return Err(format!("bad this_class: {other:?}")),
    };
    let super_name = if super_class == 0 {
        None
    } else {
        match constants.get(super_class as usize) {
            Some(Cp::Class(name_index)) => Some(constant_utf8(&constants, *name_index)?.to_owned()),
            other => return Err(format!("bad super_class: {other:?}")),
        }
    };

    let interfaces = reader.u2()?;
    reader.skip(interfaces as usize * 2)?;

    let fields = reader.u2()?;
    for _ in 0..fields {
        reader.u2()?; // access
        reader.u2()?; // name
        reader.u2()?; // descriptor
        skip_attributes(&mut reader)?;
    }

    let mut methods = Vec::new();
    let method_count = reader.u2()?;
    for _ in 0..method_count {
        reader.u2()?; // access
        let method_name = reader.u2()?;
        let method_desc = reader.u2()?;
        let method_name = constant_utf8(&constants, method_name)?.to_owned();
        let method_desc = constant_utf8(&constants, method_desc)?.to_owned();
        let attribute_count = reader.u2()?;
        let mut code = None;
        for _ in 0..attribute_count {
            let attr_name_index = reader.u2()?;
            let length = reader.u4()? as usize;
            let body = reader.take(length)?;
            if constant_utf8(&constants, attr_name_index)? == "Code" {
                let mut code_reader = Reader::new(body);
                code_reader.u2()?; // max_stack
                code_reader.u2()?; // max_locals
                let code_length = code_reader.u4()? as usize;
                let bytes = code_reader.take(code_length)?.to_vec();
                let exception_table = code_reader.u2()?;
                code_reader.skip(exception_table as usize * 8)?;
                skip_attributes(&mut code_reader)?;
                code = Some(bytes);
            }
        }
        methods.push(Method {
            name: method_name,
            desc: method_desc,
            code,
        });
    }

    skip_attributes(&mut reader)?;

    Ok(ClassFile {
        name,
        super_name,
        methods,
        constants,
    })
}

// ---------------------------------------------------------------------------
// bytecode decoder (only the opcodes vanilla's initialisers use)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Instr {
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
    Pop,
    Pop2,
    Dup,
    ArrayStore(u8),
    Getstatic(u16),
    Putstatic(u16),
    Invokevirtual(u16),
    Invokespecial(u16),
    Invokestatic(u16),
    Invokeinterface(u16),
    Invokedynamic(u16),
    New(u16),
    NewArray,
    ANewArray(u16),
    Checkcast(u16),
    Instanceof(u16),
    Arraylength,
    I2D,
    I2F,
    I2L,
    L2I,
    L2F,
    L2D,
    F2I,
    F2L,
    F2D,
    D2I,
    D2L,
    D2F,
    I2B,
    I2C,
    I2S,
    Fneg,
    Dneg,
    Areturn,
    Return,
}

fn opcode_name(op: u8) -> &'static str {
    match op {
        0x15 => "iload",
        0x19 => "aload",
        0x36 => "istore",
        0x3a => "astore",
        0x60 => "iadd",
        0x61 => "ladd",
        0x62 => "fadd",
        0x63 => "dadd",
        0x99 => "ifeq",
        0xa7 => "goto",
        0xaa => "tableswitch",
        0xab => "lookupswitch",
        0xb1 => "return",
        0xc4 => "wide",
        _ => "unknown opcode",
    }
}

fn decode(code: &[u8], pc: &mut usize) -> Result<Instr, String> {
    if *pc >= code.len() {
        return Err("instruction pointer past end of code".into());
    }
    let start = *pc;
    let op = code[*pc];
    *pc += 1;
    let read_u1 = |pc: &mut usize| -> Result<u8, String> {
        if *pc >= code.len() {
            return Err(format!("truncated opcode at {start}"));
        }
        let value = code[*pc];
        *pc += 1;
        Ok(value)
    };
    let read_u2 = |pc: &mut usize| -> Result<u16, String> {
        if *pc + 2 > code.len() {
            return Err(format!("truncated opcode at {start}"));
        }
        let value = u16::from_be_bytes([code[*pc], code[*pc + 1]]);
        *pc += 2;
        Ok(value)
    };
    let instr = match op {
        0x00 => Instr::Nop,
        0x01 => Instr::AconstNull,
        0x02..=0x08 => Instr::Int(op as i32 - 3),
        0x09 => Instr::Long(0),
        0x0a => Instr::Long(1),
        0x0b => Instr::Float(0.0),
        0x0c => Instr::Float(1.0),
        0x0d => Instr::Float(2.0),
        0x0e => Instr::Double(0.0),
        0x0f => Instr::Double(1.0),
        0x10 => Instr::Bipush(read_u1(pc)? as i8 as i32),
        0x11 => Instr::Sipush(read_u2(pc)? as i16 as i32),
        0x12 => Instr::Ldc(read_u1(pc)? as u16),
        0x13 => Instr::Ldc(read_u2(pc)?),
        0x14 => Instr::Ldc2(read_u2(pc)?),
        0x4f..=0x56 => Instr::ArrayStore(op),
        0x57 => Instr::Pop,
        0x58 => Instr::Pop2,
        0x59 => Instr::Dup,
        0x85 => Instr::I2L,
        0x86 => Instr::I2F,
        0x87 => Instr::I2D,
        0x88 => Instr::L2I,
        0x89 => Instr::L2F,
        0x8a => Instr::L2D,
        0x8b => Instr::F2I,
        0x8c => Instr::F2L,
        0x8d => Instr::F2D,
        0x8e => Instr::D2I,
        0x8f => Instr::D2L,
        0x90 => Instr::D2F,
        0x91 => Instr::I2B,
        0x92 => Instr::I2C,
        0x93 => Instr::I2S,
        0x94 => Instr::Fneg,
        0x95..=0x96 => Instr::Dneg,
        0xac..=0xaf => Instr::Return,
        0xb0 => Instr::Areturn,
        0xb1 => Instr::Return,
        0xb2 => Instr::Getstatic(read_u2(pc)?),
        0xb3 => Instr::Putstatic(read_u2(pc)?),
        0xb6 => Instr::Invokevirtual(read_u2(pc)?),
        0xb7 => Instr::Invokespecial(read_u2(pc)?),
        0xb8 => Instr::Invokestatic(read_u2(pc)?),
        0xb9 => {
            let index = read_u2(pc)?;
            read_u1(pc)?;
            read_u1(pc)?;
            Instr::Invokeinterface(index)
        }
        0xba => {
            let index = read_u2(pc)?;
            read_u2(pc)?;
            Instr::Invokedynamic(index)
        }
        0xbb => Instr::New(read_u2(pc)?),
        0xbc => {
            read_u1(pc)?;
            Instr::NewArray
        }
        0xbd => Instr::ANewArray(read_u2(pc)?),
        0xbe => Instr::Arraylength,
        0xbf => return Err(format!("unsupported opcode athrow at pc {start}")),
        0xc0 => Instr::Checkcast(read_u2(pc)?),
        0xc1 => Instr::Instanceof(read_u2(pc)?),
        other => {
            return Err(format!(
                "unsupported opcode {:#04x} ({}) at pc {}",
                other,
                opcode_name(other),
                start
            ))
        }
    };
    Ok(instr)
}

// ---------------------------------------------------------------------------
// shared value stack helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct AttrState {
    id: Option<String>,
    default: f64,
    min: f64,
    max: f64,
    ranged: bool,
    syncable: bool,
}

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
    Array,
    Null,
    Unknown,
    BuilderMarker,
    Attr(AttrState),
}

impl Val {
    fn as_str(&self) -> Result<&str, String> {
        match self {
            Val::Str(text) => Ok(text),
            other => Err(format!("expected string, got {other:?}")),
        }
    }

    fn as_static_name(&self) -> Result<&str, String> {
        match self {
            Val::Static { name, .. } => Ok(name),
            other => Err(format!("expected static field, got {other:?}")),
        }
    }

    fn as_f32(&self) -> Result<f32, String> {
        match self {
            Val::Float(value) => Ok(*value),
            other => Err(format!("expected float, got {other:?}")),
        }
    }

    fn as_f64(&self) -> Result<f64, String> {
        match self {
            Val::Double(value) => Ok(*value),
            other => Err(format!("expected double, got {other:?}")),
        }
    }

    fn as_int(&self) -> Result<i32, String> {
        match self {
            Val::Int(value) => Ok(*value),
            other => Err(format!("expected int, got {other:?}")),
        }
    }
}

fn pop(stack: &mut Vec<Val>) -> Result<Val, String> {
    stack
        .pop()
        .ok_or_else(|| "operand stack underflow".to_string())
}

fn push(stack: &mut Vec<Val>, value: Val) {
    stack.push(value);
}

fn duplicate(stack: &mut Vec<Val>) -> Result<(), String> {
    let value = stack
        .last()
        .cloned()
        .ok_or_else(|| "dup on empty stack".to_string())?;
    stack.push(value);
    Ok(())
}

/// Descriptor argument kinds: `D`, `F`, `J`, `I` (all int-like), `A` (reference).
fn arg_kinds(desc: &str) -> Result<Vec<char>, String> {
    let open = desc
        .find('(')
        .ok_or_else(|| format!("bad method descriptor {desc}"))?;
    let close = desc
        .rfind(')')
        .ok_or_else(|| format!("bad method descriptor {desc}"))?;
    let body = &desc[open + 1..close];
    let mut kinds = Vec::new();
    let mut chars = body.bytes().peekable();
    while let Some(byte) = chars.next() {
        match byte {
            b'[' => {
                while let Some(next) = chars.next() {
                    if next == b'L' {
                        for inner in chars.by_ref() {
                            if inner == b';' {
                                break;
                            }
                        }
                        break;
                    }
                    if next != b'[' {
                        break;
                    }
                }
                kinds.push('A');
            }
            b'L' => {
                for inner in chars.by_ref() {
                    if inner == b';' {
                        break;
                    }
                }
                kinds.push('A');
            }
            b'D' => kinds.push('D'),
            b'J' => kinds.push('J'),
            b'F' => kinds.push('F'),
            b'Z' | b'B' | b'C' | b'S' | b'I' => kinds.push('I'),
            other => return Err(format!("bad descriptor arg {other} in {desc}")),
        }
    }
    Ok(kinds)
}

fn return_kind(desc: &str) -> Result<char, String> {
    let close = desc
        .rfind(')')
        .ok_or_else(|| format!("bad method descriptor {desc}"))?;
    let byte = desc[close + 1..]
        .bytes()
        .next()
        .ok_or_else(|| format!("bad return descriptor {desc}"))?;
    Ok(match byte {
        b'V' => 'V',
        b'D' | b'J' => 'D',
        b'F' => 'F',
        b'[' | b'L' => 'A',
        b'Z' | b'B' | b'C' | b'S' | b'I' => 'I',
        other => return Err(format!("bad return type {other} in {desc}")),
    })
}

fn pop_args(stack: &mut Vec<Val>, desc: &str) -> Result<Vec<Val>, String> {
    let kinds = arg_kinds(desc)?;
    let mut values = Vec::with_capacity(kinds.len());
    for kind in kinds.iter().rev() {
        let value = pop(stack)?;
        match kind {
            'D' => match &value {
                Val::Double(_) => {}
                other => return Err(format!("expected double arg, got {other:?}")),
            },
            'F' => match &value {
                Val::Float(_) => {}
                other => return Err(format!("expected float arg, got {other:?}")),
            },
            'J' => match &value {
                Val::Long(_) => {}
                other => return Err(format!("expected long arg, got {other:?}")),
            },
            'I' => match &value {
                Val::Int(_) => {}
                other => return Err(format!("expected int arg, got {other:?}")),
            },
            'A' => match &value {
                Val::Null
                | Val::Unknown
                | Val::Str(_)
                | Val::ClassRef(_)
                | Val::Static { .. }
                | Val::Uninit(_)
                | Val::Array
                | Val::BuilderMarker
                | Val::Attr(_) => {}
                other => return Err(format!("expected reference arg, got {other:?}")),
            },
            other => return Err(format!("bad argument kind {other}")),
        }
        values.push(value);
    }
    values.reverse();
    Ok(values)
}

fn ldc_value(class: &ClassFile, index: u16) -> Result<Val, String> {
    match class.constants.get(index as usize) {
        Some(Cp::Integer(value)) => Ok(Val::Int(*value)),
        Some(Cp::Float(value)) => Ok(Val::Float(*value)),
        Some(Cp::String(text_index)) => Ok(Val::Str(class.utf8(*text_index)?.to_owned())),
        Some(Cp::Class(name_index)) => Ok(Val::ClassRef(class.utf8(*name_index)?.to_owned())),
        other => Err(format!(
            "{}: unsupported ldc constant #{index}: {other:?}",
            class.name
        )),
    }
}

fn ldc2_value(class: &ClassFile, index: u16) -> Result<Val, String> {
    match class.constants.get(index as usize) {
        Some(Cp::Long(value)) => Ok(Val::Long(*value)),
        Some(Cp::Double(value)) => Ok(Val::Double(*value)),
        other => Err(format!(
            "{}: unsupported ldc2_w constant #{index}: {other:?}",
            class.name
        )),
    }
}

// ---------------------------------------------------------------------------
// interpreter context
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct AttrDef {
    field: String,
    id: String,
    default: f64,
    min: f64,
    max: f64,
    ranged: bool,
    syncable: bool,
}

#[derive(Clone, Debug)]
struct EntityTypeDef {
    field: String,
    key: String,
    category: String,
    width: f32,
    height: f32,
    eye_height: Option<f32>,
    fire_immune: bool,
}

#[derive(Clone, Debug, Default)]
struct BuilderState {
    /// Attribute field name (e.g. `MAX_HEALTH`) and optional explicit base.
    attrs: Vec<(String, Option<f64>)>,
}

struct Ctx {
    jar: Jar,
    classes: HashMap<String, Arc<ClassFile>>,
    suppliers: HashMap<(String, String), BuilderState>,
}

impl Ctx {
    fn new(jar: Jar) -> Self {
        Self {
            jar,
            classes: HashMap::new(),
            suppliers: HashMap::new(),
        }
    }

    fn class(&mut self, internal_name: &str) -> Result<Arc<ClassFile>, String> {
        if let Some(class) = self.classes.get(internal_name) {
            return Ok(class.clone());
        }
        let data = self.jar.read_class(internal_name)?;
        let class = Arc::new(parse_class(&data)?);
        self.classes.insert(internal_name.to_owned(), class.clone());
        Ok(class)
    }

    fn find_method(
        &mut self,
        mut class_name: String,
        method_name: &str,
    ) -> Result<(String, Method), String> {
        for _ in 0..64 {
            let class = self.class(&class_name)?;
            if let Some(method) = class.methods.iter().find(|m| m.name == method_name) {
                return Ok((class_name, method.clone()));
            }
            match &class.super_name {
                Some(super_name) if super_name != "java/lang/Object" => {
                    class_name = super_name.clone();
                }
                _ => break,
            }
        }
        Err(format!(
            "method {method_name} not found in {class_name} or its superclasses"
        ))
    }

    fn clinit(&mut self, class_name: &str) -> Result<(Arc<ClassFile>, Method), String> {
        let class = self.class(class_name)?;
        let method = class
            .methods
            .iter()
            .find(|m| m.name == "<clinit>")
            .cloned()
            .ok_or_else(|| format!("{class_name}: no <clinit>"))?;
        Ok((class, method))
    }

    // -- entity ids ------------------------------------------------------

    fn entity_type_ids(&mut self) -> Result<HashMap<String, String>, String> {
        let (class, method) = self.clinit(ENTITY_TYPE_IDS)?;
        let code = method
            .code
            .ok_or_else(|| format!("{ENTITY_TYPE_IDS}.<clinit> has no code"))?;
        let mut ids = HashMap::new();
        let mut pending: Option<String> = None;
        let mut pc = 0usize;
        loop {
            let instr = decode(&code, &mut pc)?;
            match instr {
                Instr::Nop => {}
                Instr::Ldc(index) => {
                    if let Val::Str(text) = ldc_value(&class, index)? {
                        pending = Some(text);
                    }
                }
                Instr::Putstatic(index) => {
                    let (_, name, _) = class.field_ref(index)?;
                    if let Some(value) = pending.take() {
                        ids.insert(name.to_owned(), value);
                    }
                }
                Instr::Invokestatic(_) => {}
                Instr::Return => break,
                other => {
                    return Err(format!(
                        "{ENTITY_TYPE_IDS}.<clinit>: unexpected instruction {other:?}"
                    ))
                }
            }
        }
        Ok(ids)
    }

    // -- mob categories --------------------------------------------------

    fn mob_categories(&mut self) -> Result<HashMap<String, String>, String> {
        let (class, method) = self.clinit(MOB_CATEGORY)?;
        let code = method
            .code
            .ok_or_else(|| format!("{MOB_CATEGORY}.<clinit> has no code"))?;
        let mut categories = HashMap::new();
        let mut strings: Vec<String> = Vec::new();
        let mut pc = 0usize;
        loop {
            let instr = decode(&code, &mut pc)?;
            match instr {
                Instr::Ldc(index) => {
                    if let Val::Str(text) = ldc_value(&class, index)? {
                        strings.push(text);
                    }
                }
                Instr::Putstatic(index) => {
                    let (_, field, _) = class.field_ref(index)?;
                    // MobCategory(String enumName, int ordinal, String name, ...)
                    if let Some(name) = strings.get(1) {
                        categories.insert(field.to_owned(), name.clone());
                    }
                    strings.clear();
                }
                Instr::Return => break,
                _ => {}
            }
        }
        Ok(categories)
    }

    // -- entity type registrations --------------------------------------

    fn entity_types(
        &mut self,
        ids: &HashMap<String, String>,
    ) -> Result<Vec<EntityTypeDef>, String> {
        let (class, method) = self.clinit(ENTITY_TYPES)?;
        let code = method
            .code
            .ok_or_else(|| format!("{ENTITY_TYPES}.<clinit> has no code"))?;
        let mut stack: Vec<Val> = Vec::new();
        let mut entities: Vec<EntityTypeDef> = Vec::new();
        let mut current: Option<EntityTypeDef> = None;
        let mut pc = 0usize;
        loop {
            let instr = decode(&code, &mut pc)?;
            match instr {
                Instr::Nop => {}
                Instr::AconstNull => push(&mut stack, Val::Null),
                Instr::Int(value) => push(&mut stack, Val::Int(value)),
                Instr::Long(value) => push(&mut stack, Val::Long(value)),
                Instr::Float(value) => push(&mut stack, Val::Float(value)),
                Instr::Double(value) => push(&mut stack, Val::Double(value)),
                Instr::Ldc(index) => {
                    let value = ldc_value(&class, index)?;
                    push(&mut stack, value);
                }
                Instr::Ldc2(index) => {
                    let value = ldc2_value(&class, index)?;
                    push(&mut stack, value);
                }
                Instr::Bipush(value) => push(&mut stack, Val::Int(value)),
                Instr::Sipush(value) => push(&mut stack, Val::Int(value)),
                Instr::Pop => {
                    pop(&mut stack)?;
                }
                Instr::Pop2 => {
                    pop(&mut stack)?;
                    pop(&mut stack)?;
                }
                Instr::Dup => duplicate(&mut stack)?,
                Instr::ArrayStore(_) => {
                    pop(&mut stack)?;
                    pop(&mut stack)?;
                    pop(&mut stack)?;
                }
                Instr::Getstatic(index) => {
                    let (owner, name, desc) = class.field_ref(index)?;
                    if owner == ENTITY_TYPE_IDS {
                        let id = ids.get(name).ok_or_else(|| {
                            format!("{ENTITY_TYPE_IDS}.{name} has no registered id")
                        })?;
                        // Vanilla keys are `ResourceKey`s, but every
                        // registration below is `minecraft:<id>`.
                        push(&mut stack, Val::Str(id.clone()));
                    } else {
                        push(
                            &mut stack,
                            Val::Static {
                                owner: owner.to_owned(),
                                name: name.to_owned(),
                                desc: desc.to_owned(),
                            },
                        );
                    }
                }
                Instr::Putstatic(index) => {
                    let (_, field, _) = class.field_ref(index)?;
                    let value = pop(&mut stack)?;
                    if ids.contains_key(field) {
                        let mut entity = current.take().ok_or_else(|| {
                            format!("{ENTITY_TYPES}.{field}: no builder was registered")
                        })?;
                        if !matches!(value, Val::BuilderMarker) {
                            return Err(format!(
                                "{ENTITY_TYPES}.{field}: expected builder, got {value:?}"
                            ));
                        }
                        entity.field = field.to_owned();
                        if entity.key.is_empty() {
                            return Err(format!("{ENTITY_TYPES}.{field}: no id was registered"));
                        }
                        entities.push(entity);
                    }
                }
                Instr::Invokedynamic(_) => push(&mut stack, Val::Unknown),
                Instr::New(index) => {
                    let name = class.class_name(index)?.to_owned();
                    push(&mut stack, Val::Uninit(name));
                }
                Instr::NewArray | Instr::ANewArray(_) => {
                    pop(&mut stack)?;
                    push(&mut stack, Val::Array);
                }
                Instr::Invokespecial(index) => {
                    let (_, _, desc) = class.method_ref(index)?;
                    pop_args(&mut stack, desc)?;
                    pop(&mut stack)?; // receiver
                }
                Instr::Invokestatic(index) => {
                    let (owner, name, desc) = class.method_ref(index)?;
                    let args = pop_args(&mut stack, desc)?;
                    match (owner, name) {
                        (ENTITY_TYPE_BUILDER, "of") => {
                            let category = args
                                .get(1)
                                .ok_or_else(|| {
                                    "EntityType$Builder.of: missing category".to_string()
                                })?
                                .as_static_name()?
                                .to_owned();
                            current = Some(EntityTypeDef {
                                field: String::new(),
                                key: String::new(),
                                category,
                                width: 0.0,
                                height: 0.0,
                                eye_height: None,
                                fire_immune: false,
                            });
                            push(&mut stack, Val::BuilderMarker);
                        }
                        (ENTITY_TYPE_BUILDER, "createNothing") => {
                            let category = args
                                .first()
                                .ok_or_else(|| {
                                    "EntityType$Builder.createNothing: missing category".to_string()
                                })?
                                .as_static_name()?
                                .to_owned();
                            current = Some(EntityTypeDef {
                                field: String::new(),
                                key: String::new(),
                                category,
                                width: 0.0,
                                height: 0.0,
                                eye_height: None,
                                fire_immune: false,
                            });
                            push(&mut stack, Val::BuilderMarker);
                        }
                        (_, "register") if desc.contains("EntityType$Builder") => {
                            let key = args
                                .first()
                                .ok_or_else(|| "register: missing key".to_string())?
                                .as_str()?
                                .to_owned();
                            let mut entity = current.take().ok_or_else(|| {
                                format!("register: no current entity type for key {key}")
                            })?;
                            entity.key = key;
                            current = Some(entity);
                            push(&mut stack, Val::BuilderMarker);
                        }
                        _ => push(&mut stack, Val::Unknown),
                    }
                }
                Instr::Invokevirtual(index) => {
                    let (owner, name, desc) = class.method_ref(index)?;
                    let args = pop_args(&mut stack, desc)?;
                    pop(&mut stack)?; // receiver
                    if owner == ENTITY_TYPE_BUILDER {
                        let entity = current.as_mut().ok_or_else(|| {
                            format!("EntityType$Builder.{name}: no current entity type")
                        })?;
                        match name {
                            "sized" => {
                                entity.width = args
                                    .first()
                                    .ok_or_else(|| "sized: missing width".to_string())?
                                    .as_f32()?;
                                entity.height = args
                                    .get(1)
                                    .ok_or_else(|| "sized: missing height".to_string())?
                                    .as_f32()?;
                            }
                            "eyeHeight" => {
                                entity.eye_height = Some(
                                    args.first()
                                        .ok_or_else(|| "eyeHeight: missing height".to_string())?
                                        .as_f32()?,
                                );
                            }
                            "fireImmune" => entity.fire_immune = true,
                            _ => {}
                        }
                        push(&mut stack, Val::BuilderMarker);
                    } else {
                        push(&mut stack, Val::Unknown);
                    }
                }
                Instr::Invokeinterface(index) => {
                    let (_, _, desc) = class.method_ref(index)?;
                    pop_args(&mut stack, desc)?;
                    pop(&mut stack)?;
                    push(&mut stack, Val::Unknown);
                }
                Instr::Checkcast(_) => {}
                Instr::Return => break,
                other => {
                    return Err(format!(
                        "{ENTITY_TYPES}.<clinit>: unsupported instruction {other:?}"
                    ))
                }
            }
        }
        Ok(entities)
    }

    // -- attribute definitions ------------------------------------------

    fn attribute_defs(&mut self) -> Result<Vec<AttrDef>, String> {
        let (class, method) = self.clinit(ATTRIBUTES)?;
        let code = method
            .code
            .ok_or_else(|| format!("{ATTRIBUTES}.<clinit> has no code"))?;
        let mut stack: Vec<Val> = Vec::new();
        let mut defs: Vec<AttrDef> = Vec::new();
        let mut pc = 0usize;
        loop {
            let instr = decode(&code, &mut pc)?;
            match instr {
                Instr::Nop => {}
                Instr::Int(value) => push(&mut stack, Val::Int(value)),
                Instr::Long(value) => push(&mut stack, Val::Long(value)),
                Instr::Float(value) => push(&mut stack, Val::Float(value)),
                Instr::Double(value) => push(&mut stack, Val::Double(value)),
                Instr::Ldc(index) => {
                    let value = ldc_value(&class, index)?;
                    push(&mut stack, value);
                }
                Instr::Ldc2(index) => {
                    let value = ldc2_value(&class, index)?;
                    push(&mut stack, value);
                }
                Instr::Bipush(value) => push(&mut stack, Val::Int(value)),
                Instr::Sipush(value) => push(&mut stack, Val::Int(value)),
                Instr::New(index) => {
                    let name = class.class_name(index)?.to_owned();
                    push(&mut stack, Val::Uninit(name));
                }
                Instr::Getstatic(_) => push(&mut stack, Val::Unknown),
                Instr::Dup => duplicate(&mut stack)?,
                Instr::Invokespecial(index) => {
                    let (owner, name, desc) = class.method_ref(index)?;
                    if name != "<init>" {
                        return Err(format!(
                            "{ATTRIBUTES}.<clinit>: unexpected invokespecial {owner}.{name}"
                        ));
                    }
                    // RangedAttribute(String descriptionId, double default,
                    //                 double min, double max)
                    let args = pop_args(&mut stack, desc)?;
                    if args.len() != 4 {
                        return Err(format!(
                            "{ATTRIBUTES}.<clinit>: attribute constructor takes {} args",
                            args.len()
                        ));
                    }
                    let default = args[1].as_f64()?;
                    let min = args[2].as_f64()?;
                    let max = args[3].as_f64()?;
                    pop(&mut stack)?; // duplicated uninitialised object
                    pop(&mut stack)?; // the object itself
                    let ranged = owner == RANGED_ATTRIBUTE || owner.ends_with("RangedAttribute");
                    push(
                        &mut stack,
                        Val::Attr(AttrState {
                            id: None,
                            default,
                            min,
                            max,
                            ranged,
                            syncable: false,
                        }),
                    );
                }
                Instr::Invokevirtual(index) => {
                    let (owner, name, desc) = class.method_ref(index)?;
                    let args = pop_args(&mut stack, desc)?;
                    let receiver = pop(&mut stack)?;
                    match receiver {
                        Val::Attr(mut attr)
                            if name == "setSyncable" && owner.ends_with("Attribute") =>
                        {
                            attr.syncable =
                                args.first().map(Val::as_int).transpose()?.unwrap_or(1) != 0;
                            push(&mut stack, Val::Attr(attr));
                        }
                        other => push(&mut stack, other),
                    }
                }
                Instr::Invokestatic(index) => {
                    let (_, name, desc) = class.method_ref(index)?;
                    let args = pop_args(&mut stack, desc)?;
                    if name == "register" {
                        let mut attribute = match args.get(1) {
                            Some(Val::Attr(attr)) => attr.clone(),
                            other => {
                                return Err(format!(
                                "{ATTRIBUTES}.<clinit>: register without attribute, got {other:?}"
                            ))
                            }
                        };
                        let id = args
                            .first()
                            .ok_or_else(|| "register: missing id".to_string())?
                            .as_str()?;
                        attribute.id = Some(format!("minecraft:{id}"));
                        push(&mut stack, Val::Attr(attribute));
                    } else {
                        push(&mut stack, Val::Unknown);
                    }
                }
                Instr::Putstatic(index) => {
                    let (_, field, _) = class.field_ref(index)?;
                    let value = pop(&mut stack)?;
                    if let Val::Attr(attr) = value {
                        let id = attr.id.ok_or_else(|| {
                            format!("{ATTRIBUTES}.{field}: attribute has no registry id")
                        })?;
                        defs.push(AttrDef {
                            field: field.to_owned(),
                            id,
                            default: attr.default,
                            min: attr.min,
                            max: attr.max,
                            ranged: attr.ranged,
                            syncable: attr.syncable,
                        });
                    }
                }
                Instr::Return => break,
                other => {
                    return Err(format!(
                        "{ATTRIBUTES}.<clinit>: unsupported instruction {other:?}"
                    ))
                }
            }
        }
        Ok(defs)
    }

    // -- default attribute suppliers ------------------------------------

    fn default_attribute_pairs(&mut self) -> Result<Vec<(String, String, String)>, String> {
        let (class, method) = self.clinit(DEFAULT_ATTRIBUTES)?;
        let code = method
            .code
            .ok_or_else(|| format!("{DEFAULT_ATTRIBUTES}.<clinit> has no code"))?;
        let mut pairs = Vec::new();
        let mut pending: Option<String> = None;
        let mut pc = 0usize;
        loop {
            let instr = decode(&code, &mut pc)?;
            match instr {
                Instr::Nop => {}
                Instr::Getstatic(index) => {
                    let (owner, name, _) = class.field_ref(index)?;
                    pending = if owner == ENTITY_TYPES {
                        Some(name.to_owned())
                    } else {
                        None
                    };
                }
                Instr::Invokestatic(index) => {
                    if let Some(field) = pending.take() {
                        let (owner, name, desc) = class.method_ref(index)?;
                        if desc.contains("AttributeSupplier$Builder") {
                            pairs.push((field, owner.to_owned(), name.to_owned()));
                        } else {
                            return Err(format!(
                                "{DEFAULT_ATTRIBUTES}.<clinit>: {field} is not backed by an attribute supplier ({owner}.{name})"
                            ));
                        }
                    }
                }
                Instr::Return => break,
                _ => {
                    pending = None;
                }
            }
        }
        Ok(pairs)
    }

    // -- supplier evaluation --------------------------------------------

    fn eval_supplier(
        &mut self,
        class_name: &str,
        method_name: &str,
    ) -> Result<BuilderState, String> {
        let (declaring, method) = self.find_method(class_name.to_owned(), method_name)?;
        let cache_key = (declaring.clone(), method_name.to_owned());
        if let Some(state) = self.suppliers.get(&cache_key) {
            return Ok(state.clone());
        }
        let code = method
            .code
            .ok_or_else(|| format!("{declaring}.{method_name} has no code"))?;
        let class = self.class(&declaring)?;
        let mut state = BuilderState::default();
        let mut stack: Vec<Val> = Vec::new();
        let mut pc = 0usize;
        loop {
            let instr = decode(&code, &mut pc)?;
            match instr {
                Instr::Nop => {}
                Instr::AconstNull => push(&mut stack, Val::Null),
                Instr::Int(value) => push(&mut stack, Val::Int(value)),
                Instr::Long(value) => push(&mut stack, Val::Long(value)),
                Instr::Float(value) => push(&mut stack, Val::Float(value)),
                Instr::Double(value) => push(&mut stack, Val::Double(value)),
                Instr::Ldc(index) => {
                    let value = ldc_value(&class, index)?;
                    push(&mut stack, value);
                }
                Instr::Ldc2(index) => {
                    let value = ldc2_value(&class, index)?;
                    push(&mut stack, value);
                }
                Instr::Bipush(value) => push(&mut stack, Val::Int(value)),
                Instr::Sipush(value) => push(&mut stack, Val::Int(value)),
                Instr::Pop => {
                    pop(&mut stack)?;
                }
                Instr::Pop2 => {
                    pop(&mut stack)?;
                    pop(&mut stack)?;
                }
                Instr::Dup => duplicate(&mut stack)?,
                Instr::Getstatic(index) => {
                    let (owner, name, desc) = class.field_ref(index)?;
                    if owner == ATTRIBUTES {
                        push(
                            &mut stack,
                            Val::Static {
                                owner: owner.to_owned(),
                                name: name.to_owned(),
                                desc: desc.to_owned(),
                            },
                        );
                    } else {
                        push(&mut stack, Val::Unknown);
                    }
                }
                Instr::Invokestatic(index) => {
                    let (owner, name, desc) = class.method_ref(index)?;
                    if owner == ATTRIBUTE_SUPPLIER && name == "builder" {
                        state = BuilderState::default();
                        push(&mut stack, Val::Unknown);
                    } else if return_kind(desc)? == 'A'
                        && desc.contains("AttributeSupplier$Builder")
                    {
                        pop_args(&mut stack, desc)?;
                        let sub = self.eval_supplier(owner, name)?;
                        if !state.attrs.is_empty() {
                            return Err(format!(
                                "{declaring}.{method_name}: second base builder call to {owner}.{name}"
                            ));
                        }
                        state = sub;
                        push(&mut stack, Val::Unknown);
                    } else {
                        pop_args(&mut stack, desc)?;
                        push(&mut stack, Val::Unknown);
                    }
                }
                Instr::Invokevirtual(index) => {
                    let (owner, name, desc) = class.method_ref(index)?;
                    let args = pop_args(&mut stack, desc)?;
                    pop(&mut stack)?; // receiver
                    match name {
                        "add" if owner == ATTRIBUTE_BUILDER => {
                            let attribute = args
                                .first()
                                .ok_or_else(|| "add: missing attribute".to_string())?
                                .as_static_name()?
                                .to_owned();
                            let base = match args.get(1) {
                                None => None,
                                Some(value) => Some(value.as_f64()?),
                            };
                            state.attrs.push((attribute, base));
                        }
                        "build" => {}
                        other => {
                            return Err(format!(
                            "{declaring}.{method_name}: unsupported virtual call {owner}.{other}"
                        ))
                        }
                    }
                    push(&mut stack, Val::Unknown);
                }
                Instr::Invokeinterface(index) => {
                    let (_, _, desc) = class.method_ref(index)?;
                    pop_args(&mut stack, desc)?;
                    pop(&mut stack)?;
                    push(&mut stack, Val::Unknown);
                }
                Instr::I2D => {
                    let value = pop(&mut stack)?.as_int()?;
                    push(&mut stack, Val::Double(value as f64));
                }
                Instr::I2F => {
                    let value = pop(&mut stack)?.as_int()?;
                    push(&mut stack, Val::Float(value as f32));
                }
                Instr::I2L => {
                    let value = pop(&mut stack)?.as_int()?;
                    push(&mut stack, Val::Long(value as i64));
                }
                Instr::F2D => {
                    let value = pop(&mut stack)?.as_f32()?;
                    push(&mut stack, Val::Double(value as f64));
                }
                Instr::Fneg => {
                    let value = pop(&mut stack)?.as_f32()?;
                    push(&mut stack, Val::Float(-value));
                }
                Instr::Dneg => {
                    let value = pop(&mut stack)?.as_f64()?;
                    push(&mut stack, Val::Double(-value));
                }
                Instr::Areturn => {
                    self.suppliers.insert(cache_key, state.clone());
                    return Ok(state);
                }
                Instr::Return => {
                    return Err(format!(
                        "{declaring}.{method_name}: attribute supplier method returned void"
                    ))
                }
                other => {
                    return Err(format!(
                        "{declaring}.{method_name}: unsupported instruction {other:?}"
                    ))
                }
            }
        }
    }

    fn suppliers_for(
        &mut self,
        pairs: &[(String, String, String)],
    ) -> Result<HashMap<String, BuilderState>, String> {
        let mut suppliers = HashMap::new();
        for (field, owner, method) in pairs {
            let state = self.eval_supplier(owner, method)?;
            suppliers.insert(field.clone(), state);
        }
        Ok(suppliers)
    }

    fn data_version(&mut self) -> Result<u64, String> {
        let raw = self.jar.read("version.json")?;
        let value: Value =
            serde_json::from_slice(&raw).map_err(|e| format!("version.json: {e}"))?;
        value
            .get("world_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| "version.json: missing world_version".to_string())
    }
}

// ---------------------------------------------------------------------------
// JSON assembly
// ---------------------------------------------------------------------------

fn number(value: f64) -> Result<Value, String> {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| format!("non-finite number {value}"))
}

/// Serialise a vanilla `float` the way Gson does: the shortest decimal string
/// that round-trips through `f32`, parsed back as a JSON number.
fn float_number(value: f32) -> Result<Value, String> {
    if !value.is_finite() {
        return Err(format!("non-finite float {value}"));
    }
    let text = format!("{value}");
    let as_f64: f64 = text.parse().map_err(|e| format!("float {text}: {e}"))?;
    number(as_f64)
}

fn bits(value: f64) -> String {
    format!("{:016x}", value.to_bits())
}

fn build_entity_types(
    entities: &[EntityTypeDef],
    categories: &HashMap<String, String>,
    attrs: &[AttrDef],
    suppliers: &HashMap<String, BuilderState>,
) -> Result<Value, String> {
    let field_to_id: HashMap<&str, &str> = attrs
        .iter()
        .map(|def| (def.field.as_str(), def.id.as_str()))
        .collect();
    let defs_by_id: HashMap<&str, &AttrDef> =
        attrs.iter().map(|def| (def.id.as_str(), def)).collect();

    let mut root = Map::new();
    for entity in entities {
        let mut object = Map::new();
        object.insert("width".into(), float_number(entity.width)?);
        object.insert("height".into(), float_number(entity.height)?);
        let eye_height = entity.eye_height.unwrap_or(entity.height * 0.85f32);
        object.insert("eye_height".into(), float_number(eye_height)?);
        let category = categories.get(&entity.category).ok_or_else(|| {
            format!(
                "entity {}: unknown mob category field {}",
                entity.key, entity.category
            )
        })?;
        object.insert("category".into(), Value::String(category.clone()));
        object.insert("fire_immune".into(), Value::Bool(entity.fire_immune));
        if let Some(state) = suppliers.get(&entity.field) {
            // AttributeSupplier.Builder.buildKeepingLast: the last add wins.
            let mut bases: HashMap<&str, f64> = HashMap::new();
            for (field, base) in &state.attrs {
                let id = field_to_id.get(field.as_str()).ok_or_else(|| {
                    format!(
                        "entity {}: attribute {field} is not in the vanilla attribute registry",
                        entity.key
                    )
                })?;
                let value = match base {
                    Some(value) => *value,
                    None => {
                        let def = defs_by_id.get(id).ok_or_else(|| {
                            format!("entity {}: attribute {id} has no definition", entity.key)
                        })?;
                        def.default
                    }
                };
                bases.insert(id, value);
            }
            let mut attributes = Map::new();
            // Registry order, matching the exporter's `asHolderIdMap` loop.
            for def in attrs {
                if let Some(value) = bases.get(def.id.as_str()) {
                    attributes.insert(def.id.clone(), number(*value)?);
                }
            }
            object.insert("attributes".into(), Value::Object(attributes));
        }
        root.insert(format!("minecraft:{}", entity.key), Value::Object(object));
    }
    Ok(Value::Object(root))
}

fn build_attributes(
    data_version: u64,
    attrs: &[AttrDef],
    entities: &[EntityTypeDef],
    pairs: &[(String, String, String)],
    suppliers: &HashMap<String, BuilderState>,
) -> Result<Value, String> {
    let field_to_id: HashMap<&str, &str> = attrs
        .iter()
        .map(|def| (def.field.as_str(), def.id.as_str()))
        .collect();
    let defs_by_id: HashMap<&str, &AttrDef> =
        attrs.iter().map(|def| (def.id.as_str(), def)).collect();
    let key_by_field: HashMap<&str, &str> = entities
        .iter()
        .map(|entity| (entity.field.as_str(), entity.key.as_str()))
        .collect();

    let mut definitions = Vec::with_capacity(attrs.len());
    for def in attrs {
        let mut object = Map::new();
        object.insert("id".into(), Value::String(def.id.clone()));
        object.insert("default".into(), Value::String(bits(def.default)));
        object.insert("syncable".into(), Value::Bool(def.syncable));
        if def.ranged {
            object.insert("min".into(), Value::String(bits(def.min)));
            object.insert("max".into(), Value::String(bits(def.max)));
        }
        definitions.push(Value::Object(object));
    }

    let mut entity_object = Map::new();
    for (field, _, _) in pairs {
        let key = key_by_field.get(field.as_str()).ok_or_else(|| {
            format!("supplier for {field} has no matching entity type registration")
        })?;
        let state = suppliers
            .get(field)
            .ok_or_else(|| format!("supplier for {field} was not evaluated"))?;
        // AttributeSupplier.Builder.buildKeepingLast: the last add wins.
        let mut bases: HashMap<&str, f64> = HashMap::new();
        for (attribute, base) in &state.attrs {
            let id = field_to_id.get(attribute.as_str()).ok_or_else(|| {
                format!(
                    "entity {key}: attribute {attribute} is not in the vanilla attribute registry"
                )
            })?;
            let value = match base {
                Some(value) => *value,
                None => {
                    let def = defs_by_id
                        .get(id)
                        .ok_or_else(|| format!("entity {key}: attribute {id} has no definition"))?;
                    def.default
                }
            };
            bases.insert(id, value);
        }
        let mut attributes = Map::new();
        for def in attrs {
            if let Some(value) = bases.get(def.id.as_str()) {
                attributes.insert(def.id.clone(), Value::String(bits(*value)));
            }
        }
        entity_object.insert(format!("minecraft:{key}"), Value::Object(attributes));
    }

    let mut root = Map::new();
    root.insert(
        "data_version".into(),
        Value::Number(Number::from(data_version)),
    );
    root.insert("attributes".into(), Value::Array(definitions));
    root.insert("entities".into(), Value::Object(entity_object));
    Ok(Value::Object(root))
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

/// Build both derived tables from the player's jar.
pub fn generate(jar_path: impl AsRef<Path>) -> Result<(Value, Value), String> {
    let jar = Jar::open(jar_path)?;
    let mut ctx = Ctx::new(jar);

    let data_version = ctx.data_version()?;
    let ids = ctx.entity_type_ids()?;
    let categories = ctx.mob_categories()?;
    let entities = ctx.entity_types(&ids)?;
    if entities.len() != ids.len() {
        let missing: Vec<&String> = ids
            .iter()
            .filter(|(field, _)| !entities.iter().any(|entity| &entity.field == *field))
            .map(|(field, _)| field)
            .collect();
        return Err(format!(
            "entity type registrations: parsed {} of {} ids; missing {missing:?}",
            entities.len(),
            ids.len()
        ));
    }
    let attrs = ctx.attribute_defs()?;
    let pairs = ctx.default_attribute_pairs()?;
    let suppliers = ctx.suppliers_for(&pairs)?;

    let entity_types = build_entity_types(&entities, &categories, &attrs, &suppliers)?;
    let attributes = build_attributes(data_version, &attrs, &entities, &pairs, &suppliers)?;
    Ok((entity_types, attributes))
}

pub fn export_entity_types(jar_path: impl AsRef<Path>) -> Result<Value, String> {
    generate(jar_path).map(|(entity_types, _)| entity_types)
}

pub fn export_attributes(jar_path: impl AsRef<Path>) -> Result<Value, String> {
    generate(jar_path).map(|(_, attributes)| attributes)
}

pub fn write_entity_types(jar_path: impl AsRef<Path>, out: impl AsRef<Path>) -> Result<(), String> {
    let value = export_entity_types(jar_path)?;
    write_pretty(&value, out)
}

pub fn write_attributes(jar_path: impl AsRef<Path>, out: impl AsRef<Path>) -> Result<(), String> {
    let value = export_attributes(jar_path)?;
    write_pretty(&value, out)
}

fn write_pretty(value: &Value, out: impl AsRef<Path>) -> Result<(), String> {
    let out = out.as_ref();
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(out, text + "\n").map_err(|e| format!("{}: {e}", out.display()))
}
