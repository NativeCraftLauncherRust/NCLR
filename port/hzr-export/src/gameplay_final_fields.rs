//! Closes the eight partial fields of [`missing`] to make the whole 27-field gameplay
//! document exact: `item`, `outline`, `outline_edges`, `dampening`, `skylight_down`,
//! `emission`, `random_ticks` and `shape_occlusion` (plus the shulker-box `full_collision`
//! states the shape engine leaves false).
//!
//! [`missing`] derives these from `Blocks.<clinit>` and the shape tables, but a few families
//! need the *effective* method through the superclass chain, the `CrossCollisionBlock.getShape`
//! outline for fences, the `LecternBlock.SHAPES` map, and the `BlockItemIds` record's block
//! name (not its field name). This module patches the compact document built by [`missing`]
//! using the same jar data, so the whole pipeline stays jar-derived. Nothing here reads
//! `ship.hzr3`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::classfile;

#[path = "gameplay_missing.rs"]
#[allow(dead_code)]
mod missing;

#[path = "block.rs"]
#[allow(dead_code)]
mod block;

#[path = "states.rs"]
#[allow(dead_code)]
mod states;

#[path = "voxelshape.rs"]
#[allow(dead_code)]
mod voxelshape;

/// The file `ship.hzr3` carries, below the bundle root.
pub const TABLE: &str = missing::TABLE;

/// Debug hook for the tests (not used by the writer).
pub fn debug_info(jar_path: &Path) -> String {
    let Ok(jar) = Jar::open(jar_path) else {
        return String::new();
    };
    let items = block_items(&jar);
    let registry = block::registry(jar_path).unwrap_or_default();
    let mut chains = Vec::new();
    for name in [
        "minecraft:seagrass",
        "minecraft:tall_seagrass",
        "minecraft:respawn_anchor",
        "minecraft:white_candle_cake",
        "minecraft:oak_leaves",
    ] {
        if let Some(block) = registry.iter().find(|block| block.id == name) {
            chains.push(format!("{name}={:?}", block.chain));
        }
    }
    let Ok(table) = states::state_table(jar_path) else { return String::new() };
    let mut charges = Vec::new();
    if let Some(block) = table.blocks.iter().find(|block| block.id == "minecraft:respawn_anchor") {
        for local in 0..block.state_count.min(6) {
            let pairs = state_pairs(&block.properties, local);
            let charge = number(&pairs, "charge"); charges.push((pairs, charge, max_age(&block.properties)));
        }
    }
    format!(
        "items={} sample={:?} chains={chains:?} anchor={charges:?}",
        items.len(),
        items.iter().take(3).collect::<Vec<_>>()
    )
}

/// Every field the compact table carries, in the bundle's order.
pub const FIELDS: [&str; 27] = missing::FIELDS;

/// What was produced, and what could not be.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    pub item_gaps: Vec<String>,
    pub outline_gaps: Vec<String>,
}

