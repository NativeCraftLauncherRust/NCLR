//! Instrument column and simple flag bits of `block-behaviours.json#states`.
//!
//! The prepared state rows are
//! `[sturdy, flags, fluidId, fluidAmount, falling, weak, strong, signalError, instrument]`.
//! This module derives the parts that are pure static configuration:
//!
//! * **instrument** — `BlockBehaviour.Properties.instrument(NoteBlockInstrument)`
//!   on the registration chain (default `HARP`), inherited through
//!   `ofFullCopy`/`ofLegacyCopy` and the stair/slab/wall helpers, and evaluated
//!   inside the colour-family properties lambdas. Vanilla's `state.instrument()`
//!   is the block-level value, so it is constant per block.
//! * **flag bit 2 (`isAir`)** — blocks whose class chain is `AirBlock`
//!   (`air`, `cave_air`, `void_air`).
//! * **flag bit 1 (`isSolid`)** — `hasCollision && !isAir` (the properties flag
//!   from `shape_flags::has_collision`).
//!
//! The remaining flag bits need runtime predicates/shapes and are reported, not
//! guessed: `redstoneConductor` (`Properties.isRedstoneConductor` predicates),
//! `signalSource`/`analogOutput` (block-class overrides), `collisionEmpty`
//! (shape engine), `suffocating` (`Properties.isSuffocating` predicates), plus
//! the whole `sturdy`/fluid/signal half of the row.

#![allow(dead_code, clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "shape_flags.rs"]
#[allow(unused)]
mod shape_flags;

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
/// `NoteBlockInstrument` simple names are the exported instrument names.
const INSTRUMENT: &str = "NoteBlockInstrument";

/// What was derived, and what still needs runtime predicates.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub instrument_from_chain: usize,
    pub instrument_default: usize,
    pub gaps: Vec<&'static str>,
}

/// Block id → `NoteBlockInstrument` enum name.
pub fn block_instruments(jar_path: &Path) -> Result<(BTreeMap<String, String>, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let scan = Scan::run(&jar)?;
    if scan.instruments.len() != registry.len() {
        return Err(format!(
            "instrument scan produced {} blocks, registry has {}",
            scan.instruments.len(),
            registry.len()
        ));
    }
    let mut out = BTreeMap::new();
    let mut coverage = Coverage {
        blocks: registry.len(),
        gaps: vec![
            "flags.redstoneConductor (Properties.isRedstoneConductor predicates)",
            "flags.signalSource/analogOutput (block-class overrides)",
            "flags.collisionEmpty (shape engine)",
            "flags.suffocating (Properties.isSuffocating predicates)",
            "sturdy bitmask / fluid / weak-strong signals",
        ],
        ..Default::default()
    };
    for (row, instrument) in registry.iter().zip(&scan.instruments) {
        out.insert(row.id.clone(), instrument.clone());
        if scan.from_chain[row_index(&registry, &row.id)] {
            coverage.instrument_from_chain += 1;
        } else {
            coverage.instrument_default += 1;
        }
    }
    Ok((out, coverage))
}

fn row_index(registry: &[block::Block], id: &str) -> usize {
    registry
        .iter()
        .position(|row| row.id == id)
        .unwrap_or(0)
}

