//! Residual instrument fixes for `block-behaviours.json` (task-43).
//!
//! Three mechanisms remain after the per-family scan:
//!
//! 1. `WeatheringCopperCollection.registerBlocks` gives its eight members a
//!    **per-member** instrument cycle. The properties `Function` argument's
//!    lambda body lists the four `NoteBlockInstrument` constants in weathering
//!    order (`TRUMPET`, `_EXPOSED`, `_WEATHERED`, `_OXIDIZED`); member `i` of
//!    the family uses `list[i % 4]` (the waxed half repeats the cycle).
//! 2. `WeatheringCopperCollection.zipMap` variants (e.g. cut-copper stairs and
//!    slabs) inherit their base family's per-member instruments; the base field
//!    is named by the map lambda (`Blocks.CUT_COPPER.pick(...)`).
//! 3. Fresh-properties helpers such as `wallVariant(Block, boolean)` build
//!    `Properties.of()` and only override loot table/description, so a chain
//!    that calls one does **not** inherit the base block's instrument
//!    (`creeper_wall_head` = HARP, not CREEPER).
//!
//! The module starts from `behaviour_instrument_zip::block_instruments` (which
//! resolves the plain colour `zipMap` families) and patches the rest to
//! 1,286/1,286 instruments. Everything comes from `client.jar`.

#![allow(dead_code, clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "behaviour_instrument_zip.rs"]
#[allow(unused)]
mod behaviour_instrument_zip;

#[path = "block.rs"]
#[allow(unused)]
mod block;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant};

const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
const INSTRUMENT: &str = "NoteBlockInstrument";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub weathering_families: Vec<String>,
    pub zip_map_families: Vec<String>,
    pub fresh_property_resets: Vec<String>,
    pub residual: Vec<String>,
}

/// Block id → `NoteBlockInstrument` enum name.
pub fn block_instruments(jar_path: &Path) -> Result<(BTreeMap<String, String>, Coverage), String> {
    let (mut instruments, _) = behaviour_instrument_zip::block_instruments(jar_path)?;
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
    // Family field → per-member instruments in registry order.
    let mut family_members: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut shared_cycle: Option<Vec<String>> = None;
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
        let weathering = owner.ends_with("WeatheringCopperCollection");
        let color = owner.ends_with("ColorCollection");
        let single = owner == BLOCKS
            && matches!(
                name.as_str(),
                "register"
                    | "registerStair"
                    | "registerSlab"
                    | "registerWall"
                    | "registerLegacyStair"
            );
        if !weathering && !color && !single {
            index += 1;
            continue;
        }
        let is_zip_map = (weathering || color) && name == "zipMap";
        let count = if weathering || color {
            if weathering {
                8
            } else {
                16
            }
        } else {
            1
        };
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
        if weathering && name == "registerBlocks" && !is_zip_map {
            // The last functional argument is the properties function; its
            // lambda body lists the weathering instruments.
            let cycle = instrument_cycle(&parsed, &instructions, index, &jar);
            if let Some(list) = cycle {
                if shared_cycle.is_none() && list.len() >= 4 {
                    shared_cycle = Some(list.clone());
                }
                let mut members = Vec::new();
                for offset in 0..count {
                    members.push(list[offset % list.len()].clone());
                }
                if let Some(field) = &field {
                    family_members.insert(field.clone(), members.clone());
                }
                for (offset, instrument) in members.iter().enumerate() {
                    if let Some(row) = registry.get(registered + offset) {
                        instruments.insert(row.id.clone(), instrument.clone());
                    }
                }
                coverage
                    .weathering_families
                    .push(field.clone().unwrap_or_default());
            } else {
                // Functional copper families (bulb, door, grate, …) create fresh
                // properties with no instrument.
                for offset in 0..count {
                    if let Some(row) = registry.get(registered + offset) {
                        instruments.insert(row.id.clone(), "HARP".to_string());
                    }
                }
                if let Some(field) = &field {
                    family_members.insert(field.clone(), vec!["HARP".to_string()]);
                }
                coverage.residual.push(format!("{name}: no per-member instrument cycle (HARP)"));
            }
        } else if is_zip_map {
            // Base family field and its per-member instruments.
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
                                &family_members,
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
                if let Some(members) = family_members.get(&base).cloned() {
                    let mut new_members = Vec::new();
                    for offset in 0..count {
                        let instrument = members[offset % members.len()].clone();
                        new_members.push(instrument.clone());
                        if let Some(row) = registry.get(registered + offset) {
                            instruments.insert(row.id.clone(), instrument);
                        }
                    }
                    if let Some(field) = &field {
                        family_members.insert(field.clone(), new_members);
                    }
                    coverage.zip_map_families.push(format!(
                        "{} <- {base}",
                        field.clone().unwrap_or_default()
                    ));
                } else {
                    coverage
                        .residual
                        .push(format!("zipMap base {base}: unknown members"));
                }
            } else {
                coverage
                    .residual
                    .push(format!("zipMap {field:?}: base not resolved"));
            }
        } else if single && registered < registry.len() {
            // Fresh-properties helpers (`wallVariant`) must not inherit.
            if !span_has_instrument(&parsed, &instructions, index)
                && span_calls_fresh_properties_helper(&jar, &parsed, &instructions, index)
            {
                instruments.insert(registry[registered].id.clone(), "HARP".to_string());
                coverage
                    .fresh_property_resets
                    .push(registry[registered].id.clone());
            } else if let Some(field) = &field {
                if let Some(instrument) = instruments.get(&registry[registered].id).cloned() {
                    family_members.insert(field.clone(), vec![instrument]);
                }
            }
        }
        if let Some(field) = field {
            if let Some(instrument) = instruments.get(&registry[registered].id).cloned() {
                family_members.entry(field).or_insert_with(|| vec![instrument]);
            }
        }
        registered += count;
        index += 1;
        let _ = descriptor;
    }
    Ok((instruments, coverage))
}