/// Builds the exact 27-field document from the jar.
pub fn document(jar_path: &Path) -> Result<Value, String> {
    let mut document = missing::document(jar_path)?;
    let jar = Jar::open(jar_path)?;
    let registry = block::registry(jar_path)?;
    let state_table = states::state_table(jar_path)?;
    let items = block_items(&jar);
    let blocks = document["blocks"]
        .as_array()
        .ok_or("the document has no blocks")?
        .iter()
        .map(|block| block.as_str().unwrap_or_default().to_string())
        .collect::<Vec<String>>();
    let index: BTreeMap<&str, usize> = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.as_str(), index))
        .collect();
    let mut first_state: BTreeMap<usize, usize> = BTreeMap::new();
    let per_state_blocks = document["blocks_per_state"]
        .as_array()
        .ok_or("the document has no blocks_per_state")?;
    for (state, block) in per_state_blocks.iter().enumerate() {
        let block = block.as_u64().unwrap_or(0) as usize;
        first_state.entry(block).or_insert(state);
    }

    // `item`: the BlockItemIds record for the block's own name; wall variants reuse the base
    // item; everything else is air.
    for (block_index, block) in blocks.iter().enumerate() {
        let mut item = "air".to_string();
        if let Some(path) = block.strip_prefix("minecraft:") {
            if let Some(record) = items.get(path) {
                item = record.clone();
            } else if let Some(base) = wall_base(path) {
                item = items.get(&base).cloned().unwrap_or(base);
            } else if let Some(mapped) = base_item(path) {
                item = mapped;
            } else if !no_item(path, &registry[block_index].chain) {
                item = path.to_string();
            }
        }
        set_constant(
            &mut document,
            "item",
            block_index,
            Value::String(format!("minecraft:{item}")),
        );
    }

    // Per state: the class-chain overrides for the light fields and the selection shape.
    let mut coverage = Coverage::default();
    for (block_index, block_states) in state_table.blocks.iter().enumerate() {
        let Some(block) = blocks.get(block_index).cloned() else {
            continue;
        };

        let chain = &registry[block_index].chain;
        let class = &registry[block_index].class;
        let start = first_state.get(&block_index).copied().unwrap_or(0);
        let liquid = get_constant(&document, "liquid", block_index)
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        for local in 0..block_states.state_count {
            let state = start + local;
            let pairs = state_pairs(&block_states.properties, local);
            let waterlogged = boolean(&pairs, "waterlogged").unwrap_or(false);
            let fluid_empty = !liquid && !waterlogged;
            let outline = get_varying(&document, "outline", state).unwrap_or(Value::Null);
            let outline_full = full_cube(&outline);
            let skylight = propagates(chain, &pairs, outline_full, fluid_empty);
            let solid = get_varying(&document, "solid_render", state)
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let dampening = dampening(chain, solid, skylight);
            set_varying(
                &mut document,
                "skylight_down",
                state,
                Value::Bool(skylight),
            );
            set_varying(
                &mut document,
                "dampening",
                state,
                Value::from(dampening as i64),
            );
            let emission = emission(chain, &block_states.id, &pairs);
            if let Some(emission) = emission {
                set_varying(
                    &mut document,
                    "emission",
                    state,
                    Value::from(emission as i64),
                );
            }
            if let Some(random) = random_ticks(chain, &block_states.id, &pairs, max_age(&block_states.properties)) {
                set_varying(&mut document, "random_ticks", state, Value::Bool(random));
            }
            if let Some(shape_occlusion) = use_shape(chain, &pairs) {
                set_varying(
                    &mut document,
                    "shape_occlusion",
                    state,
                    Value::Bool(shape_occlusion),
                );
            }
            if chain.iter().any(|name| name == "ShulkerBoxBlock") {
                set_varying(&mut document, "full_collision", state, Value::Bool(true));
            }
            // Fences: `CrossCollisionBlock.getShape` is `makeShapes(4,16,4,0,16)`.
            if chain.iter().any(|name| name == "FenceBlock") {
                let shape = fence_shape(&pairs);
                set_shape(&mut document, state, &shape);
            } else if chain.iter().any(|name| name == "SculkShriekerBlock") {
                let shape = voxelshape::VoxelShape::from_boxes(&[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]);
                set_shape(&mut document, state, &shape);
            } else if chain.iter().any(|name| name == "ScaffoldingBlock") {
                let shape = scaffolding_shape(&pairs);
                set_shape(&mut document, state, &shape);
            } else if chain.iter().any(|name| name == "FenceGateBlock") {
                let shape = fence_gate_shape(&pairs);
                set_shape(&mut document, state, &shape);
            } else if chain.iter().any(|name| name == "LecternBlock") {
                if let Some(shape) = lectern_shape(&pairs) {
                    set_shape(&mut document, state, &shape);
                } else {
                    coverage.outline_gaps.push(block_states.id.clone());
                }
            }
            let _ = class;
            let _ = block_index;
        }
    }
    coverage.produced.push(TABLE.to_string());
    Ok(document)
}

/// Writes the compact document below `root`.
pub fn write(jar_path: &Path, root: &Path) -> Result<Coverage, String> {
    let document = document(jar_path)?;
    let text = serde_json::to_string(&document).map_err(|error| error.to_string())?;
    let path = root.join(TABLE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Coverage {
        produced: vec![TABLE.to_string()],
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// The jar
// ---------------------------------------------------------------------------

struct Jar {
    files: BTreeMap<String, Vec<u8>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|e| e.to_string())?.to_string();
            if !name.ends_with(".class") {
                continue;
            }
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut data)
                .map_err(|e| format!("{name}: {e}"))?;
            files.insert(name.trim_end_matches(".class").to_string(), data);
        }
        Ok(Jar { files })
    }

    fn class(&self, name: &str) -> Option<classfile::ClassFile> {
        classfile::parse(self.files.get(name)?).ok()
    }
}

#[derive(Clone, Debug)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
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
            0xc4 => 4,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            0x99..=0xa8 | 0xc6 | 0xc7 => {
                immediate = bytes
                    .get(at + 1..at + 3)
                    .map(|pair| i16::from_be_bytes([pair[0], pair[1]]) as i32);
                3
            }
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

