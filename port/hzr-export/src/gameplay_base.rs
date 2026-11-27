//! The gameplay base layer, fixed: `_shared/gameplay.compact.json` with the exact
//! `can_occlude` rule, the default sound type, and the `mapColor` function/`BlockIds` cases.
//!
//! [`base`] (`gameplay_final`) builds the per-block rows from the jar, but three of its
//! fields are wrong:
//!
//! * `can_occlude` was derived from the occlusion shape. The vanilla rule is the
//!   `BlockBehaviour.Properties.canOcclude` field: `true` unless the chain calls
//!   **`noOcclusion()` or `noCollision()`** (both clear it — `BlockStateBase` copies the
//!   field verbatim), so this module reads those calls instead of any shape;
//! * `sound` was left `null` when the chain did not call `sound(...)`. Vanilla's
//!   `Properties` default is **`SoundType.STONE`**, so every block without a call gets the
//!   stone sound;
//! * `map_color` was `0` whenever the chain passed a **function** (`mapColor(BlockState ->
//!   MapColor)`) or a `DyeColor`. This module reads the call: a constant `MapColor.X` is
//!   resolved through `MapColor`'s own constructor table, and a `DyeColor.getMapColor()` is
//!   resolved through `DyeColor`'s table, per colour.
//!
//! It also re-runs the "copied block" resolution (`registerStair`/`registerSlab`/... and the
//! `ColorCollection.zipMap` families) because those rows are copies of their base block, and
//! it accepts the `BlockIds.*` colour families (`createSimpleColored` over `ResourceKey`s)
//! that [`base`] does not reach.
//!
//! The selection outline, the light-engine fields and the item mapping stay out of reach and
//! are reported by [`Coverage`]. Nothing here reads `ship.hzr3`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::classfile;

#[path = "gameplay_final.rs"]
#[allow(dead_code)]
mod base;

#[path = "gameplay.rs"]
#[allow(dead_code)]
mod gameplay;

#[path = "shape_final.rs"]
#[allow(dead_code)]
mod shape_final;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = gameplay::TABLE;

/// The fields the reference has and this module does not derive yet.
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
    /// Blocks whose per-block row is still `null`.
    pub unresolved: Vec<String>,
    /// Blocks whose `mapColor` is a per-state function this module does not evaluate.
    pub map_color_functions: Vec<String>,
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
// The jar and the tables
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

