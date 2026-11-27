//! Rebuild the registry-shaped tables Horizon derives from vanilla's code, from the
//! player's own `client.jar` alone.
//!
//! `horizon-prepare` gets these tables from two different places, and the difference
//! decides what this module can reproduce without running Minecraft:
//!
//! * `command-registries.json` is `VanillaCommandParseOracle` (`registries` mode). Most
//!   of it is read from the jar's data pack (`data/minecraft/**`), but three arrays and
//!   the code-registered half of `by_registry` are the in-memory registries, i.e. the
//!   order vanilla *registers* entries in its static initialisers. This module recovers
//!   that order from the initialisers' bytecode:
//!   - `items` — `Items.<clinit>` calls `registerBlock`/`registerItem`/`registerSpawnEgg`
//!     in registration order. The ids are `BlockItemIds`/`ItemIds` reference fields; the
//!     colour helpers (`createSimpleColored`, `createSimpleCopper`) expand to the sixteen
//!     dye colours / eight weathering states in `ColorCollection`/`WeatheringCopperCollection`
//!     iteration order, and the helpers `ColorCollection.registerBlockItems` /
//!     `registerItems`/`registerBlocks`/`zipMap` expand the same way.
//!   - `block_entities` — `Blocks.<clinit>` in registration order, filtered to the blocks
//!     whose class implements `net/minecraft/world/level/block/EntityBlock`
//!     (`BlockStateBase.hasBlockEntity` is exactly `getBlock() instanceof EntityBlock`).
//!   - `by_registry` — the same id lists, sorted as the oracle sorts them.
//! * `registries.json` is vanilla's **data generator** report `reports/registries.json`
//!   (`RegistryDumpReport`). Its protocol ids are the entries' registration order, so the
//!   same bytecode reading recovers the registries whose registration is a plain sequence
//!   of literal registrations. This module reproduces the 66 registries whose extraction it
//!   verifies exactly against a Java-produced table, plus `minecraft:item` and
//!   `minecraft:block` from the dedicated registration traces: 68 of 95. The other 27
//!   register through loops or through classes that need values this reader cannot
//!   evaluate (see [`Coverage::missing`]).
//! * `items.json` (the data generator's per-item component report), `commands.json`
//!   (Brigadier's tree, `Commands.<init>`) and `creative-inventory.json`
//!   (`CreativeModeTabs.tryRebuildTabContents`) are produced by running vanilla's code,
//!   not by reading a data pack: item components come from each item constructor,
//!   creative tabs are built from those stacks. They cannot be read out of class files
//!   without executing vanilla, so they stay listed in
//!   [`BLOCKED_TABLES`] rather than being guessed.
//! * `command-registries.json`'s `blocks` (every block's state properties) is the one
//!   remaining fragment: properties are declared in each block class'
//!   `createBlockStateDefinition`, and the blockstate assets only name the properties a
//!   model happens to use (measured: 629 of 1286 blocks' property *names* are complete
//!   from the assets), so a partial table would be wrong for the rest.
//!
//! Every value this module writes comes from the jar the player already has; nothing is
//! copied from `ship.hzr3` and nothing is taken from the reference tables this crate's
//! tests compare against.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;
use classfile::{field_ref, method_ref, string_constant, ClassFile, Constant};

/// Which table this module writes.
pub const TABLE: &str = "registries.json";

/// The tables of this slice, and which of them this module fills.
pub const TABLES: [&str; 5] = [
    "registries.json",
    "items.json",
    "commands.json",
    "creative-inventory.json",
    "command-registries.json",
];

/// The tables that need vanilla's own code to run, with the reason.
pub const BLOCKED_TABLES: [(&str, &str); 3] = [
    (
        "items.json",
        "the data generator's per-item component report (`reports/minecraft/components/item`): \
         every item's components are built by its constructor, so reproducing it means \
         executing the item classes",
    ),
    (
        "commands.json",
        "the data generator's command-syntax report: Brigadier's tree is built by \
         `Commands.<init>` and the command classes at runtime",
    ),
    (
        "creative-inventory.json",
        "`CreativeModeTabs.tryRebuildTabContents` builds each tab from item stacks (item \
         constructors plus creative-tab code), not from a data pack",
    ),
];

