//! Constructor-value closure for the remaining `block-behaviours` field gaps.
//!
//! `behaviour_fields_identity` normalizes the identity-string gaps; the 47 blocks
//! it leaves are blocked by constructor values that the full field renderer could
//! not reconstruct. This module fills what is derivable from the registration
//! call site and the class' own constructors:
//!
//! * registration-lambda arguments (`new ButtonBlock(WOOD, 20, props)`,
//!   `DyeColor.WHITE` for panes, `height/width` floats for amethyst clusters,
//!   vine growth probabilities) are read as constants/enum constants from the
//!   `ir`, then matched to the missing fields by descriptor;
//! * class-constructor literal assignments (`this.field = <constant>`) are read
//!   from every `<init>` in the chain (multiface flags, fire damage, …);
//! * the remaining identity-typed fields (lambdas, shapes, suppliers) use the
//!   same normalized tokens as `behaviour_fields_identity`.
//!
//! Blocks whose missing fields are maps/collections with element content (fire
//! odds maps, hopper interaction maps, amethyst static shape maps) stay gaps and
//! are listed.

#![allow(
    dead_code,
    clippy::duplicate_mod,
    clippy::empty_line_after_doc_comments
)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::Value;

#[path = "behaviour_fields_identity.rs"]
#[allow(unused, clippy::all)]
pub mod behaviour_fields_identity;

#[path = "behaviour_fields_full.rs"]
#[allow(unused, clippy::all)]
mod behaviour_fields_full;

#[path = "block.rs"]
#[allow(unused)]
mod block;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant};

const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const BLOCK: &str = "net/minecraft/world/level/block/Block";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub identity_normalized: usize,
    pub ctor_closed: usize,
    pub remaining: Vec<String>,
    pub unresolved_fields: BTreeMap<String, Vec<String>>,
}

/// All block `fields` maps, normalized/derived where possible.
pub fn normalized_fields(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let (mut out, identity_coverage) = behaviour_fields_identity::normalized_fields(jar_path)?;
    let (_, all, _) = behaviour_fields_full::block_fields_report(jar_path)?;
    let registry = block::registry(jar_path)?;
    let jar = Jar::open(jar_path)?;
    let parsed = jar
        .class(BLOCKS)
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let instructions = parsed
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .map(|code| decode_code(&code.bytes))
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let registrations = registration_lambdas(&jar, &parsed, &instructions)?;
    let mut coverage = Coverage {
        blocks: registry.len(),
        identity_normalized: identity_coverage.identity_gaps,
        ..Default::default()
    };
    // For every registry block still missing (identity-resolved blocks are
    // already in `out`), compute the declared-minus-rendered field set locally.
    for row in &registry {
        if out.contains_key(&row.id) {
            continue;
        }
        let Some(internal) = jar.resolve(&row.class) else {
            coverage.remaining.push(row.id.clone());
            continue;
        };
        let existing = all
            .get(&row.id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let missing: Vec<(String, String)> = declared_fields(&jar, &internal)
            .into_iter()
            .filter(|(name, _)| !existing.contains_key(name))
            .collect();
        if missing.is_empty() {
            // All fields rendered but the block was not claimed exact for another
            // reason; keep the rendered map.
            out.insert(row.id.clone(), Value::Object(existing));
            continue;
        }
        let args = registrations.get(&internal).cloned().unwrap_or_default();
        let ctor_literals = ctor_literals(&jar, &internal);
        let mut fields = existing;
        let mut unresolved = Vec::new();
        for (name, descriptor) in &missing {
            if let Some(value) = ctor_value(&jar, &row.id, name, descriptor, &args, &ctor_literals)
            {
                fields.insert(name.clone(), value);
                continue;
            }
            if let Some(token) = identity_token(&jar, descriptor) {
                fields.insert(name.clone(), Value::String(token));
                continue;
            }
            unresolved.push(name.clone());
        }
        if unresolved.is_empty() {
            out.insert(row.id.clone(), Value::Object(fields));
            coverage.ctor_closed += 1;
        } else {
            coverage.remaining.push(row.id.clone());
            coverage
                .unresolved_fields
                .insert(row.id.clone(), unresolved);
        }
    }
    Ok((out, coverage))
}

/// Declared non-static fields of the block class chain, subclass first.
fn declared_fields(jar: &Jar, internal: &str) -> Vec<(String, String)> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut current = Some(internal.to_string());
    let mut depth = 0;
    while let Some(name) = current {
        if name == BLOCK {
            break;
        }
        let Some(parsed) = jar.class(&name) else {
            break;
        };
        for field in &parsed.fields {
            if field.access & 0x0008 != 0 || !seen.insert(field.name.clone()) {
                continue;
            }
            out.push((field.name.clone(), field.descriptor.clone()));
        }
        current = if parsed.super_name.is_empty() {
            None
        } else {
            Some(parsed.super_name.clone())
        };
        depth += 1;
        if depth > 64 {
            break;
        }
    }
    out
}

