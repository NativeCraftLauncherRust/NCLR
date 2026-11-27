//! `_shared/gameplay.compact.json`, closed: the last 218 blocks and the residual per-block
//! fields, on top of [`gameplay_final`].
//!
//! `gameplay_final` reads `Blocks.<clinit>`'s inline property chains and the
//! `ColorCollection.registerBlocks` families. Two registration shapes are left:
//!
//! 1. **`ColorCollection.zipMap` families** (bed/wool stairs and slabs, stained glass,
//!    terracotta, shulker boxes, candles, banners, concrete, ...). Their call site is
//!    `zipMap(ColorCollection.VALUES, BlockItemIds.<FAMILY>, lambda)` and the lambda body is
//!    just `registerStair(id, WOOL.pick(colour))` / `registerSlab(...)`: the `Properties` are
//!    `ofFullCopy`/`ofLegacyCopy` of the colour family the callback picks from
//!    `Blocks.<FAMILY>`. The row of `<colour>_<suffix>` is therefore exactly the row of its
//!    base block `<colour>_<base suffix>`.
//! 2. **Helper-registered blocks** whose chain lives in a `Blocks` helper instead of
//!    `<clinit>` (stairs, slabs, walls, fences, doors, ...: `registerStair(id, OAK_PLANKS)`),
//!    and the `Properties.ofFullCopy(Blocks.X)` chains. Their fields are a full copy of the
//!    block the helper was handed.
//!
//! Both come down to "the fields are a copy of the base block", so this module copies the base
//! block's row (including `can_occlude`, a block flag plus the shape check, which the copies
//! keep). Everything is reported: a block whose base cannot be identified keeps its `null` row
//! and is listed in [`Coverage::unresolved`]. Nothing here reads `ship.hzr3`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::classfile;

#[path = "gameplay_final.rs"]
#[allow(dead_code)]
mod base;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = base::TABLE;

/// The fields the reference has and the modules do not derive yet (the light engine, the
/// selection-outline geometry and the item registry mapping are not ported).
pub const MISSING: [&str; 11] = [
    "dampening",
    "emission",
    "item",
    "outline",
    "outline_edges",
    "outline_offset",
    "random_ticks",
    "render_shape",
    "shape_occlusion",
    "skylight_down",
    "solid_render",
];

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    /// Blocks whose base block could not be identified (their row stays `null`).
    pub unresolved: Vec<String>,
}

// ---------------------------------------------------------------------------
// Bytecode helpers (the shared reader is at the crate root)
// ---------------------------------------------------------------------------

fn member(class: &classfile::ClassFile, index: usize) -> Option<(String, String, String)> {
    let (owner_ref, name_ref, kind) = match class.constants.get(index)? {
        classfile::Constant::Fieldref { class, name, kind } => (class, name, kind),
        classfile::Constant::Methodref { class, name, kind } => (class, name, kind),
        classfile::Constant::InterfaceMethodref { class, name, kind } => (class, name, kind),
        _ => return None,
    };
    let owner_index: usize = owner_ref.trim_start_matches('#').parse().ok()?;
    let owner = match class.constants.get(owner_index)? {
        classfile::Constant::Class(reference) => utf8_at(class, reference)?,
        _ => return None,
    };
    let name_index: usize = name_ref.trim_start_matches('#').parse().ok()?;
    let (name, descriptor) = match class.constants.get(name_index)? {
        classfile::Constant::NameAndType { name, kind } => {
            (utf8_at(class, name)?, utf8_at(class, kind)?)
        }
        classfile::Constant::Utf8(text) => (text.clone(), kind.clone()),
        _ => return None,
    };
    Some((owner, name, descriptor))
}

fn utf8_at(class: &classfile::ClassFile, reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match class.constants.get(index)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
}

fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let length = match opcode {
            0x10 => 2,
            0x11 => 3,
            0x12 => {
                index = bytes.get(at + 1).map(|value| *value as usize);
                2
            }
            0x13 | 0x14 | 0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]) as usize);
                3
            }
            0xb9 | 0xba => {
                index = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]) as usize);
                5
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => {
                index = bytes.get(at + 1).map(|value| *value as usize);
                2
            }
            0x84 => 3,
            0x99..=0xa8 | 0xc6 | 0xc7 => 3,
            0xc4 => 4,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            0xaa => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base_at = at + 1 + padding;
                let low = bytes
                    .get(base_at + 4..base_at + 8)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                let high = bytes
                    .get(base_at + 8..base_at + 12)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                let (Some(low), Some(high)) = (low, high) else { break };
                let count = (high as i64 - low as i64 + 1).max(0) as usize;
                1 + padding + 12 + count * 4
            }
            0xab => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base_at = at + 1 + padding;
                let pairs = bytes
                    .get(base_at + 4..base_at + 8)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                    .unwrap_or(0)
                    .max(0) as usize;
                1 + padding + 8 + pairs * 8
            }
            _ => 1,
        };
        out.push(Instruction { opcode, index });
        at += length;
    }
    out
}

/// The implementation method behind an `invokedynamic`: `(owner, name, descriptor)`.
fn lambda_ref(class: &classfile::ClassFile, index: usize) -> Option<(String, String, String)> {
    let bootstrap = match class.constants.get(index)? {
        classfile::Constant::InvokeDynamic { bootstrap, .. } => *bootstrap,
        _ => return None,
    };
    let row = class.bootstraps.get(bootstrap)?;
    for argument in &row.arguments {
        let Some(classfile::Constant::MethodHandle { reference }) =
            class.constants.get(*argument)
        else {
            continue;
        };
        if let Some(found) = member(class, *reference) {
            return Some(found);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// The jar
// ---------------------------------------------------------------------------

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    by_short: BTreeMap<String, String>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut files = BTreeMap::new();
        let mut by_short = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|e| e.to_string())?.to_string();
            let Some(internal) = name.strip_suffix(".class") else {
                continue;
            };
            let mut data = Vec::new();
            entry.read_to_end(&mut data).map_err(|e| format!("{name}: {e}"))?;
            by_short.insert(short(internal).to_string(), internal.to_string());
            files.insert(internal.to_string(), data);
        }
        Ok(Jar { files, by_short })
    }

    fn class(&self, name: &str) -> Option<classfile::ClassFile> {
        let bytes = if name.contains('/') {
            self.files.get(name)
        } else {
            self.by_short
                .get(name)
                .and_then(|internal| self.files.get(internal))
        }?;
        classfile::parse(bytes).ok()
    }
}

// ---------------------------------------------------------------------------
// The id tables
// ---------------------------------------------------------------------------

/// `BlockItemIds`: field -> id for plain ids, field -> suffix for the colour collections.
#[derive(Default)]
struct Ids {
    plain: BTreeMap<String, String>,
    colour: BTreeMap<String, String>,
}