/// The 66 registries whose entry order this module recovers exactly, verified against a
/// Java-produced `reports/registries.json` (26.3). `minecraft:item` and `minecraft:block`
/// are recovered by the dedicated [`item_ids`]/[`block_ids`] traces and are emitted too.
///
/// Registries outside this list are left out rather than guessed: a wrong protocol id is
/// worse than a missing registry.
pub const VERIFIED_REGISTRIES: [&str; 66] = [
    "minecraft:activity",
    "minecraft:attribute",
    "minecraft:attribute_type",
    "minecraft:block_predicate_type",
    "minecraft:chunk_status",
    "minecraft:consume_effect_type",
    "minecraft:data_component_predicate_type",
    "minecraft:data_component_type",
    "minecraft:debug_subscription",
    "minecraft:dialog_body_type",
    "minecraft:dialog_type",
    "minecraft:enchantment_effect_component_type",
    "minecraft:enchantment_entity_effect_type",
    "minecraft:enchantment_level_based_value_type",
    "minecraft:enchantment_location_based_effect_type",
    "minecraft:enchantment_provider_type",
    "minecraft:enchantment_value_effect_type",
    "minecraft:entity_sub_predicate_type",
    "minecraft:environment_attribute",
    "minecraft:float_provider_type",
    "minecraft:height_provider_type",
    "minecraft:input_control_type",
    "minecraft:int_provider_type",
    "minecraft:loot_condition_type",
    "minecraft:loot_function_type",
    "minecraft:loot_nbt_provider_type",
    "minecraft:loot_pool_entry_type",
    "minecraft:loot_score_provider_type",
    "minecraft:memory_module_type",
    "minecraft:menu",
    "minecraft:number_format_type",
    "minecraft:particle_type",
    "minecraft:permission_check_type",
    "minecraft:permission_type",
    "minecraft:pos_rule_test",
    "minecraft:position_source_type",
    "minecraft:recipe_book_category",
    "minecraft:recipe_display",
    "minecraft:recipe_serializer",
    "minecraft:recipe_type",
    "minecraft:rule_block_entity_modifier",
    "minecraft:rule_test",
    "minecraft:sensor_type",
    "minecraft:slot_display",
    "minecraft:slot_source_type",
    "minecraft:spawn_condition_type",
    "minecraft:test_environment_definition_type",
    "minecraft:ticket_type",
    "minecraft:worldgen/biome_source",
    "minecraft:worldgen/block_state_provider_type",
    "minecraft:worldgen/carver_type",
    "minecraft:worldgen/chunk_generator",
    "minecraft:worldgen/feature_size_type",
    "minecraft:worldgen/feature_type",
    "minecraft:worldgen/foliage_placer_type",
    "minecraft:worldgen/placement_modifier_type",
    "minecraft:worldgen/pool_alias_binding",
    "minecraft:worldgen/root_placer_type",
    "minecraft:worldgen/structure_placement",
    "minecraft:worldgen/structure_pool_element",
    "minecraft:worldgen/structure_processor",
    "minecraft:worldgen/structure_type",
    "minecraft:worldgen/tree_decorator_type",
    "minecraft:worldgen/trunk_placer_type",
    "minecraft:context_float_provider_type",
    "minecraft:context_int_provider_type",
];

/// What this module could and could not derive from one jar.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub produced: Vec<String>,
    pub missing: Vec<String>,
    pub notes: Vec<String>,
    /// Every `registries.json` registry emitted with exact protocol ids.
    pub registries: Vec<String>,
    /// The registries of `registries.json` left out, with the reason.
    pub absent_registries: Vec<String>,
}

impl Coverage {
    fn new() -> Coverage {
        Coverage::default()
    }

    fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }
}

/// The player's `client.jar`, held in memory.
pub struct Jar {
    files: BTreeMap<String, Vec<u8>>,
    classes: RefCell<BTreeMap<String, Rc<ClassFile>>>,
}

impl Jar {
    pub fn open(path: &Path) -> Result<Jar, String> {
        let file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut zip =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().map_err(|error| error.to_string())?.to_string();
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| format!("{name}: {error}"))?;
            files.insert(name, data);
        }
        Ok(Jar {
            files,
            classes: RefCell::new(BTreeMap::new()),
        })
    }

    fn bytes(&self, name: &str) -> Result<&[u8], String> {
        self.files
            .get(name)
            .map(|bytes| bytes.as_slice())
            .ok_or_else(|| format!("{name}: not in the jar"))
    }

    /// Parse (and cache) one class, addressed by its internal name.
    fn class(&self, internal: &str) -> Result<Rc<ClassFile>, String> {
        if let Some(class) = self.classes.borrow().get(internal) {
            return Ok(class.clone());
        }
        let name = format!("{internal}.class");
        let class = std::rc::Rc::new(
            classfile::parse(self.bytes(&name)?).map_err(|error| format!("{name}: {error}"))?,
        );
        self.classes
            .borrow_mut()
            .insert(internal.to_string(), class.clone());
        Ok(class)
    }

    /// Every class file under `net/minecraft/`, as internal names.
    fn class_names(&self) -> Vec<String> {
        self.files
            .keys()
            .filter_map(|name| {
                let internal = name.strip_prefix("net/minecraft/")?;
                let internal = internal.strip_suffix(".class")?;
                Some(format!("net/minecraft/{internal}"))
            })
            .collect()
    }

    /// The ids of the data-pack tags of one registry directory, sorted.
    fn tag_ids(&self, directory: &str) -> Vec<String> {
        let prefix = format!("data/minecraft/tags/{directory}/");
        let mut out: Vec<String> = self
            .files
            .keys()
            .filter_map(|name| {
                let rest = name.strip_prefix(&prefix)?;
                let id = rest.strip_suffix(".json")?;
                Some(format!("minecraft:{id}"))
            })
            .collect();
        out.sort();
        out
    }
}

// ---------------------------------------------------------------------------
// Bytecode reading
// ---------------------------------------------------------------------------

/// One decoded instruction. Only the operand this reader needs is kept.
#[derive(Debug, Clone)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
    immediate: Option<i32>,
}

fn u16_at(code: &[u8], at: usize) -> Result<usize, String> {
    if at + 2 > code.len() {
        return Err(format!("bytecode ends inside an operand at {at}"));
    }
    Ok(((code[at] as usize) << 8) | code[at + 1] as usize)
}

