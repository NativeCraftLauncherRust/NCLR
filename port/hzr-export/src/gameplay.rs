//! `_shared/gameplay.compact.json`: the per-state gameplay facts the bundle stores, rebuilt
//! from the player's jar.
//!
//! The Java exporter (`VanillaGameplayFactsExporter`) writes a 56 MB report: one object per
//! block state with 27 fields — light, hardness, sounds, tool requirements, selection outline,
//! ... `hzr-fetch`'s `gameplay.rs` then re-encodes it losslessly: 16 of the fields never vary
//! inside a block (they are per-block values repeated per state), the other 11 vary over small
//! domains, so the compact form keeps one dictionary per field, the constant fields once per
//! block and the varying ones as one index per state.
//!
//! This module rebuilds that compact document from the jar. The facts come from
//! `hzr-bytecode`'s evaluator of `Blocks.<clinit>`'s property chains
//! (`hzr_bytecode::interp::Properties`) — which is where hardness, resistance, friction, the
//! speed/jump factors, the tool and random-tick flags, replaceability, the liquid/push/offset
//! facts, the light emission and the sound type live — from the block class chain (whether it
//! declares the `EntityBlock` interface, which properties its chain called), from
//! `SoundType`/`SoundEvents`/`MapColor`'s static tables, and from `shape_final`'s collision
//! shapes for `full_collision`.
//!
//! # What is exact, and what is a gap
//!
//! The compact file is field-parallel — one wrong row would corrupt every index that follows
//! it — so a field is only written when it is exact for **every** block and state. Today that
//! is true of [`EXACT`] (`friction`, `ignited_by_lava`, `jump_factor`, `push`, `replaceable`,
//! `speed_factor`; `tests/gameplay.rs` compares them against the prepared copy for every
//! block). The other fields this module derives are reported instead of written:
//!
//! * [`NOT_YET_EXACT`] — derived, but the evaluator loses a few values to per-class overrides
//!   or calls it does not model: `block_entity` (25 blocks whose `EntityBlock` comes from a
//!   class the scan resolves to the wrong simple name), `can_occlude` (104 plant/cross blocks
//!   whose occlusion shape, not `noOcclusion()`, decides it), `explosion_resistance` and
//!   `hardness` (3 blocks whose `strength()` chain the evaluator cannot finish), `liquid`
//!   (water/lava/bubble column, whose `liquid` flag is set outside the property chain),
//!   `map_color` (30 blocks whose colour is a function), `offset` (37 plant blocks whose
//!   offset type is a class constant), `requires_tool` (9 deepslate ores), `sound` (the
//!   `DecoratedPotBlock` per-state sound) and `full_collision` (the 102 dynamic-shape states —
//!   the shulker boxes — whose shape `shape_final` leaves uncached);
//! * [`MISSING`] — no jar-driven source yet: `dampening`, `emission`, `item`, `outline`,
//!   `outline_edges`, `outline_offset`, `random_ticks`, `render_shape`, `shape_occlusion`,
//!   `skylight_down` and `solid_render`, which need the light engine, the selection outline
//!   geometry, the item registry mapping or per-class per-state overrides.
//!
//! [`Facts::uncovered`] carries the larger gap: 410 of the 1286 blocks are registered inside
//! `ColorCollection` loops (the coloured beds, wool, banners, concrete, ...), and
//! `hzr-bytecode`'s evaluator does not model `ColorCollection.forEach`, so their property
//! chains are never built. Their per-block values are `null`, and [`write`] refuses to emit a
//! partial document rather than guess at them.
//!
//! Nothing here reads `ship.hzr3`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value};

// The shared class-file reader lives at the crate root — the wiring `render_full` uses: the
// binary declares it, the integration test declares it, and `interp` refers to it as
// `crate::classfile`.
use crate::classfile;

#[path = "../../hzr-bytecode/src/interp.rs"]
#[allow(dead_code)]
mod interp;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

#[path = "shape_final.rs"]
#[allow(dead_code)]
mod shape_final;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = "_shared/gameplay.compact.json";

/// What was produced, and what is still out of reach.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    /// The fields this module does not derive yet.
    pub missing: Vec<String>,
    pub notes: Vec<String>,
}

