//! `Properties.hasCollision` per registered block, including the blocks whose
//! properties are built inside a registration lambda.
//!
//! `BlockBehaviour` turns `Properties.noCollision()` into `hasCollision = false`,
//! and `getCollisionShape` then returns `Shapes.empty()` without consulting the
//! block's shape methods. The flag therefore decides 320 collision states and
//! the matching `full_block` entries.
//!
//! # Why a plain `Blocks.<clinit>` scan is not enough
//!
//! Many blocks are registered through the colour families:
//!
//! ```text
//! getstatic BlockItemIds.DYED_BANNER      // ColorCollection
//! invokedynamic  #76  (TriFunction)      // block constructor
//! invokedynamic  #77  (BiFunction)       // loot/description
//! invokedynamic  #78  (Function)         // `DyeColor -> Properties`
//! invokestatic  ColorCollection.registerBlocks(...)
//! ```
//!
//! The `Properties` object (and its `noCollision()` call) lives in that last
//! lambda; the clinit span only contains the indy. This module resolves the
//! invokedynamic bootstrap target, parses its body and looks for
//! `Properties.noCollision()` there (and one level of `Blocks.*(…Properties)`
//! helper calls), then assigns the flag to the 16/8 blocks the family registers.
//!
//! # Registry alignment
//!
//! Registration calls appear in `BuiltInRegistries.BLOCK` order, so the flags
//! are returned in that order (length 1,286) and the caller can align them with
//! `block::registry`/`states::state_table`.

#![allow(dead_code, unused_variables, clippy::collapsible_if)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant};

/// The block registry's `hasCollision` flags, in registration order.
pub fn has_collision(jar_path: &Path) -> Result<Vec<bool>, String> {
    let jar = Jar::open(jar_path)?;
    let bytes = jar
        .classes
        .get("net/minecraft/world/level/block/Blocks")
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let parsed = classfile::parse(bytes).map_err(|error| format!("Blocks: {error}"))?;
    let code = parsed
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let instructions = decode_code(&code.bytes);
    let mut flags = Vec::new();
    let mut seen = 0usize;
    for (position, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xb8 {
            continue;
        }
        let Some((owner, name, descriptor)) = method_ref(&parsed, instruction) else {
            continue;
        };
        let family =
            owner.ends_with("ColorCollection") || owner.ends_with("WeatheringCopperCollection");
        let single = owner == "net/minecraft/world/level/block/Blocks"
            && matches!(
                name.as_str(),
                "register"
                    | "registerStair"
                    | "registerSlab"
                    | "registerWall"
                    | "registerLegacyStair"
            );
        if !single && !family {
            continue;
        }
        let mut no_collision = false;
        // The properties `Function` is the last functional argument of the
        // family registration, so only the *last* lambda in the chain builds the
        // `Properties`; earlier ones are the block constructor and loot factory.
        let mut properties_lambda: Option<(String, String, String)> = None;
        // The chain starts at the block-id push and ends at this call.
        for previous in instructions[..position].iter().rev() {
            if previous.opcode == 0xb2 {
                if let Some((field_owner, _, _)) = field_ref(&parsed, previous) {
                    if field_owner.ends_with("BlockItemIds") || field_owner.ends_with("BlockIds") {
                        break;
                    }
                }
            }
            if previous.opcode == 0xb6 {
                if let Some((call_owner, call_name, _)) = method_ref(&parsed, previous) {
                    if call_owner.ends_with("Properties") && call_name == "noCollision" {
                        no_collision = true;
                    }
                }
            }
            if previous.opcode == 0xba {
                if let Some((lambda_owner, lambda_name, lambda_descriptor)) =
                    indy_target(&parsed, previous.index)
                {
                    if !(lambda_owner == "net/minecraft/world/level/block/Blocks"
                        && matches!(
                            lambda_name.as_str(),
                            "register"
                                | "registerStair"
                                | "registerSlab"
                                | "registerWall"
                                | "registerLegacyStair"
                        ))
                    {
                        properties_lambda = Some((lambda_owner, lambda_name, lambda_descriptor));
                    }
                }
            }
            if previous.opcode == 0xb8 {
                if let Some((call_owner, call_name, call_descriptor)) =
                    method_ref(&parsed, previous)
                {
                    if call_owner == "net/minecraft/world/level/block/Blocks"
                        && call_descriptor.ends_with(
                            ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
                        )
                    {
                        if body_has_no_collision(&jar, &call_owner, &call_name, &call_descriptor, 0)
                        {
                            no_collision = true;
                        }
                    }
                }
            }
        }
        if let Some((lambda_owner, lambda_name, lambda_descriptor)) = properties_lambda {
            if body_has_no_collision(&jar, &lambda_owner, &lambda_name, &lambda_descriptor, 0) {
                no_collision = true;
            }
        }
        let count = if family {
            if owner.ends_with("WeatheringCopperCollection") {
                8
            } else {
                16
            }
        } else {
            1
        };
        for _ in 0..count {
            flags.push(!no_collision);
            seen += 1;
        }
        // Some blocks are registered through *both* a single call and a family?
        // No: the family call replaces the single one, so `descriptor` is unused
        // here but kept for diagnostics.
        let _ = &descriptor;
    }
    if seen != 1_286 {
        return Err(format!(
            "the registration scan produced {seen} blocks, expected 1,286"
        ));
    }
    Ok(flags)
}

