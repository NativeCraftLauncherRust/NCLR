//! A Java class file reader: enough of it to read what a block is made of.
//!
//! The constants that `gameplay-facts.json` carries — hardness, light, sound,
//! the map colour — are literals in the constructors of `Blocks`. They live in
//! the player's own `client.jar` as bytecode, so nothing has to be shipped to
//! read them: this crate reads the class files the player already has.
//!
//! Only the JVM subset a constructor uses is decoded.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Utf8(String),
    Integer(i32),
    Float(f32),
    Long(i64),
    Double(f64),
    Class(String),
    String(String),
    Fieldref {
        class: String,
        name: String,
        kind: String,
    },
    Methodref {
        class: String,
        name: String,
        kind: String,
    },
    InterfaceMethodref {
        class: String,
        name: String,
        kind: String,
    },
    NameAndType {
        name: String,
        kind: String,
    },
    MethodHandle {
        reference: usize,
    },
    MethodType(String),
    Dynamic {
        name: String,
        kind: String,
    },
    InvokeDynamic {
        name: String,
        kind: String,
        bootstrap: usize,
    },
    Module(String),
    Package(String),
    Unused,
}

#[derive(Debug, Clone, Default)]
pub struct Member {
    pub access: u16,
    pub name: String,
    pub descriptor: String,
    pub code: Option<Code>,
}

#[derive(Debug, Clone, Default)]
pub struct Code {
    pub max_stack: u16,
    pub max_locals: u16,
    pub bytes: Vec<u8>,
}

/// One row of the `BootstrapMethods` attribute: the bootstrap and its arguments.
#[derive(Debug, Clone)]
pub struct Bootstrap {
    pub method: usize,
    pub arguments: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ClassFile {
    pub name: String,
    pub super_name: String,
    pub constants: Vec<Constant>,
    pub fields: Vec<Member>,
    pub methods: Vec<Member>,
    pub bootstraps: Vec<Bootstrap>,
}

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u1(&mut self) -> Result<u8, Error> {
        if self.at >= self.bytes.len() {
            return Err(Error("truncated".into()));
        }
        let value = self.bytes[self.at];
        self.at += 1;
        Ok(value)
    }

    fn u2(&mut self) -> Result<u16, Error> {
        Ok(((self.u1()? as u16) << 8) | self.u1()? as u16)
    }

    fn u4(&mut self) -> Result<u32, Error> {
        Ok(((self.u2()? as u32) << 16) | self.u2()? as u32)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if self.at + count > self.bytes.len() {
            return Err(Error("truncated".into()));
        }
        let slice = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(slice)
    }

    fn skip(&mut self, count: usize) -> Result<(), Error> {
        self.take(count).map(|_| ())
    }
}

