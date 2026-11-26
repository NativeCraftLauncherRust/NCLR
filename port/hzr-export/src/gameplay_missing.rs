//! The 11 gameplay fields `gameplay_exact` does not derive: `item`, `outline`,
//! `outline_edges`, `outline_offset`, `render_shape`, `shape_occlusion`, `dampening`,
//! `emission`, `skylight_down`, `solid_render` and `random_ticks`.
//!
//! Everything is still derived from the player's own `client.jar`; nothing reads `ship.hzr3`.
//!
//! * `item` is `BuiltInRegistries.ITEM.getKey(block.asItem())`: a `BlockItemIds` record's item
//!   key, the base block's item for the `wallVariant` helpers, or `minecraft:air` for blocks
//!   registered without an item.
//! * `render_shape` is `Block.getRenderShape` (`INVISIBLE` for air, barriers, light, liquids,
//!   portals, the moving piston, structure void and bubble columns).
//! * `random_ticks`, `emission`, `solid_render`'s force flags and the `lightLevel` function are
//!   read from the `Properties` chains in `Blocks.<clinit>` (helpers and lambdas included).
//! * `shape_occlusion` is `Block.useShapeForLightOcclusion`, `propagatesSkylightDown` and
//!   `getLightDampening` follow the `BlockBehaviour` formulas with the classes that override
//!   them; `solid_render` is `BlockStateBase.calculateSolid` over the collision shape.
//! * `outline`/`outline_edges` are the selection shape (`getShape`) as AABBs and edges.
//!   `getOcclusionShape` defaults to `getShape`, so the occlusion table is the selection shape
//!   for every class that does not override it; fences, fence gates, lecterns and sculk
//!   shriekers are patched. `outline_offset` is `hasOffsetFunction` and a `getShape` that calls
//!   `getOffset`; the shifted shapes are moved back by the `BlockPos.ZERO` offset.
//!
//! Gaps are reported through [`Coverage`] instead of being filled with guessed values.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

use crate::classfile;

#[path = "gameplay_exact.rs"]
#[allow(dead_code)]
mod exact;

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

#[path = "geometry.rs"]
#[allow(dead_code)]
mod geometry;

#[path = "shape_final.rs"]
#[allow(dead_code)]
mod shape_final;

#[path = "shape_offset.rs"]
#[allow(dead_code)]
mod shape_offset;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = exact::TABLE;

const DYE_COLOURS: [&str; 16] = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray",
    "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black",
];

/// Every field the compact table carries, in the bundle's order.
pub const FIELDS: [&str; 27] = [
    "block_entity",
    "can_occlude",
    "dampening",
    "emission",
    "explosion_resistance",
    "friction",
    "full_collision",
    "hardness",
    "ignited_by_lava",
    "item",
    "jump_factor",
    "liquid",
    "map_color",
    "offset",
    "outline",
    "outline_edges",
    "outline_offset",
    "push",
    "random_ticks",
    "render_shape",
    "replaceable",
    "requires_tool",
    "shape_occlusion",
    "skylight_down",
    "solid_render",
    "sound",
    "speed_factor",
];

/// What was produced, and what could not be.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    /// States whose selection shape the geometry engine does not evaluate.
    pub outline_gaps: Vec<usize>,
    /// Blocks whose `item` could not be derived.
    pub item_gaps: Vec<String>,
    /// Blocks whose light fields could not be derived.
    pub light_gaps: Vec<String>,
}

/// The compact document, built from `gameplay_exact`'s rows plus these fields.
pub struct Document {
    pub facts: gameplay_facts::Facts,
    pub coverage: Coverage,
}

#[path = "gameplay.rs"]
#[allow(dead_code)]
mod gameplay_facts;

// ---------------------------------------------------------------------------
// The jar and its bytecode
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

    fn bytes(&self, name: &str) -> Option<&Vec<u8>> {
        if name.contains('/') {
            self.files.get(name)
        } else {
            self.by_short
                .get(name)
                .and_then(|internal| self.files.get(internal))
        }
    }

    fn internal(&self, simple: &str) -> Option<String> {
        self.by_short.get(simple).cloned()
    }
}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
    at: usize,
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
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                immediate = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| i16::from_be_bytes([pair[0], pair[1]]) as i32);
                3
            }
            0xc4 => 4,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            0xaa => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base = at + 1 + padding;
                let low = i32_at(bytes, base + 4).unwrap_or(0);
                let high = i32_at(bytes, base + 8).unwrap_or(-1);
                let count = if high >= low {
                    (high as i64 - low as i64 + 1) as usize
                } else {
                    0
                };
                1 + padding + 12 + count * 4
            }
            0xab => {
                let padding = (4 - ((at + 1) % 4)) % 4;
                let base = at + 1 + padding;
                let pairs = i32_at(bytes, base + 4).unwrap_or(0).max(0) as usize;
                1 + padding + 8 + pairs * 8
            }
            _ => 1,
        };
        out.push(Instruction {
            opcode,
            index,
            immediate,
            at,
        });
        at += length;
    }
    out
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
    bytes
        .get(at..at + 4)
        .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn constant(class: &classfile::ClassFile, index: usize) -> Option<&classfile::Constant> {
    class.constants.get(index)
}