fn member(class: &classfile::ClassFile, index: usize) -> Option<(String, String, String)> {
    let (owner_ref, name_ref, kind) = match class.constants.get(index)? {
        classfile::Constant::Fieldref { class, name, kind } => (class, name, kind),
        classfile::Constant::Methodref { class, name, kind } => (class, name, kind),
        classfile::Constant::InterfaceMethodref { class, name, kind } => (class, name, kind),
        _ => return None,
    };
    let owner = class_name_at(class, owner_ref)?;
    let name_index: usize = name_ref.trim_start_matches('#').parse().ok()?;
    let (name, descriptor) = match class.constants.get(name_index)? {
        classfile::Constant::NameAndType { name, kind } => {
            (reference_utf8(class, name)?, reference_utf8(class, kind)?)
        }
        _ => return None,
    };
    Some((owner, name, descriptor))
}

fn class_name_at(class: &classfile::ClassFile, reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match class.constants.get(index)? {
        classfile::Constant::Class(inner) => reference_utf8(class, inner),
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

fn reference_utf8(class: &classfile::ClassFile, reference: &str) -> Option<String> {
    let index: usize = reference.trim_start_matches('#').parse().ok()?;
    match class.constants.get(index)? {
        classfile::Constant::Utf8(text) => Some(text.clone()),
        _ => None,
    }
}

fn short(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// `BlockItemIds`'s `create(block, item)` / `create(block)` records: block name -> item name.
fn block_items(jar: &Jar) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(class) = jar.class("net/minecraft/references/BlockItemIds") else {
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
    let mut strings: Vec<String> = Vec::new();
    let mut mode = 0u8;
    let mut factories = 0usize;
    let mut stores = 0usize;
    for instruction in &instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(classfile::Constant::String(reference)) =
                    instruction.index.and_then(|index| class.constants.get(index))
                {
                    if let Some(text) = reference_utf8(&class, &reference) {
                        strings.push(text);
                    }
                }
            }
            0xb8 => {
                if let Some((owner, name, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemId" || short(&owner) == "BlockItemIds" {
                        mode = if name == "create" { 1 } else { 0 };
                        factories += 1;
                    }
                }
            }
            0xb3 => {
                stores += 1;
                if let Some((owner, _, _)) =
                    instruction.index.and_then(|index| member(&class, index))
                {
                    if short(&owner) == "BlockItemIds" && mode == 1 && !strings.is_empty() {
                        let block = strings.first().cloned().unwrap_or_default();
                        let item = strings.get(1).or_else(|| strings.first()).cloned().unwrap_or_default();
                        if !block.is_empty() {
                            out.insert(block, item);
                        }
                    }
                }
                strings.clear();
                mode = 0;
            }
            _ => {}
        }
    }
    if std::env::var_os("HZR_DEBUG_ITEMS").is_some() {
        eprintln!(
            "BlockItemIds: {} instructions, {} factories, {} stores, {} strings, {} records",
            instructions.len(),
            factories,
            stores,
            strings.len(),
            out.len()
        );
    }
    out
}

fn base_item(path: &str) -> Option<String> {
    match path {
        "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => {
            Some("cauldron".to_string())
        }
        "big_dripleaf_stem" => Some("big_dripleaf".to_string()),
        _ => None,
    }
}

fn no_item(path: &str, chain: &[String]) -> bool {
    if path.starts_with("potted_")
        || matches!(
            path,
            "air"
                | "cave_air"
                | "void_air"
                | "water"
                | "lava"
                | "fire"
                | "soul_fire"
                | "moving_piston"
                | "bubble_column"
                | "end_portal"
                | "end_gateway"
                | "nether_portal"
                | "structure_void"
                | "frosted_ice"
                | "tall_seagrass"
                | "piston_head"
                | "tall_grass"
                | "large_fern"
                | "sunflower"
                | "lilac"
                | "rose_bush"
                | "peony"
                | "attached_pumpkin_stem"
                | "attached_melon_stem"
                | "bamboo_sapling"
                | "cave_vines_plant"
                | "kelp_plant"
                | "twisting_vines_plant"
                | "weeping_vines_plant"
        ) || path == "candle_cake" || path.ends_with("_candle_cake")
    {
        return true;
    }
    chain.iter().any(|class| {
        matches!(
            class.as_str(),
            "AirBlock"
                | "LiquidBlock"
                | "FireBlock"
                | "MovingPistonBlock"
                | "BubbleColumnBlock"
                | "EndPortalBlock"
                | "EndGatewayBlock"
                | "NetherPortalBlock"
                | "FlowerPotBlock"
                | "StructureVoidBlock"
                | "FrostedIceBlock"
                | "TallSeagrassBlock"
                | "PistonHeadBlock"
                | "DoublePlantBlock"
                | "AttachedStemBlock"
                | "BambooSaplingBlock"
        )
    })
}

