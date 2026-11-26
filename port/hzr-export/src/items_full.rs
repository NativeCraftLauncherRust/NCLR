//! Completes the registry tables `items.rs` could not reach, on top of its output.
//!
//! `items.rs` rebuilds 68 of vanilla's 95 `reports/registries.json` registries
//! (`minecraft:item` and `minecraft:block` from dedicated registration traces, 66
//! through a first registration-order reader). This module adds the other 27, without
//! reading `ship.hzr3`, so the whole report comes out of the player's jar:
//!
//! * bootstrapped registries whose `bootstrap(Registry)` method registers through a
//!   local `register(...)` helper, with ids passed as string literals or as
//!   `ResourceKey`/`Identifier` reference fields (`ArgumentTypeInfos`, `CreativeModeTabs`,
//!   `GameEvent`, `PoiTypes`, `VillagerProfession`, `Potions`, ...);
//! * registries registered from a class initializer through such a helper
//!   (`Fluids`, `BlockEntityTypes`, `EntityTypes`, `CriteriaTriggers`, ...);
//! * the `Stats` pair (`stat_type`, `custom_stat`), built by `makeRegistryStatType` /
//!   `makeCustomStat`, whose constructors register the entries;
//! * `worldgen/structure_piece` (`StructurePieceType.setPieceId`), `test_function`
//!   (`BuiltinTestFunctions.load`), and `dialog_action_type` (the `ClickEvent.Action`
//!   values wrapped by `ActionTypes.bootstrap`, then its two dynamic actions);
//! * `sound_event`'s variant sounds: the `SoundSet` enum constants and the
//!   `makeConcatWithConstants` recipes inside `SoundEvents`' variant lambdas are
//!   literals, so the 94 generated ids expand exactly;
//! * `worldgen/density_function_type`'s four enum loops (`SimpleDensityFunction`,
//!   `UnaryFunction$Type`, `RoundFunction$Type`, `BinaryFunction$Type`) and the two
//!   JSON-RPC registries (`IncomingRpcMethods` service builders, `OutgoingRpcMethods`
//!   notification builders).
//!
//! It also merges the complete `command-registries.json` (`command_registries`):
//! `registries.rs`'s 32 top-level keys plus this module's `items` (1658), the complete
//! `by_registry` (155 registries: the 95 code registries with their data-pack tags plus
//! every data-driven registry the jar's `data/minecraft/**` names) and
//! `block_entities` (191). The only fragment still absent is `blocks`, which needs
//! `states.rs`'s `block_properties` (task-7, parallel); [`BLOCKS_READY`] flips it on
//! once that reader's own oracle tests are green, and there are two `#[ignore]`d tests
//! that then pin `blocks` and the whole merged document against the oracle.
//!
//!
//! `blocks` (the state property lists) comes from task-7's `states.rs`
//! (`states::block_properties`); its builder insertion order is sorted by name here and
//! the values already come out in `getPossibleValues()` order. With that fragment the
//! merged `command-registries.json` is complete (all 32 top-level keys).
//! `items.json` (per-item component report), `commands.json` (Brigadier tree) and
//! `creative-inventory.json` are produced by executing vanilla's code (the data
//! generator / `CreativeModeTabs`), not by reading a data pack; they stay reported as
//! blocked rather than guessed. Running vanilla's own data generator from the player's
//! jar reproduces `registries.json`, `items.json` and `commands.json` byte-identically
//! (checked while analysing), but that needs a JVM, which the device-side export path
//! does not have.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value};

#[path = "items.rs"]
mod base;

#[path = "registries.rs"]
mod registries_base;

#[path = "states.rs"]
mod states;

#[path = "../../hzr-bytecode/src/classfile.rs"]
mod classfile;
use classfile::{field_ref, method_ref, string_constant, ClassFile, Constant};

/// The registries this module adds to `items.rs`'s 68, with the class/method each one
/// is read from. Verified against a Java-produced `reports/registries.json` (26.3).
pub const ADDED_REGISTRIES: [&str; 27] = [
    "minecraft:block_entity_type",
    "minecraft:command_argument_type",
    "minecraft:context_key_set",
    "minecraft:creative_mode_tab",
    "minecraft:entity_type",
    "minecraft:fluid",
    "minecraft:game_event",
    "minecraft:game_rule",
    "minecraft:map_decoration_type",
    "minecraft:mob_effect",
    "minecraft:point_of_interest_type",
    "minecraft:potion",
    "minecraft:test_instance_type",
    "minecraft:trigger_type",
    "minecraft:villager_profession",
    "minecraft:villager_type",
    "minecraft:worldgen/material_condition_type",
    "minecraft:worldgen/material_rule_type",
    "minecraft:stat_type",
    "minecraft:custom_stat",
    "minecraft:worldgen/structure_piece",
    "minecraft:test_function",
    "minecraft:dialog_action_type",
    "minecraft:incoming_rpc_methods",
    "minecraft:outgoing_rpc_methods",
    "minecraft:sound_event",
    "minecraft:worldgen/density_function_type",
];

/// Registries still reported as missing. Empty: all 95 `reports/registries.json`
/// registries of 26.3 are reproduced exactly.
pub const STILL_BLOCKED: [(&str, &str); 0] = [];

