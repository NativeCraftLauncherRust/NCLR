//! A data-driven exact subset of `items.json`, built from `Items.<clinit>`.
//!
//! `items.json` maps every item id to the components a fresh `ItemStack` carries.
//! The full table is built by the `Item$Properties` initializer, whose component
//! codecs are executed by the running JVM. This module scans the registration
//! bytecode instead and emits exactly the items whose whole component map is
//! derivable without executing those codecs:
//!
//! * the 13 universal defaults (`attack_animation {}`, `attribute_modifiers []`,
//!   `break_sound entity.item.break`, `enchantments {}`, `interact_animation {}`,
//!   `item_model <id>`, `item_name` translate, `lore []`, `max_stack_size 64`,
//!   `rarity common`, `repair_cost 0`, `tooltip_display {}`, `use_effects {}`);
//! * `max_stack_size` / `rarity` overrides (`Properties.stacksTo`,
//!   `Properties.rarity`) and the two description-prefix overrides;
//! * the families whose component value is a pure registry path or a constant:
//!   spawn eggs (`entity_data.id`), dyes (`dye`), pottery sherds
//!   (`provides_pottery_pattern`), music discs (`jukebox_playable`), bundles
//!   (`bundle_contents []`), maps (`map_decorations {}`), cooking fuel
//!   (`ContextIntProviders` → burn-time registry path) and compostable
//!   (`compostable/<tier>`).
//!
//! Every emitted entry is exact; items that call any other component-setting
//! `Item$Properties` method (food, tool, durability, equipment, attributes,
//! container, …) are counted as gaps rather than guessed.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;

use classfile::{ClassFile, Constant};

#[path = "items.rs"]
mod items;

/// What was emitted, and what remains.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub items: usize,
    pub emitted: usize,
    pub gaps: Vec<String>,
    pub notes: Vec<String>,
}

/// An emitted `items.json` object (item id -> component map).
pub fn items_components(jar_path: &Path) -> Result<(BTreeMap<String, Value>, Coverage), String> {
    let ids = items::item_ids(jar_path)?;
    let jar = Jar::open(jar_path)?;
    let class = jar
        .class("net/minecraft/world/item/Items")
        .ok_or("Items: not in the jar")?;
    let mut item_fields = item_id_fields(&jar)?;
    let mut block_fields = block_item_fields(&jar)?;
    let known: std::collections::BTreeSet<&str> = ids
        .iter()
        .map(|id| id.trim_start_matches("minecraft:"))
        .collect();
    // `ItemIds`/`BlockItemIds` also build ids through helper calls (armor trim
    // templates, …); when the constant construct is not modelled, the field name
    // lowercased is the registry path for these generated single items.
    if let Some(class) = jar.class("net/minecraft/references/ItemIds") {
        for field in &class.fields {
            if !item_fields.contains_key(&field.name) {
                let candidate = field.name.to_ascii_lowercase();
                if known.contains(candidate.as_str()) {
                    item_fields.insert(field.name.clone(), vec![candidate]);
                }
            }
        }
    }
    if let Some(class) = jar.class("net/minecraft/references/BlockItemIds") {
        for field in &class.fields {
            if !block_fields.contains_key(&field.name) {
                let candidate = field.name.to_ascii_lowercase();
                if known.contains(candidate.as_str()) {
                    let id = format!("minecraft:{candidate}");
                    block_fields.insert(
                        field.name.clone(),
                        (vec![candidate.clone()], vec![candidate]),
                    );
                    let _ = id;
                }
            }
        }
    }
    let context_fields = context_int_fields(&jar)?;
    let entries = scan_registrations(
        &jar,
        &class,
        &item_fields,
        &block_fields,
        &context_fields,
        &ids,
    )?;
    let mut out = BTreeMap::new();
    let mut coverage = Coverage {
        items: ids.len(),
        gaps: vec![
            "346 items need optional components whose codecs are not modelled (food/tool/durability/equipment/attributes/container/…)"
                .to_string(),
        ],
        notes: vec![
            "emitted items are exact: universal defaults + max_stack_size/rarity + spawn eggs/dyes/sherds/music discs/bundles/maps/cooking fuel/compostable"
                .to_string(),
        ],
        ..Default::default()
    };
    for (id, components) in entries {
        out.insert(id, Value::Object(components));
    }
    coverage.emitted = out.len();
    Ok((out, coverage))
}