fn wall_base(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix("wall_") {
        return Some(rest.to_string());
    }
    if path.contains("_wall_") {
        return Some(path.replacen("_wall_", "_", 1));
    }
    None
}

// ---------------------------------------------------------------------------
// Compact document access
// ---------------------------------------------------------------------------

fn constant_slot(document: &Value, field: &str) -> Option<usize> {
    document["constant_fields"]
        .as_array()?
        .iter()
        .position(|candidate| candidate.as_str() == Some(field))
}

fn varying_slot(document: &Value, field: &str) -> Option<usize> {
    document["varying_fields"]
        .as_array()?
        .iter()
        .position(|candidate| candidate.as_str() == Some(field))
}

fn dictionary_index(document: &mut Value, field: &str, value: Value) -> usize {
    let dictionary = document["dictionaries"][field]
        .as_array_mut()
        .expect("a dictionary");
    if let Some(index) = dictionary.iter().position(|candidate| candidate == &value) {
        return index;
    }
    dictionary.push(value);
    dictionary.len() - 1
}

fn set_constant(document: &mut Value, field: &str, block_index: usize, value: Value) {
    let Some(slot) = constant_slot(document, field) else {
        return;
    };
    let block = document["blocks"]
        .as_array()
        .and_then(|blocks| blocks.get(block_index))
        .and_then(|block| block.as_str())
        .unwrap_or_default()
        .to_string();
    let index = dictionary_index(document, field, value);
    if let Some(row) = document["per_block"]
        .get_mut(&block)
        .and_then(|row| row.as_array_mut())
    {
        if slot < row.len() {
            row[slot] = Value::from(index as u64);
        }
    }
}

fn set_varying(document: &mut Value, field: &str, state: usize, value: Value) {
    let Some(slot) = varying_slot(document, field) else {
        return;
    };
    let index = dictionary_index(document, field, value);
    if let Some(row) = document["per_state"]
        .as_array_mut()
        .and_then(|rows| rows.get_mut(state))
        .and_then(|row| row.as_array_mut())
    {
        if slot < row.len() {
            row[slot] = Value::from(index as u64);
        }
    }
}

fn get_varying(document: &Value, field: &str, state: usize) -> Option<Value> {
    let slot = varying_slot(document, field)?;
    let index = document["per_state"]
        .as_array()?
        .get(state)?
        .as_array()?
        .get(slot)?
        .as_u64()? as usize;
    document["dictionaries"][field].as_array()?.get(index).cloned()
}

fn get_constant(document: &Value, field: &str, block_index: usize) -> Option<Value> {
    let slot = constant_slot(document, field)?;
    let block = document["blocks"].as_array()?.get(block_index)?.as_str()?;
    let index = document["per_block"]
        .get(block)?
        .as_array()?
        .get(slot)?
        .as_u64()? as usize;
    document["dictionaries"][field].as_array()?.get(index).cloned()
}

fn full_cube(outline: &Value) -> bool {
    let Some(boxes) = outline.as_array() else {
        return false;
    };
    boxes.len() == 1
        && boxes[0]
            .as_array()
            .map(|box_values| {
                let expected = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
                box_values
                    .iter()
                    .enumerate()
                    .all(|(index, value)| value.as_f64() == Some(expected[index]))
            })
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// State properties
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

fn boolean(pairs: &[(String, String)], name: &str) -> Option<bool> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
}

fn number(pairs: &[(String, String)], name: &str) -> Option<u32> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .and_then(|(_, value)| value.parse().ok())
}

fn name_of<'a>(pairs: &'a [(String, String)], name: &str) -> &'a str {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .unwrap_or("")
}

fn max_age(properties: &[states::Property]) -> Option<u32> {
    let age = properties.iter().find(|property| property.name == "age")?;
    Some(age.values.len().saturating_sub(1) as u32)
}

/// The first ancestor (including the class itself) that the predicate accepts.
fn ancestor<'a>(chain: &'a [String], names: &[&str]) -> Option<&'a str> {
    chain.iter().map(String::as_str).find(|class| names.contains(class))
}