/// Parse a class file.
pub fn parse(bytes: &[u8]) -> Result<ClassFile, Error> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.u4()? != 0xCAFE_BABE {
        return Err(Error("not a class file".into()));
    }
    let _minor = reader.u2()?;
    let _major = reader.u2()?;

    let count = reader.u2()? as usize;
    let mut constants = vec![Constant::Unused; count];
    let mut index = 1;
    while index < count {
        let tag = reader.u1()?;
        constants[index] = match tag {
            1 => {
                let length = reader.u2()? as usize;
                let text = reader.take(length)?;
                Constant::Utf8(String::from_utf8_lossy(text).into_owned())
            }
            3 => Constant::Integer(reader.u4()? as i32),
            4 => Constant::Float(f32::from_bits(reader.u4()?)),
            5 => {
                let high = reader.u4()? as u64;
                let low = reader.u4()? as u64;
                index += 1;
                Constant::Long(((high << 32) | low) as i64)
            }
            6 => {
                let high = reader.u4()? as u64;
                let low = reader.u4()? as u64;
                index += 1;
                Constant::Double(f64::from_bits((high << 32) | low))
            }
            7 => Constant::Class(format!("#{}", reader.u2()?)),
            8 => Constant::String(format!("#{}", reader.u2()?)),
            9 => {
                let class = reader.u2()? as usize;
                let name_and_type = reader.u2()? as usize;
                Constant::Fieldref {
                    class: format!("#{class}"),
                    name: format!("#{name_and_type}"),
                    kind: "F".into(),
                }
            }
            10 | 11 => {
                let class = reader.u2()? as usize;
                let name_and_type = reader.u2()? as usize;
                let value = Constant::Methodref {
                    class: format!("#{class}"),
                    name: format!("#{name_and_type}"),
                    kind: if tag == 11 { "I".into() } else { "M".into() },
                };
                if tag == 11 {
                    Constant::InterfaceMethodref {
                        class: match &value {
                            Constant::Methodref { class, .. } => class.clone(),
                            _ => unreachable!(),
                        },
                        name: match &value {
                            Constant::Methodref { name, .. } => name.clone(),
                            _ => unreachable!(),
                        },
                        kind: "I".into(),
                    }
                } else {
                    value
                }
            }
            12 => Constant::NameAndType {
                name: format!("#{}", reader.u2()?),
                kind: format!("#{}", reader.u2()?),
            },
            15 => {
                let _kind = reader.u1()?;
                Constant::MethodHandle {
                    reference: reader.u2()? as usize,
                }
            }
            16 => Constant::MethodType(format!("#{}", reader.u2()?)),
            17 | 18 => {
                let bootstrap = reader.u2()? as usize;
                let name_and_type = reader.u2()? as usize;
                Constant::InvokeDynamic {
                    name: format!("#{name_and_type}"),
                    kind: if tag == 17 { "D".into() } else { "I".into() },
                    bootstrap,
                }
            }
            19 | 20 => Constant::Module(format!("#{}", reader.u2()?)),
            other => return Err(Error(format!("constant tag {other} at {index}"))),
        };
        index += 1;
    }

    let _access = reader.u2()?;
    let this_class = reader.u2()? as usize;
    let super_class = reader.u2()? as usize;
    let name = class_name(&constants, this_class);
    let super_name = class_name(&constants, super_class);

    let interfaces = reader.u2()? as usize;
    reader.skip(interfaces * 2)?;

    let mut fields = Vec::new();
    let field_count = reader.u2()? as usize;
    for _ in 0..field_count {
        fields.push(read_member(&mut reader, &constants)?);
    }

    let mut methods = Vec::new();
    let method_count = reader.u2()? as usize;
    for _ in 0..method_count {
        methods.push(read_member(&mut reader, &constants)?);
    }

    // Class-level attributes: only BootstrapMethods matters here.
    let mut bootstraps = Vec::new();
    let attribute_count = reader.u2()? as usize;
    for _ in 0..attribute_count {
        let name_index = reader.u2()? as usize;
        let length = reader.u4()? as usize;
        let body = reader.take(length)?;
        let is_bootstrap = matches!(
            constants.get(name_index),
            Some(Constant::Utf8(text)) if text == "BootstrapMethods"
        );
        if !is_bootstrap {
            continue;
        }
        let mut inner = Reader { bytes: body, at: 0 };
        let count = inner.u2()? as usize;
        for _ in 0..count {
            let method = inner.u2()? as usize;
            let argument_count = inner.u2()? as usize;
            let mut arguments = Vec::with_capacity(argument_count);
            for _ in 0..argument_count {
                arguments.push(inner.u2()? as usize);
            }
            bootstraps.push(Bootstrap { method, arguments });
        }
    }

    Ok(ClassFile {
        name,
        super_name,
        constants,
        fields,
        methods,
        bootstraps,
    })
}