/// Decode a method body in order. Switch instructions are skipped by their own length
/// arithmetic (their jump targets are never followed here).
fn decode(code: &[u8]) -> Result<Vec<Instruction>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < code.len() {
        let opcode = code[at];
        let mut index = None;
        let mut immediate = None;
        let length = match opcode {
            0x12 => {
                index = Some(*code.get(at + 1).ok_or("ldc ends the code")? as usize);
                2
            }
            0x13 | 0x14 => {
                index = Some(u16_at(code, at + 1)?);
                3
            }
            0x10 => {
                immediate = Some(*code.get(at + 1).ok_or("bipush ends the code")? as i8 as i32);
                2
            }
            0x11 => {
                immediate = Some(i16::from_be_bytes(
                    code.get(at + 1..at + 3)
                        .ok_or("sipush ends the code")?
                        .try_into()
                        .map_err(|_| "sipush")?,
                ) as i32);
                3
            }
            0x15..=0x19 | 0x36..=0x3a => {
                index = Some(*code.get(at + 1).ok_or("local ends the code")? as usize);
                2
            }
            0x84 => 3,
            0xa9 => 2,
            0x99..=0xa8 | 0xc6 | 0xc7 => 3,
            0xaa => {
                let pad = (4 - ((at + 1) % 4)) % 4;
                let body = at + 1 + pad;
                let low = i32::from_be_bytes(
                    code.get(body + 4..body + 8)
                        .ok_or("tableswitch out of range")?
                        .try_into()
                        .map_err(|_| "tableswitch")?,
                );
                let high = i32::from_be_bytes(
                    code.get(body + 8..body + 12)
                        .ok_or("tableswitch out of range")?
                        .try_into()
                        .map_err(|_| "tableswitch")?,
                );
                1 + pad + 12 + ((high - low + 1).max(0) as usize) * 4
            }
            0xab => {
                let pad = (4 - ((at + 1) % 4)) % 4;
                let body = at + 1 + pad;
                let pairs = i32::from_be_bytes(
                    code.get(body + 4..body + 8)
                        .ok_or("lookupswitch out of range")?
                        .try_into()
                        .map_err(|_| "lookupswitch")?,
                );
                1 + pad + 8 + (pairs.max(0) as usize) * 8
            }
            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = Some(u16_at(code, at + 1)?);
                3
            }
            0xb9 | 0xba => {
                index = Some(u16_at(code, at + 1)?);
                5
            }
            0xbc => 2,
            0xc5 => 4,
            0xc8 | 0xc9 => 5,
            0xc4 => {
                let next = *code.get(at + 1).ok_or("wide out of range")?;
                if next == 0x84 {
                    6
                } else {
                    4
                }
            }
            0x00..=0x0f
            | 0x1a..=0x35
            | 0x3b..=0x83
            | 0x85..=0x98
            | 0xac..=0xb1
            | 0xbe
            | 0xbf
            | 0xc2
            | 0xc3 => 1,
            other => return Err(format!("unmodelled opcode {other:#04x} at {at}")),
        };
        if at + length > code.len() {
            return Err(format!("instruction at {at} runs past the code"));
        }
        out.push(Instruction {
            opcode,
            index,
            immediate,
        });
        at += length;
    }
    Ok(out)
}

fn class_name_at(constants: &[Constant], index: usize) -> String {
    match constants.get(index) {
        Some(Constant::Class(reference)) => {
            let inner: usize = reference.trim_start_matches('#').parse().unwrap_or(0);
            match constants.get(inner) {
                Some(Constant::Utf8(text)) => text.clone(),
                _ => String::new(),
            }
        }
        _ => String::new(),
    }
}