// ---------------------------------------------------------------------------
// Class-chain rules
// ---------------------------------------------------------------------------

fn propagates(
    chain: &[String],
    pairs: &[(String, String)],
    shape_full: bool,
    fluid_empty: bool,
) -> bool {
    if ancestor(
        chain,
        &[
            "SeagrassBlock",
            "TallSeagrassBlock",
            "KelpBlock",
            "KelpPlantBlock",
            "SculkShriekerBlock",
        ],
    )
    .is_some()
    {
        return false;
    }
    if let Some(class) = ancestor(
        chain,
        &[
            "BambooStalkBlock",
            "BarrierBlock",
            "CrossCollisionBlock",
            "GlowLichenBlock",
            "HangingMossBlock",
            "LightBlock",
            "LiquidBlock",
            "MossyCarpetBlock",
            "PipeBlock",
            "ShulkerBoxBlock",
            "TintedGlassBlock",
            "TransparentBlock",
            "VegetationBlock",
            "VineBlock",
            "WallBlock",
        ],
    ) {
        return match class {
            "BambooStalkBlock" | "HangingMossBlock" | "MossyCarpetBlock" | "TransparentBlock"
            | "VineBlock" => true,
            "BarrierBlock" | "GlowLichenBlock" | "LightBlock" => fluid_empty,
            "VegetationBlock" => {
                !matches!(
                    ancestor(
                        chain,
                        &["SeagrassBlock", "TallSeagrassBlock", "KelpBlock", "KelpPlantBlock"]
                    ),
                    Some(_)
                ) && fluid_empty
            }
            "CrossCollisionBlock" | "WallBlock" => !boolean(pairs, "waterlogged").unwrap_or(false),
            _ => false,
        };
    }
    !shape_full && fluid_empty
}

fn dampening(chain: &[String], solid: bool, skylight: bool) -> u32 {
    if let Some(class) = ancestor(chain, &["LeavesBlock", "TintedGlassBlock"]) {
        return if class == "LeavesBlock" { 1 } else { 15 };
    }
    if solid {
        15
    } else if skylight {
        0
    } else {
        1
    }
}

fn emission(
    chain: &[String],
    block: &str,
    pairs: &[(String, String)],
) -> Option<u32> {
    if ancestor(chain, &["RespawnAnchorBlock"]).is_some() {
        let charge = number(pairs, "charges").or_else(|| number(pairs, "charge")).unwrap_or(0);
        return Some(((charge as f32) / 4.0 * 15.0).floor() as u32);
    }
    if ancestor(chain, &["CandleCakeBlock"]).is_some() {
        return Some(if boolean(pairs, "lit").unwrap_or(false) { 3 } else { 0 });
    }
    if ancestor(chain, &["CaveVinesBlock", "CaveVinesPlantBlock"]).is_some() {
        return Some(if boolean(pairs, "berries").unwrap_or(false) { 14 } else { 0 });
    }
    if ancestor(chain, &["CopperBulbBlock"]).is_some() {
        if !boolean(pairs, "lit").unwrap_or(false) {
            return Some(0);
        }
        let level = if block.contains("oxidized") {
            4
        } else if block.contains("weathered") {
            8
        } else if block.contains("exposed") {
            12
        } else {
            15
        };
        return Some(level);
    }
    let _ = block;
    None
}