fn utf8_at(class: &classfile::ClassFile, reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match class.constants.get(index)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

fn class_name_at(class: &classfile::ClassFile, index: usize) -> Option<String> {
    match class.constants.get(index)? {
        classfile::Constant::Class(reference) => utf8_at(class, reference),
        _ => None,
    }
}

fn member(class: &classfile::ClassFile, index: usize) -> Option<(String, String, String)> {
    let (owner_ref, name_ref, kind) = match class.constants.get(index)? {
        classfile::Constant::Fieldref { class, name, kind } => (class, name, kind),
        classfile::Constant::Methodref { class, name, kind } => (class, name, kind),
        classfile::Constant::InterfaceMethodref { class, name, kind } => (class, name, kind),
        _ => return None,
    };
    let owner = class_name_at(class, owner_ref.trim_start_matches('#').parse().ok()?)?;
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

fn lambda_ref(class: &classfile::ClassFile, index: usize) -> Option<(String, String)> {
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
        if let Some((owner, name, _)) = member(class, *reference) {
            return Some((short(&owner).to_string(), name));
        }
    }
    None
}

fn argument_count(descriptor: &str) -> usize {
    let mut count = 0usize;
    let mut chars = descriptor.chars();
    if chars.next() != Some('(') {
        return 0;
    }
    loop {
        match chars.next() {
            Some(')') | None => break,
            Some('[') => {
                while let Some(next) = chars.next() {
                    if next != '[' {
                        if next == 'L' {
                            while let Some(inner) = chars.next() {
                                if inner == ';' {
                                    break;
                                }
                            }
                        }
                        break;
                    }
                }
                count += 1;
            }
            Some('L') => {
                while let Some(inner) = chars.next() {
                    if inner == ';' {
                        break;
                    }
                }
                count += 1;
            }
            Some(_) => count += 1,
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Item keys
// ---------------------------------------------------------------------------

/// `BlockItemId.create(block, item)` records: block key -> (item key, field).
fn read_block_items(jar: &Jar) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut by_block = BTreeMap::new();
    let mut colour = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/references/BlockItemIds") else {
        return (by_block, colour);
    };
    let Some(code) = class
        .methods
        .iter()
        .find(|method| method.name == "<clinit>")
        .and_then(|method| method.code.as_ref())
    else {
        return (by_block, colour);
    };
    let instructions = decode(&code.bytes);
    let mut strings: Vec<String> = Vec::new();
    let mut mode = 0u8;
    let mut blocks: Vec<String> = Vec::new();
    for instruction in &instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) =
                    instruction.index.and_then(|index| constant(&class, index))
                {
                    if let Some(text) = utf8_at(&class, reference) {
                        strings.push(text);
                    }
                }
            }
            0xb8 => {
                if let Some((owner, name, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemId" || short(&owner) == "BlockItemIds" {
                        if name == "create" && strings.len() >= 2 {
                            mode = 1;
                        } else if name == "createSimpleColored" {
                            mode = 2;
                        }
                    }
                }
            }
            0xb3 => {
                let mut stored = false;
                if let Some((owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemIds" || short(&owner) == "BlockIds" {
                        stored = true;
                        match mode {
                            1 => {
                                if strings.len() >= 2 {
                                    by_block
                                        .insert(strings[0].clone(), strings[1].clone());
                                    blocks.push(field.clone());
                                }
                            }
                            2 => {
                                if let Some(suffix) = strings.last() {
                                    colour.insert(field.clone(), suffix.clone());
                                }
                            }
                            _ => {
                                if let Some(name) = strings.last() {
                                    by_block.insert(name.clone(), name.clone());
                                    blocks.push(field.clone());
                                }
                            }
                        }
                    }
                }
                if stored || true {
                    strings.clear();
                    mode = 0;
                }
            }
            _ => {}
        }
    }
    let _ = blocks;
    (by_block, colour)
}

/// Debug hook for the tests (not used by the writer).
pub fn debug_items(jar_path: &Path) -> (usize, usize, Vec<String>) {
    let Ok(jar) = Jar::open(jar_path) else {
        return (0, 0, Vec::new());
    };
    let (by_block, colour) = read_block_items(&jar);
    let sample = by_block
        .iter()
        .take(5)
        .map(|(block, item)| format!("{block}={item}"))
        .collect();
    (by_block.len(), colour.len(), sample)
}

// ---------------------------------------------------------------------------
// Properties facts
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Light {
    None,
    Constant(i32),
    Lit(i32),
    Candle,
    Level,
    Glow(i32),
    Berries(i32),
    SeaPickle,
    RespawnAnchor,
    TrialSpawner,
    Vault,
}