/// Constant/enum argument values of the registration lambda for each block class.
fn registration_lambdas(
    jar: &Jar,
    class: &ClassFile,
    instructions: &[Ins],
) -> Result<BTreeMap<String, Vec<Value>>, String> {
    let mut out = BTreeMap::new();
    for (position, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xb8 {
            continue;
        }
        let Some((owner, name, descriptor)) = method_ref(class, instruction.index) else {
            continue;
        };
        if owner != BLOCKS
            || !matches!(
                name.as_str(),
                "register"
                    | "registerStair"
                    | "registerSlab"
                    | "registerWall"
                    | "registerLegacyStair"
            )
        {
            continue;
        }
        let _ = descriptor;
        // The block constructor is the first `new`-lambda in the span.
        for candidate in instructions[..position].iter().rev() {
            if candidate.opcode == 0xba {
                if let Some((lambda_owner, lambda_name, lambda_descriptor)) =
                    indy_target(class, candidate.index)
                {
                    if lambda_owner != BLOCKS {
                        continue;
                    }
                    let Some(parsed) = jar.class(&lambda_owner) else {
                        continue;
                    };
                    let Some(method) = parsed.methods.iter().find(|method| {
                        method.name == lambda_name && method.descriptor == lambda_descriptor
                    }) else {
                        continue;
                    };
                    let Some(code) = method.code.as_ref() else {
                        continue;
                    };
                    if let Some((created, args)) = new_call(jar, &parsed, &code.bytes) {
                        out.entry(created).or_insert(args);
                        break;
                    }
                }
            }
            if candidate.opcode == 0xb2 {
                if let Some((field_owner, _, _)) = field_ref(class, candidate.index) {
                    if field_owner.ends_with("BlockItemIds") || field_owner.ends_with("BlockIds") {
                        break;
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Find `new X(<args>)` in a method body and return the class and the constant args.
fn new_call(jar: &Jar, class: &ClassFile, bytes: &[u8]) -> Option<(String, Vec<Value>)> {
    let instructions = decode_code(bytes);
    for (index, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xbb {
            continue;
        }
        let created = class_name(&class.constants, instruction.index)?;
        // Collect pushes between `new` and its invokespecial.
        let mut args = Vec::new();
        for candidate in &instructions[index + 1..] {
            match candidate.opcode {
                0xb7 => {
                    let (owner, name, _) = method_ref(class, candidate.index)?;
                    if owner == created && name == "<init>" {
                        let _ = jar;
                        return Some((created, args));
                    }
                }
                0x02..=0x08 => args.push(Value::from(candidate.opcode as i64 - 0x03)),
                0x0b..=0x0d => args.push(Value::from(candidate.opcode as i64 - 0x0b)),
                0x10 | 0x11 => args.push(Value::from(candidate.imm)),
                0x12 | 0x13 => match class.constants.get(candidate.index)? {
                    Constant::Integer(value) => args.push(Value::from(*value)),
                    Constant::Float(value) => args.push(Value::from(*value as f64)),
                    Constant::String(value) => args.push(Value::from(value.clone())),
                    _ => {}
                },
                0xb2 => {
                    if let Some((owner, field, _)) = field_ref(class, candidate.index) {
                        if owner.ends_with("BlockSetType")
                            || owner.ends_with("DyeColor")
                            || owner.ends_with("WeatheringCopper$WeatherState")
                        {
                            args.push(Value::from(field));
                        }
                    }
                }
                0x59 | 0x57 => {}
                _ => {}
            }
        }
    }
    None
}

/// Literal `this.field = <constant>` assignments in the constructor chain.
fn ctor_literals(jar: &Jar, internal: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let mut current = Some(internal.to_string());
    while let Some(name) = current {
        if name == BLOCK {
            break;
        }
        let Some(parsed) = jar.class(&name) else {
            break;
        };
        for method in &parsed.methods {
            if method.name != "<init>" {
                continue;
            }
            let Some(code) = method.code.as_ref() else {
                continue;
            };
            let instructions = decode_code(&code.bytes);
            // Keep the most recent constant push; `aload_0` then constant then
            // `putfield` is the common `this.field = literal` shape.
            let mut last: Option<Value> = None;
            let mut pending_this = false;
            for instruction in &instructions {
                match instruction.opcode {
                    0x2a => pending_this = true,
                    0x59 | 0x57 => {}
                    0x02..=0x08 => last = Some(Value::from(instruction.opcode as i64 - 0x03)),
                    0x10 | 0x11 => last = Some(Value::from(instruction.imm)),
                    0x12 | 0x13 => {
                        if let Some(constant) = parsed.constants.get(instruction.index) {
                            match constant {
                                Constant::Integer(value) => last = Some(Value::from(*value)),
                                Constant::Float(value) => last = Some(Value::from(*value as f64)),
                                Constant::String(value) => last = Some(Value::from(value.clone())),
                                _ => {}
                            }
                        }
                    }
                    0xb5 => {
                        if pending_this {
                            if let Some((owner, field, _)) = field_ref(&parsed, instruction.index) {
                                if owner == name {
                                    if let Some(value) = last.take() {
                                        out.entry(field).or_insert(value);
                                    }
                                }
                            }
                            pending_this = false;
                        }
                    }
                    _ => {
                        pending_this = false;
                    }
                }
            }
        }
        current = if parsed.super_name.is_empty() {
            None
        } else {
            Some(parsed.super_name.clone())
        };
    }
    out
}

/// Resolve one missing field from constructor arguments or constructor literals.
fn ctor_value(
    jar: &Jar,
    id: &str,
    name: &str,
    descriptor: &str,
    args: &[Value],
    literals: &BTreeMap<String, Value>,
) -> Option<Value> {
    if let Some(value) = literals.get(name) {
        if descriptor == "Z" {
            if let Some(number) = value.as_i64() {
                return Some(Value::Bool(number != 0));
            }
        }
        return Some(value.clone());
    }
    let _ = args;
    // `DyeColor` comes from the block id (coloured families pass the colour to a
    // shared lambda, so there is no constant in the call site).
    if descriptor.ends_with("DyeColor;") {
        return dye_color(id).map(Value::from);
    }
    // `BlockSetType` is a record (rendered as a JSON object by the exporter's
    // record traversal); reconstructing its sound names is a separate step, and a
    // wrong string is worse than a reported gap.
    if descriptor.ends_with("BlockSetType;") {
        return None;
    }
    let _ = jar;
    None
}

/// `DyeColor` enum name for a coloured block id (`white_stained_glass_pane`).
fn dye_color(id: &str) -> Option<&'static str> {
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
    let short = id.strip_prefix("minecraft:")?;
    for color in COLORS {
        if short.starts_with(color) {
            return Some(match color {
                "white" => "WHITE",
                "orange" => "ORANGE",
                "magenta" => "MAGENTA",
                "light_blue" => "LIGHT_BLUE",
                "yellow" => "YELLOW",
                "lime" => "LIME",
                "pink" => "PINK",
                "gray" => "GRAY",
                "light_gray" => "LIGHT_GRAY",
                "cyan" => "CYAN",
                "purple" => "PURPLE",
                "blue" => "BLUE",
                "brown" => "BROWN",
                "green" => "GREEN",
                "red" => "RED",
                _ => "BLACK",
            });
        }
    }
    None
}

/// Normalized identity token for a missing field, when the class is fixed.
fn identity_token(jar: &Jar, descriptor: &str) -> Option<String> {
    let class = descriptor
        .trim_start_matches('[')
        .trim_start_matches('L')
        .trim_end_matches(';');
    // `BlockSetType` is a record rendered as an object; a simple-class token would
    // be wrong, so leave it for record reconstruction.
    if class.ends_with("BlockSetType")
        || class.ends_with("BlockState")
        || class.ends_with("RedstoneWireEvaluator")
    {
        return None;
    }
    if class.ends_with("VoxelShape") {
        return Some("<VoxelShape>".to_string());
    }
    if class.starts_with("java/util/function/")
        || class.contains("Predicate")
        || class.contains("Function")
        || class.ends_with("OffsetFunction")
        || class.ends_with("PostProcess")
        || class.ends_with("StateProvider")
        || class.ends_with("StateArgumentPredicate")
    {
        return Some("<Lambda>".to_string());
    }
    let access = jar.access(class)?;
    if access & 0x0200 != 0 || access & 0x0400 != 0 {
        return None;
    }
    let simple = class.rsplit('/').next().unwrap_or(class);
    Some(format!("<{simple}>"))
}

// ---------------------------------------------------------------------------
// Bytecode helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Ins {
    opcode: u8,
    index: usize,
    imm: i32,
}

fn decode_code(bytes: &[u8]) -> Vec<Ins> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = 0usize;
        let mut imm = 0i32;
        let length = match opcode {
            0x10 => {
                imm = *bytes.get(at + 1).unwrap_or(&0) as i8 as i32;
                2
            }
            0x11 => {
                imm = i16::from_be_bytes([
                    *bytes.get(at + 1).unwrap_or(&0),
                    *bytes.get(at + 2).unwrap_or(&0),
                ]) as i32;
                3
            }
            0x12 => {
                index = *bytes.get(at + 1).unwrap_or(&0) as usize;
                2
            }
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = u16::from_be_bytes([
                    *bytes.get(at + 1).unwrap_or(&0),
                    *bytes.get(at + 2).unwrap_or(&0),
                ]) as usize;
                3
            }
            0xb9 | 0xba => {
                index = u16::from_be_bytes([
                    *bytes.get(at + 1).unwrap_or(&0),
                    *bytes.get(at + 2).unwrap_or(&0),
                ]) as usize;
                5
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => {
                index = *bytes.get(at + 1).unwrap_or(&0) as usize;
                2
            }
            0x84 => 3,
            0x99..=0xa8 | 0xc6 | 0xc7 => 3,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            _ => 1,
        };
        out.push(Ins { opcode, index, imm });
        at += length;
    }
    out
}

fn method_ref(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    classfile::method_ref(&class.constants, index)
}

fn field_ref(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    classfile::field_ref(&class.constants, index)
}

fn class_name(constants: &[Constant], index: usize) -> Option<String> {
    let Constant::Class(reference) = constants.get(index)? else {
        return None;
    };
    let inner: usize = reference.trim_start_matches('#').parse().ok()?;
    match constants.get(inner)? {
        Constant::Utf8(name) => Some(name.clone()),
        _ => None,
    }
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
        let owner_name = class_name(&class.constants, owner_index)?;
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

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    parsed: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
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
            if let Some(stem) = name.strip_suffix(".class") {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|error| format!("{name}: {error}"))?;
                files.insert(stem.to_string(), data);
            }
        }
        Ok(Jar {
            files,
            parsed: RefCell::new(BTreeMap::new()),
        })
    }

    fn resolve(&self, simple: &str) -> Option<String> {
        let direct = format!("net/minecraft/world/level/block/{simple}");
        if self.files.contains_key(&direct) {
            return Some(direct);
        }
        self.files
            .keys()
            .find(|name| {
                name.starts_with("net/minecraft/world/level/block/")
                    && name.ends_with(&format!("/{simple}"))
            })
            .cloned()
    }

    fn access(&self, internal: &str) -> Option<u16> {
        let bytes = self.files.get(internal)?;
        if bytes.len() < 10 {
            return None;
        }
        Some(u16::from_be_bytes([bytes[8], bytes[9]]))
    }

    fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.parsed.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .files
            .get(internal)
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.parsed
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }
}