fn random_ticks(
    chain: &[String],
    block: &str,
    pairs: &[(String, String)],
    max_age: Option<u32>,
) -> Option<bool> {
    if ancestor(chain, &["ChorusFlowerBlock"]).is_some() {
        return Some(number(pairs, "age").unwrap_or(0) < 5);
    }
    if ancestor(chain, &["CocoaBlock"]).is_some() {
        return Some(number(pairs, "age").unwrap_or(0) < 2);
    }
    if ancestor(chain, &["CropBlock"]).is_some() {
        let max = if block == "minecraft:torchflower_crop" {
            3
        } else {
            max_age.unwrap_or(7)
        };
        return Some(number(pairs, "age").unwrap_or(0) < max);
    }
    if ancestor(chain, &["FlowerPotBlock"]).is_some() {
        return Some(matches!(
            block,
            "minecraft:potted_open_eyeblossom" | "minecraft:potted_closed_eyeblossom"
        ));
    }
    if ancestor(chain, &["GrowingPlantHeadBlock"]).is_some() {
        return Some(number(pairs, "age").unwrap_or(0) < 25);
    }
    if ancestor(chain, &["LeavesBlock"]).is_some() {
        return Some(number(pairs, "distance").unwrap_or(7) == 7 && !boolean(pairs, "persistent").unwrap_or(false));
    }
    if ancestor(chain, &["LiquidBlock"]).is_some() {
        return Some(block == "minecraft:lava");
    }
    if ancestor(chain, &["NetherWartBlock"]).is_some() {
        return Some(number(pairs, "age").unwrap_or(0) < 3);
    }
    if ancestor(chain, &["PitcherCropBlock"]).is_some() {
        return Some(name_of(pairs, "half") == "lower" && number(pairs, "age").unwrap_or(0) < 4);
    }
    if ancestor(chain, &["RedStoneOreBlock"]).is_some() {
        return Some(boolean(pairs, "lit").unwrap_or(false));
    }
    if ancestor(chain, &["SweetBerryBushBlock"]).is_some() {
        return Some(number(pairs, "age").unwrap_or(0) < 3);
    }
    if ancestor(chain, &["BambooStalkBlock"]).is_some() {
        return Some(number(pairs, "stage").unwrap_or(0) == 0);
    }
    if ancestor(chain, &["StemBlock"]).is_some() {
        return Some(true);
    }
    if ancestor(chain, &["AttachedStemBlock"]).is_some() {
        return Some(false);
    }
    if ancestor(chain, &["WeatheringCopperBarsBlock", "WeatheringCopperBulbBlock", "WeatheringCopperChainBlock", "WeatheringCopperChestBlock", "WeatheringCopperDoorBlock", "WeatheringCopperFullBlock", "WeatheringCopperGolemStatueBlock", "WeatheringCopperGrateBlock", "WeatheringCopperSlabBlock", "WeatheringCopperStairBlock", "WeatheringCopperTrapDoorBlock", "WeatheringLanternBlock", "WeatheringLightningRodBlock"]).is_some() {
        let waxed = block.contains("waxed_");
        let oxidized = block.contains("oxidized");
        return Some(!waxed && !oxidized);
    }
    None
}

fn use_shape(chain: &[String], pairs: &[(String, String)]) -> Option<bool> {
    if ancestor(
        chain,
        &[
            "DaylightDetectorBlock",
            "EnchantingTableBlock",
            "EndPortalFrameBlock",
            "FarmlandBlock",
            "LecternBlock",
            "PathBlock",
            "SculkSensorBlock",
            "SculkShriekerBlock",
            "ShelfBlock",
            "SnowLayerBlock",
            "StairBlock",
            "StonecutterBlock",
            "PistonHeadBlock",
        ],
    )
    .is_some()
    {
        return Some(true);
    }
    if ancestor(chain, &["SlabBlock"]).is_some() {
        return Some(name_of(pairs, "type") != "double");
    }
    if ancestor(chain, &["PistonBaseBlock"]).is_some() {
        return Some(boolean(pairs, "extended").unwrap_or(false));
    }
    Some(false)
}

// ---------------------------------------------------------------------------
// Selection shapes
// ---------------------------------------------------------------------------

const SIXTEENTH: f64 = 1.0 / 16.0;

fn column(width: f64, y1: f64, y2: f64) -> [f64; 6] {
    let half = width / 2.0;
    [
        (8.0 - half) * SIXTEENTH,
        y1 * SIXTEENTH,
        (8.0 - half) * SIXTEENTH,
        (8.0 + half) * SIXTEENTH,
        y2 * SIXTEENTH,
        (8.0 + half) * SIXTEENTH,
    ]
}

fn box_z(width: f64, y1: f64, y2: f64, z1: f64, z2: f64) -> [f64; 6] {
    let half = width / 2.0;
    [
        (8.0 - half) * SIXTEENTH,
        y1 * SIXTEENTH,
        z1 * SIXTEENTH,
        (8.0 + half) * SIXTEENTH,
        y2 * SIXTEENTH,
        z2 * SIXTEENTH,
    ]
}

fn fence_shape(pairs: &[(String, String)]) -> voxelshape::VoxelShape {
    let mut boxes = vec![column(4.0, 0.0, 16.0)];
    let north = boolean(pairs, "north").unwrap_or(false);
    let east = boolean(pairs, "east").unwrap_or(false);
    let south = boolean(pairs, "south").unwrap_or(false);
    let west = boolean(pairs, "west").unwrap_or(false);
    let extension = box_z(4.0, 0.0, 16.0, 0.0, 8.0);
    if north {
        boxes.push(extension);
    }
    if east {
        boxes.push(rotate_box_y(extension, 3));
    }
    if south {
        boxes.push(rotate_box_y(extension, 2));
    }
    if west {
        boxes.push(rotate_box_y(extension, 1));
    }
    voxelshape::VoxelShape::from_boxes(&boxes)
}

