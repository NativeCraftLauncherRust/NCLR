//! Residual instrument assignments for `ColorCollection.zipMap` families.
//!
//! `Blocks.<clinit>` registers the coloured stairs/slabs/walls with
//! `ColorCollection.zipMap(VALUES, BlockItemIds.X, BiFunction)`. The
//! `BiFunction`'s lambda body names the base family, e.g.
//!
//! ```text
//! lambda(DyeColor, BlockItemId) -> registerStair(id, Blocks.CONCRETE.pick(color))
//! ```
//!
//! so the new blocks copy the base family's `Properties` (including
//! `instrument`). The static scan in `behaviour_instrument` follows the family
//! properties lambda but cannot see through `zipMap`; this module resolves the
//! base field from the map lambda and patches those families, reaching
//! 1,286/1,286 instruments.
//!
//! No values are read from `ship.hzr3`; everything comes from `client.jar` and
//! the existing derivations.

#![allow(dead_code, clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "behaviour_instrument.rs"]
#[allow(unused)]
mod behaviour_instrument;

#[path = "block.rs"]
#[allow(unused)]
mod block;

#[path = "states.rs"]
#[allow(unused)]
mod states;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant};

const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const COLOR_COLLECTION: &str = "net/minecraft/world/level/block/ColorCollection";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub patched: usize,
    pub zip_map_families: Vec<String>,
    pub residual: Vec<String>,
}

/// Block id → `NoteBlockInstrument` enum name, for every registered block.
pub fn block_instruments(jar_path: &Path) -> Result<(BTreeMap<String, String>, Coverage), String> {
    let (mut instruments, _) = behaviour_instrument::block_instruments(jar_path)?;
    let registry = block::registry(jar_path)?;
    let jar = Jar::open(jar_path)?;
    let parsed = jar
        .class(BLOCKS)
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let code = parsed
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let instructions = decode_code(&code.bytes);
    let mut coverage = Coverage {
        blocks: registry.len(),
        ..Default::default()
    };
    // Field name → instrument of a family's members (from the registry order the
    // family registered, which is contiguous in `BuiltInRegistries.BLOCK`).
    let mut field_instruments: BTreeMap<String, String> = BTreeMap::new();
    let mut registered = 0usize;
    let mut index = 0usize;
    while index < instructions.len() {
        let instruction = &instructions[index];
        if instruction.opcode != 0xb8 {
            index += 1;
            continue;
        }
        let Some((owner, name, descriptor)) = method_ref(&parsed, instruction.index) else {
            index += 1;
            continue;
        };
        let is_zip_map = owner == COLOR_COLLECTION && name == "zipMap";
        let family = owner.ends_with("ColorCollection")
            || owner.ends_with("WeatheringCopperCollection");
        let single = owner == BLOCKS
            && matches!(
                name.as_str(),
                "register"
                    | "registerStair"
                    | "registerSlab"
                    | "registerWall"
                    | "registerLegacyStair"
            );
        if !is_zip_map && !family && !single {
            index += 1;
            continue;
        }
        let count = if is_zip_map {
            16
        } else if family {
            if owner.ends_with("WeatheringCopperCollection") {
                8
            } else {
                16
            }
        } else {
            1
        };
        // The next `putstatic Blocks.<FIELD>` names this registration's field.
        let mut field = None;
        for follow in &instructions[index + 1..] {
            if follow.opcode == 0xb3 {
                if let Some((field_owner, field_name, _)) = field_ref(&parsed, follow.index) {
                    if field_owner == BLOCKS {
                        field = Some(field_name);
                    }
                }
                break;
            }
        }
        if is_zip_map && registered + count <= registry.len() {
            // Base family from the map lambda's body: the getstatic Blocks field
            // passed to `pick`.
            let mut base = None;
            for candidate in instructions[..index].iter().rev() {
                if candidate.opcode == 0xba {
                    if let Some((lambda_owner, lambda_name, lambda_descriptor)) =
                        indy_target(&parsed, candidate.index)
                    {
                        if lambda_owner == BLOCKS {
                            base = lambda_base_field(
                                &jar,
                                &lambda_owner,
                                &lambda_name,
                                &lambda_descriptor,
                                &field_instruments,
                            );
                            if base.is_some() {
                                break;
                            }
                        }
                    }
                }
                if candidate.opcode == 0xb2 {
                    if let Some((field_owner, _, _)) = field_ref(&parsed, candidate.index) {
                        if field_owner.ends_with("BlockItemIds") || field_owner.ends_with("BlockIds") {
                            break;
                        }
                    }
                }
            }
            if let Some(base) = base {
                if let Some(instrument) = field_instruments.get(&base).cloned() {
                    for offset in 0..count {
                        if let Some(row) = registry.get(registered + offset) {
                            if instruments.get(&row.id) != Some(&instrument) {
                                instruments.insert(row.id.clone(), instrument.clone());
                                coverage.patched += 1;
                            }
                        }
                    }
                    if let Some(family_field) = &field {
                        field_instruments.insert(family_field.clone(), instrument.clone());
                    }
                    coverage
                        .zip_map_families
                        .push(format!("{} <- {base}", field.clone().unwrap_or_default()));
                } else {
                    coverage
                        .residual
                        .push(format!("zipMap base {base}: no instrument recorded yet"));
                }
            } else {
                coverage
                    .residual
                    .push(format!("zipMap {name}: base field not resolved"));
            }
        } else if let Some(field) = field {
            if registered < registry.len() {
                if let Some(instrument) = instruments.get(&registry[registered].id).cloned() {
                    field_instruments.insert(field, instrument);
                }
            }
        }
        registered += count;
        index += 1;
        let _ = descriptor;
    }
    Ok((instruments, coverage))
}

