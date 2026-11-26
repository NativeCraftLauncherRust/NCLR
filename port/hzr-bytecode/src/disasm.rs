//! A small JVM disassembler, enough to read a constructor.

use crate::classfile::{self, ClassFile, Constant};

/// One decoded instruction.
#[derive(Debug, Clone)]
pub struct Instruction {
    pub offset: usize,
    pub opcode: u8,
    pub name: &'static str,
    /// The constant-pool index when the opcode names one.
    pub index: Option<usize>,
    /// The immediate value for the narrow push opcodes.
    pub immediate: Option<i32>,
    pub length: usize,
}

fn opcode_name(opcode: u8) -> &'static str {
    match opcode {
        0x00 => "nop",
        0x01 => "aconst_null",
        0x02..=0x08 => "iconst",
        0x09..=0x0a => "lconst",
        0x0b..=0x0d => "fconst",
        0x0e..=0x0f => "dconst",
        0x10 => "bipush",
        0x11 => "sipush",
        0x12 => "ldc",
        0x13 => "ldc_w",
        0x14 => "ldc2_w",
        0x15..=0x19 => "load",
        0x1a..=0x2d => "load_n",
        0x2e..=0x35 => "aload",
        0x36..=0x3a => "store",
        0x3b..=0x4e => "store_n",
        0x4f..=0x56 => "astore",
        0x57 => "pop",
        0x58 => "pop2",
        0x59 => "dup",
        0x5a => "dup_x1",
        0x5b => "dup_x2",
        0x5c => "dup2",
        0x5f => "swap",
        0x60..=0x63 => "add",
        0x64..=0x67 => "sub",
        0x68..=0x6b => "mul",
        0x6c..=0x6f => "div",
        0x70..=0x73 => "rem",
        0x74 => "neg",
        0x78..=0x7d => "shift",
        0x7e..=0x83 => "and/or/xor",
        0x84 => "iinc",
        0x85..=0x93 => "cast",
        0x94..=0x98 => "cmp",
        0x99..=0x9e => "if",
        0xa0..=0xa4 => "if_icmp",
        0xa5..=0xa6 => "if_acmp",
        0xa7 => "goto",
        0xa8 => "jsr",
        0xa9 => "ret",
        0xaa => "tableswitch",
        0xab => "lookupswitch",
        0xac..=0xb0 => "return",
        0xb1 => "return",
        0xb2 => "getstatic",
        0xb3 => "putstatic",
        0xb4 => "getfield",
        0xb5 => "putfield",
        0xb6 => "invokevirtual",
        0xb7 => "invokespecial",
        0xb8 => "invokestatic",
        0xb9 => "invokeinterface",
        0xba => "invokedynamic",
        0xbb => "new",
        0xbc => "newarray",
        0xbd => "anewarray",
        0xbe => "arraylength",
        0xbf => "athrow",
        0xc0 => "checkcast",
        0xc1 => "instanceof",
        0xc2..=0xc3 => "monitor",
        0xc4 => "wide",
        0xc5 => "multianewarray",
        0xc6..=0xc7 => "ifnull",
        0xc8..=0xc9 => "goto_w",
        _ => "?",
    }
}

/// Decode the instructions of a method body.
pub fn disassemble(class: &ClassFile, bytes: &[u8]) -> Vec<Instruction> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let opcode = bytes[at];
        let start = at;
        at += 1;
        let mut index = None;
        let mut immediate = None;
        let length = match opcode {
            0x12 => {
                index = bytes.get(at).map(|b| *b as usize);
                at += 1;
                2
            }
            0x13 | 0x14 => {
                index = Some(u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize);
                at += 2;
                3
            }
            0x10 => {
                immediate = Some(bytes[at] as i8 as i32);
                at += 1;
                2
            }
            0x11 => {
                immediate = Some(i16::from_be_bytes([bytes[at], bytes[at + 1]]) as i32);
                at += 2;
                3
            }
            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = Some(u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize);
                at += 2;
                3
            }
            0xb9 | 0xba => {
                index = Some(u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize);
                at += 4;
                5
            }
            0x84 => {
                at += 2;
                3
            }
            0x99..=0xa8 | 0xc6..=0xc7 => {
                immediate = Some(i16::from_be_bytes([bytes[at], bytes[at + 1]]) as i32);
                at += 2;
                3
            }
            0x15..=0x19 | 0x36..=0x3a | 0xbc | 0xa9 => {
                at += 1;
                2
            }
            0xaa | 0xab => break,
            0xc8..=0xc9 => {
                at += 4;
                5
            }
            0xc5 => {
                at += 3;
                4
            }
            _ => 1,
        };
        let _ = class;
        let _ = &mut index;
        out.push(Instruction {
            offset: start,
            opcode,
            name: opcode_name(opcode),
            index,
            immediate,
            length,
        });
        at = start + length.max(1);
    }
    out
}

/// A readable rendering of one instruction, for a dump.
pub fn describe(class: &ClassFile, instruction: &Instruction) -> String {
    let mut text = instruction.name.to_string();
    if let Some(index) = instruction.index {
        text.push_str(&format!(" #{index}"));
        match class.constants.get(index) {
            Some(Constant::String(_)) | Some(Constant::Utf8(_)) => {
                if let Some(value) = classfile::string_constant(&class.constants, index) {
                    text.push_str(&format!(" // {value:?}"));
                }
            }
            Some(Constant::Methodref { .. }) | Some(Constant::InterfaceMethodref { .. }) => {
                if let Some((class_name, name, descriptor)) =
                    classfile::method_ref(&class.constants, index)
                {
                    text.push_str(&format!(" // {class_name}.{name}{descriptor}"));
                }
            }
            Some(Constant::Fieldref { .. }) => {
                if let Some((owner, name, descriptor)) =
                    classfile::field_ref(&class.constants, index)
                {
                    text.push_str(&format!(" // {owner}.{name}:{descriptor}"));
                }
            }
            Some(Constant::Float(value)) => text.push_str(&format!(" // {value:?}f")),
            Some(Constant::Integer(value)) => text.push_str(&format!(" // {value}")),
            Some(Constant::Double(value)) => text.push_str(&format!(" // {value:?}d")),
            Some(Constant::Long(value)) => text.push_str(&format!(" // {value}L")),
            Some(Constant::Class(_)) => {
                if let Some(Constant::Class(reference)) = class.constants.get(index) {
                    let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
                    if let Some(Constant::Utf8(name)) = class.constants.get(inner) {
                        text.push_str(&format!(" // {name}"));
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(immediate) = instruction.immediate {
        text.push_str(&format!(" {immediate}"));
    }
    text
}
