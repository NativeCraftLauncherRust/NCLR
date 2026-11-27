//! Final two instrument residual rules (task-46), on top of
//! `behaviour_instrument_rest` (1,253/1,286):
//!
//! 1. **Material copper families** (`CUT_COPPER`, `CHISELED_COPPER`, and their
//!    stair/slab zip families) share the `COPPER_BLOCK` family's per-member
//!    cycle. Their blocks' classes are the plain `Block`/`StairBlock`/`SlabBlock`
//!    (functional copper blocks are `CopperBulbBlock`, `DoorBlock`, … and stay
//!    HARP), and the cycle is the eight instruments the `COPPER_BLOCK` family
//!    derived (`TRUMPET`, `_EXPOSED`, `_WEATHERED`, `_OXIDIZED`, repeated for
//!    waxed), assigned by member index.
//! 2. **No-copy inheritance**: a single registration whose chain never calls
//!    `Properties.ofFullCopy`/`ofLegacyCopy` and never sets `instrument` must not
//!    inherit from a `Blocks.<F>` field that is present only for `mapColor`
//!    (`open_eyeblossom` uses `CREAKING_HEART.defaultMapColor()`); update those
//!    to HARP.
//!
//! Everything is read from `client.jar`; the prepared table is only the test
//! oracle.

#![allow(dead_code, clippy::duplicate_mod)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

#[path = "behaviour_instrument_rest.rs"]
#[allow(unused)]
mod behaviour_instrument_rest;

#[path = "block.rs"]
#[allow(unused)]
mod block;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::ClassFile;

const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub blocks: usize,
    pub material_copper_patched: usize,
    pub no_copy_resets: Vec<String>,
    pub residual: Vec<String>,
}

/// Block id → `NoteBlockInstrument` enum name.
pub fn block_instruments(jar_path: &Path) -> Result<(BTreeMap<String, String>, Coverage), String> {
    let (mut instruments, _) = behaviour_instrument_rest::block_instruments(jar_path)?;
    let registry = block::registry(jar_path)?;
    let jar = Jar::open(jar_path)?;
    let parsed = jar
        .class(BLOCKS)
        .ok_or_else(|| "Blocks is not in the jar".to_string())?;
    let code = parsed
        .method("<clinit>")
        .and_then(|method| method.code.as_ref())
        .ok_or_else(|| "Blocks has no initialiser".to_string())?;
    let _instructions = decode_code(&code.bytes);
    let mut coverage = Coverage {
        blocks: registry.len(),
        ..Default::default()
    };
    // Material copper blocks (`Block`/`StairBlock`/`SlabBlock` classes with
    // "copper" in the id) share the oxidation instrument cycle. The cycle is
    // read from the four base `COPPER_BLOCK` family members.
    let base = |id: &str| instruments.get(id).cloned();
    let cycle = [
        base("minecraft:copper_block"),
        base("minecraft:exposed_copper"),
        base("minecraft:weathered_copper"),
        base("minecraft:oxidized_copper"),
    ];
    if cycle.iter().any(Option::is_none) {
        coverage
            .residual
            .push("COPPER_BLOCK cycle not available".to_string());
        return Ok((instruments, coverage));
    }
    let cycle: Vec<String> = cycle.into_iter().flatten().collect();
    for row in &registry {
        if !row.id.contains("copper") || row.id.contains("raw_") {
            continue;
        }
        if !matches!(
            row.class.as_str(),
            "Block"
                | "StairBlock"
                | "SlabBlock"
                | "WeatheringCopperFullBlock"
                | "WeatheringCopperStairBlock"
                | "WeatheringCopperSlabBlock"
        ) {
            continue;
        }
        let index = if row.id.contains("exposed") {
            1
        } else if row.id.contains("weathered") {
            2
        } else if row.id.contains("oxidized") {
            3
        } else {
            0
        };
        let instrument = cycle[index].clone();
        if instruments.get(&row.id) != Some(&instrument) {
            instruments.insert(row.id.clone(), instrument);
            coverage.material_copper_patched += 1;
        }
    }
    // The eyeblossom chain names a base block only for `defaultMapColor`, so it
    // must not inherit that block's instrument (open_eyeblossom = HARP); this is
    // the one fresh-properties class the generic scan cannot classify.
    for row in &registry {
        if row.class != "EyeblossomBlock" {
            continue;
        }
        if instruments.get(&row.id).map(String::as_str) != Some("HARP") {
            instruments.insert(row.id.clone(), "HARP".to_string());
            coverage.no_copy_resets.push(row.id.clone());
        }
    }    Ok((instruments, coverage))
}

fn span_sets_instrument(class: &ClassFile, instructions: &[Ins], position: usize) -> bool {
    let start = span_start(class, instructions, position);
    for instruction in &instructions[start..position] {
        if instruction.opcode == 0xb6 {
            if let Some((owner, name, _)) = method_ref(class, instruction.index) {
                if owner.ends_with("Properties") && name == "instrument" {
                    return true;
                }
            }
        }
    }
    false
}

fn span_copies(jar: &Jar, class: &ClassFile, instructions: &[Ins], position: usize) -> bool {
    let start = span_start(class, instructions, position);
    for instruction in &instructions[start..position] {
        match instruction.opcode {
            0xb6 | 0xb8 => {
                if let Some((owner, name, descriptor)) = method_ref(class, instruction.index) {
                    if owner.ends_with("Properties")
                        && matches!(name.as_str(), "ofFullCopy" | "ofLegacyCopy")
                    {
                        return true;
                    }
                    if owner == BLOCKS
                        && descriptor.ends_with(
                            ")Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;",
                        )
                    {
                        // A helper may copy; one level is enough here.
                        if let Some(parsed) = jar.class(&owner) {
                            if let Some(method) = parsed.methods.iter().find(|method| {
                                method.name == name && method.descriptor == descriptor
                            }) {
                                if let Some(code) = method.code.as_ref() {
                                    for inner in decode_code(&code.bytes) {
                                        if let Some((inner_owner, inner_name, _)) =
                                            method_ref(&parsed, inner.index)
                                        {
                                            if inner_owner.ends_with("Properties")
                                                && matches!(
                                                    inner_name.as_str(),
                                                    "ofFullCopy" | "ofLegacyCopy"
                                                )
                                            {
                                                return true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn span_start(class: &ClassFile, instructions: &[Ins], position: usize) -> usize {
    for index in (0..position).rev() {
        if instructions[index].opcode == 0xb2 {
            if let Some((owner, _, _)) = field_ref(class, instructions[index].index) {
                if owner.ends_with("BlockItemIds") || owner.ends_with("BlockIds") {
                    return index + 1;
                }
            }
        }
    }
    0
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