/// The player's `client.jar`, held in memory (independent of `items.rs`'s copy).
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

    fn class(&self, internal: &str) -> Result<Rc<ClassFile>, String> {
        if let Some(class) = self.classes.borrow().get(internal) {
            return Ok(class.clone());
        }
        let name = format!("{internal}.class");
        let class = Rc::new(
            classfile::parse(self.bytes(&name)?).map_err(|error| format!("{name}: {error}"))?,
        );
        self.classes
            .borrow_mut()
            .insert(internal.to_string(), class.clone());
        Ok(class)
    }

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

    /// The data-pack tag ids of one registry directory, sorted (nested tags keep their
    /// path, e.g. `data/minecraft/tags/timeline/in_end.json` -> `minecraft:in_end`).
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
// Bytecode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Instruction {
    opcode: u8,
    index: Option<usize>,
}

fn u16_at(code: &[u8], at: usize) -> Result<usize, String> {
    if at + 2 > code.len() {
        return Err(format!("bytecode ends inside an operand at {at}"));
    }
    Ok(((code[at] as usize) << 8) | code[at + 1] as usize)
}

fn decode(code: &[u8]) -> Result<Vec<Instruction>, String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < code.len() {
        let opcode = code[at];
        let mut index = None;
        let length = match opcode {
            0x12 => {
                index = Some(*code.get(at + 1).ok_or("ldc ends the code")? as usize);
                2
            }
            0x13 | 0x14 => {
                index = Some(u16_at(code, at + 1)?);
                3
            }
            0x10 | 0x11 => 2 + (opcode == 0x11) as usize,
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
        out.push(Instruction { opcode, index });
        at += length;
    }
    Ok(out)
}

fn code_of<'a>(class: &'a ClassFile, name: &str) -> Option<&'a [u8]> {
    class
        .methods
        .iter()
        .find(|member| member.name == name)
        .and_then(|member| member.code.as_ref())
        .map(|code| code.bytes.as_slice())
}

/// `field -> first string of its `putstatic` segment` in a class initializer: the shape
/// the `*Ids` and `ResourceKey` holder classes use.
fn string_fields(jar: &Jar, internal: &str) -> BTreeMap<String, String> {
    let Ok(class) = jar.class(internal) else {
        return BTreeMap::new();
    };
    let Some(code) = code_of(&class, "<clinit>") else {
        return BTreeMap::new();
    };
    let Ok(instructions) = decode(code) else {
        return BTreeMap::new();
    };
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
    out
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

/// The registry a helper method ultimately registers into, following local
/// `register`-named calls (bounded).
fn registry_in_body(class: &ClassFile, code: &[u8]) -> Option<String> {
    let instructions = decode(code).ok()?;
    let mut last_getstatic = None;
    for instruction in instructions {
        match instruction.opcode {
            0xb2 => {
                if let Some((owner, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if owner.ends_with("/BuiltInRegistries") {
                        last_getstatic = Some(field);
                    }
                }
            }
            0xb8 | 0xb9 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    let direct = owner == "net/minecraft/core/Registry"
                        && matches!(name.as_str(), "register" | "registerForHolder");
                    if direct {
                        if let Some(field) = last_getstatic.take() {
                            return Some(field);
                        }
                    }
                }
            }
            0xb3 => last_getstatic = None,
            _ => {}
        }
    }
    None
}

fn registry_of(
    jar: &Jar,
    class: &Rc<ClassFile>,
    name: &str,
    depth: usize,
    cache: &mut BTreeMap<(String, String), Option<String>>,
) -> Option<String> {
    if depth > 3 {
        return None;
    }
    let key = (class.name.clone(), name.to_string());
    if let Some(hit) = cache.get(&key) {
        return hit.clone();
    }
    cache.insert(key.clone(), None);
    let mut found = None;
    for member in class.methods.iter().filter(|member| member.name == name) {
        let Some(code) = member.code.as_ref() else {
            continue;
        };
        if let Some(registry) = registry_in_body(class, &code.bytes) {
            found = Some(registry);
            break;
        }
        if let Ok(instructions) = decode(&code.bytes) {
            for instruction in instructions {
                if instruction.opcode != 0xb8 {
                    continue;
                }
                let Some((owner, callee, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                if owner == "net/minecraft/core/Registry" || !callee.starts_with("register") {
                    continue;
                }
                if let Ok(target) = jar.class(&owner) {
                    if let Some(registry) = registry_of(jar, &target, &callee, depth + 1, cache) {
                        found = Some(registry);
                        break;
                    }
                }
            }
        }
        if found.is_some() {
            break;
        }
    }
    cache.insert(key, found.clone());
    found
}

fn resolve_field(jar: &Jar, owner: &str, name: &str, descriptor: &str) -> Option<String> {
    if !(descriptor.starts_with("Lnet/minecraft/resources/ResourceKey")
        || descriptor.starts_with("Lnet/minecraft/resources/Identifier"))
    {
        return None;
    }
    if owner.ends_with("/BuiltInRegistries") || owner.ends_with("/Registries") {
        return None;
    }
    string_fields(jar, owner).get(name).cloned()
}

/// The id a registration call names: the first `ResourceKey`/`Identifier` field pushed
/// for it, else the first string literal.
fn pick_id(jar: &Jar, strings: &[String], fields: &[(String, String, String)]) -> Option<String> {
    for (owner, name, descriptor) in fields {
        if let Some(id) = resolve_field(jar, owner, name, descriptor) {
            return Some(id);
        }
    }
    strings.first().cloned()
}

fn collect_bootstrap_ids(
    jar: &Jar,
    class: &Rc<ClassFile>,
    method: &classfile::Member,
) -> Vec<String> {
    let Some(code) = method.code.as_ref() else {
        return Vec::new();
    };
    let Ok(instructions) = decode(&code.bytes) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut strings = Vec::new();
    let mut fields = Vec::new();
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
                if let Some(field) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    fields.push(field);
                }
            }
            0xb8 | 0xb9 => {
                if let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if name.starts_with("register") {
                        if let Some(id) = pick_id(jar, &strings, &fields) {
                            out.push(id);
                        }
                        strings.clear();
                        fields.clear();
                    }
                }
            }
            0xb3 => {
                strings.clear();
                fields.clear();
            }
            _ => {}
        }
    }
    out
}