fn class_name(constants: &[Constant], index: usize) -> String {
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

fn read_member(reader: &mut Reader<'_>, constants: &[Constant]) -> Result<Member, Error> {
    let access = reader.u2()?;
    let name_index = reader.u2()? as usize;
    let descriptor_index = reader.u2()? as usize;
    let attribute_count = reader.u2()? as usize;
    let mut code = None;
    for _ in 0..attribute_count {
        let name_index = reader.u2()? as usize;
        let length = reader.u4()? as usize;
        let body = reader.take(length)?;
        let is_code =
            matches!(constants.get(name_index), Some(Constant::Utf8(text)) if text == "Code");
        if is_code {
            let mut inner = Reader { bytes: body, at: 0 };
            let max_stack = inner.u2()?;
            let max_locals = inner.u2()?;
            let code_length = inner.u4()? as usize;
            let bytes = inner.take(code_length)?.to_vec();
            code = Some(Code {
                max_stack,
                max_locals,
                bytes,
            });
        }
    }
    Ok(Member {
        access,
        name: utf8(constants, name_index),
        descriptor: utf8(constants, descriptor_index),
        code,
    })
}

pub fn utf8(constants: &[Constant], index: usize) -> String {
    match constants.get(index) {
        Some(Constant::Utf8(text)) => text.clone(),
        _ => String::new(),
    }
}

impl ClassFile {
    pub fn method(&self, name: &str) -> Option<&Member> {
        self.methods.iter().find(|member| member.name == name)
    }
}

/// Resolve a constant-pool entry to (class, name, descriptor) for a method call.
pub fn method_ref(constants: &[Constant], index: usize) -> Option<(String, String, String)> {
    let reference = match constants.get(index)? {
        Constant::Methodref { class, name, .. }
        | Constant::InterfaceMethodref { class, name, .. } => (class.clone(), name.clone()),
        _ => return None,
    };
    let class_index: usize = reference.0.trim_start_matches('#').parse().ok()?;
    let nat_index: usize = reference.1.trim_start_matches('#').parse().ok()?;
    let class = class_name(constants, class_index);
    let (name, descriptor) = match constants.get(nat_index)? {
        Constant::NameAndType { name, kind } => {
            let name_index: usize = name.trim_start_matches('#').parse().ok()?;
            let kind_index: usize = kind.trim_start_matches('#').parse().ok()?;
            (utf8(constants, name_index), utf8(constants, kind_index))
        }
        _ => return None,
    };
    Some((class, name, descriptor))
}

/// Resolve a field reference to (class, name, descriptor).
pub fn field_ref(constants: &[Constant], index: usize) -> Option<(String, String, String)> {
    let (class, name_and_type) = match constants.get(index)? {
        Constant::Fieldref { class, name, .. } => (class.clone(), name.clone()),
        _ => return None,
    };
    let class_index: usize = class.trim_start_matches('#').parse().ok()?;
    let nat_index: usize = name_and_type.trim_start_matches('#').parse().ok()?;
    let class = class_name(constants, class_index);
    let (name, descriptor) = match constants.get(nat_index)? {
        Constant::NameAndType { name, kind } => {
            let name_index: usize = name.trim_start_matches('#').parse().ok()?;
            let kind_index: usize = kind.trim_start_matches('#').parse().ok()?;
            (utf8(constants, name_index), utf8(constants, kind_index))
        }
        _ => return None,
    };
    Some((class, name, descriptor))
}

/// The `Utf8` value behind an `ldc` index, when it is a string constant.
pub fn string_constant(constants: &[Constant], index: usize) -> Option<String> {
    match constants.get(index)? {
        Constant::String(reference) => {
            let inner: usize = reference.trim_start_matches('#').parse().ok()?;
            match constants.get(inner)? {
                Constant::Utf8(text) => Some(text.clone()),
                _ => None,
            }
        }
        Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

/// Split a method descriptor into parameter types and a return type.
pub fn descriptor_types(descriptor: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = descriptor.chars().collect();
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at];
        if c == '(' || c == ')' {
            at += 1;
            continue;
        }
        if c == '[' {
            let start = at;
            while at < bytes.len() && bytes[at] == '[' {
                at += 1;
            }
            if at < bytes.len() {
                if bytes[at] == 'L' {
                    while at < bytes.len() && bytes[at] != ';' {
                        at += 1;
                    }
                }
                at += 1;
            }
            out.push(bytes[start..at.min(bytes.len())].iter().collect());
            continue;
        }
        if c == 'L' {
            let start = at;
            while at < bytes.len() && bytes[at] != ';' {
                at += 1;
            }
            at += 1;
            out.push(bytes[start..at.min(bytes.len())].iter().collect());
            continue;
        }
        out.push(c.to_string());
        at += 1;
    }
    out
}

/// A quick index of the class names a file mentions, for reporting.
pub fn class_names(constants: &[Constant]) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for constant in constants {
        if let Constant::Class(reference) = constant {
            let index: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            let name = class_name(constants, index);
            if !name.is_empty() {
                *out.entry(name).or_insert(0) += 1;
            }
        }
    }
    out
}