impl Default for Light {
    fn default() -> Light {
        Light::None
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Facts {
    /// `BlockItemIds` field: the block key is `Blocks.<FIELD>` in the jar.
    item: Option<String>,
    random: bool,
    force_on: bool,
    force_off: bool,
    light: Light,
    /// `wallVariant(base, ...)` or `ofLegacyCopy(Blocks.base)`: light/force/random inherit.
    copy_base: Option<String>,
    /// The block was registered through a colour collection.
    family: bool,
}

/// The registration facts of every block, in `Blocks.<clinit>` order.
struct Scanned {
    facts: BTreeMap<String, Facts>,
    /// Block key (registry path) -> item key, from the `BlockItemIds` records.
    items: BTreeMap<String, String>,
    /// `ColorCollection` field -> family suffix (`wool`, `stained_glass`, ...).
    colour_families: BTreeMap<String, String>,
}

fn scan_facts(jar: &Jar) -> Scanned {
    let (items, colour_families) = read_block_items(jar);
    let mut out = Scanned {
        facts: BTreeMap::new(),
        items: items.clone(),
        colour_families: colour_families.clone(),
    };
    let Some(class) = jar.class("net/minecraft/world/level/block/Blocks") else {
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
    let instructions = decode(&code.bytes);
    let mut start = 0usize;
    for (slot, instruction) in instructions.iter().enumerate() {
        if instruction.opcode != 0xb3 {
            continue;
        }
        let body = &instructions[start..slot];
        start = slot + 1;
        scan_statement(jar, &class, body, &mut out);
    }
    out
}

fn scan_statement(
    jar: &Jar,
    class: &classfile::ClassFile,
    body: &[Instruction],
    out: &mut Scanned,
) {
    let mut facts = Facts::default();
    let mut id: Option<(String, String)> = None;
    let mut family_ids: Vec<String> = Vec::new();
    let mut collection: Option<String> = None;
    let mut register_family = false;
    let mut visited: BTreeSet<String> = BTreeSet::new();
    collect_calls(jar, class, body, &mut facts, &mut id, &mut collection, &mut register_family, &mut visited);
    if register_family {
        let Some(collection) = collection else { return };
        if let Some(suffix) = out.colour_families.get(&collection).cloned() {
            for colour in DYE_COLOURS {
                family_ids.push(format!("{colour}_{suffix}"));
            }
        } else {
            // Weathering copper: expand the four weathering states (both waxed and not).
            let base = collection.to_lowercase();
            for state in ["UNAFFECTED", "EXPOSED", "WEATHERED", "OXIDIZED"] {
                let prefix = match state {
                    "EXPOSED" => "exposed_",
                    "WEATHERED" => "weathered_",
                    "OXIDIZED" => "oxidized_",
                    _ => "",
                };
                if base == "copper_block" {
                    if state == "UNAFFECTED" {
                        family_ids.push("copper_block".to_string());
                    } else {
                        family_ids.push(format!("{prefix}copper"));
                    }
                } else {
                    family_ids.push(format!("{prefix}{base}"));
                }
            }
        }
        let all: Vec<String> = family_ids
            .iter()
            .flat_map(|id| {
                if collection.to_lowercase() == "copper_block" {
                    vec![id.clone(), format!("waxed_{id}")]
                } else if out.colour_families.contains_key(&collection) {
                    vec![id.clone()]
                } else {
                    vec![id.clone(), format!("waxed_{id}")]
                }
            })
            .collect();
        for id in all {
            let mut block_facts = facts.clone();
            block_facts.family = true;
            block_facts.item = Some(id.clone());
            out.facts.insert(format!("minecraft:{id}"), block_facts);
        }
        return;
    }
    let Some((owner, field)) = id else { return };
    let block_key = match owner.as_str() {
        "BlockItemIds" => out.items.get(&field).cloned().unwrap_or_else(|| field.to_lowercase()),
        "BlockIds" => field.to_lowercase(),
        _ => field.to_lowercase(),
    };
    let registry_id = format!("minecraft:{block_key}");
    facts.item = if let Some(item) = out.items.get(&block_key) {
        Some(item.clone())
    } else if let Some(base) = facts.copy_base.as_ref() {
        let base_key = base.to_lowercase();
        Some(out.items.get(&base_key).cloned().unwrap_or(base_key))
    } else {
        Some("air".to_string())
    };
    out.facts.entry(registry_id).or_insert(facts);
}

fn collect_calls(
    jar: &Jar,
    class: &classfile::ClassFile,
    body: &[Instruction],
    facts: &mut Facts,
    id: &mut Option<(String, String)>,
    collection: &mut Option<String>,
    register_family: &mut bool,
    visited: &mut BTreeSet<String>,
) {
    // `wallVariant(base, ...)`: remember the base field.
    for (index, instruction) in body.iter().enumerate() {
        let Some((owner, name, descriptor)) = instruction.index.and_then(|index| member(class, index)) else {
            continue;
        };
        let owner_short = short(&owner).to_string();
        if owner_short == "Blocks" && name == "wallVariant" {
            for earlier in body[..index].iter().rev() {
                if earlier.opcode == 0xb2 {
                    if let Some((base_owner, base_field, base_desc)) =
                        earlier.index.and_then(|index| member(class, index))
                    {
                        if short(&base_owner) == "Blocks" && base_desc.ends_with("/Block;") {
                            facts.copy_base = Some(base_field);
                            break;
                        }
                    }
                }
            }
        }
        let _ = descriptor;
    }
    for (index, instruction) in body.iter().enumerate() {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, field, descriptor)) =
                    instruction.index.and_then(|index| member(class, index))
                {
                    let owner_short = short(&owner);
                    if (owner_short == "BlockItemIds" || owner_short == "BlockIds")
                        && id.is_none()
                    {
                        *id = Some((owner_short.to_string(), field.clone()));
                    }
                    if (owner_short == "BlockItemIds" || owner_short == "BlockIds")
                        && (descriptor.contains("ColorCollection")
                            || descriptor.contains("WeatheringCopperCollection"))
                    {
                        *collection = Some(field.clone());
                    }
                }
            }
            0xb6 | 0xb8 => {
                let Some((owner, name, descriptor)) =
                    instruction.index.and_then(|index| member(class, index))
                else {
                    continue;
                };
                let owner_short = short(&owner);
                if name == "registerBlocks"
                    && (owner_short == "ColorCollection"
                        || owner_short == "WeatheringCopperCollection")
                {
                    *register_family = true;
                }
                if name == "zipMap" && owner_short == "ColorCollection" {
                    // The blocks it registers belong to the *second* collection (the last
                    // `ColorCollection` field read before the call).
                    for earlier in body[..index].iter().rev() {
                        if earlier.opcode == 0xb2 {
                            if let Some((field_owner, field, descriptor)) =
                                earlier.index.and_then(|index| member(class, index))
                            {
                                if descriptor.contains("ColorCollection") {
                                    *collection = Some(field);
                                    *register_family = true;
                                    let _ = field_owner;
                                    break;
                                }
                            }
                        }
                    }
                }
                if owner_short == "Blocks" && name == "register" {
                    // A registration: the id is the first argument if not seen yet.
                    if id.is_none() {
                        for earlier in body[..index].iter().rev() {
                            if earlier.opcode == 0xb2 {
                                if let Some((id_owner, field, _)) =
                                    earlier.index.and_then(|index| member(class, index))
                                {
                                    if short(&id_owner) == "BlockItemIds"
                                        || short(&id_owner) == "BlockIds"
                                    {
                                        *id = Some((short(&id_owner).to_string(), field));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if owner_short == "Blocks" && name != "<init>" && name != "<clinit>" {
                    let key = format!("{name}{descriptor}");
                    if visited.insert(key.clone()) && visited.len() < 64 {
                        if let Some(helper) = class
                            .methods
                            .iter()
                            .find(|method| method.name == name && method.descriptor == descriptor)
                        {
                            if let Some(code) = helper.code.as_ref() {
                                let inner = decode(&code.bytes);
                                collect_calls(jar, class, &inner, facts, id, collection, register_family, visited);
                            }
                        }
                    }
                }
                let properties = owner_short.ends_with("Properties");
                if properties {
                    match name.as_str() {
                        "randomTicks" => facts.random = true,
                        "forceSolidOn" => facts.force_on = true,
                        "forceSolidOff" => facts.force_off = true,
                        "lightLevel" => {
                            facts.light = light_at(jar, class, body, index);
                        }
                        "ofFullCopy" | "ofLegacyCopy" => {
                            for earlier in body[..index].iter().rev() {
                                if earlier.opcode == 0xb2 {
                                    if let Some((base_owner, base_field, base_desc)) =
                                        earlier.index.and_then(|index| member(class, index))
                                    {
                                        if short(&base_owner) == "Blocks"
                                            && base_desc.ends_with("/Block;")
                                        {
                                            if facts.copy_base.is_none() {
                                                facts.copy_base = Some(base_field);
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if instruction.opcode == 0xb6 && name == "lightLevel" {
                    // handled above
                }
                let _ = descriptor;
            }
            0xba => {
                // A lambda used by the chain: scan its body too.
                if let Some((owner, name)) = lambda_ref(class, instruction.index.unwrap_or(0)) {
                    let key = format!("lambda:{owner}.{name}");
                    if visited.insert(key) && visited.len() < 128 {
                        if let Some(lambda_class) = jar.class(&owner) {
                            if let Some(method) = lambda_class
                                .methods
                                .iter()
                                .find(|method| method.name == name)
                            {
                                if let Some(code) = method.code.as_ref() {
                                    let inner = decode(&code.bytes);
                                    // Only the property calls matter; recursion stays shallow.
                                    let mut ignored = None;
                                    let mut ignored_collection = None;
                                    let mut ignored_family = false;
                                    collect_calls(
                                        jar,
                                        &lambda_class,
                                        &inner,
                                        facts,
                                        &mut ignored,
                                        &mut ignored_collection,
                                        &mut ignored_family,
                                        visited,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Reads the light function argument immediately before a `lightLevel` call.
fn light_at(
    jar: &Jar,
    class: &classfile::ClassFile,
    body: &[Instruction],
    call: usize,
) -> Light {
    if call == 0 {
        return Light::None;
    }
    let previous = &body[call - 1];
    match previous.opcode {
        0xba => {
            let Some((owner, method)) = lambda_ref(class, previous.index.unwrap_or(0)) else {
                return Light::None;
            };
            classify_light_lambda(jar, &owner, &method)
        }
        0xb8 => {
            let Some((owner, name, _)) =
                previous.index.and_then(|index| member(class, index))
            else {
                return Light::None;
            };
            let owner = short(&owner).to_string();
            if name == "litBlockEmission" {
                return Light::Lit(int_before(body, call - 1));
            }
            if owner == "GlowLichenBlock" && name == "emission" {
                return Light::Glow(int_before(body, call - 1));
            }
            if owner == "CaveVines" && name == "emission" {
                return Light::Berries(int_before(body, call - 1));
            }
            Light::None
        }
        0xb2 => {
            let Some((owner, field, _)) = previous.index.and_then(|index| member(class, index))
            else {
                return Light::None;
            };
            let owner = short(&owner).to_string();
            if owner == "CandleBlock" && field == "LIGHT_EMISSION" {
                Light::Candle
            } else if owner == "LightBlock" && field == "LIGHT_EMISSION" {
                Light::Level
            } else {
                Light::None
            }
        }
        _ => Light::None,
    }
}

fn int_before(body: &[Instruction], at: usize) -> i32 {
    for instruction in body[..at].iter().rev() {
        match instruction.opcode {
            0x02 => return -1,
            0x03..=0x08 => return (instruction.opcode - 0x03) as i32,
            0x10 | 0x11 => {
                if let Some(value) = instruction.immediate {
                    return value;
                }
            }
            _ => {}
        }
    }
    0
}

fn classify_light_lambda(jar: &Jar, owner: &str, method: &str) -> Light {
    match method {
        "lambda$static$321" => return Light::SeaPickle,
        "lambda$static$351" => return Light::RespawnAnchor,
        "lambda$static$421" => return Light::TrialSpawner,
        "lambda$static$422" => return Light::Vault,
        _ => {}
    }
    let Some(class) = jar.class(owner) else {
        return Light::None;
    };
    let Some(lambda) = class.methods.iter().find(|method_ref| method_ref.name == method) else {
        return Light::None;
    };
    let Some(code) = lambda.code.as_ref() else {
        return Light::None;
    };
    let body = decode(&code.bytes);
    let mut has_get_value = false;
    let mut constant = None;
    for instruction in &body {
        match instruction.opcode {
            0x02 => constant = Some(-1),
            0x03..=0x08 => constant = Some((instruction.opcode - 0x03) as i32),
            0x10 | 0x11 => {
                if let Some(value) = instruction.immediate {
                    constant = Some(value);
                }
            }
            0xb6 => {
                if let Some((_, name, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if name == "getValue" {
                        has_get_value = true;
                    }
                }
            }
            0xb2 => {
                if let Some((field_owner, field, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if has_get_value || short(&field_owner) == "BlockStateProperties" {
                        let field = field.to_ascii_uppercase();
                        if field.contains("LEVEL") {
                            return Light::Level;
                        }
                        if field.contains("CANDLE") || field == "LIT" {
                            return Light::Candle;
                        }
                        if field.contains("BERR") {
                            return Light::Berries(0);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if has_get_value {
        Light::None
    } else if let Some(value) = constant {
        Light::Constant(value)
    } else {
        Light::None
    }
}

// ---------------------------------------------------------------------------
// Per-state helpers
// ---------------------------------------------------------------------------

fn state_pairs(properties: &[states::Property], state: usize) -> Vec<(String, String)> {
    let mut order: Vec<&states::Property> = properties.iter().collect();
    order.sort_by(|left, right| left.name.cmp(&right.name));
    let mut out = Vec::with_capacity(order.len());
    for (position, property) in order.iter().enumerate() {
        let mut stride = 1usize;
        for later in order.iter().skip(position + 1) {
            stride *= later.values.len().max(1);
        }
        let count = property.values.len().max(1);
        let index = (state / stride) % count;
        out.push((
            property.name.clone(),
            property.values.get(index).cloned().unwrap_or_default(),
        ));
    }
    out
}

fn value_u32(pairs: &[(String, String)], name: &str) -> Option<u32> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| value.parse::<u32>().ok())
}

fn value_bool(pairs: &[(String, String)], name: &str) -> Option<bool> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
}

fn value_name<'a>(pairs: &'a [(String, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .unwrap_or("")
}

fn is_max_age(pairs: &[(String, String)], max: u32) -> bool {
    value_u32(pairs, "age").unwrap_or(0) >= max
}

fn weather_state_of(block: &str) -> Option<&'static str> {
    let path = block.strip_prefix("minecraft:")?;
    let path = path.strip_prefix("waxed_").unwrap_or(path);
    if path.starts_with("exposed_") || path.contains("exposed_copper") {
        Some("EXPOSED")
    } else if path.starts_with("weathered_") || path.contains("weathered_copper") {
        Some("WEATHERED")
    } else if path.starts_with("oxidized_") || path.contains("oxidized_copper") {
        Some("OXIDIZED")
    } else if path.contains("copper") || path.contains("lightning_rod") {
        Some("UNAFFECTED")
    } else {
        None
    }
}

fn is_random_tick(
    class_simple: &str,
    block: &str,
    pairs: &[(String, String)],
    property_random: bool,
) -> bool {
    match class_simple {
        "ChorusFlowerBlock" => value_u32(pairs, "age").unwrap_or(0) < 5,
        "CocoaBlock" => value_u32(pairs, "age").unwrap_or(0) < 2,
        "CropBlock" => !is_max_age(pairs, 7),
        "FlowerPotBlock" => matches!(
            block,
            "minecraft:potted_open_eyeblossom" | "minecraft:potted_closed_eyeblossom"
        ),
        "GrowingPlantHeadBlock" => value_u32(pairs, "age").unwrap_or(0) < 25,
        "LeavesBlock" => property_random,
        "LiquidBlock" => block == "minecraft:lava",
        "NetherWartBlock" => value_u32(pairs, "age").unwrap_or(0) < 3,
        "PitcherCropBlock" => {
            value_name(pairs, "half") == "lower" && !is_max_age(pairs, 4)
        }
        "RedStoneOreBlock" => value_bool(pairs, "lit").unwrap_or(false),
        "SweetBerryBushBlock" => value_u32(pairs, "age").unwrap_or(0) < 3,
        "BambooStalkBlock" => value_u32(pairs, "stage").unwrap_or(0) == 0,
        class if class.starts_with("Weathering") => {
            let waxed = block.contains("waxed_");
            !waxed && weather_state_of(block) != Some("OXIDIZED")
        }
        _ => property_random,
    }
}

fn light_value(
    light: &Light,
    block: &str,
    pairs: &[(String, String)],
) -> u32 {
    match light {
        Light::None => 0,
        Light::Constant(value) => *value as u32,
        Light::Lit(value) => {
            if value_bool(pairs, "lit").unwrap_or(false) {
                *value as u32
            } else {
                0
            }
        }
        Light::Candle => {
            if value_bool(pairs, "lit").unwrap_or(false) {
                3 * value_u32(pairs, "candles").unwrap_or(1)
            } else {
                0
            }
        }
        Light::Level => value_u32(pairs, "level").unwrap_or(0),
        Light::Glow(value) => {
            let any = ["north", "east", "south", "west", "up", "down"]
                .iter()
                .any(|face| value_bool(pairs, face).unwrap_or(false));
            if any {
                *value as u32
            } else {
                0
            }
        }
        Light::Berries(value) => {
            if value_bool(pairs, "berries").unwrap_or(false) {
                *value as u32
            } else {
                0
            }
        }
        Light::SeaPickle => {
            if value_bool(pairs, "waterlogged").unwrap_or(false) {
                3 + 3 * value_u32(pairs, "pickles").unwrap_or(1)
            } else {
                0
            }
        }
        Light::RespawnAnchor => {
            let charge = value_u32(pairs, "charge").unwrap_or(0);
            ((charge as f32) / 4.0 * 15.0).floor() as u32
        }
        Light::TrialSpawner => {
            match value_name(pairs, "trial_spawner_state") {
                "waiting_for_players" => 4,
                "active" | "waiting_for_reward_ejection" | "ejecting_reward" => 8,
                _ => 0,
            }
        }
        Light::Vault => match value_name(pairs, "vault_state") {
            "active" | "unlocking" | "ejecting" => 12,
            _ => 6,
        },
    }
    .min(15)
    .max(0)
    .max(if block.is_empty() { 0 } else { 0 })
}

fn propagates_skylight(
    class_simple: &str,
    pairs: &[(String, String)],
    shape_full: bool,
    fluid_empty: bool,
) -> bool {
    match class_simple {
        "BambooStalkBlock" | "HangingMossBlock" | "MossyCarpetBlock" | "TransparentBlock"
        | "VineBlock" => true,
        "BarrierBlock" | "GlowLichenBlock" | "LightBlock" | "VegetationBlock" => fluid_empty,
        "CrossCollisionBlock" | "WallBlock" => !value_bool(pairs, "waterlogged").unwrap_or(false),
        "LiquidBlock" | "PipeBlock" | "ShulkerBoxBlock" | "TintedGlassBlock" => false,
        _ => !shape_full && fluid_empty,
    }
}

fn shape_occlusion_class(class_simple: &str, pairs: &[(String, String)]) -> bool {
    match class_simple {
        "DaylightDetectorBlock" | "EnchantingTableBlock" | "EndPortalFrameBlock"
        | "FarmlandBlock" | "LecternBlock" | "PathBlock" | "SculkSensorBlock"
        | "SculkShriekerBlock" | "ShelfBlock" | "SnowLayerBlock" | "StairBlock"
        | "StonecutterBlock" => true,
        "SlabBlock" => value_name(pairs, "type") != "double",
        "PistonBaseBlock" => value_bool(pairs, "extended").unwrap_or(false),
        "PistonHeadBlock" => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Discrete shape algorithms: `VoxelShape.toAabbs()` / `.forAllEdges()`
// ---------------------------------------------------------------------------

struct Grid {
    x: Vec<f64>,
    y: Vec<f64>,
    z: Vec<f64>,
    occupied: Vec<bool>,
    ny: usize,
    nz: usize,
}

impl Grid {
    fn new(shape: &geometry::Shape) -> Grid {
        let ny = shape.y.len().saturating_sub(1);
        let nz = shape.z.len().saturating_sub(1);
        let occupied = shape
            .occupied
            .iter()
            .map(|value| *value != 0)
            .collect();
        Grid {
            x: shape.x.clone(),
            y: shape.y.clone(),
            z: shape.z.clone(),
            occupied,
            ny,
            nz,
        }
    }

    fn nx(&self) -> usize {
        self.x.len().saturating_sub(1)
    }

    fn full(&self, x: usize, y: usize, z: usize) -> bool {
        if x >= self.nx() || y >= self.ny || z >= self.nz {
            return false;
        }
        let index = (x * self.ny + y) * self.nz + z;
        self.occupied.get(index).copied().unwrap_or(false)
    }

    fn set(&mut self, x: usize, y: usize, z: usize, value: bool) {
        if x >= self.nx() || y >= self.ny || z >= self.nz {
            return;
        }
        let index = (x * self.ny + y) * self.nz + z;
        if index < self.occupied.len() {
            self.occupied[index] = value;
        }
    }

    fn clear_z_strip(&mut self, z1: usize, z2: usize, x: usize, y: usize) {
        for z in z1..z2 {
            self.set(x, y, z, false);
        }
    }

    fn z_strip_full(&self, z1: usize, z2: usize, x: usize, y: usize) -> bool {
        if x >= self.nx() || y >= self.ny {
            return false;
        }
        (z1..z2).all(|z| self.full(x, y, z))
    }

    fn xz_rectangle_full(&self, x1: usize, x2: usize, z1: usize, z2: usize, y: usize) -> bool {
        (x1..x2).all(|x| (z1..z2).all(|z| self.full(x, y, z)))
    }
}

fn for_all_boxes(shape: &geometry::Shape) -> Vec<[f64; 6]> {
    let mut grid = Grid::new(shape);
    let mut out = Vec::new();
    let ny = grid.ny;
    let nz = grid.nz;
    let nx = grid.nx();
    for y in 0..ny {
        for x in 0..nx {
            let mut start_z: Option<usize> = None;
            for z in 0..=nz {
                let full = z < nz && grid.full(x, y, z);
                if full {
                    if start_z.is_none() {
                        start_z = Some(z);
                    }
                } else if let Some(start) = start_z.take() {
                    let mut x2 = x;
                    let mut y2 = y;
                    grid.clear_z_strip(start, z, x2, y2);
                    while grid.z_strip_full(start, z, x2 + 1, y2) {
                        grid.clear_z_strip(start, z, x2 + 1, y2);
                        x2 += 1;
                    }
                    while grid.xz_rectangle_full(x, x2 + 1, start, z, y2 + 1) {
                        for x3 in x..=x2 {
                            grid.clear_z_strip(start, z, x3, y2 + 1);
                        }
                        y2 += 1;
                    }
                    out.push([
                        grid.x[x],
                        grid.y[y],
                        grid.z[start],
                        grid.x[x2 + 1],
                        grid.y[y2 + 1],
                        grid.z[z],
                    ]);
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Cycle {
    None,
    Forward,
    Backward,
}

impl Cycle {
    fn inverse(self) -> Cycle {
        match self {
            Cycle::None => Cycle::None,
            Cycle::Forward => Cycle::Backward,
            Cycle::Backward => Cycle::Forward,
        }
    }

    fn axis(self, axis: usize) -> usize {
        match self {
            Cycle::None => axis,
            Cycle::Forward => (axis + 1) % 3,
            Cycle::Backward => (axis + 2) % 3,
        }
    }

    fn coords(self, x: usize, y: usize, z: usize, axis: usize) -> usize {
        match self {
            Cycle::None => [x, y, z][axis],
            Cycle::Forward => [z, x, y][axis],
            Cycle::Backward => [y, z, x][axis],
        }
    }
}

fn for_all_edges(shape: &geometry::Shape) -> Vec<[f64; 6]> {
    let grid = Grid::new(shape);
    let mut out = Vec::new();
    for cycle in [Cycle::None, Cycle::Forward, Cycle::Backward] {
        let inverse = cycle.inverse();
        let sx = match inverse.axis(0) {
            0 => grid.nx(),
            1 => grid.ny,
            _ => grid.nz,
        };
        let sy = match inverse.axis(1) {
            0 => grid.nx(),
            1 => grid.ny,
            _ => grid.nz,
        };
        let sz = match inverse.axis(2) {
            0 => grid.nx(),
            1 => grid.ny,
            _ => grid.nz,
        };
        let full = |x: isize, y: isize, z: isize| -> bool {
            if x < 0 || y < 0 || z < 0 {
                return false;
            }
            let real = [
                inverse.coords(x as usize, y as usize, z as usize, 0),
                inverse.coords(x as usize, y as usize, z as usize, 1),
                inverse.coords(x as usize, y as usize, z as usize, 2),
            ];
            grid.full(real[0], real[1], real[2])
        };
        for a in 0..=sx {
            for b in 0..=sy {
                let mut start: Option<usize> = None;
                for c in 0..=sz {
                    let mut count = 0usize;
                    let mut parity = 0usize;
                    for i in 0..=1 {
                        for j in 0..=1 {
                            if full(a as isize + i as isize - 1, b as isize + j as isize - 1, c as isize)
                            {
                                count += 1;
                                parity ^= i ^ j;
                            }
                        }
                    }
                    let edge =
                        count == 1 || count == 3 || (count == 2 && (parity & 1) == 0);
                    if edge {
                        if start.is_none() {
                            start = Some(c);
                        }
                    } else if let Some(start) = start.take() {
                        let first = [
                            inverse.coords(a, b, start, 0),
                            inverse.coords(a, b, start, 1),
                            inverse.coords(a, b, start, 2),
                        ];
                        let second = [
                            inverse.coords(a, b, c, 0),
                            inverse.coords(a, b, c, 1),
                            inverse.coords(a, b, c, 2),
                        ];
                        out.push([
                            grid.x[first[0]],
                            grid.y[first[1]],
                            grid.z[first[2]],
                            grid.x[second[0]],
                            grid.y[second[1]],
                            grid.z[second[2]],
                        ]);
                    }
                }
            }
        }
    }
    out
}

fn shift(shape: &geometry::Shape, delta: [f64; 3]) -> geometry::Shape {
    let mut out = shape.clone();
    for value in &mut out.x {
        *value -= delta[0];
    }
    for value in &mut out.y {
        *value -= delta[1];
    }
    for value in &mut out.z {
        *value -= delta[2];
    }
    out
}

// ---------------------------------------------------------------------------
// Offsets
// ---------------------------------------------------------------------------

struct Dummy;

impl shape_offset::vm::ShapeFactory for Dummy {
    type Shape = geometry::Shape;

    fn empty(&mut self) -> geometry::Shape {
        geometry::Shape::empty()
    }

    fn block(&mut self) -> geometry::Shape {
        geometry::Shape::block()
    }

    fn create(&mut self, _x1: f64, _y1: f64, _z1: f64, _x2: f64, _y2: f64, _z2: f64) -> geometry::Shape {
        geometry::Shape::empty()
    }

    fn or(&mut self, first: &geometry::Shape, _second: &geometry::Shape) -> geometry::Shape {
        first.clone()
    }

    fn join(
        &mut self,
        first: &geometry::Shape,
        _second: &geometry::Shape,
        _operation: shape_offset::vm::JoinOp,
    ) -> geometry::Shape {
        first.clone()
    }

    fn rotate(
        &mut self,
        shape: &geometry::Shape,
        _rotation: &shape_offset::vm::Rotation,
        _center: [f64; 3],
    ) -> geometry::Shape {
        shape.clone()
    }

    fn translate(&mut self, shape: &geometry::Shape, _delta: [f64; 3]) -> geometry::Shape {
        shape.clone()
    }

    fn is_empty(&mut self, shape: &geometry::Shape) -> bool {
        shape.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Facts resolution
// ---------------------------------------------------------------------------

fn resolve_facts(scan: &mut Scanned) {
    let snapshot = scan.facts.clone();
    for _ in 0..8 {
        let before = scan.facts.clone();
        for (id, facts) in scan.facts.iter_mut() {
            let Some(base) = facts.copy_base.clone() else {
                continue;
            };
            let base_id = format!("minecraft:{}", base.to_lowercase());
            let Some(source) = snapshot.get(&base_id) else {
                continue;
            };
            if !facts.random {
                facts.random = source.random;
            }
            if !facts.force_on {
                facts.force_on = source.force_on;
            }
            if !facts.force_off {
                facts.force_off = source.force_off;
            }
            if facts.light == Light::None {
                facts.light = source.light.clone();
            }
            let _ = id;
        }
        if before == scan.facts {
            break;
        }
    }
    for facts in scan.facts.values_mut() {
        if facts.light == Light::None {
            facts.light = Light::Constant(0);
        }
    }
}

fn render_shape(class_simple: &str) -> &'static str {
    match class_simple {
        "AirBlock" | "BarrierBlock" | "BubbleColumnBlock" | "EndGatewayBlock"
        | "EndPortalBlock" | "LightBlock" | "LiquidBlock" | "MovingPistonBlock"
        | "StructureVoidBlock" => "INVISIBLE",
        _ => "MODEL",
    }
}

fn full_block(shape: &geometry::Shape) -> bool {
    shape.x == vec![0.0, 1.0] && shape.y == vec![0.0, 1.0] && shape.z == vec![0.0, 1.0]
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

/// Builds the compact gameplay document from the jar.
pub fn document(jar_path: &Path) -> Result<Value, String> {
    let jar = Jar::open(jar_path)?;
    let mut scan = scan_facts(&jar);
    resolve_facts(&mut scan);
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    let exact = exact::rows(jar_path)?;
    let shapes = shape_final::evaluate(jar_path)?;
    let offset_vm = shape_offset::OffsetVm::new(jar_path, Dummy).ok();
    if registry.len() != state_table.blocks.len() || exact.blocks.len() != registry.len() {
        return Err("the block registry and the state table disagree".to_string());
    }
    let internal_names: BTreeMap<String, String> = registry
        .iter()
        .filter_map(|block| jar.internal(&block.class).map(|internal| (block.class.clone(), internal)))
        .collect();
    let mut coverage = Coverage {
        produced: vec![TABLE.to_string()],
        ..Default::default()
    };
    let mut values: Vec<Vec<Value>> = Vec::with_capacity(shapes.full_block.len());
    let mut blocks_per_state: Vec<usize> = Vec::with_capacity(shapes.full_block.len());
    let mut state = 0usize;
    for (block_index, block_states) in state_table.blocks.iter().enumerate() {
        let id = &block_states.id;
        let class_simple = registry[block_index].class.clone();
        let facts = scan.facts.get(id).cloned().unwrap_or_default();
        let item = facts.item.clone().unwrap_or_else(|| "air".to_string());
        let render = render_shape(class_simple.as_str());
        let row = &exact.rows[block_index];
        for local in 0..block_states.state_count {
            let pairs = state_pairs(&block_states.properties, local);
            macro_rules! convert_shape {
                ($source:expr) => {
                    geometry::Shape {
                        x: $source.x.clone(),
                        y: $source.y.clone(),
                        z: $source.z.clone(),
                        canonical: $source.canonical,
                        cube: $source.cube,
                        cube_axes: $source.cube_axes,
                        discrete_min: $source.discrete_min,
                        discrete_max: $source.discrete_max,
                        occupied: $source.occupied.clone(),
                    }
                };
            }
            let shape: Option<geometry::Shape> = match class_simple.as_str() {
                "FenceBlock" => shapes
                    .occlusion
                    .get(state)
                    .and_then(|shape| shape.as_ref())
                    .map(|source| convert_shape!(source)),
                "FenceGateBlock" => shapes
                    .collision
                    .get(state)
                    .and_then(|shape| shape.as_ref())
                    .map(|source| convert_shape!(source)),
                _ => shapes
                    .occlusion
                    .get(state)
                    .and_then(|shape| shape.as_ref())
                    .map(|source| convert_shape!(source)),
            };
            let collision: Option<geometry::Shape> = shapes
                .collision
                .get(state)
                .and_then(|shape| shape.as_ref())
                .map(|source| convert_shape!(source));
            let fluid_empty = if row.get("liquid").and_then(Value::as_bool).unwrap_or(false) {
                false
            } else {
                !value_bool(&pairs, "waterlogged").unwrap_or(false)
            };
            let shape_full = shape.as_ref().map(full_block).unwrap_or(false);
            let skylight = propagates_skylight(class_simple.as_str(), &pairs, shape_full, fluid_empty);
            let occlusion_full = shapes
                .occlusion
                .get(state)
                .and_then(|shape| shape.as_ref())
                .map(|shape| {
                    full_block(&convert_shape!(shape))
                })
                .unwrap_or(false);
            let can_occlude = row.get("can_occlude").and_then(Value::as_bool).unwrap_or(true);
            let solid = can_occlude && occlusion_full;
            let dampening = match class_simple.as_str() {
                "LeavesBlock" => 1,
                "TintedGlassBlock" => 15,
                _ => {
                    if solid {
                        15
                    } else if skylight {
                        0
                    } else {
                        1
                    }
                }
            };
            let offset_flag = row.get("offset").and_then(Value::as_bool).unwrap_or(false);
            let mut outline_offset = false;
            let mut outline_shape = shape.clone();
            if offset_flag {
                if let Some(vm) = offset_vm.as_ref() {
                    let internal = internal_names
                        .get(&class_simple)
                        .cloned()
                        .unwrap_or_else(|| class_simple.clone());
                    let mut value_map = BTreeMap::new();
                    for (name, value) in &pairs {
                        value_map.insert(name.clone(), value.clone());
                    }
                    let offset_state = shape_offset::vm::State {
                        class: internal,
                        block: id.clone(),
                        values: value_map,
                        has_collision: true,
                    };
                    let uses = vm.uses_offset(&offset_state.class);
                    outline_offset = uses;
                    if uses {
                        let delta = vm.offset(&offset_state);
                        if let Some(current) = &outline_shape {
                            outline_shape = Some(shift(current, delta));
                        }
                    }
                }
            }
            let (boxes, edges) = match &outline_shape {
                Some(shape) => (
                    for_all_boxes(shape)
                        .into_iter()
                        .map(|b| Value::Array(b.iter().map(|v| Value::from(*v)).collect()))
                        .collect::<Vec<Value>>(),
                    for_all_edges(shape)
                        .into_iter()
                        .map(|b| Value::Array(b.iter().map(|v| Value::from(*v)).collect()))
                        .collect::<Vec<Value>>(),
                ),
                None => {
                    coverage.outline_gaps.push(state);
                    (Vec::new(), Vec::new())
                }
            };
            let mut row_values = Vec::with_capacity(FIELDS.len());
            for field in FIELDS {
                let value = match field {
                    "dampening" => Value::from(dampening),
                    "emission" => Value::from(light_value(&facts.light, id, &pairs)),
                    "item" => Value::String(format!("minecraft:{item}")),
                    "outline" => Value::Array(boxes.clone()),
                    "outline_edges" => Value::Array(edges.clone()),
                    "outline_offset" => Value::Bool(outline_offset),
                    "random_ticks" => Value::Bool(is_random_tick(
                        class_simple.as_str(),
                        id,
                        &pairs,
                        facts.random,
                    )),
                    "render_shape" => Value::String(render.to_string()),
                    "shape_occlusion" => Value::Bool(shape_occlusion_class(class_simple.as_str(), &pairs)),
                    "skylight_down" => Value::Bool(skylight),
                    "solid_render" => Value::Bool(solid),
                    "full_collision" => {
                        Value::Bool(shapes.full_block.get(state).cloned().flatten().unwrap_or(false))
                    }
                    "map_color" => exact
                        .map_colors
                        .get(&block_index)
                        .and_then(|colors| colors.get(local))
                        .cloned()
                        .or_else(|| row.get("map_color").cloned())
                        .unwrap_or(Value::Null),
                    "sound" => exact
                        .sounds
                        .get(&block_index)
                        .and_then(|sounds| sounds.get(local))
                        .cloned()
                        .or_else(|| row.get("sound").cloned())
                        .unwrap_or(Value::Null),
                    other => row.get(other).cloned().unwrap_or(Value::Null),
                };
                row_values.push(value);
            }
            values.push(row_values);
            blocks_per_state.push(block_index);
            state += 1;
        }
    }
    let fields: Vec<String> = FIELDS.iter().map(|field| field.to_string()).collect();
    let facts = gameplay_facts::Facts {
        fields,
        blocks: registry.iter().map(|block| block.id.clone()).collect(),
        values,
        blocks_per_state,
        uncovered: Vec::new(),
    };
    let text = gameplay_facts::render(&facts);
    let document = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let _ = internal_names;
    Ok(document)
}

fn bounds_size(coords: &[f64]) -> f64 {
    match (coords.first(), coords.last()) {
        (Some(first), Some(last)) => (last - first).abs(),
        _ => 0.0,
    }
}

/// Builds the table and writes it below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let document = document(jar_path)?;
    let text = serde_json::to_string(&document).map_err(|error| error.to_string())?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Coverage {
        produced: vec![TABLE.to_string()],
        missing: Vec::new(),
        notes: Vec::new(),
        outline_gaps: Vec::new(),
        item_gaps: Vec::new(),
        light_gaps: Vec::new(),
    })
}