/// Collect the `NoteBlockInstrument` names referenced by the properties lambda
/// (the last functional argument) in first-appearance order.
fn instrument_cycle(
    class: &ClassFile,
    instructions: &[Ins],
    position: usize,
    jar: &Jar,
) -> Option<Vec<String>> {
    let mut start = 0usize;
    for index in (0..position).rev() {
        if instructions[index].opcode == 0xb2 {
            if let Some((owner, _, _)) = field_ref(class, instructions[index].index) {
                if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                    start = index + 1;
                    break;
                }
            }
        }
    }
    // Any functional argument of the family registration may carry the
    // instrument cycle (the material families put it in a shared
    // `(WeatherState) -> Properties` lambda, functional families in none).
    let mut targets = Vec::new();
    for instruction in &instructions[start..position] {
        if instruction.opcode == 0xba {
            if let Some((owner, name, descriptor)) = indy_target(class, instruction.index) {
                if owner == BLOCKS {
                    targets.push((name, descriptor));
                }
            }
        }
    }
    let parsed = jar.class(BLOCKS)?;
    for (name, descriptor) in targets {
        let Some(method) = parsed
            .methods
            .iter()
            .find(|method| method.name == name && method.descriptor == descriptor)
        else {
            continue;
        };
        let Some(code) = method.code.as_ref() else {
            continue;
        };
        let mut list: Vec<String> = Vec::new();
        for instruction in decode_code(&code.bytes) {
            if instruction.opcode == 0xb2 {
                if let Some((field_owner, field_name, _)) = field_ref(&parsed, instruction.index) {
                    if field_owner.ends_with(INSTRUMENT) && !list.contains(&field_name) {
                        list.push(field_name);
                    }
                }
            }
        }
        if list.len() >= 4 {
            return Some(list);
        }
    }
    None
}
fn lambda_base_field(
    jar: &Jar,
    owner: &str,
    name: &str,
    descriptor: &str,
    family_members: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    let class = jar.class(owner)?;
    let method = class
        .methods
        .iter()
        .find(|method| method.name == name && method.descriptor == descriptor)?;
    let code = method.code.as_ref()?;
    for instruction in decode_code(&code.bytes) {
        if instruction.opcode != 0xb2 {
            continue;
        }
        let Some((field_owner, field_name, _)) = field_ref(&class, instruction.index) else {
            continue;
        };
        if field_owner == BLOCKS && family_members.contains_key(&field_name) {
            return Some(field_name);
        }
    }
    None
}