/// The document before compression: `[state][field]` values, plus the block of each state.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    /// Field names, sorted (the bundle sorts them).
    pub fields: Vec<String>,
    /// Block ids, in registry order.
    pub blocks: Vec<String>,
    /// `[state][field]` in `fields` order.
    pub values: Vec<Vec<Value>>,
    /// The block index of each state, in state-id order.
    pub blocks_per_state: Vec<usize>,
    /// Blocks whose per-block values could not be derived, with the reason.
    pub uncovered: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// The jar
// ---------------------------------------------------------------------------

/// The jar's class files, read once.
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
            entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
            by_short.insert(short(internal).to_string(), internal.to_string());
            files.insert(internal.to_string(), data);
        }
        Ok(Jar { files, by_short })
    }

    fn bytes(&self, name: &str) -> Option<&Vec<u8>> {
        if name.contains('/') {
            self.files.get(name)
        } else {
            self.by_short
                .get(name)
                .and_then(|internal| self.files.get(internal))
        }
    }

    fn class(&self, name: &str) -> Option<classfile::ClassFile> {
        classfile::parse(self.bytes(name)?).ok()
    }
}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// `hzr-bytecode`'s evaluator loads classes by binary name.
struct Loader<'a> {
    jar: &'a Jar,
    cache: RefCell<HashMap<String, Option<classfile::ClassFile>>>,
}

impl<'a> interp::Loader for Loader<'a> {
    fn load(&mut self, name: &str) -> Option<classfile::ClassFile> {
        if let Some(hit) = self.cache.borrow().get(name) {
            return hit.clone();
        }
        let parsed = self.jar.class(name);
        self.cache
            .borrow_mut()
            .insert(name.to_string(), parsed.clone());
        parsed
    }
}

// ---------------------------------------------------------------------------
// Interface and hierarchy helpers
// ---------------------------------------------------------------------------

/// The `interface` list of a class file, which the shared parser does not keep.
fn interfaces(bytes: &[u8]) -> Option<Vec<String>> {
    let mut at = 10usize; // magic, minor, major, constant-pool count
    let count = u16::from_be_bytes([*bytes.get(8)?, *bytes.get(9)?]) as usize;
    let mut pool: Vec<Option<String>> = vec![None; count];
    let mut index = 1;
    while index < count {
        let tag = *bytes.get(at)?;
        at += 1;
        match tag {
            1 => {
                let length = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
                at += 2;
                pool[index] =
                    Some(String::from_utf8_lossy(bytes.get(at..at + length)?).into_owned());
                at += length;
            }
            7 => {
                pool[index] = Some(format!(
                    "#{}",
                    u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?])
                ));
                at += 2;
            }
            8 | 16 | 19 | 20 => at += 2,
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => at += 4,
            5 | 6 => {
                at += 8;
                index += 1;
            }
            15 => at += 3,
            _ => return None,
        }
        index += 1;
    }
    at += 6; // access flags, this class, super class
    let count = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
    at += 2;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let class = u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]) as usize;
        at += 2;
        let reference = pool.get(class)?.clone()?;
        let inner: usize = reference.trim_start_matches('#').parse().ok()?;
        out.push(pool.get(inner)?.clone()?);
    }
    Some(out)
}