fn simple(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn clinit(class: &ClassFile) -> Result<&[u8], String> {
    class
        .method("<clinit>")
        .and_then(|member| member.code.as_ref())
        .map(|code| code.bytes.as_slice())
        .ok_or_else(|| format!("{}.<clinit>: no code", class.name))
}

/// The first string constant of every `putstatic` segment of a `<clinit>`: the shape
/// vanilla's type-reference classes use (`BlockItemIds`, `EntityTypeIds`, ...).
fn field_strings(jar: &Jar, internal: &str) -> Result<BTreeMap<String, String>, String> {
    let class = jar.class(internal)?;
    let instructions = decode(clinit(&class)?)?;
    let mut out = BTreeMap::new();
    let mut pending: Option<String> = None;
    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&class.constants, index))
                {
                    pending = Some(text);
                }
            }
            0xb3 => {
                if let Some((_, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if let Some(text) = pending.take() {
                        out.insert(field, text);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Reference classes: BlockItemIds / BlockIds / ItemIds
// ---------------------------------------------------------------------------

/// One reference field's expansion: a single registry id or a colour/weathering family.
#[derive(Clone, Debug)]
enum IdSet {
    Single { block: String, item: String },
    Many(Vec<(String, String)>),
}

impl IdSet {
    fn blocks(&self) -> Vec<String> {
        match self {
            IdSet::Single { block, .. } => vec![block.clone()],
            IdSet::Many(pairs) => pairs.iter().map(|(block, _)| block.clone()).collect(),
        }
    }

    fn items(&self) -> Vec<String> {
        match self {
            IdSet::Single { item, .. } => vec![item.clone()],
            IdSet::Many(pairs) => pairs.iter().map(|(_, item)| item.clone()).collect(),
        }
    }
}

/// `ColorCollection`'s sixteen values, in its own iteration order.
const COLORS: [&str; 16] = [
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

/// `WeatheringCopperCollection.PREFIXES`, in its own iteration order.
const COPPER: [&str; 8] = [
    "",
    "exposed_",
    "weathered_",
    "oxidized_",
    "waxed_",
    "waxed_exposed_",
    "waxed_weathered_",
    "waxed_oxidized_",
];

/// The id a `*Ids` reference field names, after the helper that built it.
///
/// `internal` is `BlockItemIds`, `BlockIds` or `ItemIds`. The three classes share the
/// helper names; only the helpers each class actually calls are used.
fn reference_ids(jar: &Jar, internal: &str) -> Result<BTreeMap<String, IdSet>, String> {
    let class = jar.class(internal)?;
    let instructions = decode(clinit(&class)?)?;
    let mut out = BTreeMap::new();
    let mut strings: Vec<String> = Vec::new();
    let mut calls: Vec<(String, String)> = Vec::new();
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut special: Option<Vec<String>> = None;

    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&class.constants, index))
                {
                    strings.push(text);
                }
            }
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    fields.push((owner, name));
                }
            }
            0xb7 | 0xb8 | 0xb6 => {
                if let Some(call) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    calls.push((call.0, call.1));
                }
            }
            0xb3 => {
                let Some((_, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                else {
                    continue;
                };
                let has = |name: &str| calls.iter().any(|(_, call)| call == name);
                let source = fields.iter().find(|(owner, _)| {
                    let owner = simple(owner);
                    owner == "EntityTypeIds"
                        || owner == "JukeboxSongs"
                        || owner == "TrimPatterns"
                        || owner == "DecoratedPotPatterns"
                });
                let mut ids: Option<IdSet> = None;
                if has("create") {
                    let block = strings
                        .first()
                        .cloned()
                        .ok_or_else(|| format!("{internal}.{field}: create without a string"))?;
                    let item = strings.get(1).cloned().unwrap_or_else(|| block.clone());
                    ids = Some(IdSet::Single { block, item });
                } else if has("createSimpleColored") {
                    let base = strings.first().cloned().unwrap_or_default();
                    ids = Some(IdSet::Many(
                        COLORS
                            .iter()
                            .map(|color| {
                                let id = format!("{color}_{base}");
                                (id.clone(), id)
                            })
                            .collect(),
                    ));
                } else if has("createSimpleCopper") {
                    let base = strings.first().cloned().unwrap_or_default();
                    ids = Some(IdSet::Many(
                        COPPER
                            .iter()
                            .map(|prefix| {
                                let id = format!("{prefix}{base}");
                                (id.clone(), id)
                            })
                            .collect(),
                    ));
                } else if has("createSpawnEgg")
                    || has("createArmorTrimSmithingTemplate")
                    || has("createPotterySherd")
                    || has("createMusicDisc")
                {
                    let (owner, name) = source
                        .cloned()
                        .ok_or_else(|| format!("{internal}.{field}: helper without a source"))?;
                    let owner = simple(&owner);
                    if has("createSpawnEgg") {
                        let base = if owner == "EntityTypeIds" {
                            field_strings(jar, "net/minecraft/world/entity/EntityTypeIds")?
                                .get(&name)
                                .cloned()
                        } else {
                            None
                        };
                        if let Some(base) = base {
                            let id = format!("{base}_spawn_egg");
                            ids = Some(IdSet::Single {
                                block: id.clone(),
                                item: id,
                            });
                        }
                    } else if has("createArmorTrimSmithingTemplate") {
                        let base = if owner == "TrimPatterns" {
                            field_strings(
                                jar,
                                "net/minecraft/world/item/equipment/trim/TrimPatterns",
                            )?
                            .get(&name)
                            .cloned()
                        } else {
                            None
                        };
                        if let Some(base) = base {
                            let id = format!("{base}_armor_trim_smithing_template");
                            ids = Some(IdSet::Single {
                                block: id.clone(),
                                item: id,
                            });
                        }
                    } else if has("createPotterySherd") {
                        let base = if owner == "DecoratedPotPatterns" {
                            field_strings(
                                jar,
                                "net/minecraft/world/level/block/entity/DecoratedPotPatterns",
                            )?
                            .get(&name)
                            .cloned()
                        } else {
                            None
                        };
                        if let Some(base) = base {
                            let id = format!("{base}_pottery_sherd");
                            ids = Some(IdSet::Single {
                                block: id.clone(),
                                item: id,
                            });
                        }
                    } else {
                        let base = if owner == "JukeboxSongs" {
                            field_strings(jar, "net/minecraft/world/item/JukeboxSongs")?
                                .get(&name)
                                .cloned()
                        } else {
                            None
                        };
                        if let Some(base) = base {
                            let id = format!("music_disc_{base}");
                            ids = Some(IdSet::Single {
                                block: id.clone(),
                                item: id,
                            });
                        }
                    }
                } else if strings.len() == 4
                    && !calls
                        .iter()
                        .any(|(_, name)| name.starts_with("create") || name.starts_with("register"))
                {
                    // `BlockItemIds.COPPER_BLOCK_SPECIAL_NAMES`: the four names of the
                    // copper-block ByState, before `same`/`prefixWithState` zip the
                    // weathering and waxed prefixes onto them.
                    special = Some(strings.clone());
                } else if has("same") && special.is_some() && strings.is_empty() {
                    let names = special.clone().unwrap_or_default();
                    let mut pairs = Vec::new();
                    for group in [&COPPER[..4], &COPPER[4..]] {
                        for (prefix, value) in group.iter().zip(names.iter()) {
                            let id = format!("{prefix}{value}");
                            pairs.push((id.clone(), id));
                        }
                    }
                    ids = Some(IdSet::Many(pairs));
                }
                if let Some(ids) = ids {
                    out.insert(field, ids);
                }
                strings.clear();
                calls.clear();
                fields.clear();
            }
            _ => {}
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Item registry order
// ---------------------------------------------------------------------------

/// Every item registry id (`minecraft:`-prefixed), in `Items.<clinit>` registration
/// order — the order `BuiltInRegistries.ITEM` iterates.
pub fn item_ids(jar_path: &Path) -> Result<Vec<String>, String> {
    let jar = Jar::open(jar_path)?;
    item_ids_of(&jar)
}

fn item_ids_of(jar: &Jar) -> Result<Vec<String>, String> {
    let block_ids = reference_ids(jar, "net/minecraft/references/BlockItemIds")?;
    let item_ids = reference_ids(jar, "net/minecraft/references/ItemIds")?;
    let class = jar.class("net/minecraft/world/item/Items")?;
    let instructions = decode(clinit(&class)?)?;
    let mut out = Vec::new();
    let mut last: Option<(bool, String)> = None;
    for instruction in instructions {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    let owner = simple(&owner);
                    if owner == "BlockItemIds" {
                        last = Some((true, name));
                    } else if owner == "ItemIds" {
                        last = Some((false, name));
                    }
                }
            }
            0xb8 => {
                let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                if !matches!(
                    name.as_str(),
                    "registerBlock"
                        | "registerItem"
                        | "registerSpawnEgg"
                        | "registerBlockItems"
                        | "registerItems"
                ) {
                    continue;
                }
                let (block, field) = last
                    .take()
                    .ok_or_else(|| format!("Items.<clinit>: {name} without an id"))?;
                let table = if block { &block_ids } else { &item_ids };
                let set = table
                    .get(&field)
                    .ok_or_else(|| format!("Items.<clinit>: {field} has no ids"))?;
                out.extend(set.items().into_iter().map(|id| format!("minecraft:{id}")));
            }
            _ => {}
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Block registry order and block entities
// ---------------------------------------------------------------------------

struct BlockEvent {
    ids: Vec<(String, String)>,
    classes: BTreeSet<String>,
}

fn method_code<'a>(class: &'a ClassFile, name: &str) -> Result<&'a [u8], String> {
    class
        .methods
        .iter()
        .find(|member| member.name == name)
        .and_then(|member| member.code.as_ref())
        .map(|code| code.bytes.as_slice())
        .ok_or_else(|| format!("{}.{name}: no code", class.name))
}