/// Whether the registration span itself sets `Properties.instrument` or copies
/// another block's properties (then the "fresh helper" rule must not fire).
fn span_has_instrument(class: &ClassFile, instructions: &[Ins], position: usize) -> bool {
    let mut start = 0usize;
    for index in (0..position).rev() {
        if instructions[index].opcode == 0xb2 {
            if let Some((owner, _, _)) = field_ref(class, instructions[index].index) {
                if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                    start = index + 1;
                    break;
                }
            }
        }
    }
    for instruction in &instructions[start..position] {
        if instruction.opcode == 0xb6 {
            if let Some((owner, name, _)) = method_ref(class, instruction.index) {
                if owner.ends_with("Properties") && name == "instrument" {
                    return true;
                }
            }
        }
        if instruction.opcode == 0xb8 {
            if let Some((owner, name, _)) = method_ref(class, instruction.index) {
                if owner.ends_with("Properties")
                    && matches!(name.as_str(), "ofFullCopy" | "ofLegacyCopy")
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether the registration chain calls a `Blocks.*Properties` helper that
/// creates fresh properties (no `ofFullCopy`/`ofLegacyCopy`).
fn span_calls_fresh_properties_helper(
    jar: &Jar,
    class: &ClassFile,
    instructions: &[Ins],
    position: usize,
) -> bool {
    let mut start = 0usize;
    for index in (0..position).rev() {
        if instructions[index].opcode == 0xb2 {
            if let Some((owner, _, _)) = field_ref(class, instructions[index].index) {
                if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                    start = index + 1;
                    break;
                }
            }
        }
    }
    for instruction in &instructions[start..position] {
        if instruction.opcode != 0xb8 {
            continue;
        }
        let Some((owner, name, descriptor)) = method_ref(class, instruction.index) else {
            continue;
        };
        if owner != BLOCKS
            || !descriptor.ends_with(
                ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
            )
        {
            continue;
        }
        let Some(parsed) = jar.class(&owner) else {
            continue;
        };
        let Some(method) = parsed
            .methods
            .iter()
            .find(|method| method.name == name && method.descriptor == descriptor)
        else {
            continue;
        };
        let Some(code) = method.code.as_ref() else {
            continue;
        };
        let mut fresh = false;
        let mut copies = false;
        let mut sets_instrument = false;
        for instruction in decode_code(&code.bytes) {
            match instruction.opcode {
                0xb8 => {
                    if let Some((call_owner, call_name, _)) = method_ref(&parsed, instruction.index) {
                        if call_owner.ends_with("Properties") {
                            match call_name.as_str() {
                                "of" => fresh = true,
                                "ofFullCopy" | "ofLegacyCopy" => copies = true,
                                _ => {}
                            }
                        }
                    }
                }
                0xb2 => {
                    if let Some((field_owner, _, _)) = field_ref(&parsed, instruction.index) {
                        if field_owner.ends_with(INSTRUMENT) {
                            sets_instrument = true;
                        }
                    }
                }
                0xb6 => {
                    if let Some((call_owner, call_name, _)) = method_ref(&parsed, instruction.index) {
                        if call_owner.ends_with("Properties") && call_name == "instrument" {
                            sets_instrument = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if fresh && !copies && !sets_instrument {
            return true;
        }
    }
    false
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