fn read_ids(class: &classfile::ClassFile) -> Ids {
    let mut ids = Ids::default();
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return ids;
    };
    let mut pending: Option<(String, bool)> = None;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) =
                    instruction.index.and_then(|index| class.constants.get(index))
                {
                    if let Some(text) = utf8_at(class, reference) {
                        pending = Some((text, false));
                    }
                }
            }
            0xb8 => {
                if let Some((owner, name, _)) =
                    instruction.index.and_then(|index| member(class, index))
                {
                    if short(&owner) == "BlockItemIds" {
                        if let Some((text, _)) = pending.clone() {
                            pending = Some((text, name == "createSimpleColored"));
                        }
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(class, index))
                {
                    if short(&owner) == "BlockItemIds" {
                        if let Some((text, colour)) = pending.take() {
                            if colour {
                                ids.colour.insert(field, text);
                            } else {
                                ids.plain.insert(field, text);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// The scan
// ---------------------------------------------------------------------------

/// A `zipMap` family: the block id suffix and the base collection's `Blocks` field.
#[derive(Clone, Debug)]
struct ZipFamily {
    suffix: String,
    base_field: String,
}

/// `id -> base id`, plus the `zipMap` families.
#[derive(Default)]
struct Bases {
    of: BTreeMap<String, String>,
    zip: Vec<ZipFamily>,
}

fn scan(class: &classfile::ClassFile, ids: &Ids) -> Bases {
    let mut bases = Bases::default();
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return bases;
    };
    let instructions = decode(&code.bytes);
    let mut pending: Option<String> = None;
    let mut window: Vec<usize> = Vec::new();
    let mut colour_pending: Option<String> = None;
    let mut indy: Vec<(String, String, String)> = Vec::new();
    for (slot, instruction) in instructions.iter().enumerate() {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, descriptor)) = instruction.index.and_then(|index| member(class, index)) {
                    if short(&owner) == "BlockItemIds" {
                        if descriptor.contains("ColorCollection") {
                            colour_pending = Some(name);
                            indy.clear();
                        } else if descriptor.contains("references/BlockItemId;") {
                            pending = Some(name);
                            window.clear();
                        }
                    }
                }
            }
            0xba => {
                if let Some(reference) = lambda_ref(class, instruction.index.unwrap_or(0)) {
                    indy.push(reference);
                }
            }
            0xb8 => {
                let Some((owner, name, _)) = instruction.index.and_then(|index| member(class, index))
                else {
                    continue;
                };
                if short(&owner) == "ColorCollection" && name == "zipMap" {
                    if let Some(ids_field) = colour_pending.take() {
                        if let Some(suffix) = ids.colour.get(&ids_field).cloned() {
                            if let Some((_, method_name, _)) = indy.last() {
                                if let Some(base_field) = base_collection(class, method_name) {
                                    bases.zip.push(ZipFamily { suffix, base_field });
                                }
                            }
                        }
                    }
                    indy.clear();
                }
                if short(&owner) == "Blocks" && name.starts_with("register") {
                    if let Some(ids_field) = pending.take() {
                        if let Some(block_id) = ids.plain.get(&ids_field) {
                            let base = if name == "register" {
                                copy_target(class, &instructions, &window, slot)
                            } else {
                                // A helper's second argument is the block it copies.
                                previous_block_field(class, &instructions, &window, slot)
                            };
                            if let Some(base_field) = base {
                                if let Some(base_id) = field_id(&base_field, ids) {
                                    bases.of.insert(format!("minecraft:{block_id}"), base_id);
                                }
                            }
                        }
                    }
                    window.clear();
                }
            }
            _ => {}
        }
        window.push(slot);
    }
    bases
}

/// The `Blocks.<FIELD>` argument of the `ofFullCopy`/`ofLegacyCopy` call in a window.
fn copy_target(
    class: &classfile::ClassFile,
    instructions: &[Instruction],
    window: &[usize],
    end: usize,
) -> Option<String> {
    let mut range: Vec<usize> = window.to_vec();
    range.push(end);
    for slot in range.iter().rev().take(100) {
        let instruction = instructions.get(*slot)?;
        if instruction.opcode != 0xb8 {
            continue;
        }
        let (owner, name, _) = member(class, instruction.index?)?;
        if short(&owner) != "Properties" && short(&owner) != "BlockBehaviour$Properties" {
            continue;
        }
        if name != "ofFullCopy" && name != "ofLegacyCopy" {
            continue;
        }
        if let Some(field) = previous_block_field(class, instructions, window, *slot) {
            return Some(field);
        }
    }
    None
}

/// The last `getstatic Blocks.<FIELD>:Block` before `end` in the window.
fn previous_block_field(
    class: &classfile::ClassFile,
    instructions: &[Instruction],
    window: &[usize],
    end: usize,
) -> Option<String> {
    let mut range: Vec<usize> = window.to_vec();
    range.push(end);
    for slot in range.iter().rev().take(60) {
        let instruction = instructions.get(*slot)?;
        if instruction.opcode != 0xb2 {
            continue;
        }
        let (owner, field, descriptor) = member(class, instruction.index?)?;
        if short(&owner) == "Blocks" && descriptor == "Lnet/minecraft/world/level/block/Block;" {
            return Some(field);
        }
    }
    None
}

/// The base `Blocks.<FIELD>` a `zipMap` callback picks from.
fn base_collection(class: &classfile::ClassFile, method_name: &str) -> Option<String> {
    let method = class.methods.iter().find(|method| method.name == method_name)?;
    let code = method.code.as_ref()?;
    for instruction in decode(&code.bytes) {
        if instruction.opcode != 0xb2 {
            continue;
        }
        let (owner, field, descriptor) = member(class, instruction.index?)?;
        if short(&owner) == "Blocks" && descriptor.contains("ColorCollection") {
            return Some(field);
        }
    }
    None
}

/// A `Blocks.<FIELD>`'s registry id.
fn field_id(field: &str, ids: &Ids) -> Option<String> {
    if let Some(id) = ids.plain.get(field) {
        return Some(format!("minecraft:{id}"));
    }
    Some(format!("minecraft:{}", field.to_lowercase()))
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// Builds the table and returns the rendered compact document, with the copied rows filled in
/// from their base blocks.
fn build(jar_path: &Path) -> Result<(String, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let ids_class = jar
        .class("net/minecraft/references/BlockItemIds")
        .ok_or("the jar has no BlockItemIds class")?;
    let ids = read_ids(&ids_class);
    let blocks_class = jar
        .class("net/minecraft/world/level/block/Blocks")
        .ok_or("the jar has no Blocks class")?;
    let bases = scan(&blocks_class, &ids);
    let mut facts = base::facts(jar_path)?;
    let position: BTreeMap<String, usize> = facts
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.clone(), index))
        .collect();
    let slots: Vec<usize> = facts
        .fields
        .iter()
        .enumerate()
        .filter(|(_, field)| *field != "full_collision")
        .map(|(slot, _)| slot)
        .collect();
    // The first state of every block.
    let mut first = vec![usize::MAX; facts.blocks.len()];
    for (state, block) in facts.blocks_per_state.iter().enumerate() {
        if first[*block] == usize::MAX {
            first[*block] = state;
        }
    }
    fn row_missing(values: &[Vec<Value>], state: usize, slots: &[usize]) -> bool {
        slots.iter().any(|slot| {
            values
                .get(state)
                .and_then(|row| row.get(*slot))
                .is_none_or(Value::is_null)
        })
    }
    let unresolved = |facts: &Vec<Vec<Value>>, block: usize| -> bool {
        row_missing(facts, first[block], &slots)
    };
    // The wanted copy edges: the helper-registered blocks and the zipMap families.
    let mut wanted: BTreeMap<String, String> = bases.of.clone();
    for family in &bases.zip {
        let base_suffix = ids
            .colour
            .get(&family.base_field)
            .cloned()
            .unwrap_or_else(|| family.base_field.to_lowercase());
        for colour in [
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
        ] {
            wanted.insert(
                format!("minecraft:{colour}_{}", family.suffix),
                format!("minecraft:{colour}_{base_suffix}"),
            );
        }
    }
    // A copy may chain (stairs of stairs); resolve until nothing moves.
    let mut copied = 0usize;
    for _ in 0..6 {
        let mut progress = false;
        for (block, base_id) in &wanted {
            let (Some(block_index), Some(base_index)) = (
                position.get(block.as_str()).copied(),
                position.get(base_id.as_str()).copied(),
            ) else {
                continue;
            };
            if !unresolved(&facts.values, block_index) || unresolved(&facts.values, base_index) {
                continue;
            }
            let states: Vec<usize> = facts
                .blocks_per_state
                .iter()
                .enumerate()
                .filter(|(_, candidate)| **candidate == block_index)
                .map(|(state, _)| state)
                .collect();
            for state in &states {
                for slot in &slots {
                    facts.values[*state][*slot] = facts.values[first[base_index]][*slot].clone();
                }
            }
            copied += 1;
            progress = true;
        }
        if !progress {
            break;
        }
    }
    let unresolved_blocks: Vec<String> = (0..facts.blocks.len())
        .filter(|block| unresolved(&facts.values, *block))
        .map(|block| facts.blocks[block].clone())
        .collect();
    facts.uncovered = unresolved_blocks
        .iter()
        .map(|block| (block.clone(), "no base block identifies its row".to_string()))
        .collect();
    if std::env::var("GF_DEBUG").is_ok() {
        eprintln!(
            "DBG zip bases={} zip_families={} copied={} unresolved={}",
            bases.of.len(),
            bases.zip.len(),
            copied,
            unresolved_blocks.len()
        );
    }
    let rendered = base::render(&facts);
    let mut coverage = Coverage::default();
    coverage.produced.push(TABLE.to_string());
    coverage.unresolved = unresolved_blocks;
    coverage
        .missing
        .extend(MISSING.iter().map(|field| format!("{TABLE}#{field}")));
    if !coverage.unresolved.is_empty() {
        coverage.notes.push(format!(
            "{} of {} blocks still have no row",
            coverage.unresolved.len(),
            facts.blocks.len()
        ));
    }
    Ok((rendered, coverage))
}