/// Registrations made from class initializers through local helpers, keyed by the
/// `BuiltInRegistries` field the helper writes to.
fn helper_events(jar: &Jar) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cache = BTreeMap::new();
    for internal in jar.class_names() {
        if internal.ends_with("BuiltInRegistries") {
            continue;
        }
        let Ok(class) = jar.class(&internal) else {
            continue;
        };
        for member in &class.methods {
            if member.name != "<clinit>" && member.name != "init" {
                continue;
            }
            let Some(code) = member.code.as_ref() else {
                continue;
            };
            let Ok(instructions) = decode(&code.bytes) else {
                continue;
            };
            let mut strings = Vec::new();
            let mut fields = Vec::new();
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
                        if let Some(field) = instruction
                            .index
                            .and_then(|index| field_ref(&class.constants, index))
                        {
                            fields.push(field);
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
                        let Some(registry) = registry_of(jar, &callee, &name, 0, &mut cache) else {
                            continue;
                        };
                        if let Some(id) = pick_id(jar, &strings, &fields) {
                            out.entry(registry).or_default().push(id);
                        }
                        strings.clear();
                        fields.clear();
                    }
                    0xb3 => {
                        strings.clear();
                        fields.clear();
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Registry creation order
// ---------------------------------------------------------------------------

struct RegistryInfo {
    /// `Registries.<FIELD>` name.
    registry_field: String,
    /// `BuiltInRegistries.<FIELD>` name (they differ for e.g. `TRIGGER_TYPE`).
    builtin_field: String,
    default: Option<String>,
    impl_target: Option<(String, String, String)>,
    key: String,
}

fn registries_fields(jar: &Jar) -> BTreeMap<String, String> {
    let class = jar
        .class("net/minecraft/core/registries/Registries")
        .expect("Registries class");
    let Some(code) = code_of(&class, "<clinit>") else {
        return BTreeMap::new();
    };
    let Ok(instructions) = decode(code) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    let mut pending = None;
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
    out
}

fn registry_order(jar: &Jar) -> Vec<RegistryInfo> {
    let fields = registries_fields(jar);
    let class = jar
        .class("net/minecraft/core/registries/BuiltInRegistries")
        .expect("BuiltInRegistries class");
    let Some(code) = code_of(&class, "<clinit>") else {
        return Vec::new();
    };
    let Ok(instructions) = decode(code) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut last_registry: Option<String> = None;
    let mut last_impl = None;
    let mut default: Option<String> = None;
    // The field pushed after each `register*` call is the BuiltInRegistries field.
    let mut awaiting_builtin: Option<RegistryInfo> = None;
    for instruction in instructions {
        match instruction.opcode {
            0x12 | 0x13 => {
                if let Some(text) = instruction
                    .index
                    .and_then(|index| string_constant(&class.constants, index))
                {
                    default = Some(text);
                }
            }
            0xb2 => {
                if let Some((owner, field, _)) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    if owner.ends_with("/Registries") {
                        last_registry = Some(field);
                    }
                }
            }
            0xba => {
                if let Some(index) = instruction.index {
                    last_impl = bootstrap_impl(&class, index);
                }
            }
            0xb8 => {
                if let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if name.starts_with("register") {
                        if let Some(registry_field) = last_registry.take() {
                            let key = fields
                                .get(&registry_field)
                                .cloned()
                                .unwrap_or_else(|| registry_field.clone());
                            awaiting_builtin = Some(RegistryInfo {
                                builtin_field: registry_field.clone(),
                                registry_field,
                                default: default.take(),
                                impl_target: last_impl.take(),
                                key: format!("minecraft:{key}"),
                            });
                        }
                    }
                }
            }
            0xb3 => {
                if let Some(mut info) = awaiting_builtin.take() {
                    if let Some((owner, field, _)) = instruction
                        .index
                        .and_then(|index| field_ref(&class.constants, index))
                    {
                        if owner.ends_with("/BuiltInRegistries") {
                            info.builtin_field = field;
                        }
                    }
                    out.push(info);
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The registries `items.rs` does not emit
// ---------------------------------------------------------------------------

/// `incoming_rpc_methods`: the ids each `IncomingRpcMethods` service hands to
/// `IncomingRpcMethodBuilder.register(registry, id)`, in service/registration order.
fn incoming_rpc_ids(jar: &Jar) -> Vec<String> {
    fn scan(
        jar: &Jar,
        class: &Rc<ClassFile>,
        member: &classfile::Member,
        depth: usize,
        out: &mut Vec<String>,
    ) {
        if depth > 3 {
            return;
        }
        let Some(code) = member.code.as_ref() else {
            return;
        };
        let Ok(instructions) = decode(&code.bytes) else {
            return;
        };
        let mut strings: Vec<String> = Vec::new();
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
                0xb8 if depth < 3 => {
                    let Some((owner, name, _)) = instruction
                        .index
                        .and_then(|index| method_ref(&class.constants, index))
                    else {
                        continue;
                    };
                    if owner == class.name && name.starts_with("register") {
                        if let Some(target) = class
                            .methods
                            .iter()
                            .find(|candidate| candidate.name == name)
                        {
                            scan(jar, class, target, depth + 1, out);
                        }
                        strings.clear();
                    }
                }
                0xb6 | 0xb9 => {
                    let Some((owner, name, _)) = instruction
                        .index
                        .and_then(|index| method_ref(&class.constants, index))
                    else {
                        continue;
                    };
                    if name == "register" && owner.ends_with("IncomingRpcMethodBuilder") {
                        if let Some(id) = strings.last() {
                            out.push(id.clone());
                        }
                        strings.clear();
                    }
                }
                0xb3 => strings.clear(),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    let Ok(class) = jar.class("net/minecraft/server/jsonrpc/IncomingRpcMethods") else {
        return out;
    };
    for member in &class.methods {
        if member.name == "bootstrap" || member.name == "<clinit>" {
            scan(jar, &class, member, 0, &mut out);
        }
    }
    out
}

/// `outgoing_rpc_methods`: the `notification/` ids `OutgoingRpcMethods.<clinit>` builds
/// with `OutgoingRpcMethodBuilder.register("server/started")` and friends.
fn outgoing_rpc_ids(jar: &Jar) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(class) = jar.class("net/minecraft/server/jsonrpc/OutgoingRpcMethods") else {
        return out;
    };
    let Some(code) = code_of(&class, "<clinit>") else {
        return out;
    };
    let Ok(instructions) = decode(code) else {
        return out;
    };
    let mut strings: Vec<String> = Vec::new();
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
            0xb6 | 0xb9 => {
                if let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if name == "register" && owner.ends_with("OutgoingRpcMethodBuilder") {
                        if let Some(id) = strings.last() {
                            out.push(format!("notification/{id}"));
                        }
                        strings.clear();
                    }
                }
            }
            0xb3 => strings.clear(),
            _ => {}
        }
    }
    out
}

/// `sound_event`: `SoundEvents.<clinit>`'s literal registrations plus the variant
/// sounds generated by `register*SoundVariants`, in the order vanilla registers them.
///
/// The variant helpers stream over a `SoundSet` enum and build each id with
/// `makeConcatWithConstants` (recipe like `entity.\u0001.ambient`) from the enum
/// constant's base id. Both are literals in the class files, so the expansion is exact.
fn sound_event_ids(jar: &Jar) -> Vec<String> {
    const VARIANT_HELPERS: [(&str, &str, &str); 5] = [
        (
            "registerCatSoundVariants",
            "net/minecraft/world/entity/animal/feline/CatSoundVariants$SoundSet",
            "lambda$registerCatSoundVariants$1",
        ),
        (
            "registerChickenSoundVariants",
            "net/minecraft/world/entity/animal/chicken/ChickenSoundVariants$SoundSet",
            "lambda$registerChickenSoundVariants$1",
        ),
        (
            "registerCowSoundVariants",
            "net/minecraft/world/entity/animal/cow/CowSoundVariants$SoundSet",
            "lambda$registerCowSoundVariants$1",
        ),
        (
            "registerPigSoundVariants",
            "net/minecraft/world/entity/animal/pig/PigSoundVariants$SoundSet",
            "lambda$registerPigSoundVariants$1",
        ),
        (
            "registerWolfSoundVariants",
            "net/minecraft/world/entity/animal/wolf/WolfSoundVariants$SoundSet",
            "lambda$registerWolfSoundVariants$1",
        ),
    ];

    fn concat_recipe(class: &ClassFile, index: usize) -> Option<String> {
        let Constant::InvokeDynamic { bootstrap, .. } = class.constants.get(index)? else {
            return None;
        };
        let row = class.bootstraps.get(*bootstrap)?;
        row.arguments
            .first()
            .and_then(|argument| string_constant(&class.constants, *argument))
    }

    fn variant_ids(jar: &Jar, enum_class: &str, lambda: &str) -> Vec<String> {
        let Ok(sounds) = jar.class("net/minecraft/sounds/SoundEvents") else {
            return Vec::new();
        };
        let Some(member) = sounds.methods.iter().find(|member| member.name == lambda) else {
            return Vec::new();
        };
        let Some(code) = member.code.as_ref() else {
            return Vec::new();
        };
        let Ok(instructions) = decode(&code.bytes) else {
            return Vec::new();
        };
        let mut recipes = Vec::new();
        for instruction in instructions {
            if instruction.opcode == 0xba {
                if let Some(index) = instruction.index {
                    if let Some(recipe) = concat_recipe(&sounds, index) {
                        recipes.push(recipe);
                    }
                }
            }
        }
        let Ok(enum_file) = jar.class(enum_class) else {
            return Vec::new();
        };
        let table = string_fields(jar, enum_class);
        let mut bases = Vec::new();
        for member in &enum_file.fields {
            if member.descriptor != format!("L{enum_class};") {
                continue;
            }
            if let Some(base) = table.get(&member.name) {
                bases.push(base.clone());
            }
        }
        let mut out = Vec::new();
        for base in bases {
            for recipe in &recipes {
                out.push(recipe.replace('\u{1}', &base));
            }
        }
        out
    }

    let Ok(class) = jar.class("net/minecraft/sounds/SoundEvents") else {
        return Vec::new();
    };
    let Some(code) = code_of(&class, "<clinit>") else {
        return Vec::new();
    };
    let Ok(instructions) = decode(code) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut strings: Vec<String> = Vec::new();
    let mut fields = Vec::new();
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
                if let Some(field) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    fields.push(field);
                }
            }
            0xb8 => {
                let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                if owner != "net/minecraft/sounds/SoundEvents" {
                    continue;
                }
                if let Some((_, enum_class, lambda)) = VARIANT_HELPERS
                    .iter()
                    .find(|(helper, _, _)| *helper == name)
                {
                    out.extend(variant_ids(jar, enum_class, lambda));
                    strings.clear();
                    fields.clear();
                } else if name == "registerGoatHornSoundVariants" {
                    let recipes = class
                        .methods
                        .iter()
                        .find(|member| member.name == "lambda$registerGoatHornSoundVariants$0")
                        .and_then(|member| member.code.as_ref())
                        .and_then(|code| decode(&code.bytes).ok())
                        .map(|instructions| {
                            let mut recipes = Vec::new();
                            for instruction in instructions {
                                if instruction.opcode == 0xba {
                                    if let Some(index) = instruction.index {
                                        if let Some(recipe) = concat_recipe(&class, index) {
                                            recipes.push(recipe);
                                        }
                                    }
                                }
                            }
                            recipes
                        })
                        .unwrap_or_default();
                    for index in 0..8 {
                        for recipe in &recipes {
                            out.push(recipe.replace('\u{1}', &index.to_string()));
                        }
                    }
                    strings.clear();
                    fields.clear();
                } else if name == "register" || name == "registerForHolder" {
                    if let Some(id) = pick_id(jar, &strings, &fields) {
                        out.push(id);
                    }
                    strings.clear();
                    fields.clear();
                }
            }
            0xb3 => {
                strings.clear();
                fields.clear();
            }
            _ => {}
        }
    }
    out
}