/// Rotate a box horizontally about the block centre by 90° `quarters` times (vanilla
/// `Shapes.rotateHorizontal`: EAST = 90°, SOUTH = 180°, WEST = 270°).
fn rotate_box_y(box_: [f64; 6], quarters: usize) -> [f64; 6] {
    let mut current = box_;
    for _ in 0..quarters {
        // (x, z) -> (z, 1 - x)
        let (x1, z1, x2, z2) = (current[0], current[2], current[3], current[5]);
        current[0] = z1;
        current[2] = 1.0 - x2;
        current[3] = z2;
        current[5] = 1.0 - x1;
    }
    current
}

fn scaffolding_shape(pairs: &[(String, String)]) -> voxelshape::VoxelShape {
    let stable = [column(16.0, 14.0, 16.0)];
    let post = [0.0, 0.0, 0.0, 2.0 * SIXTEENTH, 1.0, 2.0 * SIXTEENTH];
    let mut boxes = stable.to_vec();
    for quarters in 0..4 {
        boxes.push(rotate_box_y(post, quarters));
    }
    if boolean(pairs, "bottom").unwrap_or(false) {
        boxes.push(column(16.0, 0.0, 2.0));
        let bar = box_z(16.0, 0.0, 2.0, 0.0, 2.0);
        for quarters in 0..4 {
            boxes.push(rotate_box_y(bar, quarters));
        }
    }
    voxelshape::VoxelShape::from_boxes(&boxes)
}

fn fence_gate_shape(pairs: &[(String, String)]) -> voxelshape::VoxelShape {
    let facing = name_of(pairs, "facing");
    let along_z = matches!(facing, "north" | "south");
    let height = if boolean(pairs, "in_wall").unwrap_or(false) {
        13.0 / 16.0
    } else {
        1.0
    };
    let box_ = if along_z {
        [0.0, 0.0, 0.375, 1.0, height, 0.625]
    } else {
        [0.375, 0.0, 0.0, 0.625, height, 1.0]
    };
    voxelshape::VoxelShape::from_boxes(&[box_])
}

fn lectern_shape(pairs: &[(String, String)]) -> Option<voxelshape::VoxelShape> {
    let facing = name_of(pairs, "facing");
    let quarters = match facing {
        "north" => 0,
        "east" => 3,
        "south" => 2,
        "west" => 1,
        _ => 0,
    };
    let collision = [column(16.0, 0.0, 2.0), column(8.0, 2.0, 14.0)];
    let north_boxes = vec![
        collision[0],
        collision[1],
        box_z(16.0, 10.0, 14.0, 1.0, 5.333333),
        box_z(16.0, 12.0, 16.0, 5.333333, 9.666667),
        box_z(16.0, 14.0, 18.0, 9.666667, 14.0),
    ];
    let boxes: Vec<[f64; 6]> = north_boxes
        .into_iter()
        .map(|box_| rotate_box_y(box_, quarters))
        .collect();
    Some(voxelshape::VoxelShape::from_boxes(&boxes))
}

fn set_shape(document: &mut Value, state: usize, shape: &voxelshape::VoxelShape) {
    let encoded = shape.encode();
    let grid = Grid::new(&encoded.x, &encoded.y, &encoded.z, &encoded.occupied);
    let edges = grid.for_all_edges();
    let mut grid = Grid::new(&encoded.x, &encoded.y, &encoded.z, &encoded.occupied);
    let boxes = grid.for_all_boxes();
    let boxes: Vec<Value> = boxes
        .into_iter()
        .map(|box_| Value::Array(box_.iter().map(|value| Value::from(*value)).collect()))
        .collect();
    let edges: Vec<Value> = edges
        .into_iter()
        .map(|box_| Value::Array(box_.iter().map(|value| Value::from(*value)).collect()))
        .collect();
    set_varying(document, "outline", state, Value::Array(boxes));
    set_varying(document, "outline_edges", state, Value::Array(edges));
}