/// Whether a method body (or one of the `Blocks.*Properties` helpers it calls)
/// contains `Properties.noCollision()`.
fn body_has_no_collision(
    jar: &Jar,
    owner: &str,
    name: &str,
    descriptor: &str,
    depth: usize,
) -> bool {
    if depth > 3 {
        return false;
    }
    let Some(parsed) = jar.class(owner) else {
        return false;
    };
    let Some(method) = parsed
        .methods
        .iter()
        .find(|method| method.name == name && method.descriptor == descriptor)
    else {
        return false;
    };
    let Some(code) = method.code.as_ref() else {
        return false;
    };
    for instruction in decode_code(&code.bytes) {
        if instruction.opcode == 0xb6 {
            if let Some((call_owner, call_name, _)) = method_ref(&parsed, &instruction) {
                if call_owner.ends_with("Properties") && call_name == "noCollision" {
                    return true;
                }
            }
        }
        if instruction.opcode == 0xb8 {
            if let Some((call_owner, call_name, call_descriptor)) =
                method_ref(&parsed, &instruction)
            {
                if call_owner == "net/minecraft/world/level/block/Blocks"
                    && call_descriptor.ends_with(
                        ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
                    )
                    && body_has_no_collision(
                        jar,
                        &call_owner,
                        &call_name,
                        &call_descriptor,
                        depth + 1,
                    )
                {
                    return true;
                }
            }
        }
        if instruction.opcode == 0xba {
            if let Some((lambda_owner, lambda_name, lambda_descriptor)) =
                indy_target(&parsed, instruction.index)
            {
                if body_has_no_collision(
                    jar,
                    &lambda_owner,
                    &lambda_name,
                    &lambda_descriptor,
                    depth + 1,
                ) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Bytecode reading
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Ins {
    opcode: u8,
    index: usize,
    immediate: i32,
}

fn decode_code(bytes: &[u8]) -> Vec<Ins> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = 0usize;
        let mut immediate = 0i32;
        let length = match opcode {
            0x10 => {
                immediate = *bytes.get(at + 1).unwrap_or(&0) as i8 as i32;
                2
            }
            0x11 => {
                immediate = i16::from_be_bytes([
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
            0x84 => {
                index = *bytes.get(at + 1).unwrap_or(&0) as usize;
                immediate = *bytes.get(at + 2).unwrap_or(&0) as i8 as i32;
                3
            }
            0x99..=0xa8 | 0xc6 | 0xc7 => 3,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            _ => 1,
        };
        out.push(Ins {
            opcode,
            index,
            immediate,
        });
        at += length;
    }
    out
}

fn method_ref(class: &ClassFile, instruction: &Ins) -> Option<(String, String, String)> {
    classfile::method_ref(&class.constants, instruction.index)
}

fn field_ref(class: &ClassFile, instruction: &Ins) -> Option<(String, String, String)> {
    classfile::field_ref(&class.constants, instruction.index)
}

/// The method handle a `BootstrapMethods` entry points at (the lambda body).
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

struct Jar {
    classes: BTreeMap<String, Vec<u8>>,
    parsed: std::cell::RefCell<BTreeMap<String, Option<std::rc::Rc<ClassFile>>>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut classes = BTreeMap::new();
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
                classes.insert(stem.to_string(), data);
            }
        }
        Ok(Jar {
            classes,
            parsed: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    fn class(&self, internal: &str) -> Option<std::rc::Rc<ClassFile>> {
        if let Some(hit) = self.parsed.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .classes
            .get(internal)
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(std::rc::Rc::new);
        self.parsed
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }
}