/// `worldgen/density_function_type`: `DensityFunctions.bootstrap`'s registrations,
/// including the four places it loops over an enum's `values()` and registers each
/// constant under its serialized id (`SimpleDensityFunction`, `UnaryFunction$Type`,
/// `RoundFunction$Type`, `BinaryFunction$Type`).
fn density_function_ids(jar: &Jar) -> Vec<String> {
    fn enum_ids(jar: &Jar, enum_class: &str) -> Vec<String> {
        let Ok(class) = jar.class(enum_class) else {
            return Vec::new();
        };
        let table = string_fields(jar, enum_class);
        let suffix = format!("L{enum_class};");
        let mut out = Vec::new();
        for member in &class.fields {
            if member.descriptor != suffix {
                continue;
            }
            if let Some(id) = table.get(&member.name) {
                out.push(id.clone());
            }
        }
        out
    }

    let Ok(class) =
        jar.class("net/minecraft/world/level/levelgen/densityfunction/DensityFunctions")
    else {
        return Vec::new();
    };
    let Some(member) = class
        .methods
        .iter()
        .find(|member| member.name == "bootstrap")
    else {
        return Vec::new();
    };
    let Some(code) = member.code.as_ref() else {
        return Vec::new();
    };
    let Ok(instructions) = decode(&code.bytes) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut strings: Vec<String> = Vec::new();
    let mut fields = Vec::new();
    let mut last_enum: Option<String> = None;
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
                if let Some(field) = instruction
                    .index
                    .and_then(|index| field_ref(&class.constants, index))
                {
                    fields.push(field);
                }
            }
            0xb8 => {
                let Some((owner, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                else {
                    continue;
                };
                if name == "values" && owner.starts_with("net/minecraft/world/level/levelgen/") {
                    last_enum = Some(owner);
                    continue;
                }
                if name != "register" {
                    continue;
                }
                if let Some(enum_class) = last_enum.take() {
                    out.extend(enum_ids(jar, &enum_class));
                } else if let Some(id) = pick_id(jar, &strings, &fields) {
                    out.push(id);
                }
                strings.clear();
                fields.clear();
            }
            0xb3 => {
                strings.clear();
                fields.clear();
                last_enum = None;
            }
            _ => {}
        }
    }
    out
}