/// The implementation method an `invokedynamic` names (the lambda's body).
fn bootstrap_impl(class: &ClassFile, index: usize) -> Option<(String, String, String)> {
    let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
        return None;
    };
    let row = class.bootstraps.get(*bootstrap)?;
    for argument in &row.arguments {
        if let Some(Constant::MethodHandle { reference }) = class.constants.get(*argument) {
            if let Some(target) = method_ref(&class.constants, *reference) {
                return Some(target);
            }
        }
    }
    None
}

/// The block classes the lambdas in one registration's argument window construct.
fn lambda_classes(
    jar: &Jar,
    class: &Rc<ClassFile>,
    index: usize,
    depth: usize,
    out: &mut BTreeSet<String>,
) {
    if depth > 2 {
        return;
    }
    let Some((owner, name, _)) = bootstrap_impl(class, index) else {
        return;
    };
    if name == "<init>" {
        out.insert(owner);
        return;
    }
    let Ok(target) = jar.class(&owner) else {
        return;
    };
    let Some(member) = target.methods.iter().find(|member| member.name == name) else {
        return;
    };
    let Some(code) = member.code.as_ref() else {
        return;
    };
    let Ok(instructions) = decode(&code.bytes) else {
        return;
    };
    for instruction in instructions {
        match instruction.opcode {
            0xbb => {
                if let Some(index) = instruction.index {
                    let built = class_name_at(&target.constants, index);
                    if !built.is_empty() {
                        out.insert(built);
                    }
                }
            }
            0xba => {
                if let Some(index) = instruction.index {
                    lambda_classes(jar, &target, index, depth + 1, out);
                }
            }
            _ => {}
        }
    }
}

fn block_events(jar: &Jar) -> Result<Vec<BlockEvent>, String> {
    let block_ids = reference_ids(jar, "net/minecraft/references/BlockItemIds")?;
    let plain_ids = reference_ids(jar, "net/minecraft/references/BlockIds")?;
    let class = jar.class("net/minecraft/world/level/block/Blocks")?;
    let instructions = decode(clinit(&class)?)?;
    let mut events = Vec::new();
    let mut pending: Option<Vec<(String, String)>> = None;
    let mut pending_at = 0usize;
    for i in 0..instructions.len() {
        let instruction = &instructions[i];
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    let owner = simple(&owner);
                    if owner == "BlockItemIds" {
                        if let Some(set) = block_ids.get(&name) {
                            pending = Some(
                                set.blocks()
                                    .into_iter()
                                    .map(|block| (block.clone(), block))
                                    .collect(),
                            );
                            pending_at = i;
                        }
                    } else if owner == "BlockIds" {
                        if let Some(set) = plain_ids.get(&name) {
                            pending = Some(
                                set.blocks()
                                    .into_iter()
                                    .map(|block| (block.clone(), block))
                                    .collect(),
                            );
                            pending_at = i;
                        }
                    }
                }
            }
            0xb8 => {
                let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                let registers = (owner.ends_with("/Blocks") && name.starts_with("register"))
                    || (owner.ends_with("ColorCollection")
                        || owner.ends_with("WeatheringCopperCollection"))
                        && (name == "registerBlocks" || name == "zipMap");
                if !registers {
                    continue;
                }
                let ids = pending
                    .take()
                    .ok_or_else(|| format!("Blocks.<clinit> at {i}: {name} without an id"))?;
                let mut classes = BTreeSet::new();
                let helper = match name.as_str() {
                    "registerStair" | "registerLegacyStair" => {
                        Some("net/minecraft/world/level/block/StairBlock")
                    }
                    "registerSlab" => Some("net/minecraft/world/level/block/SlabBlock"),
                    "registerWall" => Some("net/minecraft/world/level/block/WallBlock"),
                    _ => None,
                };
                if let Some(helper) = helper {
                    classes.insert(helper.to_string());
                } else {
                    for j in pending_at..i {
                        if instructions[j].opcode == 0xba {
                            if let Some(index) = instructions[j].index {
                                lambda_classes(jar, &class, index, 0, &mut classes);
                            }
                        }
                    }
                    if classes.is_empty() {
                        classes.insert("net/minecraft/world/level/block/Block".to_string());
                    }
                }
                events.push(BlockEvent { ids, classes });
            }
            _ => {}
        }
    }
    Ok(events)
}

/// Every block registry id (`minecraft:`-prefixed), in `Blocks.<clinit>` registration
/// order — the order `BuiltInRegistries.BLOCK` iterates.
pub fn block_ids(jar_path: &Path) -> Result<Vec<String>, String> {
    let jar = Jar::open(jar_path)?;
    block_ids_of(&jar)
}