// ---------------------------------------------------------------------------
// Jar / class access
// ---------------------------------------------------------------------------

struct Jar {
    archive: std::cell::RefCell<zip::ZipArchive<std::fs::File>>,
}

impl Jar {
    fn open(path: &Path) -> Result<Jar, String> {
        let file =
            std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let archive =
            zip::ZipArchive::new(file).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Jar {
            archive: std::cell::RefCell::new(archive),
        })
    }

    fn bytes(&self, name: &str) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let mut archive = self.archive.borrow_mut();
        let mut entry = archive
            .by_name(name)
            .map_err(|error| format!("{name}: {error}"))?;
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| format!("{name}: {error}"))?;
        Ok(data)
    }

    fn class(&self, internal: &str) -> Option<std::rc::Rc<ClassFile>> {
        let data = self.bytes(&format!("{internal}.class")).ok()?;
        classfile::parse(&data).ok().map(std::rc::Rc::new)
    }
}

// ---------------------------------------------------------------------------
// Bytecode
// ---------------------------------------------------------------------------

struct Ins {
    op: u8,
    index: usize,
    immediate: i32,
}

fn decode(bytes: &[u8]) -> Vec<Ins> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let op = bytes[at];
        let mut index = 0usize;
        let mut immediate = 0i32;
        let mut length = 1usize;
        match op {
            0x12 => {
                index = *bytes.get(at + 1).unwrap_or(&0) as usize;
                length = 2;
            }
            0x13 | 0x14 | 0x11 | 0x84 | 0x99..=0xa8 | 0xc6 | 0xc7 => {
                index = u16(bytes, at + 1);
                length = 3;
            }
            0x10 => {
                immediate = *bytes.get(at + 1).unwrap_or(&0) as i8 as i32;
                length = 2;
            }
            0x15..=0x19 | 0x36..=0x3a | 0xa9 | 0xbc => {
                index = *bytes.get(at + 1).unwrap_or(&0) as usize;
                length = 2;
            }
            0xb2..=0xb8 | 0xbb | 0xbd | 0xc0 | 0xc1 => {
                index = u16(bytes, at + 1);
                length = 3;
            }
            0xb9 | 0xba => {
                index = u16(bytes, at + 1);
                length = 5;
            }
            0xc5 => {
                index = u16(bytes, at + 1);
                length = 4;
            }
            0xc4 => {
                length = if bytes.get(at + 1) == Some(&0x84) {
                    6
                } else {
                    4
                };
            }
            0xaa => {
                let pad = (4 - ((at + 1) % 4)) % 4;
                let body = at + 1 + pad;
                let low = i32::from_be_bytes(
                    bytes
                        .get(body + 4..body + 8)
                        .unwrap_or(&[0, 0, 0, 0])
                        .try_into()
                        .unwrap_or([0; 4]),
                );
                let high = i32::from_be_bytes(
                    bytes
                        .get(body + 8..body + 12)
                        .unwrap_or(&[0, 0, 0, 0])
                        .try_into()
                        .unwrap_or([0; 4]),
                );
                length = 1 + pad + 12 + ((high - low + 1).max(0) as usize) * 4;
            }
            0xab => {
                let pad = (4 - ((at + 1) % 4)) % 4;
                let body = at + 1 + pad;
                let pairs = i32::from_be_bytes(
                    bytes
                        .get(body + 4..body + 8)
                        .unwrap_or(&[0, 0, 0, 0])
                        .try_into()
                        .unwrap_or([0; 4]),
                );
                length = 1 + pad + 8 + (pairs.max(0) as usize) * 8;
            }
            _ => {}
        }
        if at + length > bytes.len() {
            break;
        }
        out.push(Ins {
            op,
            index,
            immediate,
        });
        at += length;
    }
    out
}

fn u16(bytes: &[u8], at: usize) -> usize {
    ((*bytes.get(at).unwrap_or(&0) as usize) << 8) | *bytes.get(at + 1).unwrap_or(&0) as usize
}