/// `stat_type` (9) and `custom_stat` (78), from `Stats.<clinit>`.
fn stats_events(jar: &Jar) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Ok(class) = jar.class("net/minecraft/stats/Stats") else {
        return out;
    };
    let Some(code) = code_of(&class, "<clinit>") else {
        return out;
    };
    let Ok(instructions) = decode(code) else {
        return out;
    };
    let mut strings: Vec<String> = Vec::new();
    let mut stat_types = Vec::new();
    let mut custom = Vec::new();
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
            0xb8 => {
                if let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if name == "makeRegistryStatType" || name == "makeCustomStat" {
                        if let Some(id) = strings.first().cloned() {
                            if name == "makeRegistryStatType" {
                                stat_types.push(id);
                            } else {
                                custom.push(id);
                            }
                        }
                        strings.clear();
                    }
                }
            }
            0xb3 => strings.clear(),
            _ => {}
        }
    }
    out.insert("STAT_TYPE".to_string(), stat_types);
    out.insert("CUSTOM_STAT".to_string(), custom);
    out
}

/// `worldgen/structure_piece`: `StructurePieceType.setPieceId(type, "MSCorridor")`, the
/// registration id being the lowercased piece label.
fn structure_piece_ids(jar: &Jar) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(class) =
        jar.class("net/minecraft/world/level/levelgen/structure/pieces/StructurePieceType")
    else {
        return out;
    };
    let Some(code) = code_of(&class, "<clinit>") else {
        return out;
    };
    let Ok(instructions) = decode(code) else {
        return out;
    };
    let mut strings: Vec<String> = Vec::new();
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
            0xb8 => {
                if let Some((_, name, _)) = instruction
                    .index
                    .and_then(|index| method_ref(&class.constants, index))
                {
                    if matches!(
                        name.as_str(),
                        "setPieceId" | "setTemplatePieceId" | "setFullContextPieceId"
                    ) {
                        if let Some(label) = strings.first() {
                            out.push(label.to_ascii_lowercase());
                        }
                        strings.clear();
                    }
                }
            }
            0xb3 => strings.clear(),
            _ => {}
        }
    }
    out
}