/// `MapColor.<FIELD>` -> `col`.
fn map_colours(jar: &Jar) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/level/material/MapColor") else {
        return out;
    };
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return out;
    };
    let mut numbers: Vec<i32> = Vec::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => numbers.clear(),
            0x12 | 0x13 => {
                if let Some(classfile::Constant::Integer(value)) =
                    instruction.index.and_then(|index| class.constants.get(index))
                {
                    numbers.push(*value);
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction.index.and_then(|index| member(&class, index)) {
                    if short(&owner) == "MapColor" {
                        if let Some(col) = numbers.get(1) {
                            out.insert(field, *col as u32);
                        }
                        numbers.clear();
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `SoundEvents.<FIELD>` -> `minecraft:<id>`.
fn sound_events(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/sounds/SoundEvents") else {
        return out;
    };
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return out;
    };
    let mut id: Option<String> = None;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) =
                    instruction.index.and_then(|index| class.constants.get(index))
                {
                    if let Some(text) = utf8_at(&class, reference) {
                        id = Some(format!("minecraft:{text}"));
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction.index.and_then(|index| member(&class, index)) {
                    if short(&owner) == "SoundEvents" {
                        if let Some(id) = id.take() {
                            out.insert(field, id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `SoundType.<FIELD>` -> the JSON sound object.
fn sound_types(jar: &Jar, events: &BTreeMap<String, String>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/level/block/SoundType") else {
        return out;
    };
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return out;
    };
    let mut numbers: Vec<f32> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => {
                numbers.clear();
                used.clear();
            }
            0x0b..=0x0d => numbers.push(match instruction.opcode {
                0x0b => 0.0,
                0x0c => 1.0,
                _ => 2.0,
            }),
            0x12 | 0x13 => {
                if let Some(classfile::Constant::Float(value)) =
                    instruction.index.and_then(|index| class.constants.get(index))
                {
                    numbers.push(*value);
                }
            }
            0xb2 => {
                if let Some((owner, field, _)) = instruction.index.and_then(|index| member(&class, index)) {
                    if short(&owner) == "SoundEvents" {
                        used.push(field);
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction.index.and_then(|index| member(&class, index)) {
                    if short(&owner) == "SoundType" && used.len() == 5 {
                        let mut sound = serde_json::Map::new();
                        sound.insert(
                            "volume".to_string(),
                            float_value(numbers.first().copied().unwrap_or(1.0)),
                        );
                        sound.insert(
                            "pitch".to_string(),
                            float_value(numbers.get(1).copied().unwrap_or(1.0)),
                        );
                        for (slot, name) in
                            ["break", "step", "place", "hit", "fall"].iter().enumerate()
                        {
                            let id = events.get(&used[slot]).cloned().unwrap_or_default();
                            let mut event = serde_json::Map::new();
                            event.insert("id".to_string(), Value::String(id));
                            sound.insert(name.to_string(), Value::Object(event));
                        }
                        out.insert(field, Value::Object(sound));
                        numbers.clear();
                        used.clear();
                    }
                }
            }
            _ => {}
        }
    }
    // `EMPTY` is built by a factory, not `register`.
    out.entry("EMPTY".to_string())
        .or_insert_with(|| {
            let mut sound = serde_json::Map::new();
            sound.insert("volume".to_string(), float_value(1.0));
            sound.insert("pitch".to_string(), float_value(1.0));
            for name in ["break", "step", "place", "hit", "fall"] {
                let mut event = serde_json::Map::new();
                event.insert(
                    "id".to_string(),
                    Value::String("minecraft:empty".to_string()),
                );
                sound.insert(name.to_string(), Value::Object(event));
            }
            Value::Object(sound)
        });
    out
}

/// `DyeColor.<FIELD>` -> the first `MapColor` argument of its constructor.
fn dye_map_colours(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/item/DyeColor") else {
        return out;
    };
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return out;
    };
    let mut first: Option<String> = None;
    let mut seen_new = false;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => {
                seen_new = true;
                first = None;
            }
            0xb2 => {
                if let Some((owner, field, _)) = instruction.index.and_then(|index| member(&class, index)) {
                    if seen_new && short(&owner) == "MapColor" && first.is_none() {
                        first = Some(field);
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction.index.and_then(|index| member(&class, index)) {
                    if short(&owner) == "DyeColor" && seen_new {
                        if let Some(colour) = first.take() {
                            out.insert(field, colour);
                        }
                        seen_new = false;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `BlockItemIds`/`BlockIds`: field -> id, and field -> colour suffix.
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
                if let Some((owner, name, _)) = instruction.index.and_then(|index| member(class, index)) {
                    if short(&owner) == "BlockItemIds" || short(&owner) == "BlockIds" {
                        if let Some((text, _)) = pending.clone() {
                            pending = Some((text, name == "createSimpleColored"));
                        }
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field, _)) = instruction.index.and_then(|index| member(class, index)) {
                    if short(&owner) == "BlockItemIds" || short(&owner) == "BlockIds" {
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

fn float_value(value: f32) -> Value {
    let mut text = format!("{value}");
    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
        text.push_str(".0");
    }
    serde_json::from_str(&text).unwrap_or_else(|_| Value::from(value as f64))
}

const DYE_COLOURS: [&str; 16] = [
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

// ---------------------------------------------------------------------------
// The scan over `Blocks.<clinit>`
// ---------------------------------------------------------------------------

/// What one block's property chain said about the three fields this module fixes.
#[derive(Clone, Debug, Default)]
struct Chain {
    sound: Option<String>,
    no_collision: bool,
    no_occlusion: bool,
    /// The constant `MapColor.X` argument, when the chain passes one.
    map_color: Option<String>,
    /// The chain passes a `DyeColor` (families) instead of a map colour.
    dye: bool,
    /// The chain passes a per-state function.
    function: bool,
}

impl Chain {
    fn absorb(&mut self, other: &Chain) {
        self.sound = other.sound.clone().or(self.sound.clone());
        self.no_collision |= other.no_collision;
        self.no_occlusion |= other.no_occlusion;
        self.map_color = other.map_color.clone().or(self.map_color.clone());
    }
}

/// `Blocks.<clinit>` scan: one `Chain` per plain registration and per colour family.
#[derive(Default)]
struct Scanned {
    plain: BTreeMap<String, Chain>,
    /// family suffix -> the chain its properties lambda shows
    families: BTreeMap<String, Chain>,
    /// block id -> base block id (helper registrations and `zipMap` families)
    bases: BTreeMap<String, String>,
}

fn scan(class: &classfile::ClassFile, ids: &Ids) -> Scanned {
    let mut out = Scanned::default();
    let Some(code) = class.method("<clinit>").and_then(|method| method.code.as_ref()) else {
        return out;
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
                    if short(&owner) == "BlockItemIds" || short(&owner) == "BlockIds" {
                        if descriptor.contains("ColorCollection") {
                            colour_pending = Some(name);
                            indy.clear();
                        } else if descriptor.contains("BlockItemId;")
                            || descriptor.contains("resources/ResourceKey;")
                        {
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
                let Some((owner, name, _)) = instruction.index.and_then(|index| member(class, index)) else {
                    continue;
                };
                if short(&owner) == "ColorCollection" && name == "registerBlocks" {
                    if let Some(ids_field) = colour_pending.take() {
                        if let Some(suffix) = ids.colour.get(&ids_field) {
                            let mut chain = Chain::default();
                            if let Some((_, method_name, _)) = indy.last() {
                                chain = lambda_chain(class, method_name);
                            }
                            out.families.insert(suffix.clone(), chain);
                        }
                    }
                    indy.clear();
                }
                if short(&owner) == "ColorCollection" && name == "zipMap" {
                    if let Some(ids_field) = colour_pending.take() {
                        if let Some(suffix) = ids.colour.get(&ids_field) {
                            if let Some((_, method_name, _)) = indy.last() {
                                if let Some(base_field) = base_collection(class, method_name) {
                                    let base_suffix = ids
                                        .colour
                                        .get(&base_field)
                                        .cloned()
                                        .unwrap_or_else(|| base_field.to_lowercase());
                                    for colour in DYE_COLOURS {
                                        out.bases.insert(
                                            format!("minecraft:{colour}_{suffix}"),
                                            format!("minecraft:{colour}_{base_suffix}"),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    indy.clear();
                }
                if short(&owner) == "Blocks" && name.starts_with("register") {
                    if let Some(ids_field) = pending.take() {
                        if let Some(block_id) = ids.plain.get(&ids_field) {
                            let id = format!("minecraft:{block_id}");
                            let window_calls = window_chain(class, &instructions, &window, slot);
                            let mut chain = window_calls.clone();
                            out.plain.insert(id.clone(), window_calls);
                            // A helper's argument is the block it copies; the helper's own
                            // body carries the calls the block's chain makes.
                            let mut chain = chain;
                            if name != "register" {
                                if let Some(base) = previous_block_field(class, &instructions, &window, slot) {
                                    if let Some(base_id) = field_id(&base, ids) {
                                        out.bases.insert(id.clone(), base_id);
                                    }
                                }
                                let helper = lambda_chain(class, &name);
                                // The helper's sound may be a parameter: take the call site's
                                // last SoundType argument.
                                let from_call =
                                    previous_field(class, &instructions, &window, slot, "SoundType");
                                chain.sound = chain.sound.clone().or(from_call).or(helper.sound.clone());
                                chain.no_collision |= helper.no_collision;
                                chain.no_occlusion |= helper.no_occlusion;
                                chain.map_color = chain.map_color.clone().or(helper.map_color.clone());
                                chain.function |= helper.function;
                                chain.dye |= helper.dye;
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
    out
}

/// What a plain registration's inline property chain calls.
fn window_chain(
    class: &classfile::ClassFile,
    instructions: &[Instruction],
    window: &[usize],
    end: usize,
) -> Chain {
    let mut chain = Chain::default();
    let mut range: Vec<usize> = window.to_vec();
    range.push(end);
    for slot in range {
        let Some(instruction) = instructions.get(slot) else {
            continue;
        };
        match instruction.opcode {
            0xb6 => {
                let Some((owner, name, _)) = instruction.index.and_then(|index| member(class, index)) else {
                    continue;
                };
                if short(&owner) != "Properties" && short(&owner) != "BlockBehaviour$Properties" {
                    continue;
                }
                match name.as_str() {
                    "sound" => {
                        chain.sound = previous_field(class, instructions, window, slot, "SoundType");
                    }
                    "mapColor" => {
                        chain.map_color = previous_field(class, instructions, window, slot, "MapColor");
                        if chain.map_color.is_none() {
                            if let Some(previous) = slot.checked_sub(1) {
                                if instructions.get(previous).map(|i| i.opcode) == Some(0xba) {
                                    chain.function = true;
                                }
                            }
                        }
                    }
                    "noCollision" => chain.no_collision = true,
                    "noOcclusion" => chain.no_occlusion = true,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    chain
}

/// The chain a colour family's properties lambda shows (its body is a property chain).
fn lambda_chain(class: &classfile::ClassFile, method_name: &str) -> Chain {
    let Some(method) = class.methods.iter().find(|method| method.name == method_name) else {
        return Chain::default();
    };
    let Some(code) = method.code.as_ref() else {
        return Chain::default();
    };
    let instructions = decode(&code.bytes);
    let all: Vec<usize> = (0..instructions.len()).collect();
    window_chain(class, &instructions, &all, instructions.len().saturating_sub(1))
}

/// The last `getstatic <owner>.<FIELD>` before `end`.
fn previous_field(
    class: &classfile::ClassFile,
    instructions: &[Instruction],
    window: &[usize],
    end: usize,
    owner: &str,
) -> Option<String> {
    let mut range: Vec<usize> = window.to_vec();
    range.push(end);
    for slot in range.iter().rev().take(40) {
        let instruction = instructions.get(*slot)?;
        if instruction.opcode != 0xb2 {
            continue;
        }
        let (owner_name, field, _) = member(class, instruction.index?)?;
        if short(&owner_name) == owner {
            return Some(field);
        }
    }
    None
}

/// The last `getstatic Blocks.<FIELD>:Block` before `end`.
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

/// The per-block rows with the three fields fixed, plus the copy resolution.
pub fn rows(jar_path: &Path) -> Result<Rows, String> {
    let jar = Jar::open(jar_path)?;
    let ids = read_ids(
        &jar.class("net/minecraft/references/BlockItemIds")
            .ok_or("no BlockItemIds")?,
    );
    let resource_ids = read_ids(
        &jar.class("net/minecraft/references/BlockIds")
            .ok_or("no BlockIds")?,
    );
    let blocks_class = jar
        .class("net/minecraft/world/level/block/Blocks")
        .ok_or("no Blocks")?;
    let scanned = scan(&blocks_class, &ids);
    let resource_scanned = scan(&blocks_class, &resource_ids);
    let colours = map_colours(&jar);
    let events = sound_events(&jar);
    let sounds = sound_types(&jar, &events);
    let dye_colours = dye_map_colours(&jar);
    let mut base_rows = base::rows(jar_path)?;
    // Per block: the chain (plain registration, colour family, or resource family).
    // BlockIds covers the blocks without an item (water, lava, fire, torch walls, ...).
    let mut chains: BTreeMap<String, Chain> = resource_scanned.plain.clone();
    chains.extend(scanned.plain.clone());
    for (suffix, chain) in &scanned.families {
        for colour in DYE_COLOURS {
            chains.insert(format!("minecraft:{colour}_{suffix}"), chain.clone());
        }
    }
    for (suffix, chain) in &resource_scanned.families {
        for colour in DYE_COLOURS {
            chains
                .entry(format!("minecraft:{colour}_{suffix}"))
                .or_insert_with(|| chain.clone());
        }
    }
    for (block_id, chain) in &chains {
        let Some(index) = base_rows.blocks.iter().position(|block| block == block_id) else {
            continue;
        };
        let row = &mut base_rows.rows[index];
        // `can_occlude` is the property field: cleared by either call.
        row.insert(
            "can_occlude",
            Value::Bool(!(chain.no_collision || chain.no_occlusion)),
        );
        // `sound`: the call's type, else the `Properties` default (stone).
        if row.get("sound").map_or(true, Value::is_null) {
            let name = chain.sound.clone().unwrap_or_else(|| "STONE".to_string());
            if let Some(sound) = sounds.get(&name) {
                row.insert("sound", sound.clone());
            }
        }
        // `map_color`: a constant argument, a dye colour, or a function.
        let current = row.get("map_color").and_then(Value::as_i64).unwrap_or(0);
        if current == 0 && !matches!(chain.map_color.as_deref(), Some("NONE")) {
            let colour = chain
                .map_color
                .as_ref()
                .and_then(|name| colours.get(name).copied())
                .or_else(|| {
                    // The colour families call `DyeColor.getMapColor()`.
                    let colour_name = block_id
                        .trim_start_matches("minecraft:")
                        .split('_')
                        .next()
                        .unwrap_or("");
                    let dye = colour_name.to_ascii_uppercase().replace(' ', "_");
                    if chain.dye
                        || dye_colours
                            .keys()
                            .any(|candidate| candidate.eq_ignore_ascii_case(colour_name))
                    {
                        dye_colours
                            .get(&dye)
                            .and_then(|map| colours.get(map).copied())
                    } else {
                        None
                    }
                });
            if let Some(colour) = colour {
                row.insert("map_color", Value::from(colour as i64));
            }
        }
        if chain.function {
            // Reported below; the value stays as computed.
        }
    }
    // The copied rows: a block's row is its base block's row.
    let position: BTreeMap<&str, usize> = base_rows
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.as_str(), index))
        .collect();
    let mut bases: BTreeMap<String, String> = scanned.bases.clone();
    bases.extend(resource_scanned.bases.clone());
    let mut unresolved: Vec<String> = Vec::new();
    for block in &base_rows.blocks {
        let Some(base_id) = bases.get(block) else {
            continue;
        };
        let (Some(&block_index), Some(&base_index)) = (position.get(block.as_str()), position.get(base_id.as_str()))
        else {
            continue;
        };
        let row = base_rows.rows[base_index].clone();
        for field in gameplay::FIELDS {
            if field == "full_collision" {
                continue;
            }
            if let Some(value) = row.get(field) {
                base_rows.rows[block_index].insert(field, value.clone());
            }
        }
    }
    for (index, block) in base_rows.blocks.iter().enumerate() {
        let missing = gameplay::FIELDS.iter().any(|field| {
            field != &"full_collision"
                && base_rows.rows[index]
                    .get(field)
                    .map_or(true, Value::is_null)
        });
        if missing {
            unresolved.push(block.clone());
        }
    }
    let mut map_color_functions = Vec::new();
    for (block, chain) in &chains {
        if chain.function {
            map_color_functions.push(block.clone());
        }
    }
    Ok(Rows {
        blocks: base_rows.blocks,
        rows: base_rows.rows,
        unresolved,
        map_color_functions,
    })
}

/// The rows, and what could not be filled.
pub struct Rows {
    pub blocks: Vec<String>,
    pub rows: Vec<BTreeMap<&'static str, Value>>,
    pub unresolved: Vec<String>,
    pub map_color_functions: Vec<String>,
}

/// The compact document for already-built rows (`full_collision` comes from the shapes).
fn document_from(rows: &Rows, jar_path: &Path) -> Result<Value, String> {
    let state_table = states::state_table(jar_path)?;
    let shapes = shape_final::evaluate(jar_path)?;
    if shapes.full_block.len() != state_table.state_count() {
        return Err("shape_final does not cover every state".into());
    }
    let fields: Vec<String> = gameplay::FIELDS
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut values = Vec::with_capacity(shapes.full_block.len());
    let mut blocks_per_state = Vec::with_capacity(shapes.full_block.len());
    let mut state = 0usize;
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        for _ in 0..row.state_count {
            let mut value = Vec::with_capacity(fields.len());
            for field in &fields {
                if field == "full_collision" {
                    value.push(Value::Bool(shapes.full_block[state].unwrap_or(false)));
                } else {
                    value.push(
                        rows.rows[block_index]
                            .get(field.as_str())
                            .cloned()
                            .unwrap_or(Value::Null),
                    );
                }
            }
            values.push(value);
            blocks_per_state.push(block_index);
            state += 1;
        }
    }
    let facts = gameplay::Facts {
        fields,
        blocks: rows.blocks.clone(),
        values,
        blocks_per_state,
        uncovered: rows
            .unresolved
            .iter()
            .map(|block| (block.clone(), "no property chain reaches it".to_string()))
            .collect(),
    };
    let text = gameplay::render(&facts);
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// The compact document as JSON.
pub fn document(jar_path: &Path) -> Result<Value, String> {
    let rows = rows(jar_path)?;
    document_from(&rows, jar_path)
}

/// Builds the table and writes it below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let rows = rows(jar_path)?;
    let document = document_from(&rows, jar_path)?;
    let text = serde_json::to_string(&document).map_err(|error| error.to_string())?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut coverage = Coverage::default();
    coverage.produced.push(TABLE.to_string());
    coverage.unresolved = rows.unresolved.clone();
    coverage.map_color_functions = rows.map_color_functions.clone();
    coverage
        .missing
        .extend(MISSING.iter().map(|field| format!("{TABLE}#{field}")));
    if !coverage.unresolved.is_empty() {
        coverage.notes.push(format!(
            "{} of {} blocks still have no row",
            coverage.unresolved.len(),
            rows.blocks.len()
        ));
    }
    if !coverage.map_color_functions.is_empty() {
        coverage.notes.push(format!(
            "{} blocks use a per-state `mapColor` function (not evaluated)",
            coverage.map_color_functions.len()
        ));
    }
    Ok(coverage)
}