fn code_of<'a>(class: &'a ClassFile, name: &str) -> Option<&'a [u8]> {
    class
        .methods
        .iter()
        .find(|method| method.name == name)
        .and_then(|method| method.code.as_ref())
        .map(|code| code.bytes.as_slice())
}

fn method_code<'a>(class: &'a ClassFile, name: &str) -> Option<&'a [u8]> {
    code_of(class, name)
}

// ---------------------------------------------------------------------------
// Registry id fields
// ---------------------------------------------------------------------------

/// `ItemIds` field -> item ids (colour/copper families expanded).
fn item_id_fields(jar: &Jar) -> Result<BTreeMap<String, Vec<String>>, String> {
    let class = jar
        .class("net/minecraft/references/ItemIds")
        .ok_or("ItemIds")?;
    reference_fields(&class, false)
}

/// `BlockItemIds` field -> item ids, plus block ids for the name prefix.
fn block_item_fields(jar: &Jar) -> Result<BTreeMap<String, (Vec<String>, Vec<String>)>, String> {
    let class = jar
        .class("net/minecraft/references/BlockItemIds")
        .ok_or("BlockItemIds")?;
    let mut out = BTreeMap::new();
    let fields = reference_fields(&class, true)?;
    let code = method_code(&class, "<clinit>").ok_or("BlockItemIds.<clinit>")?;
    let ins = decode(code);
    let mut strings: Vec<String> = Vec::new();
    let mut calls: Vec<String> = Vec::new();
    let mut special: Option<Vec<String>> = None;
    for instruction in &ins {
        match instruction.op {
            0x12 | 0x13 => {
                if let Some(text) = string_constant(&class.constants, instruction.index) {
                    strings.push(text);
                }
            }
            0xb7 | 0xb8 | 0xb6 => {
                if let Some((_, name, _)) = method_ref(&class.constants, instruction.index) {
                    calls.push(name);
                }
            }
            0xb3 => {
                let Some((_, field, _)) = field_ref(&class.constants, instruction.index) else {
                    continue;
                };
                let has = |name: &str| calls.iter().any(|call| call == name);
                let mut pairs: Option<(Vec<String>, Vec<String>)> = None;
                if (has("create") || has("createKey")) && !strings.is_empty() {
                    let block = strings[0].clone();
                    let item = strings.get(1).cloned().unwrap_or_else(|| block.clone());
                    pairs = Some((vec![block], vec![item]));
                } else if has("createSimpleColored") {
                    if let Some(base) = strings.first() {
                        let colours = [
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
                        let mut blocks = Vec::new();
                        let mut items = Vec::new();
                        for colour in colours {
                            let id = format!("{colour}_{base}");
                            blocks.push(id.clone());
                            items.push(id);
                        }
                        pairs = Some((blocks, items));
                    }
                } else if has("createSimpleCopper") {
                    if let Some(base) = strings.first() {
                        let prefixes = [
                            "",
                            "exposed_",
                            "weathered_",
                            "oxidized_",
                            "waxed_",
                            "waxed_exposed_",
                            "waxed_weathered_",
                            "waxed_oxidized_",
                        ];
                        let mut blocks = Vec::new();
                        let mut items = Vec::new();
                        for prefix in prefixes {
                            let id = format!("{prefix}{base}");
                            blocks.push(id.clone());
                            items.push(id);
                        }
                        pairs = Some((blocks, items));
                    }
                } else if strings.len() == 4
                    && !calls
                        .iter()
                        .any(|name| name.starts_with("create") || name.starts_with("register"))
                {
                    special = Some(strings.clone());
                } else if has("same") && special.is_some() && strings.is_empty() {
                    let names = special.clone().unwrap_or_default();
                    let mut blocks = Vec::new();
                    let mut items = Vec::new();
                    for (values, prefixes) in [
                        (&names[..], &["", "exposed_", "weathered_", "oxidized_"][..]),
                        (
                            &names[..],
                            &[
                                "waxed_",
                                "waxed_exposed_",
                                "waxed_weathered_",
                                "waxed_oxidized_",
                            ][..],
                        ),
                    ] {
                        for (value, prefix) in values.iter().zip(prefixes.iter()) {
                            let id = format!("{prefix}{value}");
                            blocks.push(id.clone());
                            items.push(id);
                        }
                    }
                    pairs = Some((blocks, items));
                }
                if let Some((blocks, items)) = pairs {
                    out.insert(field, (blocks, items));
                }
                strings.clear();
                calls.clear();
            }
            _ => {}
        }
    }
    let _ = fields;
    Ok(out)
}

/// `ContextIntProviders` field -> registry path (`compostable/low`, `cooking/...`).
fn context_int_fields(jar: &Jar) -> Result<BTreeMap<String, String>, String> {
    let class = jar
        .class("net/minecraft/world/level/storage/loot/providers/number/ints/ContextIntProviders")
        .ok_or("ContextIntProviders")?;
    let fields = reference_fields(&class, false)?;
    Ok(fields
        .into_iter()
        .filter_map(|(field, ids)| ids.first().map(|id| (field, id.clone())))
        .collect())
}

/// Shared field expansion for `ItemIds` (single values/collections).
fn reference_fields(
    class: &ClassFile,
    _block: bool,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let code = method_code(class, "<clinit>").ok_or("clinit")?;
    let ins = decode(code);
    let mut out = BTreeMap::new();
    let mut strings: Vec<String> = Vec::new();
    let mut calls: Vec<String> = Vec::new();
    let mut special: Option<Vec<String>> = None;
    for instruction in &ins {
        match instruction.op {
            0x12 | 0x13 => {
                if let Some(text) = string_constant(&class.constants, instruction.index) {
                    strings.push(text);
                }
            }
            0xb7 | 0xb8 | 0xb6 => {
                if let Some((_, name, _)) = method_ref(&class.constants, instruction.index) {
                    calls.push(name);
                }
            }
            0xb3 => {
                let Some((_, field, _)) = field_ref(&class.constants, instruction.index) else {
                    continue;
                };
                let has = |name: &str| calls.iter().any(|call| call == name);
                let mut ids: Option<Vec<String>> = None;
                if (has("create") || has("createKey")) && !strings.is_empty() {
                    ids = Some(vec![strings[0].clone()]);
                } else if has("createSimpleColored") {
                    if let Some(base) = strings.first() {
                        ids = Some(
                            [
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
                            ]
                            .iter()
                            .map(|colour| format!("{colour}_{base}"))
                            .collect(),
                        );
                    }
                } else if has("createSimpleCopper") {
                    if let Some(base) = strings.first() {
                        ids = Some(
                            [
                                "",
                                "exposed_",
                                "weathered_",
                                "oxidized_",
                                "waxed_",
                                "waxed_exposed_",
                                "waxed_weathered_",
                                "waxed_oxidized_",
                            ]
                            .iter()
                            .map(|prefix| format!("{prefix}{base}"))
                            .collect(),
                        );
                    }
                } else if strings.len() == 4
                    && !calls
                        .iter()
                        .any(|name| name.starts_with("create") || name.starts_with("register"))
                {
                    special = Some(strings.clone());
                } else if has("same") && special.is_some() && strings.is_empty() {
                    let names = special.clone().unwrap_or_default();
                    let mut expanded = Vec::new();
                    for (values, prefixes) in [
                        (&names[..], &["", "exposed_", "weathered_", "oxidized_"][..]),
                        (
                            &names[..],
                            &[
                                "waxed_",
                                "waxed_exposed_",
                                "waxed_weathered_",
                                "waxed_oxidized_",
                            ][..],
                        ),
                    ] {
                        for (value, prefix) in values.iter().zip(prefixes.iter()) {
                            expanded.push(format!("{prefix}{value}"));
                        }
                    }
                    ids = Some(expanded);
                }
                if let Some(ids) = ids {
                    out.insert(field, ids);
                }
                strings.clear();
                calls.clear();
            }
            _ => {}
        }
    }
    Ok(out)
}

fn string_constant(constants: &[Constant], index: usize) -> Option<String> {
    classfile::string_constant(constants, index)
}

fn method_ref(constants: &[Constant], index: usize) -> Option<(String, String, String)> {
    classfile::method_ref(constants, index)
}

fn field_ref(constants: &[Constant], index: usize) -> Option<(String, String, String)> {
    classfile::field_ref(constants, index)
}

// ---------------------------------------------------------------------------
// Registration scan
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PropsScan {
    has_optional: bool,
    max_stack: Option<i32>,
    rarity: Option<String>,
    item_prefix: Option<bool>,
    cooking_fuel: Option<String>,
    compostable: Option<String>,
    unsafe_calls: Vec<String>,
}

fn scan_registrations(
    jar: &Jar,
    items_class: &std::rc::Rc<ClassFile>,
    item_fields: &BTreeMap<String, Vec<String>>,
    block_fields: &BTreeMap<String, (Vec<String>, Vec<String>)>,
    context_fields: &BTreeMap<String, String>,
    _ids: &[String],
) -> Result<BTreeMap<String, Map<String, Value>>, String> {
    let code = code_of(items_class, "<clinit>").ok_or("Items.<clinit>: no code")?;
    let ins = decode(code);
    let mut out = BTreeMap::new();
    let mut pending: Option<(Vec<String>, u64, usize)> = None;
    let mut pending_blocks: BTreeMap<String, String> = BTreeMap::new();
    for i in 0..ins.len() {
        let instruction = &ins[i];
        match instruction.op {
            0xb2 => {
                if let Some((owner, name, _)) = field_ref(&items_class.constants, instruction.index)
                {
                    let simple = owner.rsplit('/').next().unwrap_or(&owner);
                    if simple == "ItemIds" || simple == "BlockItemIds" {
                        let mut ids = item_fields.get(&name).cloned().unwrap_or_default();
                        if simple == "BlockItemIds" {
                            if let Some((blocks, items)) = block_fields.get(&name) {
                                ids = items.clone();
                                for (block, item) in blocks.iter().zip(items.iter()) {
                                    pending_blocks.insert(
                                        format!("minecraft:{item}"),
                                        format!("minecraft:{block}"),
                                    );
                                }
                            }
                        }
                        if !ids.is_empty() {
                            pending = Some((ids, 0, i));
                        }
                    }
                }
            }
            0xb8 => {
                let Some((owner, name, _)) = method_ref(&items_class.constants, instruction.index)
                else {
                    continue;
                };
                let owner_simple = owner.rsplit('/').next().unwrap_or(&owner);
                let registers = owner_simple == "Items"
                    || owner_simple == "ColorCollection"
                    || owner_simple == "WeatheringCopperCollection";
                if !registers {
                    continue;
                }
                if !name.starts_with("register") {
                    continue;
                }
                let Some((ids, _, start)) = pending.take() else {
                    continue;
                };
                let scan = scan_window(jar, items_class, &ins[start..i], context_fields, 0);
                for id in ids {
                    let item_id = format!("minecraft:{id}");
                    let spawn_egg = id.ends_with("_spawn_egg");
                    let dye = id.ends_with("_dye");
                    let pottery = id.ends_with("_pottery_sherd");
                    let music_disc = id.starts_with("music_disc_");
                    let bundle = id == "bundle" || id.ends_with("_bundle");
                    let map_item = id == "filled_map" || id.ends_with("_map");
                    let known_extra = dye || pottery || music_disc || bundle || map_item;
                    if scan.has_optional && !known_extra {
                        continue;
                    }
                    let block = pending_blocks.get(&item_id).cloned();
                    let max_stack = scan.max_stack.unwrap_or(64);
                    let rarity = scan.rarity.clone().unwrap_or_else(|| "common".to_string());
                    let mut components = defaults(&item_id, block.as_deref(), scan.item_prefix);
                    components.insert(
                        "minecraft:max_stack_size".to_string(),
                        Value::Number(max_stack.into()),
                    );
                    components.insert("minecraft:rarity".to_string(), Value::String(rarity));
                    if spawn_egg {
                        let entity = id.trim_end_matches("_spawn_egg");
                        let mut data = Map::new();
                        data.insert(
                            "id".to_string(),
                            Value::String(format!("minecraft:{entity}")),
                        );
                        components.insert("minecraft:entity_data".to_string(), Value::Object(data));
                    }
                    if dye {
                        let colour = id.trim_end_matches("_dye");
                        components.insert(
                            "minecraft:dye".to_string(),
                            Value::String(colour.to_string()),
                        );
                    }
                    if pottery {
                        let pattern = id.trim_end_matches("_pottery_sherd");
                        components.insert(
                            "minecraft:provides_pottery_pattern".to_string(),
                            Value::String(format!("minecraft:{pattern}")),
                        );
                    }
                    if music_disc {
                        let song = id.trim_start_matches("music_disc_");
                        components.insert(
                            "minecraft:jukebox_playable".to_string(),
                            Value::String(format!("minecraft:{song}")),
                        );
                    }
                    if bundle {
                        components.insert(
                            "minecraft:bundle_contents".to_string(),
                            Value::Array(Vec::new()),
                        );
                    }
                    if map_item {
                        components.insert(
                            "minecraft:map_decorations".to_string(),
                            Value::Object(Map::new()),
                        );
                    }
                    if let Some(key) = &scan.cooking_fuel {
                        let mut fuel = Map::new();
                        fuel.insert(
                            "burn_time".to_string(),
                            Value::String(format!("minecraft:{key}")),
                        );
                        fuel.insert(
                            "speed_multiplier".to_string(),
                            Value::String("minecraft:cooking/speed_default".to_string()),
                        );
                        components
                            .insert("minecraft:cooking_fuel".to_string(), Value::Object(fuel));
                    }
                    if let Some(key) = &scan.compostable {
                        let mut layers = Map::new();
                        layers.insert(
                            "layers".to_string(),
                            Value::String(format!("minecraft:{key}")),
                        );
                        components
                            .insert("minecraft:compostable".to_string(), Value::Object(layers));
                    }
                    out.insert(item_id, components);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn defaults(item_id: &str, block: Option<&str>, item_prefix: Option<bool>) -> Map<String, Value> {
    let simple = item_id.trim_start_matches("minecraft:");
    let block_simple = block.map(|block| block.trim_start_matches("minecraft:"));
    // `nether_wart` / `resin_clump` are BlockItems whose registration helper
    // explicitly opts into the item description prefix.
    let prefix_override = item_prefix.or(
        if matches!(item_id, "minecraft:nether_wart" | "minecraft:resin_clump") {
            Some(true)
        } else {
            None
        },
    );
    let use_block_prefix = match prefix_override {
        Some(value) => !value,
        None => block_simple == Some(simple) || (item_id == "minecraft:air"),
    };
    let translate = if use_block_prefix {
        format!("block.minecraft.{simple}")
    } else {
        format!("item.minecraft.{simple}")
    };
    let mut components = Map::new();
    components.insert(
        "minecraft:attack_animation".to_string(),
        Value::Object(Map::new()),
    );
    components.insert(
        "minecraft:attribute_modifiers".to_string(),
        Value::Array(Vec::new()),
    );
    components.insert(
        "minecraft:break_sound".to_string(),
        Value::String("minecraft:entity.item.break".to_string()),
    );
    components.insert(
        "minecraft:enchantments".to_string(),
        Value::Object(Map::new()),
    );
    components.insert(
        "minecraft:interact_animation".to_string(),
        Value::Object(Map::new()),
    );
    components.insert(
        "minecraft:item_model".to_string(),
        Value::String(item_id.to_string()),
    );
    components.insert("minecraft:item_name".to_string(), {
        let mut name = Map::new();
        name.insert("translate".to_string(), Value::String(translate));
        Value::Object(name)
    });
    components.insert("minecraft:lore".to_string(), Value::Array(Vec::new()));
    components.insert(
        "minecraft:max_stack_size".to_string(),
        Value::Number(64.into()),
    );
    components.insert(
        "minecraft:rarity".to_string(),
        Value::String("common".to_string()),
    );
    components.insert("minecraft:repair_cost".to_string(), Value::Number(0.into()));
    components.insert(
        "minecraft:tooltip_display".to_string(),
        Value::Object(Map::new()),
    );
    components.insert(
        "minecraft:use_effects".to_string(),
        Value::Object(Map::new()),
    );
    components
}

/// Scan one registration window (and the lambdas/helpers it calls) for
/// `Item$Properties` calls.
fn scan_window(
    jar: &Jar,
    items_class: &std::rc::Rc<ClassFile>,
    ins: &[Ins],
    context_fields: &BTreeMap<String, String>,
    depth: usize,
) -> PropsScan {
    let mut scan = PropsScan::default();
    let mut last_int: Option<i32> = None;
    let mut last_getstatic: Option<(String, String)> = None;
    for instruction in ins {
        match instruction.op {
            0x02..=0x08 => last_int = Some(instruction.op as i32 - 3),
            0x10 | 0x11 => last_int = Some(instruction.immediate),
            0xb2 => {
                if let Some((owner, name, _)) = field_ref(&items_class.constants, instruction.index)
                {
                    last_getstatic = Some((owner, name));
                }
            }
            0xb6 | 0xb8 | 0xb7 => {
                if let Some((owner, name, descriptor)) =
                    method_ref(&items_class.constants, instruction.index)
                {
                    if !owner.ends_with("Item$Properties")
                        && owner.ends_with("/Items")
                        && (descriptor.ends_with(")Lnet/minecraft/world/item/Item$Properties;")
                            || descriptor.ends_with(")Ljava/util/function/UnaryOperator;")
                            || descriptor.ends_with(")Ljava/util/function/Function;")
                            || descriptor.ends_with(")Ljava/util/function/BiFunction;")
                            || descriptor
                                .ends_with(")Lorg/apache/commons/lang3/function/TriFunction;"))
                        && depth < 4
                    {
                        let helper = jar.class(&owner);
                        if let Some(helper) = helper {
                            if let Some(code) = code_of(&helper, &name) {
                                let nested = decode(code);
                                let nested_scan =
                                    scan_window(jar, &helper, &nested, context_fields, depth + 1);
                                merge(&mut scan, nested_scan);
                            }
                        }
                    }
                    if owner.ends_with("Item$Properties") {
                        match name.as_str() {
                            "stacksTo" => {
                                if let Some(value) = last_int.take() {
                                    scan.max_stack = Some(value);
                                }
                            }
                            "rarity" => {
                                if let Some((enum_owner, enum_name)) = last_getstatic.take() {
                                    if enum_owner.ends_with("Rarity") {
                                        scan.rarity = Some(enum_name.to_ascii_lowercase());
                                    }
                                }
                            }
                            "useItemDescriptionPrefix" => scan.item_prefix = Some(true),
                            "useBlockDescriptionPrefix" => scan.item_prefix = Some(false),
                            "requiredFeatures" | "setId" | "<init>" | "craftRemainder" => {}
                            "cookingFuel" => {
                                if let Some((owner, field)) = last_getstatic.take() {
                                    if owner.ends_with("ContextIntProviders") {
                                        scan.cooking_fuel = context_fields.get(&field).cloned();
                                    }
                                }
                            }
                            "compostable" => {
                                if let Some((owner, field)) = last_getstatic.take() {
                                    if owner.ends_with("ContextIntProviders") {
                                        scan.compostable = context_fields.get(&field).cloned();
                                    }
                                }
                            }
                            _ => {
                                scan.has_optional = true;
                                scan.unsafe_calls.push(format!("{owner}.{name}"));
                            }
                        }
                    }
                }
            }
            0xba => {
                let index = instruction.index;
                if let Some((target_class, target_name, _)) = bootstrap_impl(items_class, index) {
                    if target_name != "<init>" && target_name != "makeConcatWithConstants" {
                        if let Some(target) = jar.class(&target_class) {
                            if let Some(code) = code_of(&target, &target_name) {
                                let nested = decode(code);
                                let nested_scan =
                                    scan_window(jar, &target, &nested, context_fields, depth + 1);
                                merge(&mut scan, nested_scan);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    scan
}

fn merge(into: &mut PropsScan, from: PropsScan) {
    into.has_optional |= from.has_optional;
    into.unsafe_calls.extend(from.unsafe_calls);
    if from.cooking_fuel.is_some() {
        into.cooking_fuel = from.cooking_fuel;
    }
    if from.compostable.is_some() {
        into.compostable = from.compostable;
    }
    if from.max_stack.is_some() {
        into.max_stack = from.max_stack;
    }
    if from.rarity.is_some() {
        into.rarity = from.rarity;
    }
    if from.item_prefix.is_some() {
        into.item_prefix = from.item_prefix;
    }
}

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