fn block_ids_of(jar: &Jar) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for event in block_events(jar)? {
        out.extend(
            event
                .ids
                .into_iter()
                .map(|(block, _)| format!("minecraft:{block}")),
        );
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct Header {
    name: String,
    super_name: String,
    interfaces: Vec<String>,
}

/// This class, its superclass and its interfaces, from the raw class bytes.
///
/// `classfile.rs` keeps the first two but skips the interface table; the entity-block
/// test needs the interfaces, so this reads the header directly.
fn class_header(bytes: &[u8]) -> Result<Header, String> {
    struct Cursor<'a> {
        bytes: &'a [u8],
        at: usize,
    }
    impl<'a> Cursor<'a> {
        fn u1(&mut self) -> Result<u8, String> {
            let value = *self.bytes.get(self.at).ok_or("class file ends")?;
            self.at += 1;
            Ok(value)
        }
        fn u2(&mut self) -> Result<u16, String> {
            Ok(((self.u1()? as u16) << 8) | self.u1()? as u16)
        }
        fn u4(&mut self) -> Result<u32, String> {
            Ok(((self.u2()? as u32) << 16) | self.u2()? as u32)
        }
        fn skip(&mut self, count: usize) -> Result<(), String> {
            if self.at + count > self.bytes.len() {
                return Err("class file ends".into());
            }
            self.at += count;
            Ok(())
        }
    }
    let mut cursor = Cursor { bytes, at: 0 };
    if cursor.u4()? != 0xCAFE_BABE {
        return Err("not a class file".into());
    }
    cursor.u2()?;
    cursor.u2()?;
    let count = cursor.u2()? as usize;
    let mut utf8: Vec<Option<String>> = vec![None; count];
    let mut classes: Vec<Option<usize>> = vec![None; count];
    let mut index = 1usize;
    while index < count {
        let tag = cursor.u1()?;
        match tag {
            1 => {
                let length = cursor.u2()? as usize;
                let start = cursor.at;
                cursor.skip(length)?;
                utf8[index] =
                    Some(String::from_utf8_lossy(&bytes[start..start + length]).into_owned());
            }
            7 => classes[index] = Some(cursor.u2()? as usize),
            8 => {
                cursor.u2()?;
            }
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => cursor.skip(4)?,
            5 | 6 => {
                cursor.skip(8)?;
                index += 1;
            }
            15 => cursor.skip(3)?,
            16 | 19 | 20 => cursor.skip(2)?,
            other => return Err(format!("constant tag {other}")),
        }
        index += 1;
    }
    let name_of = |at: usize| -> String {
        classes
            .get(at)
            .and_then(|inner| *inner)
            .and_then(|inner| utf8.get(inner).cloned().flatten())
            .unwrap_or_default()
    };
    let _access = cursor.u2()?;
    let this_class = cursor.u2()? as usize;
    let super_class = cursor.u2()? as usize;
    let count = cursor.u2()? as usize;
    let mut interfaces = Vec::with_capacity(count);
    for _ in 0..count {
        let at = cursor.u2()? as usize;
        interfaces.push(name_of(at));
    }
    Ok(Header {
        name: name_of(this_class),
        super_name: name_of(super_class),
        interfaces,
    })
}

fn implements_entity_block(jar: &Jar, start: &str) -> bool {
    const ENTITY_BLOCK: &str = "net/minecraft/world/level/block/EntityBlock";
    let mut seen = BTreeSet::new();
    let mut todo = vec![start.to_string()];
    while let Some(name) = todo.pop() {
        if name == ENTITY_BLOCK {
            return true;
        }
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        let Ok(bytes) = jar.bytes(&format!("{name}.class")) else {
            continue;
        };
        let Ok(header) = class_header(bytes) else {
            continue;
        };
        for interface in header.interfaces {
            if interface == ENTITY_BLOCK {
                return true;
            }
            todo.push(interface);
        }
        if !header.super_name.is_empty() {
            todo.push(header.super_name);
        }
    }
    false
}

/// Every block that has a block entity (`BlockStateBase.hasBlockEntity`), in block
/// registry order.
pub fn block_entities(jar_path: &Path) -> Result<Vec<String>, String> {
    let jar = Jar::open(jar_path)?;
    block_entities_of(&jar)
}