/// A fast report of what the Blocks.<clinit> scan found (no geometry evaluation).
pub fn scan_report(jar_path: &Path) -> Result<String, String> {
    let jar = Jar::open(jar_path)?;
    let ids_class = jar
        .class("net/minecraft/references/BlockItemIds")
        .ok_or("the jar has no BlockItemIds class")?;
    let ids = read_ids(&ids_class);
    let blocks_class = jar
        .class("net/minecraft/world/level/block/Blocks")
        .ok_or("the jar has no Blocks class")?;
    let bases = scan(&blocks_class, &ids);
    let mut zip_calls = 0usize;
    let mut colour_gets = 0usize;
    if let Some(code) = blocks_class.method("<clinit>").and_then(|method| method.code.as_ref()) {
        for instruction in decode(&code.bytes) {
            if instruction.opcode != 0xb8 {
                continue;
            }
            if let Some((owner, name, _)) = instruction.index.and_then(|index| member(&blocks_class, index)) {
                if short(&owner) == "ColorCollection" && name == "zipMap" {
                    zip_calls += 1;
                }
            }
        }
    }
    for instruction in decode(blocks_class.method("<clinit>").and_then(|method| method.code.as_ref()).map(|code| code.bytes.as_slice()).unwrap_or(&[])) {
        if instruction.opcode != 0xb2 { continue; }
        if let Some((owner, _, descriptor)) = instruction.index.and_then(|index| member(&blocks_class, index)) {
            if short(&owner) == "BlockItemIds" && descriptor.contains("ColorCollection") { colour_gets += 1; }
        }
    }
    Ok(format!(
        "zip_calls={zip_calls} colour_gets={colour_gets} \
         ids={} colour_ids={} helper_bases={} zip_families={} sample={:?}",
        ids.plain.len(),
        ids.colour.len(),
        bases.of.len(),
        bases.zip.len(),
        bases.of.iter().take(6).collect::<Vec<_>>()
    ))
}

/// The compact document as JSON.
pub fn document(jar_path: &Path) -> Result<Value, String> {
    let (rendered, _) = build(jar_path)?;
    serde_json::from_str(&rendered).map_err(|error| error.to_string())
}

/// Builds the table and writes it below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let (rendered, coverage) = build(jar_path)?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, rendered).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(coverage)
}