/// Whether `start` (an internal name) or one of its supertypes implements `target`.
fn implements(jar: &Jar, start: &str, target: &str) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = vec![start.to_string()];
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if name == target {
            return true;
        }
        let Some(bytes) = jar.bytes(&name) else {
            continue;
        };
        if let Some(list) = interfaces(bytes) {
            for interface in list {
                if interface == target {
                    return true;
                }
                queue.push(interface);
            }
        }
        if let Some(file) = jar.class(&name) {
            if !file.super_name.is_empty() {
                queue.push(file.super_name.clone());
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Static tables: SoundType, SoundEvents, MapColor
// ---------------------------------------------------------------------------

/// A JSON number for a float, printed the way Java's `Float.toString` does (shortest form).
fn float_value(value: f32) -> Value {
    let mut text = format!("{value}");
    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
        text.push_str(".0");
    }
    serde_json::from_str(&text).unwrap_or_else(|_| Value::from(value as f64))
}

fn decode(bytes: &[u8]) -> Vec<Instruction> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let opcode = bytes[at];
        let mut index = None;
        let mut immediate = None;
        let length = match opcode {
            0x02 => {
                immediate = Some(-1);
                1
            }
            0x03..=0x08 => {
                immediate = Some((opcode - 0x03) as i32);
                1
            }
            0x10 => {
                immediate = bytes.get(at + 1).map(|value| *value as i8 as i32);
                2
            }
            0x11 => {
                immediate = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| i16::from_be_bytes([pair[0], pair[1]]) as i32);
                3
            }
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
                let base = at + 1 + padding;
                let (Some(low), Some(high)) = (
                    bytes
                        .get(base + 4..base + 8)
                        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
                    bytes
                        .get(base + 8..base + 12)
                        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
                ) else {
                    break;
                };
                let count = (high as i64 - low as i64 + 1).max(0) as usize;
                1 + padding + 12 + count * 4
            }
            0xab => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base = at + 1 + padding;
                let pairs = bytes
                    .get(base + 4..base + 8)
                    .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
                    .unwrap_or(0)
                    .max(0) as usize;
                1 + padding + 8 + pairs * 8
            }
            _ => 1,
        };
        out.push(Instruction {
            opcode,
            index,
            immediate,
        });
        at += length;
    }
    out
}

/// One decoded instruction.
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
}

/// `(owner, member name)` behind a field reference.
fn field_at(constants: &[classfile::Constant], index: usize) -> Option<(String, String)> {
    let (class, name_and_type) = match constants.get(index)? {
        classfile::Constant::Fieldref { class, name, .. } => (class, name),
        _ => return None,
    };
    let class_index: usize = class.trim_start_matches('#').parse().ok()?;
    let owner = match constants.get(class_index)? {
        classfile::Constant::Class(reference) => {
            let name_index: usize = reference.trim_start_matches('#').parse().ok()?;
            match constants.get(name_index)? {
                classfile::Constant::Utf8(text) => text.clone(),
                _ => return None,
            }
        }
        _ => return None,
    };
    let name_index: usize = name_and_type.trim_start_matches('#').parse().ok()?;
    let name = match constants.get(name_index)? {
        classfile::Constant::NameAndType { name, .. } => {
            let inner: usize = name.trim_start_matches('#').parse().ok()?;
            match constants.get(inner)? {
                classfile::Constant::Utf8(text) => text.clone(),
                _ => return None,
            }
        }
        _ => return None,
    };
    Some((owner, name))
}