/// The flag bits this module can derive: `(value, mask)` per state.
///
/// Bit 1 is `isSolid`, bit 2 is `isAir`; the mask marks which bits are derived.
pub fn state_flags(jar_path: &Path) -> Result<Vec<(i32, i32)>, String> {
    let registry = block::registry(jar_path)?;
    let table = states::state_table(jar_path)?;
    let has_collision = shape_flags::has_collision(jar_path)?;
    if has_collision.len() != registry.len() {
        return Err("collision flags and registry disagree".to_string());
    }
    let mut out = Vec::with_capacity(table.state_count());
    for (index, row) in table.blocks.iter().enumerate() {
        let air = registry[index]
            .chain
            .iter()
            .any(|class| class == "AirBlock");
        let solid = has_collision[index] && !air;
        let mut bits = 0i32;
        if solid {
            bits |= 1;
        }
        if air {
            bits |= 2;
        }
        for _ in 0..row.state_count {
            out.push((bits, 3));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// `Blocks.<clinit>` instrument scan
// ---------------------------------------------------------------------------

struct Scan {
    instruments: Vec<String>,
    from_chain: Vec<bool>,
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
            0xb9 => {
                index = u16::from_be_bytes([
                    *bytes.get(at + 1).unwrap_or(&0),
                    *bytes.get(at + 2).unwrap_or(&0),
                ]) as usize;
                5
            }
            0xba => {
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

impl Scan {
    fn run(jar: &Jar) -> Result<Scan, String> {
        let bytes = jar
            .classes
            .get(BLOCKS)
            .ok_or_else(|| "Blocks is not in the jar".to_string())?;
        let parsed = classfile::parse(bytes).map_err(|error| format!("Blocks: {error}"))?;
        let code = parsed
            .method("<clinit>")
            .and_then(|method| method.code.as_ref())
            .ok_or_else(|| "Blocks has no initialiser".to_string())?;
        let instructions = decode_code(&code.bytes);
        let mut field_instruments: BTreeMap<String, String> = BTreeMap::new();
        let mut instruments = Vec::new();
        let mut from_chain = Vec::new();
        let mut field_owner: Option<usize> = None;
        for (position, instruction) in instructions.iter().enumerate() {
            if instruction.opcode == 0xb8 {
                let Some((owner, name, descriptor)) = method_ref(&parsed, instruction.index) else {
                    continue;
                };
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
                if !single && !family {
                    continue;
                }
                let (instrument, direct) = instrument_of_span(
                    jar,
                    &parsed,
                    &instructions,
                    position,
                    &field_instruments,
                    &name,
                );
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
                    instruments.push(instrument.clone());
                    from_chain.push(direct);
                }
                field_owner = Some(position);
                let _ = descriptor;
            }
            if instruction.opcode == 0xb3 {
                if let Some(call_position) = field_owner.take() {
                    let _ = call_position;
                    if let Some((owner, field, _)) = field_ref(&parsed, instruction.index) {
                        if owner == BLOCKS {
                            if let Some(last) = instruments.last().cloned() {
                                field_instruments.insert(field, last);
                            }
                        }
                    }
                }
            }
        }
        Ok(Scan {
            instruments,
            from_chain,
        })
    }
}

/// Instrument configured by the registration chain ending at `position`.
fn instrument_of_span(
    jar: &Jar,
    class: &ClassFile,
    instructions: &[Ins],
    position: usize,
    field_instruments: &BTreeMap<String, String>,
    registration: &str,
) -> (String, bool) {
    // Find where this registration's chain starts (the block-id push) and walk
    // forward: `Properties.instrument` takes its enum argument from the
    // instruction *before* the call, so a backward scan cannot see it.
    let mut start = 0usize;
    for (index, instruction) in instructions[..position].iter().enumerate().rev() {
        if instruction.opcode == 0xb2 {
            if let Some((owner, _, _)) = field_ref(class, instruction.index) {
                if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                    start = index + 1;
                    break;
                }
            }
        }
    }
    let mut current: Option<String> = None;
    let mut pending_enum: Option<String> = None;
    let mut pending_block_field: Option<String> = None;
    let mut pending_collection: Option<String> = None;
    let mut direct = false;
    for instruction in &instructions[start..position] {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = field_ref(class, instruction.index) {
                    if owner.ends_with(INSTRUMENT) {
                        pending_enum = Some(name);
                    } else if owner == BLOCKS {
                        pending_block_field = Some(name);
                    } else if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                        pending_collection = Some(name);
                    }
                }
            }
            0xb6 => {
                if let Some((owner, name, _)) = method_ref(class, instruction.index) {
                    if owner.ends_with("Properties") {
                        match name.as_str() {
                            "instrument" => {
                                if let Some(enum_name) = pending_enum.take() {
                                    current = Some(enum_name);
                                    direct = true;
                                }
                            }
                            "ofFullCopy" | "ofLegacyCopy" => {
                                if let Some(field) = pending_block_field.take() {
                                    if let Some(instrument) = field_instruments.get(&field) {
                                        current = Some(instrument.clone());
                                        direct = true;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            0xb8 => {
                if let Some((owner, name, descriptor)) = method_ref(class, instruction.index) {
                    if owner == BLOCKS
                        && descriptor.ends_with(
                            ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
                        )
                    {
                        if let Some(instrument) = method_instrument(
                            jar,
                            &owner,
                            &name,
                            &descriptor,
                            field_instruments,
                            0,
                        ) {
                            current = Some(instrument);
                            direct = true;
                        }
                    }
                }
            }
            0xba => {
                if let Some((owner, name, descriptor)) = indy_target(class, instruction.index) {
                    if !(owner == BLOCKS
                        && matches!(
                            name.as_str(),
                            "register"
                                | "registerStair"
                                | "registerSlab"
                                | "registerWall"
                                | "registerLegacyStair"
                        ))
                    {
                        if let Some(instrument) = method_instrument(
                            jar,
                            &owner,
                            &name,
                            &descriptor,
                            field_instruments,
                            0,
                        ) {
                            current = Some(instrument);
                            direct = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // The stair/slab/wall helpers and the copy-based single registrations
    // (e.g. `chiseled_deepslate` via `Properties.ofFullCopy`) inherit the base
    // block's instrument; wall heads are a documented residual (vanilla's
    // `wallVariant()` does not copy it).
    if current.is_none() {
        if let Some(field) = pending_block_field.as_ref().filter(|field| field_instruments.contains_key(*field)) {
            if let Some(instrument) = field_instruments.get(field) {
                return (instrument.clone(), true);
            }
        }
        // Colour-family variants (e.g. wool stairs) inherit the base family's
        // instrument; the collection id field shares the Blocks field name.
        if let Some(collection) = pending_collection {
            if let Some(instrument) = field_instruments.get(&collection) {
                return (instrument.clone(), true);
            }
        }
    }
    match current {
        Some(instrument) => (instrument, direct),
        None => ("HARP".to_string(), false),
    }
}
/// Instrument configured inside a helper/lambda method body.
fn method_instrument(
    jar: &Jar,
    owner: &str,
    name: &str,
    descriptor: &str,
    field_instruments: &BTreeMap<String, String>,
    depth: usize,
) -> Option<String> {
    if depth > 4 {
        return None;
    }
    let class = jar.class(owner)?;
    let method = class
        .methods
        .iter()
        .find(|method| method.name == name && method.descriptor == descriptor)?;
    let code = method.code.as_ref()?;
    let instructions = decode_code(&code.bytes);
    let mut current: Option<String> = None;
    let mut pending_enum: Option<String> = None;
    let mut pending_field: Option<String> = None;
    for instruction in &instructions {
        match instruction.opcode {
            0xb2 => {
                if let Some((field_owner, field_name, _)) = field_ref(&class, instruction.index) {
                    if field_owner.ends_with(INSTRUMENT) {
                        pending_enum = Some(field_name);
                    } else if field_owner == BLOCKS {
                        pending_field = Some(field_name);
                    }
                }
            }
            0xb6 => {
                if let Some((method_owner, method_name, _)) =
                    method_ref(&class, instruction.index)
                {
                    if method_owner.ends_with("Properties") {
                        match method_name.as_str() {
                            "instrument" => {
                                if let Some(enum_name) = pending_enum.take() {
                                    current = Some(enum_name);
                                }
                            }
                            "ofFullCopy" | "ofLegacyCopy" => {
                                if let Some(field) = pending_field.take() {
                                    if let Some(instrument) = field_instruments.get(&field) {
                                        current = Some(instrument.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            0xb8 => {
                if let Some((call_owner, call_name, call_descriptor)) =
                    method_ref(&class, instruction.index)
                {
                    if call_owner == BLOCKS
                        && call_descriptor.ends_with(
                            ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
                        )
                    {
                        if let Some(instrument) = method_instrument(
                            jar,
                            &call_owner,
                            &call_name,
                            &call_descriptor,
                            field_instruments,
                            depth + 1,
                        ) {
                            current = Some(instrument);
                        }
                    }
                }
            }
            0xba => {
                if let Some((lambda_owner, lambda_name, lambda_descriptor)) =
                    indy_target(&class, instruction.index)
                {
                    if let Some(instrument) = method_instrument(
                        jar,
                        &lambda_owner,
                        &lambda_name,
                        &lambda_descriptor,
                        field_instruments,
                        depth + 1,
                    ) {
                        current = Some(instrument);
                    }
                }
            }
            _ => {}
        }
    }
    current
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