/// The `Blocks.<FIELD>` passed to `ColorCollection.pick` inside a zipMap lambda.
fn lambda_base_field(
    jar: &Jar,
    owner: &str,
    name: &str,
    descriptor: &str,
    field_instruments: &BTreeMap<String, String>,
) -> Option<String> {
    let class = jar.class(owner)?;
    let method = class
        .methods
        .iter()
        .find(|method| method.name == name && method.descriptor == descriptor)?;
    let code = method.code.as_ref()?;
    let instructions = decode_code(&code.bytes);
    for instruction in &instructions {
        if instruction.opcode != 0xb2 {
            continue;
        }
        let Some((field_owner, field_name, _)) = field_ref(&class, instruction.index) else {
            continue;
        };
        if field_owner == BLOCKS && field_instruments.contains_key(&field_name) {
            return Some(field_name);
        }
    }
    None
}

struct Jar {
    classes: BTreeMap<String, Vec<u8>>,
    parsed: RefCell<BTreeMap<String, Option<Rc<ClassFile>>>>,
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
            let entry_name = entry.name().map_err(|error| error.to_string())?.to_string();
            if let Some(stem) = entry_name.strip_suffix(".class") {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|error| format!("{entry_name}: {error}"))?;
                classes.insert(stem.to_string(), data);
            }
        }
        Ok(Jar {
            classes,
            parsed: RefCell::new(BTreeMap::new()),
        })
    }

    fn class(&self, internal: &str) -> Option<Rc<ClassFile>> {
        if let Some(hit) = self.parsed.borrow().get(internal) {
            return hit.clone();
        }
        let parsed = self
            .classes
            .get(internal)
            .and_then(|bytes| classfile::parse(bytes).ok())
            .map(Rc::new);
        self.parsed
            .borrow_mut()
            .insert(internal.to_string(), parsed.clone());
        parsed
    }
}

#[derive(Debug, Clone)]
struct Ins {
    opcode: u8,
    index: usize,
}

fn decode_code(bytes: &[u8]) -> Vec<Ins> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = 0usize;
        let length = match opcode {
            0x10 => 2,
            0x11 => 3,
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
        out.push(Ins { opcode, index });
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

fn indy_target(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let entry = class.bootstraps.get(*bootstrap)?;
    for argument in &entry.arguments {
        let Constant::MethodHandle { reference } = class.constants.get(*argument)? else {
            continue;
        };
        let Constant::Methodref { class: owner, name, .. } = class.constants.get(*reference)?
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