/// `test_function`: the keys `BuiltinTestFunctions.load` hands to the loader.
fn test_function_ids(jar: &Jar) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(class) = jar.class("net/minecraft/gametest/framework/BuiltinTestFunctions") else {
        return out;
    };
    let Some(code) = code_of(&class, "load") else {
        return out;
    };
    let Ok(instructions) = decode(code) else {
        return out;
    };
    let table = string_fields(jar, "net/minecraft/gametest/framework/BuiltinTestFunctions");
    for instruction in instructions {
        if instruction.opcode != 0xb2 {
            continue;
        }
        if let Some((_, field, _)) = instruction
            .index
            .and_then(|index| field_ref(&class.constants, index))
        {
            if let Some(id) = table.get(&field) {
                out.push(id.clone());
            }
        }
    }
    out
}

/// `dialog_action_type`: the wrapped `ClickEvent.Action` values (minus `OPEN_FILE`,
/// which has no dialog codec), then the two dynamic actions registered by
/// `ActionTypes.bootstrap`.
fn dialog_action_ids(jar: &Jar) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(class) = jar.class("net/minecraft/network/chat/ClickEvent$Action") {
        let table = string_fields(jar, "net/minecraft/network/chat/ClickEvent$Action");
        for member in &class.fields {
            if member.descriptor != "Lnet/minecraft/network/chat/ClickEvent$Action;" {
                continue;
            }
            if let Some(id) = table.get(&member.name) {
                if id != "open_file" {
                    out.push(id.clone());
                }
            }
        }
    }
    if let Ok(class) = jar.class("net/minecraft/server/dialog/action/ActionTypes") {
        if let Some(member) = class
            .methods
            .iter()
            .find(|member| member.name == "bootstrap")
        {
            out.extend(collect_bootstrap_ids(jar, &class, member));
        }
    }
    out
}

fn value_entries(ids: &[String]) -> Map<String, Value> {
    let mut entries = Map::new();
    for (index, id) in ids.iter().enumerate() {
        let id = if id.contains(':') {
            id.clone()
        } else {
            format!("minecraft:{id}")
        };
        let mut entry = Map::new();
        entry.insert("protocol_id".to_string(), Value::Number(index.into()));
        entries.insert(id, Value::Object(entry));
    }
    entries
}

/// The registries `items.rs` alone emits (68 of 95).
pub fn base_registries(jar_path: &Path) -> Result<(Value, base::Coverage), String> {
    base::registries(jar_path)
}