fn block_entities_of(jar: &Jar) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for event in block_events(jar)? {
        let entity = event
            .classes
            .iter()
            .any(|class| implements_entity_block(jar, class));
        if entity {
            out.extend(
                event
                    .ids
                    .into_iter()
                    .map(|(block, _)| format!("minecraft:{block}")),
            );
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// registries.json
// ---------------------------------------------------------------------------

struct RegistryInfo {
    key: String,
    default: Option<String>,
    ids: Vec<String>,
    protocol_id: usize,
}

/// `Registries.<clinit>`: field name -> registry key path (e.g. `BLOCK` -> `block`).
fn registries_fields(jar: &Jar) -> Result<BTreeMap<String, String>, String> {
    let class = jar.class("net/minecraft/core/registries/Registries")?;
    let instructions = decode(clinit(&class)?)?;
    let mut out = BTreeMap::new();
    let mut pending: Option<String> = None;
    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&class.constants, index))
                {
                    pending = Some(text);
                }
            }
            0xb3 => {
                if let Some((_, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if let Some(key) = pending.take() {
                        out.insert(field, key);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The registry field of every direct `Registry.register`/`BuiltInRegistries.register`
/// call in a method: the call's preceding ids in order.
fn direct_registration_ids(
    class: &ClassFile,
    method: &classfile::Member,
) -> Result<Vec<String>, String> {
    let Some(code) = method.code.as_ref() else {
        return Ok(Vec::new());
    };
    let instructions = decode(&code.bytes)?;
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&class.constants, index))
                {
                    pending = Some(text);
                }
            }
            0xb8 | 0xb9 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    let registers = name == "register"
                        && (owner == "net/minecraft/core/Registry"
                            || owner.ends_with("/BuiltInRegistries"));
                    if registers {
                        if let Some(id) = pending.take() {
                            out.push(id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The registry ids a class registers through its own `register*`/`add*` helpers: the
/// helper body names the `BuiltInRegistries` field, the call site names the id.
fn helper_registration_ids(jar: &Jar) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut events: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for internal in jar.class_names() {
        if internal.ends_with("BuiltInRegistries") {
            continue;
        }
        let Ok(bytes) = jar.bytes(&format!("{internal}.class")) else {
            continue;
        };
        let Ok(class) = classfile::parse(bytes) else {
            continue;
        };
        for method in &class.methods {
            if method.name != "<clinit>" && method.name != "init" {
                continue;
            }
            let Some(code) = method.code.as_ref() else {
                continue;
            };
            let Ok(instructions) = decode(&code.bytes) else {
                continue;
            };
            let mut pending: Option<String> = None;
            for instruction in instructions {
                match instruction.opcode {
                    0x12 | 0x13 => {
                        if let Some(text) = instruction
                            .index
                            .and_then(|index| string_constant(&class.constants, index))
                        {
                            pending = Some(text);
                        }
                    }
                    0xb8 => {
                        let Some((owner, name, _)) = instruction
                            .index
                            .and_then(|index| method_ref(&class.constants, index))
                        else {
                            continue;
                        };
                        if !(name.starts_with("register") || name.starts_with("add")) {
                            continue;
                        }
                        if owner.ends_with("/Registry") && name == "register" {
                            continue;
                        }
                        let Ok(callee) = jar.class(&owner) else {
                            continue;
                        };
                        let Some(target) = callee.methods.iter().find(|member| member.name == name)
                        else {
                            continue;
                        };
                        let Some(callee_code) = target.code.as_ref() else {
                            continue;
                        };
                        let Ok(callee_instructions) = decode(&callee_code.bytes) else {
                            continue;
                        };
                        let mut registry = None;
                        for inner in callee_instructions {
                            if inner.opcode != 0xb2 {
                                continue;
                            }
                            if let Some((field_owner, field, _)) = inner
                                .index
                                .and_then(|index| field_ref(&callee.constants, index))
                            {
                                if field_owner.ends_with("/BuiltInRegistries") {
                                    registry = Some(field);
                                    break;
                                }
                            }
                        }
                        if let (Some(registry), Some(id)) = (registry, pending.take()) {
                            events.entry(registry).or_default().push(id);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(events)
}

/// The `reports/registries.json` document this jar lets us rebuild.
///
/// Only the registries in [`VERIFIED_REGISTRIES`] plus `minecraft:item`/`minecraft:block`
/// are emitted; the rest are named in [`Coverage::absent_registries`] with their reason.
pub fn registries(jar_path: &Path) -> Result<(Value, Coverage), String> {
    let jar = Jar::open(jar_path)?;
    registries_of(&jar)
}

fn registries_of(jar: &Jar) -> Result<(Value, Coverage), String> {
    let keys = registries_fields(jar)?;
    let builtin = jar.class("net/minecraft/core/registries/BuiltInRegistries")?;
    let instructions = decode(clinit(&builtin)?)?;

    // Creation order: this is the registry's own protocol id.
    let mut fields: Vec<(String, Option<String>, Option<(String, String, String)>)> = Vec::new();
    let mut last_field: Option<String> = None;
    let mut last_impl: Option<(String, String, String)> = None;
    let mut default: Option<String> = None;
    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&builtin.constants, index))
                {
                    default = Some(text);
                }
            }
            0xb2 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&builtin.constants, index))
                {
                    if owner.ends_with("/Registries") {
                        last_field = Some(name);
                    }
                }
            }
            0xba => {
                if let Some(index) = instruction.index {
                    last_impl = bootstrap_impl(&builtin, index);
                }
            }
            0xb8 => {
                if let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&builtin.constants, index))
                {
                    if name.starts_with("register") {
                        if let Some(field) = last_field.take() {
                            fields.push((field, default.take(), last_impl.take()));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let bootstrap_ids = |impl_target: &(String, String, String)| -> Result<Vec<String>, String> {
        let (owner, name, _) = impl_target;
        let Ok(class) = jar.class(owner) else {
            return Ok(Vec::new());
        };
        let Some(member) = class.methods.iter().find(|member| member.name == *name) else {
            return Ok(Vec::new());
        };
        direct_registration_ids(&class, member)
    };

    let helper_ids = helper_registration_ids(jar)?;

    let mut infos = Vec::new();
    for (protocol_id, (field, default, impl_target)) in fields.iter().enumerate() {
        let Some(key) = keys.get(field) else {
            continue;
        };
        let mut ids = match impl_target {
            Some(target) => bootstrap_ids(target)?,
            None => Vec::new(),
        };
        if ids.is_empty() {
            ids = helper_ids.get(field).cloned().unwrap_or_default();
        }
        // Translation-key -> registry-id adjustments for classes whose literals are
        // translation keys. Only `attribute` is in the verified set; the others are
        // applied so the reader can be checked against them too.
        if key == "attribute" {
            ids = ids
                .into_iter()
                .map(|id| {
                    id.strip_prefix("attribute.name.")
                        .unwrap_or(&id)
                        .to_string()
                })
                .collect();
        } else if key == "mob_effect" {
            ids = ids
                .into_iter()
                .map(|id| id.strip_prefix("effect.").unwrap_or(&id).to_string())
                .collect();
        }
        if key == "item" {
            ids = item_ids_of(jar)?
                .into_iter()
                .map(|id| id.trim_start_matches("minecraft:").to_string())
                .collect();
        } else if key == "block" {
            ids = block_ids_of(jar)?
                .into_iter()
                .map(|id| id.trim_start_matches("minecraft:").to_string())
                .collect();
        }
        infos.push(RegistryInfo {
            key: format!("minecraft:{key}"),
            default: default.clone(),
            ids,
            protocol_id,
        });
    }

    let mut document = Map::new();
    let mut coverage = Coverage::new();
    let mut emitted = BTreeSet::new();
    for info in &infos {
        let wanted = info.key == "minecraft:item"
            || info.key == "minecraft:block"
            || VERIFIED_REGISTRIES.contains(&info.key.as_str());
        if !wanted {
            continue;
        }
        let mut entries = Map::new();
        for (index, id) in info.ids.iter().enumerate() {
            let mut entry = Map::new();
            entry.insert("protocol_id".to_string(), Value::Number(index.into()));
            entries.insert(format!("minecraft:{id}"), Value::Object(entry));
        }
        let mut registry = Map::new();
        registry.insert(
            "protocol_id".to_string(),
            Value::Number(info.protocol_id.into()),
        );
        if let Some(default) = &info.default {
            registry.insert(
                "default".to_string(),
                Value::String(format!("minecraft:{default}")),
            );
        }
        registry.insert("entries".to_string(), Value::Object(entries));
        document.insert(info.key.clone(), Value::Object(registry));
        emitted.insert(info.key.clone());
    }

    let all: BTreeSet<String> = infos.iter().map(|info| info.key.clone()).collect();
    for key in &all {
        if emitted.contains(key) {
            coverage.registries.push(key.clone());
        } else {
            coverage.absent_registries.push(format!(
                "{key}: registration order needs a loop or a value this reader cannot evaluate"
            ));
        }
    }
    // The registries this module did not reach are reported, not guessed.
    coverage.note(format!(
        "registries.json: {} of {} registries reproduced exactly (protocol ids from \
         registration order); the rest are named in absent_registries",
        emitted.len(),
        all.len()
    ));
    coverage.produced.push(format!(
        "registries.json ({} of {} registries)",
        emitted.len(),
        all.len()
    ));
    coverage.missing.push(format!(
        "registries.json ({} of {} registries)",
        all.len() - emitted.len(),
        all.len()
    ));
    coverage
        .produced
        .push("command-registries.json#items".to_string());
    coverage
        .produced
        .push("command-registries.json#block_entities".to_string());
    coverage.produced.push(format!(
        "command-registries.json#by_registry ({} registries)",
        emitted.len()
    ));
    for (table, why) in BLOCKED_TABLES {
        coverage.missing.push(table.to_string());
        coverage.note(format!("{table}: {why}"));
    }
    coverage
        .missing
        .push("command-registries.json#blocks".to_string());
    coverage.note(
        "command-registries.json#blocks: a block's state properties are declared in its \
         class' createBlockStateDefinition, and the blockstates assets name only the \
         properties a model uses (629 of 1286 blocks complete). A prototype bytecode reader \
         reaches 1159 of 1286 exactly, but predicate-based enums (rail shape), lists built \
         at runtime (chiseled bookshelf) and bounded integers computed by helper calls (sign \
         rotation) need values that are not literals, so no partial table is written",
    );
    Ok((Value::Object(document), coverage))
}

// ---------------------------------------------------------------------------
// command-registries fragments
// ---------------------------------------------------------------------------

/// The `command-registries.json` parts this module can fill: `items`, `block_entities`
/// and the code-registered `by_registry` entries. `blocks` is deliberately absent (see
/// the module docs).
pub fn command_fragments(jar_path: &Path) -> Result<Value, String> {
    let jar = Jar::open(jar_path)?;
    let items = item_ids_of(&jar)?;
    let blocks = block_ids_of(&jar)?;
    let entities = block_entities_of(&jar)?;
    let (registries, _) = registries_of(&jar)?;
    let registries = registries.as_object().cloned().unwrap_or_default();

    let mut by_registry = Map::new();
    for key in ["minecraft:item", "minecraft:block"]
        .iter()
        .map(|key| key.to_string())
        .chain(registries.keys().cloned())
    {
        let ids: Vec<String> = if key == "minecraft:item" {
            let mut ids = items.clone();
            ids.sort();
            ids
        } else if key == "minecraft:block" {
            let mut ids = blocks.clone();
            ids.sort();
            ids
        } else {
            let Some(entry) = registries.get(&key) else {
                continue;
            };
            let mut ids: Vec<String> = entry
                .get("entries")
                .and_then(Value::as_object)
                .map(|entries| entries.keys().cloned().collect())
                .unwrap_or_default();
            ids.sort();
            ids
        };
        let directory = key.trim_start_matches("minecraft:");
        let mut entry = Map::new();
        entry.insert(
            "ids".to_string(),
            Value::Array(ids.into_iter().map(Value::String).collect()),
        );
        entry.insert(
            "tags".to_string(),
            Value::Array(
                jar.tag_ids(directory)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        by_registry.insert(key, Value::Object(entry));
    }

    let mut root = Map::new();
    root.insert(
        "items".to_string(),
        Value::Array(items.into_iter().map(Value::String).collect()),
    );
    root.insert(
        "block_entities".to_string(),
        Value::Array(entities.into_iter().map(Value::String).collect()),
    );
    root.insert("by_registry".to_string(), Value::Object(by_registry));
    Ok(Value::Object(root))
}

/// Write [`TABLE`] into `out_dir` and return what could not be produced.
pub fn write(jar_path: &Path, out_dir: &Path) -> Result<Coverage, String> {
    let (document, coverage) = registries(jar_path)?;
    fs::create_dir_all(out_dir).map_err(|error| format!("{}: {error}", out_dir.display()))?;
    let path = out_dir.join(TABLE);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(coverage)
}