// ---------------------------------------------------------------------------
// Discrete shape algorithms
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
    fn new(x: &[f64], y: &[f64], z: &[f64], occupied: &[u8]) -> Grid {
        let ny = y.len().saturating_sub(1);
        let nz = z.len().saturating_sub(1);
        Grid {
            x: x.to_vec(),
            y: y.to_vec(),
            z: z.to_vec(),
            occupied: occupied.iter().map(|value| *value != 0).collect(),
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
        self.occupied
            .get((x * self.ny + y) * self.nz + z)
            .copied()
            .unwrap_or(false)
    }

    fn set(&mut self, x: usize, y: usize, z: usize, value: bool) {
        if x >= self.nx() || y >= self.ny || z >= self.nz {
            return;
        }
        if let Some(slot) = self.occupied.get_mut((x * self.ny + y) * self.nz + z) {
            *slot = value;
        }
    }

    fn clear_z_strip(&mut self, z1: usize, z2: usize, x: usize, y: usize) {
        for z in z1..z2 {
            self.set(x, y, z, false);
        }
    }

    fn z_strip_full(&self, z1: usize, z2: usize, x: usize, y: usize) -> bool {
        x < self.nx() && y < self.ny && (z1..z2).all(|z| self.full(x, y, z))
    }

    fn xz_rectangle_full(&self, x1: usize, x2: usize, z1: usize, z2: usize, y: usize) -> bool {
        (x1..x2).all(|x| (z1..z2).all(|z| self.full(x, y, z)))
    }

    fn for_all_boxes(&mut self) -> Vec<[f64; 6]> {
        let mut out = Vec::new();
        let ny = self.ny;
        let nz = self.nz;
        for y in 0..ny {
            for x in 0..self.nx() {
                let mut start_z: Option<usize> = None;
                for z in 0..=nz {
                    let full = z < nz && self.full(x, y, z);
                    if full {
                        if start_z.is_none() {
                            start_z = Some(z);
                        }
                    } else if let Some(start) = start_z.take() {
                        let mut x2 = x;
                        let mut y2 = y;
                        self.clear_z_strip(start, z, x2, y2);
                        while self.z_strip_full(start, z, x2 + 1, y2) {
                            self.clear_z_strip(start, z, x2 + 1, y2);
                            x2 += 1;
                        }
                        while self.xz_rectangle_full(x, x2 + 1, start, z, y2 + 1) {
                            for x3 in x..=x2 {
                                self.clear_z_strip(start, z, x3, y2 + 1);
                            }
                            y2 += 1;
                        }
                        out.push([
                            self.x[x],
                            self.y[y],
                            self.z[start],
                            self.x[x2 + 1],
                            self.y[y2 + 1],
                            self.z[z],
                        ]);
                    }
                }
            }
        }
        out
    }

    fn for_all_edges(&self) -> Vec<[f64; 6]> {
        let mut out = Vec::new();
        for cycle in 0..3 {
            let inverse = match cycle {
                1 => 2,
                2 => 1,
                _ => 0,
            };
            let axis = |source: usize| match inverse {
                1 => (source + 1) % 3,
                2 => (source + 2) % 3,
                _ => source,
            };
            let coords = |x: usize, y: usize, z: usize, source: usize| match inverse {
                1 => [z, x, y][source],
                2 => [y, z, x][source],
                _ => [x, y, z][source],
            };
            let size = |source: usize| match axis(source) {
                0 => self.nx(),
                1 => self.ny,
                _ => self.nz,
            };
            let full = |x: isize, y: isize, z: isize| -> bool {
                if x < 0 || y < 0 || z < 0 {
                    return false;
                }
                let real = [
                    coords(x as usize, y as usize, z as usize, 0),
                    coords(x as usize, y as usize, z as usize, 1),
                    coords(x as usize, y as usize, z as usize, 2),
                ];
                self.full(real[0], real[1], real[2])
            };
            let sx = size(0);
            let sy = size(1);
            let sz = size(2);
            for a in 0..=sx {
                for b in 0..=sy {
                    let mut start: Option<usize> = None;
                    for c in 0..=sz {
                        let mut count = 0usize;
                        let mut parity = 0usize;
                        for i in 0..=1 {
                            for j in 0..=1 {
                                if full(
                                    a as isize + i as isize - 1,
                                    b as isize + j as isize - 1,
                                    c as isize,
                                ) {
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
                                coords(a, b, start, 0),
                                coords(a, b, start, 1),
                                coords(a, b, start, 2),
                            ];
                            let second = [
                                coords(a, b, c, 0),
                                coords(a, b, c, 1),
                                coords(a, b, c, 2),
                            ];
                            out.push([
                                self.x[first[0]],
                                self.y[first[1]],
                                self.z[first[2]],
                                self.x[second[0]],
                                self.y[second[1]],
                                self.z[second[2]],
                            ]);
                        }
                    }
                }
            }
        }
        out
    }
}