/// The full `registries.json` this jar and this module can build.
pub fn registries(jar_path: &Path) -> Result<(Value, base::Coverage), String> {
    let jar = Jar::open(jar_path)?;
    let (document, mut coverage) = base::registries(jar_path)?;
    let mut object = document.as_object().cloned().unwrap_or_default();

    let stats = stats_events(&jar);
    let structure = structure_piece_ids(&jar);
    let tests = test_function_ids(&jar);
    let dialog = dialog_action_ids(&jar);
    let incoming_rpc = incoming_rpc_ids(&jar);
    let outgoing_rpc = outgoing_rpc_ids(&jar);
    let helpers = helper_events(&jar);
    let order = registry_order(&jar);

    for (protocol_id, info) in order.iter().enumerate() {
        if !ADDED_REGISTRIES.contains(&info.key.as_str()) {
            continue;
        }
        let ids: Vec<String> = match info.key.as_str() {
            "minecraft:stat_type" => stats.get("STAT_TYPE").cloned().unwrap_or_default(),
            "minecraft:custom_stat" => stats.get("CUSTOM_STAT").cloned().unwrap_or_default(),
            "minecraft:worldgen/structure_piece" => structure.clone(),
            "minecraft:test_function" => tests.clone(),
            "minecraft:dialog_action_type" => dialog.clone(),
            "minecraft:incoming_rpc_methods" => incoming_rpc.clone(),
            "minecraft:outgoing_rpc_methods" => outgoing_rpc.clone(),
            "minecraft:sound_event" => sound_event_ids(&jar),
            "minecraft:worldgen/density_function_type" => density_function_ids(&jar),
            _ => {
                // A bootstrapped registry registers in its bootstrap method; one whose
                // bootstrap only returns a default registers from a class initializer.
                if let Some((owner, name, _)) = &info.impl_target {
                    if let Ok(class) = jar.class(owner) {
                        if let Some(member) =
                            class.methods.iter().find(|member| member.name == *name)
                        {
                            let ids = collect_bootstrap_ids(&jar, &class, member);
                            if !ids.is_empty() {
                                ids
                            } else {
                                helpers
                                    .get(&info.builtin_field)
                                    .cloned()
                                    .or_else(|| helpers.get(&info.registry_field).cloned())
                                    .unwrap_or_default()
                            }
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    }
                } else {
                    helpers
                        .get(&info.builtin_field)
                        .cloned()
                        .or_else(|| helpers.get(&info.registry_field).cloned())
                        .unwrap_or_default()
                }
            }
        };
        if ids.is_empty() {
            continue;
        }
        let entries = value_entries(&ids);
        let mut registry = Map::new();
        registry.insert("protocol_id".to_string(), Value::Number(protocol_id.into()));
        if let Some(default) = &info.default {
            registry.insert(
                "default".to_string(),
                Value::String(format!("minecraft:{default}")),
            );
        }
        registry.insert("entries".to_string(), Value::Object(entries));
        object.insert(info.key.clone(), Value::Object(registry));
        coverage.registries.push(info.key.clone());
    }

    // Refresh produced/missing/absent lists after the additions.
    coverage.registries.sort();
    coverage.registries.dedup();
    let all: BTreeSet<String> = registry_order(&jar)
        .iter()
        .map(|info| info.key.clone())
        .collect();
    coverage.absent_registries = all
        .iter()
        .filter(|key| !coverage.registries.contains(key))
        .map(|key| {
            let reason = STILL_BLOCKED
                .iter()
                .find(|(blocked, _)| blocked == key)
                .map(|(_, reason)| (*reason).to_string())
                .unwrap_or_else(|| "registration order is not a literal sequence".to_string());
            format!("{key}: {reason}")
        })
        .collect();
    coverage
        .produced
        .retain(|line| !line.starts_with("registries.json ("));
    coverage.produced.push(format!(
        "registries.json ({} of {} registries)",
        coverage.registries.len(),
        all.len()
    ));
    coverage
        .missing
        .retain(|line| !line.starts_with("registries.json ("));
    if coverage.registries.len() < all.len() {
        coverage.missing.push(format!(
            "registries.json ({} of {} registries)",
            all.len() - coverage.registries.len(),
            all.len()
        ));
    }
    for (table, why) in STILL_BLOCKED {
        coverage.notes.push(format!("{table}: {why}"));
    }
    if BLOCKS_READY {
        match blocks_fragment(jar_path) {
            Ok(_) => {
                coverage
                    .produced
                    .push("command-registries.json#blocks".to_string());
                coverage
                    .missing
                    .retain(|line| line != "command-registries.json#blocks");
                coverage.notes.push(
                    "command-registries.json#blocks: exact through states.rs's \
                     StateDefinition reader (state property names and getPossibleValues \
                     order)"
                        .to_string(),
                );
            }
            Err(error) => coverage.notes.push(format!(
                "command-registries.json#blocks: omitted because states.rs's property \
                 reader failed ({error})"
            )),
        }
    } else {
        coverage.notes.push(
            "command-registries.json#blocks: omitted while states.rs (task-7) is not exact"
                .to_string(),
        );
    }
    Ok((Value::Object(object), coverage))
}

/// The `command-registries.json#blocks` fragment: every block's state properties as
/// `{id: [[name, [values in getPossibleValues order]], ...]}`, in registry order, from
/// `states.rs`'s `StateDefinition` reader. `states.rs` returns builder insertion order,
/// so properties are sorted by name here (the oracle's order).
pub fn blocks_fragment(jar_path: &Path) -> Result<Value, String> {
    let ids = base::block_ids(jar_path)?;
    let properties = states::block_properties(jar_path)?;
    let mut out = Map::new();
    for id in ids {
        let Some(list) = properties.get(&id) else {
            return Err(format!("states.rs has no properties for {id}"));
        };
        let mut list = list.clone();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        let mut array = Vec::with_capacity(list.len());
        for (name, values) in list {
            array.push(Value::Array(vec![
                Value::String(name),
                Value::Array(values.into_iter().map(Value::String).collect()),
            ]));
        }
        out.insert(id, Value::Array(array));
    }
    Ok(Value::Object(out))
}

/// Whether `states.rs`'s property reader is exact enough to emit
/// `command-registries.json#blocks`. Turned on once task-7's oracle tests were green.
pub const BLOCKS_READY: bool = true;

/// Every `by_registry` entry the oracle carries: the 95 code registries from
/// [`registries`] (ids sorted, tags from the data pack) plus the data-driven registries
/// the jar's `data/minecraft/**` names, plus the three empty ones the data pack lacks.
pub fn by_registry(jar_path: &Path) -> Result<Value, String> {
    let jar = Jar::open(jar_path)?;
    let (document, _) = registries(jar_path)?;
    let mut entries: BTreeMap<String, (BTreeSet<String>, Vec<String>)> = BTreeMap::new();

    if let Some(object) = document.as_object() {
        for (key, registry) in object {
            let directory = key.trim_start_matches("minecraft:");
            let mut ids = BTreeSet::new();
            if let Some(entries) = registry.get("entries").and_then(Value::as_object) {
                ids.extend(entries.keys().cloned());
            }
            entries.insert(key.clone(), (ids, jar.tag_ids(directory)));
        }
    }

    for name in jar.files.keys() {
        let Some(rest) = name.strip_prefix("data/minecraft/") else {
            continue;
        };
        let Some(id) = rest.strip_suffix(".json") else {
            continue;
        };
        let parts: Vec<&str> = id.split('/').collect();
        if parts.len() < 2 {
            continue;
        }
        let (directory, registry) = match parts[0] {
            "tags" | "datapacks" | "structure" => continue,
            "worldgen" if parts.len() >= 3 => (
                format!("worldgen/{}", parts[1]),
                format!("worldgen/{}", parts[1]),
            ),
            _ => (parts[0].to_string(), parts[0].to_string()),
        };
        let Some(relative) = id.strip_prefix(&format!("{directory}/")) else {
            continue;
        };
        entries
            .entry(format!("minecraft:{registry}"))
            .or_insert_with(|| (BTreeSet::new(), jar.tag_ids(&directory)))
            .0
            .insert(format!("minecraft:{relative}"));
    }

    // Empty in 26.3 and without a data directory.
    for key in [
        "minecraft:dimension",
        "minecraft:item_modifier",
        "minecraft:slot_source",
    ] {
        entries
            .entry(key.to_string())
            .or_insert_with(|| (BTreeSet::new(), Vec::new()));
    }

    let mut out = Map::new();
    for (key, (ids, tags)) in entries {
        let mut entry = Map::new();
        entry.insert(
            "ids".to_string(),
            Value::Array(ids.into_iter().map(Value::String).collect()),
        );
        entry.insert(
            "tags".to_string(),
            Value::Array(tags.into_iter().map(Value::String).collect()),
        );
        out.insert(key, Value::Object(entry));
    }
    Ok(Value::Object(out))
}

/// The `command-registries.json` fragments this module's layer adds: `items`,
/// `block_entities`, the complete `by_registry`, plus the state-property `blocks` table
/// when [`BLOCKS_READY`].
pub fn command_fragments(jar_path: &Path) -> Result<Value, String> {
    let mut object = Map::new();
    object.insert(
        "items".to_string(),
        Value::Array(
            base::item_ids(jar_path)?
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    object.insert(
        "block_entities".to_string(),
        Value::Array(
            base::block_entities(jar_path)?
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    object.insert("by_registry".to_string(), by_registry(jar_path)?);
    if BLOCKS_READY {
        object.insert("blocks".to_string(), blocks_fragment(jar_path)?);
    }
    Ok(Value::Object(object))
}

/// The complete `command-registries.json`: `registries.rs`'s document with the missing
/// `items`, `block_entities`, `by_registry` entries and (when [`BLOCKS_READY`]) `blocks`
/// filled in. `registries_base` leaves `items`/`blocks`/`block_entities` and the
/// code-registry half of `by_registry` absent; this merges them back.
pub fn command_registries(jar_path: &Path) -> Result<(Value, registries_base::Coverage), String> {
    let (document, mut coverage) = registries_base::command_registries(jar_path)?;
    let mut object = document.as_object().cloned().unwrap_or_default();
    let fragments = command_fragments(jar_path)?;
    let fragments = fragments.as_object().cloned().unwrap_or_default();
    for key in ["items", "blocks", "block_entities", "by_registry"] {
        if let Some(value) = fragments.get(key) {
            object.insert(key.to_string(), value.clone());
            if key != "by_registry" {
                coverage.missing.retain(|name| name != key);
                coverage.produced.push(key.to_string());
            }
        }
    }
    if !BLOCKS_READY {
        coverage.notes.push(
            "command-registries.json#blocks: omitted while states.rs (task-7) is not exact"
                .to_string(),
        );
    }
    Ok((Value::Object(object), coverage))
}

/// Write the completed `registries.json` into `out_dir`.
pub fn write(jar_path: &Path, out_dir: &Path) -> Result<base::Coverage, String> {
    let (document, coverage) = registries(jar_path)?;
    fs::create_dir_all(out_dir).map_err(|error| format!("{}: {error}", out_dir.display()))?;
    let path = out_dir.join(TABLE);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(coverage)
}

/// The name of the table [`write`] produces.
pub const TABLE: &str = "registries.json";

/// The name of the merged table [`write_command_registries`] produces.
pub const COMMAND_TABLE: &str = "command-registries.json";

/// Write the merged `command-registries.json` (complete except `blocks` until
/// [`BLOCKS_READY`]) into `out_dir`.
pub fn write_command_registries(
    jar_path: &Path,
    out_dir: &Path,
) -> Result<registries_base::Coverage, String> {
    let (document, coverage) = command_registries(jar_path)?;
    fs::create_dir_all(out_dir).map_err(|error| format!("{}: {error}", out_dir.display()))?;
    let path = out_dir.join(COMMAND_TABLE);
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(coverage)
}