/// `SoundEvents`: field name -> `minecraft:<id>`.
fn sound_event_ids(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/sounds/SoundEvents") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    let mut id: Option<String> = None;
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) = instruction
                    .index
                    .and_then(|index| class.constants.get(index))
                {
                    let name_index: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
                    if let Some(classfile::Constant::Utf8(text)) = class.constants.get(name_index) {
                        id = Some(format!("minecraft:{text}"));
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field)) = instruction
                    .index
                    .and_then(|index| field_at(&class.constants, index))
                {
                    if owner == "net/minecraft/sounds/SoundEvents" {
                        if let Some(id) = id.take() {
                            out.insert(field, id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // `EMPTY` is built by a factory, not `register`.
    out.entry("EMPTY".to_string())
        .or_insert_with(|| "minecraft:empty".to_string());
    out
}

/// `SoundType`: field name -> the seven constructor arguments.
fn sound_types(jar: &Jar, events: &BTreeMap<String, String>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/level/block/SoundType") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    // `new SoundType(volume, pitch, break, step, place, hit, fall)` then `putstatic`.
    let mut floats: Vec<f32> = Vec::new();
    let mut events_used: Vec<String> = Vec::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => {
                floats.clear();
                events_used.clear();
            }
            0x0b..=0x0d => {
                let value = match instruction.opcode {
                    0x0b => 0.0,
                    0x0c => 1.0,
                    _ => 2.0,
                };
                floats.push(value);
            }
            0x12 | 0x13 => {
                if let Some(classfile::Constant::Float(value)) = instruction
                    .index
                    .and_then(|index| class.constants.get(index))
                {
                    floats.push(*value);
                }
            }
            0xb2 => {
                if let Some((owner, field)) = instruction
                    .index
                    .and_then(|index| field_at(&class.constants, index))
                {
                    if owner == "net/minecraft/sounds/SoundEvents" {
                        events_used.push(field);
                    }
                }
            }
            0xb3 => {
                if let Some((owner, field)) = instruction
                    .index
                    .and_then(|index| field_at(&class.constants, index))
                {
                    if owner == "net/minecraft/world/level/block/SoundType"
                        && events_used.len() == 5
                    {
                        let mut sound = Map::new();
                        sound.insert(
                            "volume".to_string(),
                            float_value(floats.first().copied().unwrap_or(1.0)),
                        );
                        sound.insert(
                            "pitch".to_string(),
                            float_value(floats.get(1).copied().unwrap_or(1.0)),
                        );
                        for (slot, name) in
                            ["break", "step", "place", "hit", "fall"].iter().enumerate()
                        {
                            let id = events.get(&events_used[slot]).cloned().unwrap_or_default();
                            let mut event = Map::new();
                            event.insert("id".to_string(), Value::String(id));
                            sound.insert(name.to_string(), Value::Object(event));
                        }
                        out.insert(field, Value::Object(sound));
                        floats.clear();
                        events_used.clear();
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `MapColor`: field name -> its `col` value.
fn map_colours(jar: &Jar) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/world/level/material/MapColor") else {
        return out;
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return out;
    };
    // `new MapColor(id, col)`, then `putstatic`.
    let mut numbers: Vec<i32> = Vec::new();
    for instruction in decode(&code.bytes) {
        match instruction.opcode {
            0xbb => numbers.clear(),
            0x12 | 0x13 => {
                if let Some(classfile::Constant::Integer(value)) = instruction
                    .index
                    .and_then(|index| class.constants.get(index))
                {
                    numbers.push(*value);
                }
            }
            0xb3 => {
                if let Some((owner, field)) = instruction
                    .index
                    .and_then(|index| field_at(&class.constants, index))
                {
                    if owner == "net/minecraft/world/level/material/MapColor" {
                        if let Some(col) = numbers.get(1) {
                            out.insert(field, *col as u32);
                        }
                        numbers.clear();
                    }
                }
            }
            _ => {
                if let Some(value) = instruction.immediate {
                    numbers.push(value);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The facts
// ---------------------------------------------------------------------------

/// The fields this module derives, sorted the way the bundle writes them.
pub const FIELDS: [&str; 16] = [
    "block_entity",
    "can_occlude",
    "explosion_resistance",
    "friction",
    "full_collision",
    "hardness",
    "ignited_by_lava",
    "jump_factor",
    "liquid",
    "map_color",
    "offset",
    "push",
    "replaceable",
    "requires_tool",
    "sound",
    "speed_factor",
];

/// The fields the comparison in `tests/gameplay.rs` shows exact for every evaluable block.
#[allow(dead_code)] // the contract 	ests/gameplay.rs pins; not called by the writer.
pub const EXACT: [&str; 6] = [
    "friction",
    "ignited_by_lava",
    "jump_factor",
    "push",
    "replaceable",
    "speed_factor",
];

/// The fields this module derives, but not yet exactly for every block and state (see the
/// module docs for each one's reason).
pub const NOT_YET_EXACT: [&str; 10] = [
    "block_entity",
    "can_occlude",
    "explosion_resistance",
    "full_collision",
    "hardness",
    "liquid",
    "map_color",
    "offset",
    "requires_tool",
    "sound",
];

/// The fields the bundle has and this module does not derive yet.
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

/// The per-block values of one evaluable block, or why they cannot be derived.
fn block_values(
    jar: &Jar,
    block: &interp::Block,
    colours: &BTreeMap<String, u32>,
    sounds: &BTreeMap<String, Value>,
) -> Result<BTreeMap<&'static str, Value>, String> {
    let properties = &block.properties;
    let mut row: BTreeMap<&'static str, Value> = BTreeMap::new();
    let internal = jar
        .by_short
        .get(&block.class)
        .cloned()
        .unwrap_or_else(|| block.class.clone());
    row.insert(
        "block_entity",
        Value::Bool(implements(
            jar,
            &internal,
            "net/minecraft/world/level/block/EntityBlock",
        )),
    );
    let calls = |name: &str| properties.unmodelled.iter().any(|call| call == name);
    row.insert("can_occlude", Value::Bool(!calls("noOcclusion")));
    row.insert(
        "explosion_resistance",
        float_value(properties.resistance.unwrap_or(0.0)),
    );
    row.insert("friction", float_value(properties.friction.unwrap_or(0.6)));
    row.insert("hardness", float_value(properties.hardness.unwrap_or(0.0)));
    row.insert("ignited_by_lava", Value::Bool(calls("ignitedByLava")));
    row.insert(
        "jump_factor",
        float_value(properties.jump_factor.unwrap_or(1.0)),
    );
    row.insert("liquid", Value::Bool(properties.liquid.is_some()));
    let colour = properties
        .map_color
        .as_ref()
        .and_then(|name| colours.get(name).copied());
    match colour {
        Some(colour) => {
            row.insert("map_color", Value::from(colour as i64));
        }
        None => {
            return Err(format!(
                "no static map colour (the chain uses a colour function: {})",
                properties.map_color_function
            ));
        }
    }
    row.insert(
        "offset",
        Value::Bool(
            properties
                .offset
                .as_deref()
                .map(|offset| offset != "NONE")
                .unwrap_or(false),
        ),
    );
    row.insert(
        "push",
        Value::String(
            properties
                .push_reaction
                .clone()
                .unwrap_or_else(|| "PUSH_PULL".to_string()),
        ),
    );
    row.insert("replaceable", Value::Bool(properties.replaceable));
    row.insert(
        "requires_tool",
        Value::Bool(properties.requires_correct_tool),
    );
    let sound = properties
        .sound
        .as_ref()
        .and_then(|name| sounds.get(name))
        .cloned();
    match sound {
        Some(sound) => {
            row.insert("sound", sound);
        }
        None => {
            return Err(format!(
                "no static sound type (the chain names {:?})",
                properties.sound
            ));
        }
    }
    row.insert(
        "speed_factor",
        float_value(properties.speed_factor.unwrap_or(1.0)),
    );
    Ok(row)
}

/// Builds the facts for every state from the player's jar.
///
/// `full_collision` is derived for every state. The other fields are per block, so a block
/// the evaluator does not reach (or whose map colour/sound type is not a static value) is
/// left `null` and listed in [`Facts::uncovered`] — a partial row is never guessed at.
pub fn facts(jar_path: &Path) -> Result<Facts, String> {
    let jar = Jar::open(jar_path)?;
    let blocks_class = jar
        .class("net/minecraft/world/level/block/Blocks")
        .ok_or("the jar has no Blocks class")?;
    let ids = interp::ids_of(&blocks_class);
    let mut loader = Loader {
        jar: &jar,
        cache: RefCell::new(HashMap::new()),
    };
    let blocks = interp::blocks_with(&mut loader, &ids);
    if blocks.is_empty() {
        return Err("Blocks.<clinit> produced no blocks".into());
    }
    let state_table = states::state_table(jar_path)?;
    // The evaluator's blocks by registry id (its names carry no namespace).
    let mut evaluated: BTreeMap<String, &interp::Block> = BTreeMap::new();
    for block in &blocks {
        evaluated
            .entry(format!("minecraft:{}", block.name))
            .or_insert(block);
    }
    let events = sound_event_ids(&jar);
    let sounds = sound_types(&jar, &events);
    let colours = map_colours(&jar);
    let shapes = shape_final::evaluate(jar_path)?;
    if shapes.full_block.len() != state_table.state_count() {
        return Err(format!(
            "shape_final covers {} states, the state table {}",
            shapes.full_block.len(),
            state_table.state_count()
        ));
    }
    let mut uncovered = Vec::new();
    let mut per_block: Vec<Option<BTreeMap<&'static str, Value>>> =
        Vec::with_capacity(state_table.blocks.len());
    for row in &state_table.blocks {
        let Some(block) = evaluated.get(&row.id) else {
            uncovered.push((
                row.id.clone(),
                "the property evaluator does not reach it".to_string(),
            ));
            per_block.push(None);
            continue;
        };
        match block_values(&jar, block, &colours, &sounds) {
            Ok(values) => per_block.push(Some(values)),
            Err(reason) => {
                uncovered.push((row.id.clone(), reason));
                per_block.push(None);
            }
        }
    }
    // One state-major table, in the bundle's order.
    let mut values = Vec::with_capacity(shapes.full_block.len());
    let mut blocks_per_state = Vec::with_capacity(shapes.full_block.len());
    let mut state = 0usize;
    for (block_index, row) in state_table.blocks.iter().enumerate() {
        for _ in 0..row.state_count {
            let mut value = Vec::with_capacity(FIELDS.len());
            for field in FIELDS {
                if field == "full_collision" {
                    value.push(Value::Bool(shapes.full_block[state].unwrap_or(false)));
                } else {
                    value.push(
                        per_block[block_index]
                            .as_ref()
                            .map(|row| row[field].clone())
                            .unwrap_or(Value::Null),
                    );
                }
            }
            values.push(value);
            blocks_per_state.push(block_index);
            state += 1;
        }
    }
    Ok(Facts {
        fields: FIELDS.iter().map(|field| field.to_string()).collect(),
        blocks: state_table
            .blocks
            .iter()
            .map(|row| row.id.clone())
            .collect(),
        values,
        blocks_per_state,
        uncovered,
    })
}

// ---------------------------------------------------------------------------
// The compact encoding (the shape `hzr-fetch`'s `gameplay.rs` writes)
// ---------------------------------------------------------------------------

fn key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// Compresses [`Facts`] into the bundle's compact document.
pub fn compact(facts: &Facts) -> Value {
    let fields = facts.fields.len();
    // Which fields never vary inside a block.
    let mut constant = vec![true; fields];
    let mut first_seen: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for (state, block) in facts.blocks_per_state.iter().enumerate() {
        let block = facts.blocks[*block].as_str();
        let row = first_seen.entry(block).or_default();
        if row.is_empty() {
            row.extend(facts.values[state].iter());
        } else {
            for (field, value) in row.iter().enumerate() {
                if key(value) != key(&facts.values[state][field]) {
                    constant[field] = false;
                }
            }
        }
    }
    // Blocks by first appearance, and each block's first state.
    let mut blocks: Vec<String> = Vec::new();
    let mut position: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_state: BTreeMap<String, usize> = BTreeMap::new();
    for (state, block) in facts.blocks_per_state.iter().enumerate() {
        let name = &facts.blocks[*block];
        first_state.entry(name.clone()).or_insert(state);
        if !position.contains_key(name) {
            position.insert(name.clone(), blocks.len());
            blocks.push(name.clone());
        }
    }
    // One dictionary per field, in first-appearance order.
    let mut dictionaries = Map::new();
    let mut index_of: Vec<BTreeMap<String, usize>> = Vec::with_capacity(fields);
    for field in 0..fields {
        let mut order: Vec<String> = Vec::new();
        let mut map: BTreeMap<String, usize> = BTreeMap::new();
        let mut walk = |value: &Value| {
            let text = key(value);
            if !map.contains_key(&text) {
                map.insert(text.clone(), order.len());
                order.push(text);
            }
        };
        if constant[field] {
            for block in &blocks {
                walk(&facts.values[first_state[block]][field]);
            }
        } else {
            for row in &facts.values {
                walk(&row[field]);
            }
        }
        let mut list = Vec::with_capacity(order.len());
        for text in &order {
            list.push(serde_json::from_str::<Value>(text).unwrap_or(Value::Null));
        }
        dictionaries.insert(facts.fields[field].clone(), Value::Array(list));
        index_of.push(map);
    }
    // Per-block indices for the constant fields.
    let constant_fields: Vec<usize> = (0..fields).filter(|field| constant[*field]).collect();
    let mut per_block = Map::new();
    for block in &blocks {
        let state = first_state[block];
        let mut row = Vec::with_capacity(constant_fields.len());
        for field in &constant_fields {
            row.push(Value::from(
                index_of[*field][&key(&facts.values[state][*field])] as u64,
            ));
        }
        per_block.insert(block.clone(), Value::Array(row));
    }
    // Per-state indices for the fields that vary.
    let varying_fields: Vec<usize> = (0..fields).filter(|field| !constant[*field]).collect();
    let mut per_state = Vec::with_capacity(facts.values.len());
    for state in 0..facts.values.len() {
        let mut row = Vec::with_capacity(varying_fields.len());
        for field in &varying_fields {
            row.push(Value::from(
                index_of[*field][&key(&facts.values[state][*field])] as u64,
            ));
        }
        per_state.push(Value::Array(row));
    }
    let mut root = Map::new();
    root.insert(
        "fields".to_string(),
        Value::Array(
            facts
                .fields
                .iter()
                .map(|field| Value::String(field.clone()))
                .collect(),
        ),
    );
    root.insert(
        "constant_fields".to_string(),
        Value::Array(
            constant_fields
                .iter()
                .map(|field| Value::String(facts.fields[*field].clone()))
                .collect(),
        ),
    );
    root.insert(
        "varying_fields".to_string(),
        Value::Array(
            varying_fields
                .iter()
                .map(|field| Value::String(facts.fields[*field].clone()))
                .collect(),
        ),
    );
    root.insert("dictionaries".to_string(), Value::Object(dictionaries));
    root.insert(
        "blocks".to_string(),
        Value::Array(
            blocks
                .iter()
                .map(|block| Value::String(block.clone()))
                .collect(),
        ),
    );
    root.insert("per_block".to_string(), Value::Object(per_block));
    root.insert(
        "blocks_per_state".to_string(),
        Value::Array(
            facts
                .blocks_per_state
                .iter()
                .map(|block| Value::from(*block as u64))
                .collect(),
        ),
    );
    root.insert("per_state".to_string(), Value::Array(per_state));
    Value::Object(root)
}

/// Renders the compact document the way the bundle stores it (compact JSON, no trailing
/// newline).
pub fn render(facts: &Facts) -> String {
    serde_json::to_string(&compact(facts)).expect("the compact document is serializable")
}

/// Builds the facts and writes the compact document below `root`.
///
/// Nothing calls it until `main.rs` gains the arm, so a test build reports it as dead.
#[allow(dead_code)]
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let facts = facts(jar_path)?;
    let coverage = check(&facts)?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, render(&facts)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(coverage)
}

/// Whether the facts can be written at all, and the coverage report when they can.
///
/// Two gates: every block needs per-block values (the document indexes `per_block` by the
/// block order), and every field needs to be exact (a wrong row corrupts every index after
/// it). Both are reported, never guessed around.
pub fn check(facts: &Facts) -> Result<Coverage, String> {
    if !facts.uncovered.is_empty() {
        // A partial table is worse than none: the consumer indexes `per_block` by the block
        // order, so a missing row would silently shift every state that follows it.
        let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, reason) in &facts.uncovered {
            *reasons.entry(reason.as_str()).or_insert(0) += 1;
        }
        let sample: Vec<&str> = facts
            .uncovered
            .iter()
            .take(6)
            .map(|(block, _)| block.as_str())
            .collect();
        return Err(format!(
            "{} of {} blocks have no per-block values yet ({}); first: {:?}",
            facts.uncovered.len(),
            facts.blocks.len(),
            reasons
                .iter()
                .map(|(reason, count)| format!("{count}x {reason}"))
                .collect::<Vec<_>>()
                .join("; "),
            sample
        ));
    }
    let mut not_exact: Vec<&str> = NOT_YET_EXACT.to_vec();
    not_exact.retain(|field| facts.fields.iter().any(|ours| ours == field));
    if !not_exact.is_empty() {
        return Err(format!(
            "{} fields are not exact for every block yet ({}); the bundle is only reproduced \
             field for field, so nothing is written",
            not_exact.len(),
            not_exact.join(", ")
        ));
    }
    let mut coverage = Coverage::default();
    coverage.produced.push(TABLE.to_string());
    coverage.missing = MISSING
        .iter()
        .map(|field| format!("{TABLE}#{field}"))
        .collect();
    coverage.notes.push(format!(
        "{} of {} fields are written; the missing ones need the light engine, the selection \
         outline geometry or the item registry mapping",
        FIELDS.len(),
        FIELDS.len() + MISSING.len()
    ));
    Ok(coverage)
}
